#![no_std]
//! OCI bundle validation and lifecycle policy, independent of Nanami IPC.
extern crate alloc;

mod config;
mod lifecycle;
mod path;

pub use config::{Config, CONFIG_MAX_BYTES, STRING_COUNT_MAX};
pub use lifecycle::{Container, Status};
pub use path::{guest_path, host_path, valid_id};

use alloc::string::String;

pub const OCI_VERSION: &str = "1.3.0";
pub type Result<T> = core::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl core::fmt::Display for Error {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        out.write_str(&self.0)
    }
}
