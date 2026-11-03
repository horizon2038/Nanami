//! Opt-in read-only private fork aliases. Writers detach under VM quiescence.
use super::*;
use crate::nanami_core::process::{PhysicalPageReference, ProcessPhysicalAllocation};

impl Alpha {
    pub(super) fn clone_process_vm_shared_readonly(
        &mut self,
        source: usize,
        target: usize,
    ) -> Result<(), CapabilityError> {
        // Host tests exercise ownership with modeled page tables. Other
        // production backends retain eager copying until their TLB contract is verified.
        if !cfg!(any(target_arch = "x86_64", test)) {
            return self.clone_process_vm(source, target);
        }
        let parent = self
            .processes
            .find_entry_by_pid(source)
            .ok_or(CapabilityError::InvalidArgument)?;
        let child = self
            .processes
            .find_entry_by_pid(target)
            .ok_or(CapabilityError::InvalidArgument)?;
        if !parent.capabilities_sealed
            || !child.capabilities_sealed
            || parent.resource_quarantined
            || child.memory_owner != target
            || parent.reaper_pid != child.reaper_pid
        {
            return Err(CapabilityError::PermissionDenied);
        }
        if self
            .processes
            .vm_space_mut(target)
            .is_none_or(|vm| !vm.backed_pages(0, usize::MAX).is_empty())
        {
            return Err(CapabilityError::IllegalOperation);
        }
        let source_memory = self
            .processes
            .memory_entry(source)
            .ok_or(CapabilityError::InvalidArgument)?;
        let destination = self
            .processes
            .memory_entry(target)
            .ok_or(CapabilityError::InvalidArgument)?;
        let pages = self
            .processes
            .vm_space_mut(source)
            .ok_or(CapabilityError::InvalidArgument)?
            .backed_pages(0, usize::MAX);
        // One request-local ownership snapshot, never a list search per page.
        let references = self
            .processes
            .private_page_references(source, 0, usize::MAX);
        let mut shared = Vec::with_capacity(pages.len());
        for &(va, permissions) in &pages {
            let candidate = references
                .binary_search_by_key(&va, |r| r.allocation.base_va)
                .ok();
            let eligible = if let Some(index) = candidate {
                let allocation = references[index].allocation;
                let count = self.processes.physical_reference_count(allocation);
                let vm = self
                    .processes
                    .vm_space_mut(source)
                    .ok_or(CapabilityError::InvalidArgument)?;
                let known_private_alias = vm.private_page_shared(va);
                let frame = vm.find_frame(va);
                let expected =
                    process_frame_descriptor(source_memory.root_node, allocation.start_slot);
                permissions != 0
                    && permissions & !5 == 0
                    && count != 0
                    && (known_private_alias || count == 1)
                    && frame == Some(expected)
                    && self
                        .processes
                        .shared_memory_reservation(source, va, 1)
                        .is_none()
            } else {
                false
            };
            shared.push(candidate.filter(|_| eligible));
        }
        let mut begin = 0;
        while begin < pages.len() {
            let share = shared[begin].is_some();
            let mut end = begin + 1;
            while end < pages.len()
                && pages[end - 1].0.checked_add(PAGE_SIZE) == Some(pages[end].0)
                && shared[end].is_some() == share
            {
                end += 1;
            }
            let run = &pages[begin..end];
            let base = run[0].0;
            if share {
                let (root, space, first) = self.processes.reserve_process_heap_at(
                    target,
                    base,
                    run.len(),
                    PAGE_SIZE,
                    PROCESS_FRAME_TOTAL_PAGES,
                )?;
                self.ensure_process_frame_chunks(target, root, first, run.len())?;
                for (offset, &(va, permissions)) in run.iter().enumerate() {
                    let allocation = references[shared[begin + offset].unwrap()].allocation;
                    let frame = process_frame_descriptor(root, first + offset);
                    // Conservative markers survive a failed unpublished child;
                    // its references remain held until normal spawn rollback.
                    self.processes
                        .vm_space_mut(source)
                        .ok_or(CapabilityError::Fatal)?
                        .mark_private_page_shared(va, true);
                    self.processes.register_physical_allocation(
                        target,
                        va,
                        first + offset,
                        allocation.base_page,
                        1,
                    )?;
                    arch::node::copy(
                        process_frame_chunk_descriptor(
                            root,
                            (first + offset) / PROCESS_FRAME_CHUNK_PAGES,
                        ),
                        (first + offset) % PROCESS_FRAME_CHUNK_PAGES,
                        process_frame_descriptor(source_memory.root_node, allocation.start_slot),
                    )?;
                    let vm = self
                        .processes
                        .vm_space_mut(target)
                        .ok_or(CapabilityError::Fatal)?;
                    vm.mark_private_page_shared(va, true);
                    vm.set_permissions(va, permissions);
                    if !vm.page_tables_ready(va) {
                        self.memory.ensure_page_tables(space, va, vm)?;
                    }
                    let attributes =
                        nun::capability_call::address_space::Attribute::from_bits(permissions)
                            .ok_or(CapabilityError::InvalidArgument)?;
                    let mapped = arch::address_space::map(space, frame, va, attributes);
                    // Retain possible post-MAP side effects for root rollback.
                    vm.record_frame(va, frame)
                        .map_err(|_| CapabilityError::Fatal)?;
                    mapped?;
                }
            } else {
                self.map_process_initialized_pages(
                    target,
                    Some(base),
                    run.len(),
                    Some(PageSource {
                        pid: source,
                        address: base,
                        bytes: run.len() * PAGE_SIZE,
                    }),
                )?;
                for &(va, permissions) in run {
                    let frame = self.process_frame_for_page(target, va)?;
                    self.protect_process_page(
                        target,
                        destination.address_space,
                        va,
                        frame,
                        permissions,
                    )?;
                }
            }
            begin = end;
        }
        Ok(())
    }

