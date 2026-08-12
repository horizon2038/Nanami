use libnanami::Word;

use a9n_abi::CapabilityError;

use crate::state::OsPersonality;

pub type HardwareContext = [Word; libnanami::ipc::HARDWARE_CONTEXT_WORDS];

pub const REG_RAX: Word = 0;
pub const REG_RBX: Word = 1;
pub const REG_RCX: Word = 2;
pub const REG_RDX: Word = 3;
pub const REG_RDI: Word = 4;
pub const REG_RSI: Word = 5;
pub const REG_RBP: Word = 6;
pub const REG_R8: Word = 7;
pub const REG_R9: Word = 8;
pub const REG_R10: Word = 9;
pub const REG_R11: Word = 10;
pub const REG_R12: Word = 11;
pub const REG_R13: Word = 12;
pub const REG_R14: Word = 13;
pub const REG_R15: Word = 14;
pub const REG_RIP: Word = 15;
pub const REG_RFLAGS: Word = 17;
pub const REG_RSP: Word = 18;
pub const REG_GS_BASE: Word = 20;
pub const REG_FS_BASE: Word = 21;

pub const REGISTER_COUNT_FOR_SYSCALL: Word = 16;
pub const REGISTER_COUNT_FOR_FREEBSD_SYSCALL: Word = 18;
pub const REGISTER_COUNT_FULL: Word = 22;

#[derive(Clone, Copy)]
pub struct LinuxSyscallContext {
    pub number: Word,
    pub args: [Word; 6],
    pub program_counter: Word,
}

impl LinuxSyscallContext {
    pub const EMPTY: Self = Self {
        number: 0,
        args: [0; 6],
        program_counter: 0,
    };
}

pub fn syscall_context_from_hardware_context(
    hardware_context: &HardwareContext,
    kernel_call_number: Word,
    program_counter: Word,
) -> LinuxSyscallContext {
    LinuxSyscallContext {
        number: kernel_call_number,
        args: [
            hardware_context[REG_RDI as usize],
            hardware_context[REG_RSI as usize],
            hardware_context[REG_RDX as usize],
            hardware_context[REG_R10 as usize],
            hardware_context[REG_R8 as usize],
            hardware_context[REG_R9 as usize],
        ],
        program_counter,
    }
}

pub fn configure_personality_syscall_reply(
    hardware_context: &mut HardwareContext,
    context: LinuxSyscallContext,
    value: isize,
    personality: OsPersonality,
) -> usize {
    hardware_context[REG_RIP as usize] = context.program_counter;
    match personality {
        OsPersonality::Linux => {
            hardware_context[REG_RAX as usize] = value as Word;
            REGISTER_COUNT_FOR_SYSCALL as usize
        }
        OsPersonality::FreeBsd => {
            let mut rflags = hardware_context[REG_RFLAGS as usize];
            if value < 0 {
                hardware_context[REG_RAX as usize] = (-value) as Word;
                rflags |= 1;
            } else {
                hardware_context[REG_RAX as usize] = value as Word;
                rflags &= !1;
            }
            hardware_context[REG_RFLAGS as usize] = rflags;
            REGISTER_COUNT_FOR_FREEBSD_SYSCALL as usize
        }
    }
}

pub fn read_syscall_context(
    pcb: Word,
    kernel_call_number: Word,
    program_counter: Word,
) -> Result<LinuxSyscallContext, CapabilityError> {
    a9n_abi::arch::process_control_block::read_register(pcb, REGISTER_COUNT_FOR_SYSCALL)
        .map_err(|e| e)?;
    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    Ok(LinuxSyscallContext {
        number: kernel_call_number,
        args: [
            ipc.get_message((REGISTER_MESSAGE_BASE + REG_RDI) as usize),
            ipc.get_message((REGISTER_MESSAGE_BASE + REG_RSI) as usize),
            ipc.get_message((REGISTER_MESSAGE_BASE + REG_RDX) as usize),
            ipc.get_message((REGISTER_MESSAGE_BASE + REG_R10) as usize),
            ipc.get_message((REGISTER_MESSAGE_BASE + REG_R8) as usize),
            ipc.get_message((REGISTER_MESSAGE_BASE + REG_R9) as usize),
        ],
        program_counter,
    })
}

