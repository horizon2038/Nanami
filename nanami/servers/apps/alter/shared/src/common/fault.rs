use libnanami::Word;

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
        libnanami::println!(
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
        libnanami::println!(
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

    let fast_context = fault.hardware_context_count == libnanami::ipc::HARDWARE_CONTEXT_WORDS;
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
                libnanami::println!(
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
                libnanami::println!("[alter] register write failed pid={} pcb={:#x}", pid, pcb);
                return FaultAction::Terminate { pid, status: 1 };
            }
            FaultAction::Continue {
                hardware_context,
                hardware_context_count: 0,
            }
        }
        EmulationAction::Resume => FaultAction::Continue {
            hardware_context: fault.hardware_context,
            hardware_context_count: 0,
        },
        EmulationAction::Park => FaultAction::Park,
        EmulationAction::Exit(status) => FaultAction::Terminate { pid, status },
        EmulationAction::Unsupported(number) => {
            let enosys: isize = match process.personality {
                crate::state::OsPersonality::Linux => 38,
                crate::state::OsPersonality::FreeBsd => 78,
            };
            libnanami::println!(
                "[alter/{}] unsupported syscall {}, returning -ENOSYS",
                personality::name(process.personality),
                number
            );
            let mut hardware_context = fault.hardware_context;
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
