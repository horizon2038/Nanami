use nanami_runtime::Word;

use crate::abi::A9N_FAULT_INVALID_KERNEL_CALL;
use crate::linux::EmulationAction;
use crate::personality;
use crate::process::{
    configure_personality_syscall_reply, read_syscall_context,
    syscall_context_from_hardware_context, write_personality_syscall_return, HardwareContext,
};
use crate::state::Runtime;

pub struct FaultEvent {
    pub identifier: Word,
    pub reason: Word,
    pub program_counter: Word,
    pub kernel_call_number: Word,
    pub hardware_context: HardwareContext,
    pub hardware_context_count: usize,
}

pub enum FaultAction {
    Continue {
        hardware_context: HardwareContext,
        hardware_context_count: usize,
    },
    Park,
    Terminate {
        pid: Word,
        status: Word,
    },
}

pub fn handle_fault(runtime: &mut Runtime, fault: FaultEvent) -> FaultAction {
    if fault.reason != A9N_FAULT_INVALID_KERNEL_CALL {
        if runtime
            .process_for_fault_identifier(fault.identifier)
            .is_some_and(|process| process.personality == crate::state::OsPersonality::Linux)
        {
            return synchronous_signal(runtime, &fault);
        }
        nanami_runtime::println!(
            "[alter] unsupported fault pid={} reason={:#x} pc={:#x}",
            fault.identifier,
            fault.reason,
            fault.program_counter
        );
        return FaultAction::Terminate {
            pid: fault.identifier,
            status: 1,
        };
    }

    let Some(process) = runtime.process_for_fault_identifier(fault.identifier) else {
        nanami_runtime::println!(
            "[alter] fault from unmanaged pid={} syscall={}",
            fault.identifier,
            fault.kernel_call_number
        );

        return FaultAction::Terminate {
            pid: fault.identifier,
            status: 1,
        };
    };
    let pid = process.pid;
    let pcb = process.pcb;

    let fast_context = fault.hardware_context_count == nanami_runtime::ipc::HARDWARE_CONTEXT_WORDS;
    let context = if fast_context {
        syscall_context_from_hardware_context(
            &fault.hardware_context,
            fault.kernel_call_number,
            fault.program_counter,
        )
    } else {
        match read_syscall_context(pcb, fault.kernel_call_number, fault.program_counter) {
            Ok(context) => context,
            Err(error) => {
                nanami_runtime::println!(
                    "[alter] register read failed pid={} pcb={:#x} err={:?}",
                    pid,
                    pcb,
                    error
                );
                return FaultAction::Terminate { pid, status: 1 };
            }
        }
    };

    let action = personality::dispatch_syscall(process.personality, runtime, pid, context);

    match action {
        EmulationAction::Return(value) => {
            let mut hardware_context = fault.hardware_context;
            if process.personality == crate::state::OsPersonality::Linux && runtime.signals_pending
            {
                return signal_return(
                    runtime,
                    pid,
                    pcb,
                    context,
                    value,
                    hardware_context,
                    fast_context,
                    false,
                );
            }
            if fast_context {
                let hardware_context_count = configure_personality_syscall_reply(
                    &mut hardware_context,
                    context,
                    value,
                    process.personality,
                );
                return FaultAction::Continue {
                    hardware_context,
                    hardware_context_count,
                };
            }
            if write_personality_syscall_return(pcb, context, value, process.personality).is_err() {
                nanami_runtime::println!(
                    "[alter] register write failed pid={} pcb={:#x}",
                    pid,
                    pcb
                );
                return FaultAction::Terminate { pid, status: 1 };
            }
            FaultAction::Continue {
                hardware_context,
                hardware_context_count: 0,
            }
        }
        EmulationAction::Resume => {
            if process.personality == crate::state::OsPersonality::Linux && runtime.signals_pending
            {
                let Ok(mut registers) = crate::process::read_hardware_context(pcb) else {
                    return FaultAction::Terminate { pid, status: 139 };
                };
                return deliver_reply(runtime, pid, &mut registers);
            }
            FaultAction::Continue {
                hardware_context: fault.hardware_context,
                hardware_context_count: 0,
            }
        }
        EmulationAction::RestoreSignalFrame => {
            let mut registers = if fast_context {
                fault.hardware_context
            } else {
                match crate::process::read_hardware_context(pcb) {
                    Ok(registers) => registers,
                    Err(()) => return FaultAction::Terminate { pid, status: 139 },
                }
            };
            if crate::linux::signals_runtime::restore(runtime, pid, &mut registers).is_err() {
                if let Some(process) = runtime.managed_process_mut(pid) {
                    process.signals_mut().exit_signal = 11;
                }
                return FaultAction::Terminate { pid, status: 139 };
            }
            deliver_reply(runtime, pid, &mut registers)
        }
        EmulationAction::Park => {
            if process.personality == crate::state::OsPersonality::Linux && runtime.signals_pending
            {
                return signal_return(
                    runtime,
                    pid,
                    pcb,
                    context,
                    -4,
                    fault.hardware_context,
                    fast_context,
                    true,
                );
            }
            FaultAction::Park
        }
        EmulationAction::Exit(status) => FaultAction::Terminate { pid, status },
        EmulationAction::Unsupported(number) => {
            let enosys: isize = match process.personality {
                crate::state::OsPersonality::Linux => 38,
                crate::state::OsPersonality::FreeBsd => 78,
            };
            nanami_runtime::println!(
                "[alter/{}] unsupported syscall {}, returning -ENOSYS",
                personality::name(process.personality),
                number
            );
            let mut hardware_context = fault.hardware_context;
            if process.personality == crate::state::OsPersonality::Linux && runtime.signals_pending
            {
                return signal_return(
                    runtime,
                    pid,
                    pcb,
                    context,
                    -enosys,
                    hardware_context,
                    fast_context,
                    false,
                );
            }
            if fast_context {
                let hardware_context_count = configure_personality_syscall_reply(
                    &mut hardware_context,
                    context,
                    -enosys,
                    process.personality,
                );
                return FaultAction::Continue {
                    hardware_context,
                    hardware_context_count,
                };
            }
            if write_personality_syscall_return(pcb, context, -enosys, process.personality).is_err()
            {
                return FaultAction::Terminate { pid, status: 1 };
            }
            FaultAction::Continue {
                hardware_context,
                hardware_context_count: 0,
            }
        }
    }
}

