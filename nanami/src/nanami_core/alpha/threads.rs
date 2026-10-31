//! Suspended raw tasks: shared VM for threads, private snapshot for fork.
//! Roots and PCBs are independent; shared frame arenas outlive their last task.
use super::*;

impl Alpha {
    pub(super) fn handle_thread_create(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<Word, CapabilityError> {
        self.handle_raw_task_create(request, true)
    }

    pub(super) fn handle_raw_task_create(
        &mut self,
        request: OsRequestEvent,
        shared_vm: bool,
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
        let result = self.create_raw_task(
            pid,
            root_slot,
            source,
            caller,
            slot,
            request.arg2,
            shared_vm,
        );
        if result.is_err() {
            self.cleanup_failed_spawn(pid, root_slot);
        }
        result.map(|()| pid)
    }

    fn create_raw_task(
        &mut self,
        pid: Word,
        root_slot: usize,
        source: crate::nanami_core::process::ProcessEntry,
        caller: crate::nanami_core::process::ProcessEntry,
        destination: usize,
        priority: Word,
        shared_vm: bool,
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
        if shared_vm {
            arch::node::copy(root, PROCESS_SLOT_ADDRESS_SPACE, source.address_space)?;
        } else {
            arch::generic::convert(
                generic,
                CapabilityType::AddressSpace,
                0,
                1,
                root,
                PROCESS_SLOT_ADDRESS_SPACE,
            )?;
            for slot in [
                PROCESS_SLOT_L3_NODE,
                PROCESS_SLOT_L2_NODE,
                PROCESS_SLOT_L1_NODE,
            ] {
                arch::generic::convert(
                    generic,
                    CapabilityType::Node,
                    PAGE_TABLE_NODE_RADIX,
                    1,
                    root,
                    slot,
                )?;
            }
            arch::generic::convert(
                generic,
                CapabilityType::Node,
                PROCESS_FRAME_DIRECTORY_RADIX,
                1,
                root,
                PROCESS_SLOT_FRAME_NODE,
            )?;
        }
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
        let memory = self
            .processes
            .memory_entry(source.pid)
            .ok_or(CapabilityError::InvalidArgument)?;
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
            memory.user_heap_next_va,
            memory.user_heap_limit_va,
        )?;
        if shared_vm {
            self.processes.share_address_space(pid, source.pid)?;
        } else {
            self.processes.ensure_vm_space_for_pid(pid)?;
            // Walk only backed pages, including retained PROT_NONE backing.
            // No ELF parsing, pathname lookup, initial stack or zero-fill pass.
            self.clone_process_vm(source.pid, pid)?;
        }
        let types = self.processes.root_types_mut(pid)?;
        *types = crate::nanami_core::process::RootSlotTypes::empty();
        types.other(PROCESS_SLOT_PCB);
        types.other(PROCESS_SLOT_ADDRESS_SPACE);
        if !shared_vm {
            for slot in [
                PROCESS_SLOT_L3_NODE,
                PROCESS_SLOT_L2_NODE,
                PROCESS_SLOT_L1_NODE,
                PROCESS_SLOT_FRAME_NODE,
            ] {
                types.other(slot);
            }
        }
        types.ipc(PROCESS_SLOT_FAULT_RESOLVER);
        self.processes
            .root_types_mut(caller.pid)?
            .clear(destination);
        let _ = arch::node::remove(caller.root_node, destination);
        arch::node::copy(caller.root_node, destination, pcb)?;
        self.processes
            .root_types_mut(caller.pid)?
            .other(destination);
        if !shared_vm {
            crate::force_info!(
                "[smp] process pid={:>3} image=fork-vm affinity={} priority={}",
                pid, affinity, priority
            );
        }
        Ok(())
    }
}
