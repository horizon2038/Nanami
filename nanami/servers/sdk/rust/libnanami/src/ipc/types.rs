use crate::Word;

#[cfg(target_arch = "x86_64")]
pub const HARDWARE_CONTEXT_WORDS: usize = 23;

#[cfg(target_arch = "aarch64")]
pub const HARDWARE_CONTEXT_WORDS: usize = 35;

#[derive(Clone, Copy, Debug)]
pub struct ServiceRequest {
    pub identifier: Word,
    pub code: Word,
    pub arg0: Word,
    pub arg1: Word,
    pub arg2: Word,
    pub arg3: Word,
}

#[derive(Clone, Copy, Debug)]
pub enum ServiceEvent {
    Request(ServiceRequest),
    Notification {
        identifier: Word,
        value: Word,
    },
    Fault {
        identifier: Word,
        reason: Word,
        program_counter: Word,
        fault_address: Word,
        architecture_fault_code: Word,
        hardware_context: [Word; HARDWARE_CONTEXT_WORDS],
        hardware_context_count: usize,
    },
}
