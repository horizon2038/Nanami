//! Raw tasks share a VM, not capability roots or PCB ownership. The original
//! frame arena remains alive until the last task using the VM has been reaped.
use super::*;

impl Alpha {
    pub(super) fn handle_thread_create(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<Word, CapabilityError> {
        let source = self
            .processes
            .find_entry_by_pid(request.arg0)
            .filter(|entry| !entry.exited && entry.reaper_pid == request.identifier)
            .ok_or(CapabilityError::PermissionDenied)?;
        let caller = self
            .processes
            .find_entry_by_pid(request.identifier)
            .filter(|entry| !entry.exited && entry.pid != 0)
            .ok_or(CapabilityError::PermissionDenied)?;
        let slot = request.arg1;
        if request.arg3 != 0
            || request.arg2 > 31
            || slot == 0
            || slot >= (1 << PROCESS_ROOT_RADIX)
            || (20..=22).contains(&slot)
        {
            return Err(CapabilityError::InvalidArgument);
        }
        self.validate_legacy_destination(caller.pid, slot)?;
        let (pid, root_slot) = self.processes.alloc_process_slot()?;
        let result = self.create_shared_vm_task(pid, root_slot, source, caller, slot, request.arg2);
        if result.is_err() {
            // No mapping belongs to this task: only its root and PCB are disposable.
            self.cleanup_failed_spawn(pid, root_slot);
        }
        result.map(|()| pid)
    }

    fn create_shared_vm_task(
        &mut self,
        pid: Word,
        root_slot: usize,
        source: crate::nanami_core::process::ProcessEntry,
        caller: crate::nanami_core::process::ProcessEntry,
        destination: usize,
        priority: Word,
    ) -> Result<(), CapabilityError> {
        let root = make_root_slot_descriptor(self.root.root_radix, root_slot);
        let generic = self.memory.ensure_process_arena(root_slot)?;
        arch::generic::convert(
            generic,
            CapabilityType::Node,
            PROCESS_ROOT_RADIX as Word,
            1,
            self.root.root_descriptor,
            root_slot,
        )?;
        arch::generic::convert(
            generic,
            CapabilityType::ProcessControlBlock,
            14,
            1,
            root,
            PROCESS_SLOT_PCB,
        )?;
        arch::node::copy(root, PROCESS_SLOT_ADDRESS_SPACE, source.address_space)?;
        let resolver = make_child_slot_descriptor(
            caller.root_node,
            PROCESS_ROOT_RADIX,
            PROCESS_SLOT_SERVICE_PORT,
        );
        arch::node::copy(root, PROCESS_SLOT_FAULT_RESOLVER, resolver)?;
        let fault_port =
            make_child_slot_descriptor(root, PROCESS_ROOT_RADIX, PROCESS_SLOT_FAULT_RESOLVER);
        arch::ipc_port::identify(fault_port, pid)?;
        arch::node::demote(root, PROCESS_SLOT_FAULT_RESOLVER, IDENTIFIED_IPC_RIGHTS)?;
        let pcb = make_child_slot_descriptor(root, PROCESS_ROOT_RADIX, PROCESS_SLOT_PCB);
        let address_space =
            make_child_slot_descriptor(root, PROCESS_ROOT_RADIX, PROCESS_SLOT_ADDRESS_SPACE);
        let config = nun::capability_call::process_control_block::ConfigurationInfo::new(
            true, true, false, false, true, false, false, false, true, false,
        );
        arch::process_control_block::configure(
            pcb,
            config,
            address_space,
            root,
            0,
            0,
            fault_port,
            0,
            0,
            0,
            priority,
            0,
        )?;
        let affinity = self.configure_process_affinity(pcb, None)?;
        self.processes.install_process(
            pid,
            caller.pid,
            root_slot,
            root,
            pcb,
            address_space,
            0,
            pid,
            affinity,
            0,
            0,
            0,
        )?;
        self.processes.share_address_space(pid, source.pid)?;
        let types = self.processes.root_types_mut(pid)?;
        *types = crate::nanami_core::process::RootSlotTypes::empty();
        types.other(PROCESS_SLOT_PCB);
        types.other(PROCESS_SLOT_ADDRESS_SPACE);
        types.ipc(PROCESS_SLOT_FAULT_RESOLVER);
        self.processes
            .root_types_mut(caller.pid)?
            .clear(destination);
        let _ = arch::node::remove(caller.root_node, destination);
        arch::node::copy(caller.root_node, destination, pcb)?;
        self.processes
            .root_types_mut(caller.pid)?
            .other(destination);
        Ok(())
    }
}
