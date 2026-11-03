//! Separate a supervised task's executable namespace from Alpha's private
//! management root. No AddressSpace, Frame, PCB or dependency node is removed.
use super::*;

impl Alpha {
    pub(super) fn sealed_capability_root(
        &mut self,
    ) -> Result<CapabilityDescriptor, CapabilityError> {
        if let Some(root) = self.sealed_capability_root {
            return Ok(root);
        }
        // One permanently empty, Alpha-owned node serves every sealed task.
        // It is not a child of any process arena and cannot be reached from
        // a guest's implicit root. Reaping one task cannot reclaim this node.
        arch::generic::convert(
            self.root.bootstrap_generic,
            CapabilityType::Node,
            1,
            1,
            self.root.root_descriptor,
            SEALED_CAPABILITY_ROOT_SLOT,
        )?;
        let root = make_root_slot_descriptor(self.root.root_radix, SEALED_CAPABILITY_ROOT_SLOT);
        self.sealed_capability_root = Some(root);
        Ok(root)
    }

    pub(super) fn handle_process_seal_capabilities(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        if request.arg1 != 0 || request.arg2 != 0 || request.arg3 != 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let target = self
            .processes
            .find_entry_by_pid(request.arg0)
            .filter(|entry| {
                entry.pid != 0
                    && entry.pid != request.identifier
                    && !entry.exited
                    && entry.reaper_pid == request.identifier
            })
            .ok_or(CapabilityError::PermissionDenied)?;
        if target.capabilities_sealed {
            return Ok(());
        }
        // The ordinary SMP-safe suspend waits for a running remote CPU to stop.
        // Do not resume here: the supervisor still has to finish preparation.
        arch::process_control_block::suspend(target.pcb)?;
        let root = self.sealed_capability_root()?;
        let config = nun::capability_call::process_control_block::ConfigurationInfo::new(
            false, true, false, false, false, false, false, false, false, false,
        );
        arch::process_control_block::configure(
            target.pcb, config, 0, root, 0, 0, 0, 0, 0, 0, 0, 0,
        )?;
        self.processes.seal_capabilities(target.pid)
    }
}
