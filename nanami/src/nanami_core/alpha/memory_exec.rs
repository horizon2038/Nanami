//! Sealed private exec retirement: unmap leaves, bulk-clear Frame slots, then RAM.
use super::*;

impl Alpha {
    pub(super) fn drop_sealed_process_runtime_mappings(
        &mut self,
        pid: usize,
        target: crate::nanami_core::process::ProcessEntry,
    ) -> Result<(), CapabilityError> {
        if !target.capabilities_sealed
            || target.pid != pid
            || target.memory_owner != pid
            || self.processes.has_memory_peers(pid)
        {
            return Err(CapabilityError::IllegalOperation);
        }
        // The supervisor has already trapped/stopped this sole VM user. Do not
        // Suspend an accepted fault: exec's reply must still reach its resolver.
        self.invalidate_process_copy_mappings(pid)?;
        let pages = self
            .processes
            .vm_space_mut(pid)
            .ok_or(CapabilityError::InvalidArgument)?
            .backed_pages(0, usize::MAX);
        for (va, permissions) in pages {
            if permissions == 0
                && !self
                    .processes
                    .vm_space_mut(pid)
                    .is_some_and(|vm| vm.mapping_is_uncertain(va))
            {
                continue;
            }
            let frame = self
                .processes
                .vm_space_mut(pid)
                .and_then(|vm| vm.find_frame(va))
                .ok_or(CapabilityError::Fatal)?;
            if let Err(error) = arch::address_space::unmap(target.address_space, frame, va) {
                if let Some(vm) = self.processes.vm_space_mut(pid) {
                    vm.mark_mapping_uncertain(va);
                }
                return Err(error);
            }
        }
        // REVOKE clears each Node's contents but retains that Node itself.
        // Existing chunk descriptors/arena reservations remain reusable.
        let directory = process_frame_directory_descriptor(target.root_node);
        for chunk in self.processes.frame_chunk_indices(pid) {
            arch::node::revoke(directory, chunk)?;
        }
        // No RAM may be freed before all leaves and Frame aliases are gone.
        // Move records, do not decrement then re-register their references.
        self.processes
            .defer_all_active_physical_allocations_for_pid(pid);
        let retired = self
            .processes
            .reset_runtime_memory_for_exec(pid, 0, 0, USER_HEAP_LIMIT)?;
        debug_assert!(retired.is_empty());
        self.free_deferred_process_allocations(pid, None)
    }
}
