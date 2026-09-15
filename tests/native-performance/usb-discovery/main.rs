//! Production USB storage discovery/probing with a fake SCSI bus and manager IPC.
#![allow(dead_code)]
extern crate alloc;
extern crate self as libnanami;
extern crate self as nanami_services;
use std::cell::RefCell;
pub use std::{print, println};
pub type Word = usize;
pub const OS_RESPONSE_OK: Word = 0;
pub const OS_RESPONSE_INVALID_ARGUMENT: Word = 1;
pub const OS_RESPONSE_PERMISSION_DENIED: Word = 2;
pub const OS_RESPONSE_FATAL: Word = 5;
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RequestError {
    InvalidArgument,
    Unsupported,
    Protocol,
    Transport,
    Status(Word),
}
impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
pub mod input {
    pub struct Input;
}
pub mod ipc {
    #[derive(Clone, Copy)]
    pub struct ServiceRequest {
        pub identifier: usize,
        pub code: usize,
        pub arg0: usize,
        pub arg1: usize,
        pub arg2: usize,
    }
}
pub fn request_shared_memory(_: Word, _: Word) -> Result<(Word, Word), RequestError> {
    panic!("discovery must not allocate block-client shared memory")
}
fn delay(_: Word, _: Word) -> Result<(), RequestError> {
    Ok(())
}
#[path = "../../../nanami/servers/sdk/rust/nanami-services/src/block/constants.rs"]
pub mod block;
#[path = "../../../nanami/servers/core-services/usb-server/src/usb/mass_storage.rs"]
pub mod mass_storage;
pub mod usb {
    pub use super::mass_storage;
}
#[path = "../../../nanami/servers/core-services/driver-manager/src/storage.rs"]
mod selection;
#[path = "../../../nanami/servers/core-services/usb-server/src/storage/mod.rs"]
mod storage;
mod tests;
mod xhci;

struct Manager {
    selection: selection::Selection,
    reports: Vec<Word>,
    registrations: usize,
}
impl Default for Manager {
    fn default() -> Self {
        let mut selection = selection::Selection::new();
        selection.pids = [5, 7];
        selection.report(5, 0).unwrap(); // No PCI root, as in the hardware log.
        Self {
            selection,
            reports: Vec::new(),
            registrations: 0,
        }
    }
}
thread_local! { static MANAGER: RefCell<Manager> = RefCell::new(Manager::default()); }
pub mod device {
    use super::*;
    pub const STORAGE_PENDING: usize = 0;
    pub const STORAGE_SELECTED: usize = 1;
    pub const STORAGE_NOT_SELECTED: usize = 2;
    pub const STORAGE_AMBIGUOUS: usize = 3;
    pub fn select_storage_root_on_port(manager: Word, roots: Word) -> Result<bool, RequestError> {
        assert_eq!(manager, 25);
        MANAGER.with(|manager| {
            let mut manager = manager.borrow_mut();
            manager.reports.push(roots);
            match manager.selection.report(7, roots) {
                Ok(STORAGE_SELECTED) => Ok(true),
                Ok(STORAGE_NOT_SELECTED) => Ok(false),
                _ => Err(RequestError::Protocol),
            }
        })
    }
}
pub mod registry {
    pub fn register_block_device() -> Result<(), super::RequestError> {
        super::MANAGER.with(|manager| manager.borrow_mut().registrations += 1);
        Ok(())
    }
}
