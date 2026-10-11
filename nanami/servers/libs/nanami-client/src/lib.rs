#![no_std]

pub mod block;
pub mod device;
pub mod exec;
pub mod gfx;
pub mod input;
pub mod net;
pub mod posix;
pub mod power;
pub mod registry;
pub mod rtc;
pub mod terminal;
pub mod timer;
pub mod usb;
pub mod vfs;

use a9n_abi::CapabilityDescriptor;

pub use nanami_runtime::{RequestError, Word};

pub const OS_RESPONSE_OK: Word = nanami_runtime::OS_RESPONSE_OK;
pub const PROCESS_SLOT_NOTIFICATION: Word = nanami_runtime::PROCESS_SLOT_NOTIFICATION;

pub mod ipc {
    pub use nanami_runtime::ipc::*;
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
    nanami_runtime::call_service_port(
        target_port,
        request_code,
        arg0,
        arg1,
        arg2,
        arg3,
        message_length,
    )
}
