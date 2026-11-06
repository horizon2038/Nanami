//! Immutable owned RAM snapshots mapped privately into supervised sealed tasks.
use super::*;

impl Alpha {
    pub(super) fn handle_process_private_snapshot(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let descriptor = self.read_process_memory_into_static_buffer(
            request.identifier,
            request.arg3,
            2 * core::mem::size_of::<Word>(),
        )?;
        let [source, permissions] =
            unsafe { ptr::read_unaligned(descriptor.as_ptr() as *const [Word; 2]) };
        self.map_process_private_snapshot(
            request.identifier,
            request.arg0,
            source,
            request.arg1,
            request.arg2,
            permissions,
        )
    }

    pub(super) fn map_process_private_snapshot(
        &mut self,
        caller: usize,
        target: usize,
        source: usize,
        base: usize,
        bytes: usize,
        permissions: usize,
    ) -> Result<(), CapabilityError> {
        if !cfg!(any(target_arch = "x86_64", test)) {
            return Err(CapabilityError::IllegalOperation);
        }
        if caller == 0
            || caller == target
            || source == 0
            || base == 0
            || bytes == 0
            || (source | base | bytes) & (PAGE_SIZE - 1) != 0
            || (permissions != 1 && permissions != 5)
        {
            return Err(CapabilityError::InvalidArgument);
        }
        let source_end = source
            .checked_add(bytes)
            .ok_or(CapabilityError::InvalidArgument)?;
        let end = base
            .checked_add(bytes)
            .ok_or(CapabilityError::InvalidArgument)?;
        let owner = self
            .processes
            .find_entry_by_pid(caller)
            .filter(|entry| {
                !entry.exited && !entry.resource_quarantined && !entry.capabilities_sealed
            })
            .ok_or(CapabilityError::PermissionDenied)?;
        self.processes
            .find_entry_by_pid(target)
            .filter(|entry| {
                !entry.exited
                    && !entry.resource_quarantined
                    && entry.capabilities_sealed
                    && entry.reaper_pid == caller
            })
            .ok_or(CapabilityError::PermissionDenied)?;
        let source_memory = self
            .processes
            .memory_entry(owner.pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        let reservation = self
            .processes
            .shared_memory_containing_range(caller, source, bytes)
            .ok_or(CapabilityError::PermissionDenied)?;
        let reservation_end = reservation.base_va + reservation.page_count * PAGE_SIZE;
        // Every source page must already be read-only, including pages outside
        // this alias. Existing generic writable exports make an extent nonunique.
        {
            let vm = self
                .processes
                .vm_space_mut(caller)
                .ok_or(CapabilityError::InvalidArgument)?;
            if (reservation.base_va..reservation_end)
                .step_by(PAGE_SIZE)
                .any(|va| vm.permissions(va) != 1 || vm.find_frame(va).is_none())
            {
                return Err(CapabilityError::PermissionDenied);
            }
        }
        let target_vm = self
            .processes
            .vm_space_mut(target)
            .ok_or(CapabilityError::InvalidArgument)?;
        if !target_vm.backed_pages(base, end).is_empty()
            || !self
                .processes
                .physical_pages_in_range(target, base, end)
                .is_empty()
            || (base..end)
                .step_by(PAGE_SIZE)
                .any(|va| self.processes.find_lazy_mapping(target, va).is_some())
        {
            return Err(CapabilityError::IllegalOperation);
        }
        self.processes
            .canonicalize_shared_memory(caller, reservation)?;
        let references = self
            .processes
            .private_page_references(caller, source, source_end);
        if references.len() != bytes / PAGE_SIZE {
            return Err(CapabilityError::PermissionDenied);
        }
        for (offset, reference) in references.iter().enumerate() {
            let page = reference.allocation;
            let va = source + offset * PAGE_SIZE;
            let slot = reservation.start_slot + (va - reservation.base_va) / PAGE_SIZE;
            let count = self.processes.physical_reference_count(page);
            let pool = self
                .memory
                .physical_frame_descriptor_from_index(page.base_page)
                .ok_or(CapabilityError::PermissionDenied)?;
            let child = process_frame_descriptor(source_memory.root_node, slot);
            let vm = self
                .processes
                .vm_space_mut(caller)
                .ok_or(CapabilityError::InvalidArgument)?;
            if page.base_va != va
                || page.start_slot != slot
                || count == 0
                || (count != 1 && !vm.private_page_shared(va))
                || !vm
                    .find_frame(va)
                    .is_some_and(|frame| frame == pool || frame == child)
            {
                return Err(CapabilityError::PermissionDenied);
            }
        }
        let (root, space, first) = self.processes.reserve_process_heap_at(
            target,
            base,
            references.len(),
            PAGE_SIZE,
            PROCESS_FRAME_TOTAL_PAGES,
        )?;
        self.ensure_process_frame_chunks(target, root, first, references.len())?;
        // Do not fallback-copy after this point. Ownership/caps/PTEs may have
        // committed a prefix; ordinary private mapping release/root reap owns it.
        self.invalidate_process_copy_mappings(target)?;
        for (offset, reference) in references.into_iter().enumerate() {
            let page = reference.allocation;
            let va = base + offset * PAGE_SIZE;
            let slot = first + offset;
            self.processes
                .vm_space_mut(caller)
                .ok_or(CapabilityError::Fatal)?
                .mark_private_page_shared(page.base_va, true);
            self.processes
                .register_physical_allocation(target, va, slot, page.base_page, 1)?;
            arch::node::copy(
                process_frame_chunk_descriptor(root, slot / PROCESS_FRAME_CHUNK_PAGES),
                slot % PROCESS_FRAME_CHUNK_PAGES,
                process_frame_descriptor(source_memory.root_node, page.start_slot),
            )?;
            let frame = process_frame_descriptor(root, slot);
            let vm = self
                .processes
                .vm_space_mut(target)
                .ok_or(CapabilityError::Fatal)?;
            vm.mark_private_page_shared(va, true);
            if !vm.page_tables_ready(va) {
                self.memory.ensure_page_tables(space, va, vm)?;
            }
            // Record before MAP, including a possible post-install failure.
            vm.record_frame(va, frame)
                .map_err(|_| CapabilityError::Fatal)?;
            vm.set_permissions(va, permissions);
            let attributes = nun::capability_call::address_space::Attribute::from_bits(permissions)
                .ok_or(CapabilityError::InvalidArgument)?;
            arch::address_space::map(space, frame, va, attributes)?;
        }
        Ok(())
    }
}
