use libnanami::Word;

use a9n_abi::CapabilityError;

use super::{HardwareContext, LinuxSyscallContext};
use crate::state::OsPersonality;

const REG_X0: Word = 0;
const REG_X1: Word = 1;
const REG_X2: Word = 2;
const REG_X3: Word = 3;
const REG_X4: Word = 4;
const REG_X5: Word = 5;
pub const REG_RSP: Word = 31;
pub const REG_PC: Word = 32;
pub const REG_FS_BASE: Word = 34;

const REGISTER_COUNT_FOR_SYSCALL: Word = 6;
const REGISTER_COUNT_FULL: Word = 35;
const REGISTER_MESSAGE_BASE: Word = 3;

pub fn syscall_context_from_hardware_context(
    hardware_context: &HardwareContext,
    kernel_call_number: Word,
    program_counter: Word,
) -> LinuxSyscallContext {
    LinuxSyscallContext {
        number: kernel_call_number,
        args: [
            hardware_context[REG_X0 as usize],
            hardware_context[REG_X1 as usize],
            hardware_context[REG_X2 as usize],
            hardware_context[REG_X3 as usize],
            hardware_context[REG_X4 as usize],
            hardware_context[REG_X5 as usize],
        ],
        program_counter,
    }
}

pub fn configure_personality_syscall_reply(
    hardware_context: &mut HardwareContext,
    _context: LinuxSyscallContext,
    value: isize,
    _personality: OsPersonality,
) -> usize {
    hardware_context[REG_X0 as usize] = value as Word;
    1
}

pub fn read_syscall_context(
    pcb: Word,
    kernel_call_number: Word,
    program_counter: Word,
) -> Result<LinuxSyscallContext, CapabilityError> {
    a9n_abi::arch::process_control_block::read_register(pcb, REGISTER_COUNT_FOR_SYSCALL)?;
    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    Ok(LinuxSyscallContext {
        number: kernel_call_number,
        args: [
            ipc.get_message((REGISTER_MESSAGE_BASE + REG_X0) as usize),
            ipc.get_message((REGISTER_MESSAGE_BASE + REG_X1) as usize),
            ipc.get_message((REGISTER_MESSAGE_BASE + REG_X2) as usize),
            ipc.get_message((REGISTER_MESSAGE_BASE + REG_X3) as usize),
            ipc.get_message((REGISTER_MESSAGE_BASE + REG_X4) as usize),
            ipc.get_message((REGISTER_MESSAGE_BASE + REG_X5) as usize),
        ],
        program_counter,
    })
}

pub fn write_syscall_return(
    pcb: Word,
    _context: LinuxSyscallContext,
    value: isize,
) -> Result<(), ()> {
    a9n_abi::arch::process_control_block::read_register(pcb, 1).map_err(|_| ())?;
    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    ipc.configure_message(REGISTER_MESSAGE_BASE as usize, value as Word);
    a9n_abi::arch::process_control_block::write_register(pcb, 1).map_err(|_| ())
}

pub fn write_personality_syscall_return(
    pcb: Word,
    context: LinuxSyscallContext,
    value: isize,
    _personality: OsPersonality,
) -> Result<(), ()> {
    write_syscall_return(pcb, context, value)
}

pub fn write_register_value(pcb: Word, register_index: Word, value: Word) -> Result<(), ()> {
    if register_index == REG_PC {
        return configure_special_registers(pcb, Some(value), None, None);
    }
    if register_index == REG_RSP {
        return configure_special_registers(pcb, None, Some(value), None);
    }
    if register_index == REG_FS_BASE {
        return configure_special_registers(pcb, None, None, Some(value));
    }

    let count = register_index.checked_add(1).ok_or(())?;
    a9n_abi::arch::process_control_block::read_register(pcb, count).map_err(|_| ())?;
    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    ipc.configure_message((REGISTER_MESSAGE_BASE + register_index) as usize, value);
    a9n_abi::arch::process_control_block::write_register(pcb, count).map_err(|_| ())
}

pub fn write_exec_registers(
    pcb: Word,
    entry_point: Word,
    stack_pointer: Word,
    thread_pointer: Word,
    _secondary_thread_pointer: Word,
    x0: Word,
    x1: Word,
    x2: Word,
) -> Result<(), ()> {
    a9n_abi::arch::process_control_block::read_register(pcb, REGISTER_COUNT_FULL)
        .map_err(|_| ())?;
    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    for register in 0..REGISTER_COUNT_FULL {
        ipc.configure_message((REGISTER_MESSAGE_BASE + register) as usize, 0);
    }
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_X0) as usize, x0);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_X1) as usize, x1);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_X2) as usize, x2);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_RSP) as usize, stack_pointer);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_PC) as usize, entry_point);
    ipc.configure_message(
        (REGISTER_MESSAGE_BASE + REG_FS_BASE) as usize,
        thread_pointer,
    );
    a9n_abi::arch::process_control_block::write_register(pcb, REGISTER_COUNT_FULL)
        .map_err(|_| ())?;
    configure_special_registers(
        pcb,
        Some(entry_point),
        Some(stack_pointer),
        Some(thread_pointer),
    )
}

pub fn clone_registers_for_fork(
    parent_pcb: Word,
    child_pcb: Word,
    context: LinuxSyscallContext,
    child_stack: Word,
    child_thread_pointer: Word,
) -> Result<(), CapabilityError> {
    a9n_abi::arch::process_control_block::read_register(parent_pcb, REGISTER_COUNT_FULL)?;
    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_X0) as usize, 0);
    ipc.configure_message(
        (REGISTER_MESSAGE_BASE + REG_PC) as usize,
        context.program_counter,
    );
    if child_stack != 0 {
        ipc.configure_message((REGISTER_MESSAGE_BASE + REG_RSP) as usize, child_stack);
    }
    ipc.configure_message(
        (REGISTER_MESSAGE_BASE + REG_FS_BASE) as usize,
        child_thread_pointer,
    );
    a9n_abi::arch::process_control_block::write_register(child_pcb, REGISTER_COUNT_FULL)?;
    configure_special_registers(
        child_pcb,
        Some(context.program_counter),
        (child_stack != 0).then_some(child_stack),
        Some(child_thread_pointer),
    )
    .map_err(|_| CapabilityError::Fatal)
}

pub fn read_register_value(pcb: Word, register_index: Word) -> Result<Word, ()> {
    let count = register_index.checked_add(1).ok_or(())?;
    a9n_abi::arch::process_control_block::read_register(pcb, count).map_err(|_| ())?;
    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    Ok(ipc.get_message((REGISTER_MESSAGE_BASE + register_index) as usize))
}

fn configure_special_registers(
    pcb: Word,
    program_counter: Option<Word>,
    stack_pointer: Option<Word>,
    thread_pointer: Option<Word>,
) -> Result<(), ()> {
    let configuration = a9n_abi::capability_call::process_control_block::ConfigurationInfo::new(
        false,
        false,
        false,
        false,
        false,
        program_counter.is_some(),
        stack_pointer.is_some(),
        thread_pointer.is_some(),
        false,
        false,
    );
    a9n_abi::arch::process_control_block::configure(
        pcb,
        configuration,
        0,
        0,
        0,
        0,
        0,
        program_counter.unwrap_or(0),
        stack_pointer.unwrap_or(0),
        thread_pointer.unwrap_or(0),
        0,
        0,
    )
    .map_err(|_| ())
}
