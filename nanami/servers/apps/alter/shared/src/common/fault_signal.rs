use super::{
    configure_personality_syscall_reply, configure_trapped_syscall_reply, FaultAction,
    HardwareContext, Runtime, Word,
};

pub(super) fn signal_return(
    runtime: &mut Runtime,
    pid: Word,
    pcb: Word,
    context: crate::process::LinuxSyscallContext,
    value: isize,
    mut registers: HardwareContext,
    fast: bool,
    parked: bool,
) -> FaultAction {
    if !fast {
        registers = match crate::process::read_hardware_context(pcb) {
            Ok(registers) => registers,
            Err(()) => return FaultAction::Terminate { pid, status: 139 },
        };
    }
    // Prepare the same complete register values in either case. Only an
    // unchanged delivery may use the existing short trapped-syscall reply;
    // handlers still publish every register and parked tasks remain parked.
    let count = if fast {
        configure_trapped_syscall_reply(
            &mut registers, context, value, crate::state::OsPersonality::Linux,
        )
    } else {
        configure_personality_syscall_reply(
            &mut registers, context, value, crate::state::OsPersonality::Linux,
        )
    };
    match crate::linux::signals_runtime::deliver(
        runtime,
        pid,
        &mut registers,
        parked.then_some(context),
    ) {
        crate::linux::signals_runtime::Delivery::Unchanged => {
            if parked {
                return FaultAction::Park;
            }
            FaultAction::Continue {
                hardware_context: registers,
                hardware_context_count: count,
            }
        }
        crate::linux::signals_runtime::Delivery::Handler => FaultAction::Continue {
            hardware_context: registers,
            hardware_context_count: registers.len(),
        },
        crate::linux::signals_runtime::Delivery::Stopped => FaultAction::Park,
        crate::linux::signals_runtime::Delivery::Terminate(signal) => FaultAction::Terminate {
            pid,
            status: 128 + signal,
        },
    }
}
