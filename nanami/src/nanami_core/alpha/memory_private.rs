//! Opt-in read-only private fork aliases. Writers detach under VM quiescence.
use super::*;
use crate::nanami_core::process::{PhysicalPageReference, ProcessMemoryContext, ProcessPhysicalAllocation};

#[cfg(test)]
static DETACH_CONTEXT_RESOLUTIONS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(super) fn take_detach_context_resolutions() -> usize {
    DETACH_CONTEXT_RESOLUTIONS.swap(0, core::sync::atomic::Ordering::Relaxed)
}

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
            .is_none_or(|vm| vm.has_backed_pages(0, usize::MAX))
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
        let vm = self
            .processes
            .vm_space_mut(source)
            .ok_or(CapabilityError::InvalidArgument)?;
        if vm.has_uncertain_mappings(0, usize::MAX) {
            return Err(CapabilityError::Fatal);
        }
        let pages = vm.backed_pages(0, usize::MAX);
        // One request-local ownership snapshot, never a list search per page.
        // Only nonzero read/execute permissions can share. When the whole VM
        // needs eager copies, skip the ownership snapshot and validation walk.
        let can_share = |permissions: usize| permissions != 0 && permissions & !5 == 0;
        let share_start = pages.iter().position(|&(_, permissions)| can_share(permissions))
            .unwrap_or(pages.len());
        let share_end = if share_start < pages.len() {
            pages.iter().rposition(|&(_, permissions)| can_share(permissions)).unwrap() + 1
        } else { share_start };
        let candidates = &pages[share_start..share_end];
        let has_candidates = !candidates.is_empty();
        let mut references = if has_candidates {
            // Only exact ownership records at candidate VAs can be shared.
            // Keep the original stable/newest selection inside this span,
            // without collecting writable heap/stack tails outside it.
            self.processes.private_page_references(
                source, candidates[0].0, candidates[candidates.len() - 1].0.saturating_add(1),
            )
        } else { Vec::new() };
        // Eligibility only reads metadata. Keep the current owner VM borrowed
        // for this pass; publication below still obtains mutable VMs per use.
        let source_vm = if has_candidates { self.processes.vm_space(source) } else { None };
        let mut reference_index = 0;
        let mut shared_count = 0;
        for &(va, permissions) in candidates {
            if !can_share(permissions) {
                continue;
            }
            // Both snapshots are in ascending VA order. Consume ownership
            // gaps once rather than restarting a search at every backed page.
            while reference_index < references.len()
                && references[reference_index].allocation.base_va < va
            {
                reference_index += 1;
            }
            let candidate = references.get(reference_index)
                .filter(|r| r.allocation.base_va == va).map(|_| reference_index);
            let eligible = if let Some(index) = candidate {
                let allocation = references[index].allocation;
                let count = self.processes.physical_reference_count(allocation);
                let vm = source_vm.ok_or(CapabilityError::InvalidArgument)?;
                let known_private_alias = vm.private_page_shared(va);
                let frame = vm.find_frame(va);
                let expected = if allocation.owns_frame_cap {
                    Some(process_frame_descriptor(source_memory.root_node, allocation.start_slot))
                } else { self.memory.physical_frame_descriptor_from_index(allocation.base_page) };
                count != 0
                    && (known_private_alias || count == 1)
                    && expected.is_some() && frame == expected
                    && self
                        .processes
                        .shared_memory_reservation(source, va, 1)
                        .is_none()
            } else {
                false
            };
            if eligible {
                // Only overwrite an already-consumed prefix: shared_count is
                // never greater than reference_index. Keep exact ownership
                // handles in VA order without a second per-page allocation.
                references[shared_count] = references[candidate.unwrap()];
                shared_count += 1;
            }
        }
        references.truncate(shared_count);
        // Both lists are VA ordered. The cursor stays on the next unconsumed
        // shared page through eager runs and advances only after a shared run.
        let mut shared_cursor = 0;
        let mut begin = 0;
        while begin < pages.len() {
            let share = references.get(shared_cursor)
                .is_some_and(|reference| reference.allocation.base_va == pages[begin].0);
            let mut end = begin + 1;
            while end < pages.len()
                && pages[end - 1].0.checked_add(PAGE_SIZE) == Some(pages[end].0)
                && references.get(shared_cursor + if share { end - begin } else { 0 })
                    .is_some_and(|reference| reference.allocation.base_va == pages[end].0) == share
                && (share || pages[end].1 == pages[begin].1)
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
                self.process_root_slot(root)?;
                for (offset, &(va, permissions)) in run.iter().enumerate() {
                    let allocation = references[shared_cursor + offset].allocation;
                    // Conservative markers survive a failed unpublished child;
                    // its references remain held until normal spawn rollback.
                    self.processes
                        .vm_space_mut(source)
                        .ok_or(CapabilityError::Fatal)?
                        .mark_private_page_shared(va, true);
                    self.processes.register_pool_alias(
                        target,
                        va,
                        first + offset,
                        allocation.base_page,
                    )?;
                    let frame = self.memory.physical_frame_descriptor_from_index(allocation.base_page)
                        .ok_or(CapabilityError::Fatal)?;
                    let vm = self
                        .processes
                        .vm_space_mut(target)
                        .ok_or(CapabilityError::Fatal)?;
                    vm.mark_private_page_shared(va, true);
                    vm.set_permissions(va, permissions);
                    if !vm.page_tables_ready(va) {
                        self.memory.ensure_page_tables(space, va, vm)?;
                    }
                    if offset == 0 || (va >> 12) & 63 == 0 { vm.prepare_frame_word(va, run.len() - offset); }
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
                let permissions = run[0].1;
                self.map_process_initialized_pages_protected(
                    target,
                    Some(base),
                    run.len(),
                    Some(PageSource {
                        pid: source,
                        address: base,
                        bytes: run.len() * PAGE_SIZE,
                    }),
                    if permissions == 0 { 7 } else { permissions },
                )?;
                for &(va, permissions) in run.iter().filter(|(_, permissions)| *permissions == 0) {
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
            if share { shared_cursor += run.len(); }
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
        // Only this request's owner/root/space identity is reused. Kernel
        // capability calls do not dispatch another Alpha process event.
        let mut memory_context = None;
        let mut index = 0;
        for va in pages {
            // shared_private_pages and references are sorted; references keep
            // their stable ownership handles across earlier detach commits.
            while index < references.len() && references[index].allocation.base_va < va {
                index += 1;
            }
            if references.get(index).is_none_or(|r| r.allocation.base_va != va) {
                return Err(CapabilityError::Fatal);
            }
            self.detach_private_page(pid, references[index], &mut memory_context)?;
        }
        Ok(0)
    }

    fn detach_private_page(
        &mut self,
        pid: usize,
        reference: PhysicalPageReference,
        context: &mut Option<ProcessMemoryContext>,
    ) -> Result<(), CapabilityError> {
        let old = reference.allocation;
        let count = self.processes.physical_reference_count(old);
        if count == 0 {
            return Err(CapabilityError::Fatal);
        }
        let memory = match *context {
            Some(memory) => memory,
            None => {
                #[cfg(test)]
                DETACH_CONTEXT_RESOLUTIONS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                let memory = self.processes.memory_context(pid)
                    .ok_or(CapabilityError::InvalidArgument)?;
                *context = Some(memory);
                memory
            }
        };
        let old_frame = if old.owns_frame_cap {
            process_frame_descriptor(memory.root_node, old.start_slot)
        } else {
            self.memory.physical_frame_descriptor_from_index(old.base_page)
                .ok_or(CapabilityError::Fatal)?
        };
        let vm = self
            .processes
            .vm_space_mut(pid)
            .ok_or(CapabilityError::Fatal)?;
        if vm.mapping_is_uncertain(old.base_va)
            || vm.find_frame(old.base_va) != Some(old_frame)
            || vm.permissions(old.base_va) & 2 != 0
        {
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
        let owner = self.processes.memory_owner_pid(pid);
        let pool_backing = self.processes.find_entry_by_pid(owner)
            .is_some_and(|entry| entry.capabilities_sealed);
        let physical = if pool_backing {
            self.process_root_slot(root)?;
            self.memory.allocate_physical_any(PAGE_SIZE)?
        } else {
            self.ensure_process_frame_chunks(pid, root, slot, 1)?;
            self.memory.allocate_process_frame(
                process_frame_chunk_descriptor(root, slot / PROCESS_FRAME_CHUNK_PAGES),
                PROCESS_FRAME_NODE_RADIX,
                slot % PROCESS_FRAME_CHUNK_PAGES,
            )?
        };
        let replacement = ProcessPhysicalAllocation {
            base_va: old.base_va,
            start_slot: slot,
            base_page: physical,
            page_count: 1,
            owns_frame_cap: !pool_backing,
        };
        // An unpublished replacement is still owned, including on any error.
        self.processes
            .defer_physical_allocation_for_pid(pid, replacement);
        if physical == old.base_page {
            return Err(CapabilityError::Fatal);
        }
        // Track even a failed/partial conversion before any fallible pool work.
        let new_frame = if pool_backing {
            self.memory.ensure_alpha_frame_at_physical_index(physical)?;
            self.memory.physical_frame_descriptor_from_index(physical)
                .ok_or(CapabilityError::Fatal)?
        } else { process_frame_descriptor(root, slot) };
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
        if let Err(error) = if old.owns_frame_cap { arch::node::remove(
            process_frame_chunk_descriptor(root, old.start_slot / PROCESS_FRAME_CHUNK_PAGES),
            old.start_slot % PROCESS_FRAME_CHUNK_PAGES,
        ) } else { Ok(()) } {
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
