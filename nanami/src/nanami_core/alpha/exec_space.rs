//! A stopped shared task may replace its own AS alias, never its owner's VM.
use super::*;

const PRIVATE_EXEC_NODES: [(usize, usize); 4] = [
    (PROCESS_SLOT_L3_NODE, PAGE_TABLE_NODE_RADIX),
    (PROCESS_SLOT_L2_NODE, PAGE_TABLE_NODE_RADIX),
    (PROCESS_SLOT_L1_NODE, PAGE_TABLE_NODE_RADIX),
    (PROCESS_SLOT_FRAME_NODE, PROCESS_FRAME_DIRECTORY_RADIX),
];

impl Alpha {
    pub(super) fn prepare_shared_exec(
        &mut self,
        target: crate::nanami_core::process::ProcessEntry,
    ) -> Result<(), CapabilityError> {
        // Sealed raw shared tasks never received these nodes. Their private
        // management roots cannot be modified by the guest.
        if !target.capabilities_sealed || target.memory_owner == target.pid
            || PRIVATE_EXEC_NODES.iter().any(|(slot, _)| target.root_types.occupied(*slot))
        {
            return Err(CapabilityError::IllegalOperation);
        }
        self.processes.prepare_private_exec(target.pid)
    }

    pub(super) fn detach_shared_exec(
        &mut self,
        target: crate::nanami_core::process::ProcessEntry,
    ) -> Result<(), CapabilityError> {
        let arena = self.process_arena_for_root(target.root_node)?;
        // Retire Alpha's cached old-VM views before changing the child's owner.
        // The parent's leaves, Frames and RAM references remain untouched.
        self.invalidate_process_copy_mappings(target.pid)?;
        for (slot, radix) in PRIVATE_EXEC_NODES {
            arch::generic::convert(arena, CapabilityType::Node, radix, 1, target.root_node, slot)?;
            self.processes.root_types_mut(target.pid)?.other(slot);
        }
        self.recreate_process_address_space_for_exec(target)?;
        // The stopped PCB now holds the new AS. Until this point all kernel
        // failures leave metadata pointing at the still-live parent's VM.
        self.processes.commit_private_exec(target.pid)?;
        if let Some(owner) = self.processes.find_entry_by_pid(target.memory_owner) {
            if owner.deferred_vm_reap && !self.processes.has_memory_peers(owner.pid) {
                self.handle_process_reap_request(OsRequestEvent {
                    identifier: owner.reaper_pid, code: OS_REQUEST_PROCESS_REAP,
                    arg0: owner.pid, arg1: 0, arg2: 0, arg3: 0,
                })?;
            }
        }
        Ok(())
    }

    pub(super) fn recreate_process_address_space_for_exec(
        &mut self,
        target: crate::nanami_core::process::ProcessEntry,
    ) -> Result<(), CapabilityError> {
        arch::node::remove(target.root_node, PROCESS_SLOT_ADDRESS_SPACE as Word)?;
        arch::generic::convert(
            self.process_arena_for_root(target.root_node)?, CapabilityType::AddressSpace,
            0, 1, target.root_node, PROCESS_SLOT_ADDRESS_SPACE as Word,
        )?;
        let config = nun::capability_call::process_control_block::ConfigurationInfo::new(
            true, false, false, false, false, false, false, false, false, false,
        );
        arch::process_control_block::configure(
            target.pcb, config, target.address_space, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        )?;
        // Shared children have no tables under their independent AS descriptor.
        // Private exec releases its old tables after the PCB drops the old AS.
        self.memory.release_page_tables(target.address_space)
    }
}
