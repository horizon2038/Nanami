use super::{get, put, user_address};

// rt_sigframe = pretcode + ucontext (40 + sigcontext[256] + sigset[8]) + siginfo[128].
pub const FRAME_BYTES: usize = 440;
pub const UC_OFFSET: usize = 8;
pub const STACK_OFFSET: usize = UC_OFFSET + 16;
pub const MCONTEXT_OFFSET: usize = UC_OFFSET + 40;
pub const MASK_OFFSET: usize = MCONTEXT_OFFSET + 256;
pub const INFO_OFFSET: usize = MASK_OFFSET + 8;
pub const SP: usize = 18;
pub const PC: usize = 15;
pub const MINSIGSTKSZ: usize = 2048;
pub const TRAMPOLINE: &[u8] = &[0x48, 0xc7, 0xc0, 15, 0, 0, 0, 0x0f, 0x05];
const ENTER_FROM: usize = 22;
const INTERRUPT_ENTRY: usize = 2;
// Linux sigcontext order -> A9N register index. Segment selectors/bases are not user-restored.
const GREGS: [usize; 18] = [7, 8, 9, 10, 11, 12, 13, 14, 4, 5, 6, 1, 3, 0, 2, 18, 15, 17];

pub fn frame_address(stack_top: usize) -> Option<usize> {
    // Preserve the SysV red zone; a function entry has RSP % 16 == 8.
    stack_top
        .checked_sub(128 + FRAME_BYTES)
        .and_then(|sp| (sp & !15).checked_sub(8))
}

pub fn return_frame_address(registers: &[usize]) -> Option<usize> {
    registers[SP].checked_sub(8)
}

pub fn save(frame: &mut [u8], registers: &[usize], restorer: usize) {
    frame.fill(0);
    put(frame, 0, restorer);
    for (index, register) in GREGS.iter().enumerate() {
        put(frame, MCONTEXT_OFFSET + index * 8, registers[*register]);
    }
    put(
        frame,
        MCONTEXT_OFFSET + 144,
        registers[16] | (registers[19] << 48),
    );
    // fpstate stays null and UC_FP_XSTATE remains clear (FPU explicitly out of scope).
}

pub fn enter(registers: &mut [usize], address: usize, handler: usize, signal: usize, _: usize) {
    registers[4] = signal;
    registers[5] = address + INFO_OFFSET;
    registers[3] = address + UC_OFFSET;
    registers[SP] = address;
    registers[PC] = handler;
    registers[17] = (registers[17] & !(0x100 | 0x400)) | 0x202;
    registers[ENTER_FROM] = INTERRUPT_ENTRY;
}

pub fn restore(frame: &[u8], registers: &mut [usize]) -> bool {
    let pc = get(frame, MCONTEXT_OFFSET + 16 * 8);
    let sp = get(frame, MCONTEXT_OFFSET + 15 * 8);
    if !user_address(pc) || !user_address(sp) {
        return false;
    }
    for (index, register) in GREGS[..17].iter().enumerate() {
        registers[*register] = get(frame, MCONTEXT_OFFSET + index * 8);
    }
    // Only user arithmetic/debug flags are writable. Never accept IOPL/NT/VM or CS/SS.
    registers[17] = (get(frame, MCONTEXT_OFFSET + 17 * 8) & 0x0004_0dd5) | 0x202;
    registers[ENTER_FROM] = INTERRUPT_ENTRY;
    true
}

pub fn restart(registers: &mut [usize], context: crate::process::LinuxSyscallContext) {
    registers[0] = context.number;
    for (index, register) in [4, 5, 3, 9, 7, 8].iter().enumerate() {
        registers[*register] = context.args[index];
    }
    registers[PC] = context.program_counter - 2;
}
