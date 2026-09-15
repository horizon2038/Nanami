#![allow(dead_code)]
extern crate self as a9n_abi;

mod mock;
pub use mock::{arch, capability_call};

pub type Word = usize;
pub type CapabilityDescriptor = Word;
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
#[path = "../../../nanami/servers/sdk/rust/libnanami/src/ipc/ports.rs"]
mod ports;
#[path = "../../../nanami/servers/sdk/rust/libnanami/src/ipc/service.rs"]
mod service;
#[path = "../../../nanami/servers/sdk/rust/libnanami/src/ipc/types.rs"]
mod types;

mod tests;