fn synchronous_signal(runtime: &mut Runtime, fault: &FaultEvent) -> FaultAction {
    let process = runtime
        .process_for_fault_identifier(fault.identifier)
        .unwrap();
    let pid = process.pid;
    let signal = match fault.reason {
        1 | 2 if crate::linux::bus_fault(runtime, pid, fault.kernel_call_number) => 7,
        1 | 2 => 11,
        3 => 4,
        4 => 8,
        _ => 11,
    };
    let state = runtime.managed_process_mut(pid).unwrap().signals_mut();
    // Synchronous faults cannot be left pending forever behind a blocked/ignored action.
    if state.blocked & crate::common::signals::bit(signal) != 0 || state.action(signal).handler == 1
    {
        state.exit_signal = signal;
        return FaultAction::Terminate {
            pid,
            status: 128 + signal,
        };
    }
    let address = if matches!(fault.reason, 1 | 2) {
        fault.kernel_call_number
    } else {
        fault.program_counter
    };
    let code = if signal == 7 || (signal == 11 && runtime.mapping_at(pid, address).is_some()) {
        2
    } else {
        1
    };
    let info = crate::common::signals::Info::fault(signal, code, address);
    let _ = crate::linux::signals_runtime::queue(runtime, pid, info);
    let Ok(mut registers) = crate::process::read_hardware_context(process.pcb) else {
        return FaultAction::Terminate {
            pid,
            status: 128 + signal,
        };
    };
    match crate::linux::signals_runtime::deliver(runtime, pid, &mut registers, None) {
        crate::linux::signals_runtime::Delivery::Handler => {
            // A9N's inline fault-reply context applies only to INVALID_KERNEL_CALL.
            if crate::process::write_hardware_context(process.pcb, &registers).is_ok() {
                return FaultAction::Continue {
                    hardware_context: registers,
                    hardware_context_count: 0,
                };
            }
        }
        crate::linux::signals_runtime::Delivery::Terminate(signal) => {
            return FaultAction::Terminate {
                pid,
                status: 128 + signal,
            };
        }
        _ => {}
    }
    runtime
        .managed_process_mut(pid)
        .unwrap()
        .signals_mut()
        .exit_signal = signal;
    FaultAction::Terminate {
        pid,
        status: 128 + signal,
    }
}

fn signal_return(
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
    let count = configure_personality_syscall_reply(
        &mut registers,
        context,
        value,
        crate::state::OsPersonality::Linux,
    );
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

fn deliver_reply(runtime: &mut Runtime, pid: Word, registers: &mut HardwareContext) -> FaultAction {
    match crate::linux::signals_runtime::deliver(runtime, pid, registers, None) {
        crate::linux::signals_runtime::Delivery::Stopped => FaultAction::Park,
        crate::linux::signals_runtime::Delivery::Terminate(signal) => FaultAction::Terminate {
            pid,
            status: 128 + signal,
        },
        _ => FaultAction::Continue {
            hardware_context: *registers,
            hardware_context_count: registers.len(),
        },
    }
}
