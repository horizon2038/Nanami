//! Common lazy-page construction after point or uniform-range preflight.
use super::*;

impl Alpha {
    /// Only physical/VM metadata changes until fill completes. The lazy plan
    /// remains stable throughout this synchronous, non-reentrant construction.
    pub(super) fn materialize_lazy_page_from_plan(
        &mut self,
        pid: usize,
        process_root: CapabilityDescriptor,
        address_space: CapabilityDescriptor,
        page_va: usize,
        initial_permissions: usize,
        prepared_pool: bool,
        plan: (usize, usize, usize),
        batch: Option<&mut memory_pages::PreinstallPoolBatch>,
    ) -> Result<CapabilityDescriptor, CapabilityError> {
        let (mapping_index, mapping_base, mapping_slot) = plan;
        let page_offset = (page_va - mapping_base) / PAGE_SIZE;
        let frame_slot = mapping_slot + page_offset;
        let (base_page, frame) = if let Some(batch) = batch {
            if !prepared_pool || !matches!(self.processes.lazy_mapping_kind(mapping_index),
                Some(ProcessLazyMappingKind::Image { elf, .. }) if elf.ipc_buffer_start.is_none())
            {
                return Err(CapabilityError::InvalidArgument);
            }
            self.claim_preinstall_pool_frame(pid, process_root, page_va, frame_slot, batch)?
        } else if prepared_pool {
            self.allocate_preinstall_pool_frame(pid, process_root, page_va, frame_slot)?
        } else {
            let base_page = self.allocate_process_frame(pid, process_root, frame_slot)?;
            self.processes.register_physical_allocation(pid, page_va, frame_slot, base_page, 1)?;
            (base_page, process_frame_descriptor(process_root, frame_slot))
        };
        {
            let vm = self
                .processes
                .vm_space_mut(pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            let attributes = nun::capability_call::address_space::Attribute::from_bits(initial_permissions)
                .ok_or(CapabilityError::InvalidArgument)?;
            self.memory.map_frame_with_attribute(address_space, frame, page_va, attributes, vm)?;
            vm.set_permissions(page_va, initial_permissions);
        }

        let temp_va = self.map_initialization_page(base_page)?;
        // Allocation/Frame registration/VM mapping above do not modify the
        // lazy mapping list, and no reentrant dispatch occurs. Reborrow only
        // now: no mapping reference survives those mutable Alpha operations.
        let kind = self.processes.lazy_mapping_kind(mapping_index)
            .ok_or(CapabilityError::Fatal)?;
        fill_lazy_page(kind, page_va, temp_va)?;
        Ok(frame)
    }
}
