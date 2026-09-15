//! Run the production handlers/client helpers with only IPC and storage mocked.
#![allow(dead_code)]
extern crate self as a9n_abi;
extern crate self as libnanami;
extern crate self as nanami_services;

use std::cell::RefCell;
pub use std::{print, println};
pub type Word = usize;
pub type CapabilityDescriptor = Word;
pub const OS_RESPONSE_OK: Word = 0;
pub const OS_RESPONSE_INVALID_ARGUMENT: Word = 1;
pub const OS_RESPONSE_INVALID_DESCRIPTOR: Word = 3;
pub const OS_RESPONSE_ILLEGAL_OPERATION: Word = 4;
pub const OS_RESPONSE_FATAL: Word = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    InvalidArgument,
    Status(Word),
    Unsupported,
    Transport,
    Protocol,
}

pub mod ipc {
    use super::Word;
    #[derive(Clone, Copy, Default)]
    pub struct ServiceRequest {
        pub identifier: Word,
        pub code: Word,
        pub arg0: Word,
        pub arg1: Word,
        pub arg2: Word,
        pub arg3: Word,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Call {
    code: Word,
    handle: Word,
    offset: Word,
    len: Word,
    delegate: Word,
    buffer: Word,
}

#[derive(Default)]
struct Backend {
    calls: Vec<Call>,
    result: Option<Result<Word, RequestError>>,
    size: Word,
    stats: usize,
}

thread_local! {
    static BACKEND: RefCell<Backend> = RefCell::new(Backend::default());
}

fn reset() {
    BACKEND.with(|backend| *backend.borrow_mut() = Backend::default());
}

fn record(call: Call) -> Result<Word, RequestError> {
    BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        let result = backend.result.unwrap_or(Ok(call.len));
        backend.calls.push(call);
        result
    })
}

pub fn call_port(
    _port: Word,
    code: Word,
    fd: Word,
    buffer: Word,
    len: Word,
    offset: Word,
    words: Word,
) -> Result<(Word, Word, Word), RequestError> {
    let positioned = matches!(
        code,
        posix::POSIX_REQUEST_PREAD
            | posix::POSIX_REQUEST_PREAD_DIRECT
            | posix::POSIX_REQUEST_PWRITE
            | posix::POSIX_REQUEST_PWRITE_DIRECT
    );
    assert_eq!(words, if positioned { 5 } else { 4 });
    record(Call {
        code,
        handle: fd,
        buffer,
        len,
        offset,
        delegate: 0,
    })
    .map(|bytes| (OS_RESPONSE_OK, bytes, 0))
}

pub mod abi {
    pub const ALTER_IO_OFFSET: usize = 512;
}
mod alter;
mod ext2;
pub mod posix;
mod server;
pub mod vfs;