pub fn write_syscall_return(
    pcb: Word,
    context: LinuxSyscallContext,
    value: isize,
) -> Result<(), ()> {
    a9n_abi::arch::process_control_block::read_register(pcb, REGISTER_COUNT_FOR_SYSCALL)
        .map_err(|_| ())?;
    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_RAX) as usize, value as Word);
    ipc.configure_message(
        (REGISTER_MESSAGE_BASE + REG_RIP) as usize,
        context.program_counter,
    );
    a9n_abi::arch::process_control_block::write_register(pcb, REGISTER_COUNT_FOR_SYSCALL)
        .map_err(|_| ())
}

pub fn write_personality_syscall_return(
    pcb: Word,
    context: LinuxSyscallContext,
    value: isize,
    personality: OsPersonality,
) -> Result<(), ()> {
    match personality {
        OsPersonality::Linux => write_syscall_return(pcb, context, value),
        OsPersonality::FreeBsd => write_freebsd_syscall_return(pcb, context, value),
    }
}

fn write_freebsd_syscall_return(
    pcb: Word,
    context: LinuxSyscallContext,
    value: isize,
) -> Result<(), ()> {
    a9n_abi::arch::process_control_block::read_register(pcb, REGISTER_COUNT_FULL)
        .map_err(|_| ())?;
    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    let mut rflags = ipc.get_message((REGISTER_MESSAGE_BASE + REG_RFLAGS) as usize);
    if value < 0 {
        ipc.configure_message((REGISTER_MESSAGE_BASE + REG_RAX) as usize, (-value) as Word);
        rflags |= 1;
    } else {
        ipc.configure_message((REGISTER_MESSAGE_BASE + REG_RAX) as usize, value as Word);
        rflags &= !1;
    }
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_RFLAGS) as usize, rflags);
    ipc.configure_message(
        (REGISTER_MESSAGE_BASE + REG_RIP) as usize,
        context.program_counter,
    );
    a9n_abi::arch::process_control_block::write_register(pcb, REGISTER_COUNT_FULL).map_err(|_| ())
}

pub fn write_register_value(pcb: Word, register_index: Word, value: Word) -> Result<(), ()> {
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
    fs_base: Word,
    gs_base: Word,
    rdi: Word,
    rsi: Word,
    rdx: Word,
) -> Result<(), ()> {
    a9n_abi::arch::process_control_block::read_register(pcb, REGISTER_COUNT_FULL)
        .map_err(|_| ())?;
    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_RAX) as usize, 0);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_RBX) as usize, 0);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_RCX) as usize, 0);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_RDX) as usize, rdx);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_RDI) as usize, rdi);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_RSI) as usize, rsi);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_RBP) as usize, 0);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_R8) as usize, 0);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_R9) as usize, 0);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_R10) as usize, 0);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_R11) as usize, 0);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_R12) as usize, 0);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_R13) as usize, 0);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_R14) as usize, 0);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_R15) as usize, 0);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_RIP) as usize, entry_point);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_RFLAGS) as usize, 0x202);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_RSP) as usize, stack_pointer);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_GS_BASE) as usize, gs_base);
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_FS_BASE) as usize, fs_base);
    a9n_abi::arch::process_control_block::write_register(pcb, REGISTER_COUNT_FULL).map_err(|_| ())
}

pub fn clone_registers_for_fork(
    parent_pcb: Word,
    child_pcb: Word,
    context: LinuxSyscallContext,
    child_stack: Word,
    child_fs_base: Word,
) -> Result<(), CapabilityError> {
    a9n_abi::arch::process_control_block::read_register(parent_pcb, REGISTER_COUNT_FULL)?;
    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    ipc.configure_message((REGISTER_MESSAGE_BASE + REG_RAX) as usize, 0);
    ipc.configure_message(
        (REGISTER_MESSAGE_BASE + REG_RIP) as usize,
        context.program_counter,
    );
    if child_stack != 0 {
        ipc.configure_message((REGISTER_MESSAGE_BASE + REG_RSP) as usize, child_stack);
    }
    ipc.configure_message(
        (REGISTER_MESSAGE_BASE + REG_FS_BASE) as usize,
        child_fs_base,
    );
    a9n_abi::arch::process_control_block::write_register(child_pcb, REGISTER_COUNT_FULL)
}

pub fn read_register_value(pcb: Word, register_index: Word) -> Result<Word, ()> {
    let count = register_index.checked_add(1).ok_or(())?;
    a9n_abi::arch::process_control_block::read_register(pcb, count).map_err(|_| ())?;
    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    Ok(ipc.get_message((REGISTER_MESSAGE_BASE + register_index) as usize))
}

const REGISTER_MESSAGE_BASE: Word = 3;