    pub(super) fn handle_process_private_write(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        use nanami_uapi::os::{OS_PRIVATE_WRITE_DETACH, OS_PRIVATE_WRITE_QUERY};
        let pid = request.arg0;
        let target = self
            .processes
            .find_entry_by_pid(pid)
            .filter(|entry| !entry.exited)
            .ok_or(CapabilityError::InvalidArgument)?;
        if request.identifier == 0
            || request.identifier == pid
            || target.reaper_pid != request.identifier
        {
            return Err(CapabilityError::PermissionDenied);
        }
        if request.arg2 == 0
            || (request.arg3 != OS_PRIVATE_WRITE_QUERY && request.arg3 != OS_PRIVATE_WRITE_DETACH)
        {
            return Err(CapabilityError::InvalidArgument);
        }
        let end = request
            .arg1
            .checked_add(request.arg2)
            .ok_or(CapabilityError::InvalidArgument)?;
        let start = align_down(request.arg1, PAGE_SIZE);
        let vm = self
            .processes
            .vm_space_mut(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if request.arg3 == OS_PRIVATE_WRITE_QUERY {
            return Ok(usize::from(vm.private_range_shared(start, end)));
        }
        let pages = vm.shared_private_pages(start, end);
        if pages.is_empty() {
            return Ok(0);
        }
        // DETACH is a trusted supervisor contract: all users of this VM are
        // already trapped, parked, prepared, stopped, or synchronously suspended.
        let references = self.processes.private_page_references(pid, start, end);
        self.invalidate_process_copy_mappings(pid)?;
        for va in pages {
            let index = references
                .binary_search_by_key(&va, |r| r.allocation.base_va)
                .map_err(|_| CapabilityError::Fatal)?;
            self.detach_private_page(pid, references[index])?;
        }
        Ok(0)
    }

    fn detach_private_page(
        &mut self,
        pid: usize,
        reference: PhysicalPageReference,
    ) -> Result<(), CapabilityError> {
        let old = reference.allocation;
        let count = self.processes.physical_reference_count(old);
        if count == 0 {
            return Err(CapabilityError::Fatal);
        }
        let memory = self
            .processes
            .memory_entry(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        let old_frame = process_frame_descriptor(memory.root_node, old.start_slot);
        let vm = self
            .processes
            .vm_space_mut(pid)
            .ok_or(CapabilityError::Fatal)?;
        if vm.find_frame(old.base_va) != Some(old_frame) || vm.permissions(old.base_va) & 2 != 0 {
            return Err(CapabilityError::Fatal);
        }
        let permissions = vm.permissions(old.base_va);
        if count == 1 {
            vm.mark_private_page_shared(old.base_va, false);
            return Ok(());
        }
        let (root, space, slot) = self
            .processes
            .reserve_private_replacement_slot(pid, PROCESS_FRAME_TOTAL_PAGES)?;
        self.ensure_process_frame_chunks(pid, root, slot, 1)?;
        let physical = self.memory.allocate_process_frame(
            process_frame_chunk_descriptor(root, slot / PROCESS_FRAME_CHUNK_PAGES),
            PROCESS_FRAME_NODE_RADIX,
            slot % PROCESS_FRAME_CHUNK_PAGES,
        )?;
        let replacement = ProcessPhysicalAllocation {
            base_va: old.base_va,
            start_slot: slot,
            base_page: physical,
            page_count: 1,
        };
        // An unpublished replacement is still owned, including on any error.
        self.processes
            .defer_physical_allocation_for_pid(pid, replacement);
        if physical == old.base_page {
            return Err(CapabilityError::Fatal);
        }
        // Cache the permanent physical-pool capability, not the child slot
        // which is removed below. Otherwise a later slot reuse could hit a
        // stale copy-window alias after the initial PID invalidation.
        let source_frame = self
            .memory
            .physical_frame_descriptor_from_index(old.base_page)
            .ok_or(CapabilityError::Fatal)?;
        let source = self.map_process_copy_frame(pid, source_frame, None)?;
        let destination = self.map_initialization_page(physical)?;
        unsafe {
            ptr::copy_nonoverlapping(source as *const u8, destination as *mut u8, PAGE_SIZE);
        }
        let new_frame = process_frame_descriptor(root, slot);
        let attributes = nun::capability_call::address_space::Attribute::from_bits(permissions)
            .ok_or(CapabilityError::Fatal)?;
        if permissions != 0 {
            arch::address_space::unmap(space, old_frame, old.base_va)?;
            if let Err(error) = arch::address_space::map(space, new_frame, old.base_va, attributes)
            {
                return self.restore_private_page(
                    pid,
                    space,
                    old.base_va,
                    old_frame,
                    new_frame,
                    attributes,
                    error,
                );
            }
        }
        if let Err(error) = arch::node::remove(
            process_frame_chunk_descriptor(root, old.start_slot / PROCESS_FRAME_CHUNK_PAGES),
            old.start_slot % PROCESS_FRAME_CHUNK_PAGES,
        ) {
            if permissions != 0 {
                return self.restore_private_page(
                    pid,
                    space,
                    old.base_va,
                    old_frame,
                    new_frame,
                    attributes,
                    error,
                );
            }
            return Err(error);
        }
        let last = self
            .processes
            .replace_private_backing(reference, replacement)?;
        let vm = self
            .processes
            .vm_space_mut(pid)
            .ok_or(CapabilityError::Fatal)?;
        vm.record_frame(old.base_va, new_frame)
            .map_err(|_| CapabilityError::Fatal)?;
        vm.mark_private_page_shared(old.base_va, false);
        self.processes
            .recycle_private_frame_slot(pid, old.start_slot);
        if last {
            self.memory
                .free_physical(old.base_page * PAGE_SIZE, PAGE_SIZE)?;
        }
        Ok(())
    }

    fn restore_private_page(
        &mut self,
        pid: usize,
        space: CapabilityDescriptor,
        va: usize,
        old: CapabilityDescriptor,
        new: CapabilityDescriptor,
        attributes: nun::capability_call::address_space::Attribute,
        error: CapabilityError,
    ) -> Result<(), CapabilityError> {
        // UNMAP may fail because MAP installed nothing. Only a successful strict
        // restoration proves recovery. Never free either backing on failure.
        let _ = arch::address_space::unmap(space, new, va);
        if arch::address_space::map(space, old, va, attributes).is_err() {
            self.processes
                .vm_space_mut(pid)
                .ok_or(CapabilityError::Fatal)?
                .record_frame(va, new)
                .map_err(|_| CapabilityError::Fatal)?;
            return Err(CapabilityError::Fatal);
        }
        Err(error)
    }
}
