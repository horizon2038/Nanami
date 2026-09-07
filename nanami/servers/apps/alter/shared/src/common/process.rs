use libnanami::Word;

pub type HardwareContext = [Word; libnanami::ipc::HARDWARE_CONTEXT_WORDS];

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

#[cfg(target_arch = "x86_64")]
#[path = "arch/x86_64/process.rs"]
mod arch_impl;

#[cfg(target_arch = "aarch64")]
#[path = "arch/aarch64/process.rs"]
mod arch_impl;

pub use arch_impl::*;
