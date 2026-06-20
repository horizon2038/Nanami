#![no_std]

pub mod gfx;
pub mod input;
pub mod net;
pub mod registry;
pub mod rtc;
pub mod timer;

use a9n_abi::CapabilityDescriptor;

pub use libnanami::{RequestError, Word};

pub const OS_RESPONSE_OK: Word = libnanami::OS_RESPONSE_OK;
pub const PROCESS_SLOT_NOTIFICATION: Word = libnanami::PROCESS_SLOT_NOTIFICATION;

pub mod ipc {
    pub use libnanami::ipc::*;
}

pub(crate) fn call_port(
    target_port: CapabilityDescriptor,
    request_code: Word,
    arg0: Word,
    arg1: Word,
    arg2: Word,
    arg3: Word,
    message_length: u8,
) -> Result<(Word, Word, Word), RequestError> {
    libnanami::call_service_port(
        target_port,
        request_code,
        arg0,
        arg1,
        arg2,
        arg3,
        message_length,
    )
}
