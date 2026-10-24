use nanami_runtime::Word;

pub type HardwareContext = [Word; nanami_runtime::ipc::HARDWARE_CONTEXT_WORDS];

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

// Signal delivery/restoration requires a parked or synchronously suspended task.
pub fn read_hardware_context(pcb: Word) -> Result<HardwareContext, ()> {
    let mut registers = [0; nanami_runtime::ipc::HARDWARE_CONTEXT_WORDS];
    a9n_abi::arch::process_control_block::read_register(pcb, registers.len() as Word)
        .map_err(|_| ())?;
    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    for (index, value) in registers.iter_mut().enumerate() {
        *value = ipc.get_message(3 + index);
    }
    Ok(registers)
}

pub fn write_hardware_context(pcb: Word, registers: &HardwareContext) -> Result<(), ()> {
    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    for (index, value) in registers.iter().enumerate() {
        ipc.configure_message(3 + index, *value);
    }
    a9n_abi::arch::process_control_block::write_register(pcb, registers.len() as Word)
        .map_err(|_| ())
}
