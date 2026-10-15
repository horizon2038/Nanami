#![allow(dead_code)]
extern crate self as a9n_abi;
extern crate self as nanami_runtime;

mod mock;
pub use mock::{arch, capability_call};

pub type Word = usize;
pub type CapabilityDescriptor = Word;
pub const PROCESS_SLOT_NOTIFICATION: Word = nanami_uapi::os::PROCESS_SLOT_NOTIFICATION;

#[macro_export]
macro_rules! println {
    ($($arg:tt)*) => { $crate::mock::capture_log(format_args!($($arg)*)); };
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityError {
    InvalidArgument,
    Other,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestError {
    InvalidArgument,
    Transport,
    Protocol,
}
pub fn map_capability_error(error: CapabilityError) -> RequestError {
    match error {
        CapabilityError::InvalidArgument => RequestError::InvalidArgument,
        _ => RequestError::Transport,
    }
}
mod tls {
    pub fn init_ipc_tls() -> Result<(), crate::RequestError> {
        Ok(())
    }
}
#[path = "../../../nanami/servers/libs/nanami-runtime/src/ipc/ports.rs"]
mod ports;
#[path = "../../../nanami/servers/libs/nanami-runtime/src/ipc/service.rs"]
mod service;
#[path = "../../../nanami/servers/libs/nanami-runtime/src/ipc/types.rs"]
mod types;

pub mod ipc {
    pub use crate::service::{service_receive_event, service_reply, service_reply_receive_event};
    pub use crate::types::ServiceEvent;
}
#[path = "../../../nanami/servers/core-services/timer-server/src/init_trace.rs"]
mod init_trace;
#[path = "../../../nanami/servers/core-services/timer-server/src/reply.rs"]
mod timer_reply;

mod tests;
mod timer_reply_tests;
