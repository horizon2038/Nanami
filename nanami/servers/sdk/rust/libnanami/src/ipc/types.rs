use crate::Word;

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
    },
}
