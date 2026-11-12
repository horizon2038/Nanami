//! Sealed private exec: switch the sole PCB, retire old tables/caps, then RAM.
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
        self.processes.vm_space(pid).ok_or(CapabilityError::InvalidArgument)?;
        // Removing the root's AS capability alone is insufficient: the stopped
        // PCB still holds its old AS. Recreate/configure drops that last alias,
        // then successfully retires every old page-table owner. No runnable task
        // or privileged copy window can use the old leaves after this fence.
        // A failed switch/table retirement stops here with all RAM still owned.
        // Thus even an uncertain old PTE needs no individual UNMAP at exec.
        self.recreate_process_address_space_for_exec(target)?;
        // REVOKE clears each Node's contents but retains that Node itself.
        // Existing chunk descriptors/arena reservations remain reusable.
        let directory = process_frame_directory_descriptor(target.root_node);
        for chunk in self.processes.frame_chunk_indices(pid) {
            arch::node::revoke(directory, chunk)?;
        }
        // No RAM may be freed before the old AS/tables and Frame aliases retire.
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
