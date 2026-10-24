use super::{get, put, user_address};

// siginfo[128] + ucontext: flags/link/stack, 1024-bit mask, aligned sigcontext.
pub const UC_OFFSET: usize = 128;
pub const STACK_OFFSET: usize = UC_OFFSET + 16;
pub const MASK_OFFSET: usize = UC_OFFSET + 40;
pub const MCONTEXT_OFFSET: usize = UC_OFFSET + 176;
const RESERVED_OFFSET: usize = MCONTEXT_OFFSET + 288;
pub const FRAME_BYTES: usize = RESERVED_OFFSET + 4096;
pub const INFO_OFFSET: usize = 0;
pub const SP: usize = 31;
pub const PC: usize = 32;
pub const MINSIGSTKSZ: usize = 5120;
// mov x8, #139; svc #0
pub const TRAMPOLINE: &[u8] = &[0x68, 0x11, 0x80, 0xd2, 0x01, 0, 0, 0xd4];

pub fn frame_address(stack_top: usize) -> Option<usize> {
    stack_top.checked_sub(FRAME_BYTES).map(|sp| sp & !15)
}

pub fn return_frame_address(registers: &[usize]) -> Option<usize> {
    Some(registers[SP])
}

pub fn save(frame: &mut [u8], registers: &[usize], _: usize) {
    frame.fill(0);
    for (index, value) in registers[..34].iter().enumerate() {
        put(frame, MCONTEXT_OFFSET + 8 + index * 8, *value);
    }
    // The reserved context chain starts with a null terminator: no FPSIMD record.
}

pub fn enter(
    registers: &mut [usize],
    address: usize,
    handler: usize,
    signal: usize,
    restorer: usize,
) {
    registers[0] = signal;
    registers[1] = address + INFO_OFFSET;
    registers[2] = address + UC_OFFSET;
    registers[30] = restorer;
    registers[SP] = address;
    registers[PC] = handler;
    registers[33] &= 0xf000_0000; // NZCV only; EL0t and interrupts enabled.
}

pub fn restore(frame: &[u8], registers: &mut [usize]) -> bool {
    let sp = get(frame, MCONTEXT_OFFSET + 8 + SP * 8);
    let pc = get(frame, MCONTEXT_OFFSET + 8 + PC * 8);
    if !user_address(pc) || !user_address(sp) || pc & 3 != 0 || sp & 15 != 0 {
        return false;
    }
    for (index, value) in registers[..33].iter_mut().enumerate() {
        *value = get(frame, MCONTEXT_OFFSET + 8 + index * 8);
    }
    registers[33] = get(frame, MCONTEXT_OFFSET + 8 + 33 * 8) & 0xf000_0000;
    // TPIDR_EL0 is trusted current TLS state, not part of Linux sigcontext.
    true
}

pub fn restart(registers: &mut [usize], context: crate::process::LinuxSyscallContext) {
    registers[..6].copy_from_slice(&context.args);
    registers[8] = context.number;
    registers[PC] = context.program_counter - 4;
}
