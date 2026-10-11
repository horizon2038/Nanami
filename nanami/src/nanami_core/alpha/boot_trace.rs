//! Opt-in, read-only samples of the last saved user context, not live CPU state.
use super::*;

impl Alpha {
    pub(super) fn trace_boot_processes(&self, requester: usize) -> Result<(), CapabilityError> {
        if self
            .communication
            .resolve_service_with_owner("usb-service")
            .map(|(_, pid)| pid)
            != Some(requester)
        {
            return Err(CapabilityError::PermissionDenied);
        }
        for name in ["vfs-service", "timer-service", "exec-service"] {
            let Some((_, pid)) = self.communication.resolve_service_with_owner(name) else {
                continue;
            };
            let Some(entry) = self.processes.find_entry_by_pid(pid) else {
                continue;
            };
            // Neither suspend nor resume the target. Do not write its registers.
            if let Err(error) = arch::process_control_block::read_register(entry.pcb, 22) {
                crate::force_info!(
                    "[boot.pc] service={} pid={} read failed: {:?}",
                    name, pid, error
                );
                continue;
            }
            let ipc = arch::ipc_buffer::get_ipc_buffer();
            // Copy before printing: debug calls may use the same message registers.
            let rax = ipc.get_message(3);
            let rdi = ipc.get_message(3 + 4);
            let rip = ipc.get_message(3 + 15);
            let flags = ipc.get_message(3 + 17);
            let rsp = ipc.get_message(3 + 18);
            crate::force_info!(
                "[boot.pc] service={} pid={} core={} exited={} saved-rip={:#x} rsp={:#x} flags={:#x} rax={:#x} rdi={:#x}",
                name, pid, entry.affinity, entry.exited, rip, rsp, flags, rax, rdi
            );
        }
        Ok(())
    }
}
