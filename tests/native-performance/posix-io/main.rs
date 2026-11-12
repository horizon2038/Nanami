//! Run the production handlers/client helpers with only IPC and storage mocked.
#![allow(dead_code)]
extern crate self as a9n_abi;
extern crate self as nanami_client;
extern crate self as nanami_runtime;

use std::cell::RefCell;
pub use std::{print, println};
pub type Word = usize;
pub type CapabilityDescriptor = Word;
mod state { pub const LINUX_FD_MAX: usize = 64; }
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
    pub fn process_slot_descriptor(slot: Word) -> Word {
        slot
    }
    pub fn notification_notify(slot: Word) -> Result<(), super::RequestError> {
        NOTIFIED.with(|values| values.borrow_mut().push(slot));
        Ok(())
    }
    thread_local! { pub static NOTIFIED: std::cell::RefCell<Vec<Word>> = const { std::cell::RefCell::new(Vec::new()) }; }
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
pub fn request_notification_port_copy(
    _: Word,
    _: Word,
    _: Word,
    _: Word,
) -> Result<(), RequestError> {
    Ok(())
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
    cursor_position: Word,
    cursor_committed_error: Option<RequestError>,
    vfs_control_target: Word,
    read_payload: Option<ReadPayload>,
    read_results: std::collections::VecDeque<Result<Word, RequestError>>,
    copyouts: usize,
    copyout_fail_after: Option<usize>,
    granted_result: Option<Result<Word, RequestError>>,
    stat_reply: Option<(Word, Word)>,
    read_direct_result: Option<Result<Word, RequestError>>,
    calls: Vec<Call>,
    result: Option<Result<Word, RequestError>>,
    sync_error: Option<RequestError>,
    size: Word,
    stats: usize,
    dir_reply: Option<Result<(Word, Word), RequestError>>,
    verified_reply: Option<Result<(Word, Word), RequestError>>,
}

struct ReadPayload { bytes: Vec<u8>, control: Word, direct: Word }

thread_local! {
    static BACKEND: RefCell<Backend> = RefCell::new(Backend::default());
}

fn reset() {
    BACKEND.with(|backend| *backend.borrow_mut() = Backend::default());
}

fn record(call: Call) -> Result<Word, RequestError> {
    BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        let granted = matches!(call.code, vfs::VFS_REQUEST_READ_GRANTED | vfs::VFS_REQUEST_READ_VERSIONED_GRANTED
            | vfs::VFS_REQUEST_FSTAT_GRANTED | vfs::VFS_REQUEST_READ_CURSOR_GRANTED | vfs::VFS_REQUEST_SEEK_CURSOR_GRANTED);
        let mut result = if granted {
            backend.granted_result.or(backend.result).unwrap_or(Ok(call.len))
        } else if matches!(call.code, posix::POSIX_REQUEST_READ_DIRECT | posix::POSIX_REQUEST_PREAD_DIRECT) {
            backend.read_direct_result.or(backend.result).unwrap_or(Ok(call.len))
        } else { backend.result.unwrap_or(Ok(call.len)) };
        let cursor_read = matches!(call.code, vfs::VFS_REQUEST_READ_CURSOR | vfs::VFS_REQUEST_READ_CURSOR_GRANTED
            | posix::POSIX_REQUEST_READ | posix::POSIX_REQUEST_READ_DIRECT);
        let cursor_seek = matches!(call.code, vfs::VFS_REQUEST_SEEK_CURSOR | vfs::VFS_REQUEST_SEEK_CURSOR_GRANTED
            | posix::POSIX_REQUEST_SEEK);
        if cursor_read || matches!(call.code, vfs::VFS_REQUEST_READ_GRANTED | vfs::VFS_REQUEST_READ | vfs::VFS_REQUEST_READ_DELEGATED
            | posix::POSIX_REQUEST_PREAD_DIRECT | posix::POSIX_REQUEST_PREAD) {
            let position = if cursor_read { backend.cursor_position } else { call.offset };
            if let Some(scripted) = backend.read_results.pop_front() {
                result = scripted;
            } else if let Some(payload) = &backend.read_payload {
                result = result.map(|bytes| bytes.min(payload.bytes.len().saturating_sub(position)));
            }
            if let (Ok(bytes), Some(payload)) = (result, &backend.read_payload) {
                if bytes <= call.len && position.checked_add(bytes).is_some_and(|end| end <= payload.bytes.len()) {
                    let output = if matches!(call.code,vfs::VFS_REQUEST_READ_CURSOR|vfs::VFS_REQUEST_READ)
                        && call.delegate == 0 && backend.vfs_control_target != 0 {
                        backend.vfs_control_target
                    } else if matches!(call.code, posix::POSIX_REQUEST_PREAD | posix::POSIX_REQUEST_READ) { payload.control }
                    else { payload.direct };
                    unsafe { std::ptr::copy_nonoverlapping(payload.bytes.as_ptr().add(position), (output + call.buffer) as *mut u8, bytes); }
                }
            }
            if cursor_read {
                if let Ok(bytes) = result {
                    backend.cursor_position = position.saturating_add(bytes);
                    if let Some(error) = backend.cursor_committed_error { result = Err(error); }
                }
            }
        } else if cursor_seek && result.is_ok() {
            let base = match call.len {
                vfs::VFS_CURSOR_SEEK_SET => Some(0),
                vfs::VFS_CURSOR_SEEK_CUR => Some(backend.cursor_position),
                vfs::VFS_CURSOR_SEEK_END => Some(backend.read_payload.as_ref().map_or(13, |p| p.bytes.len())),
                _ => None,
            };
            result = base.map(|base| base.saturating_add(call.offset))
                .ok_or(RequestError::Status(OS_RESPONSE_INVALID_ARGUMENT));
            if let Ok(position) = result {
                backend.cursor_position = position;
                if let Some(error) = backend.cursor_committed_error { result = Err(error); }
            }
        }
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
            | posix::POSIX_REQUEST_PREAD_VERSIONED
            | posix::POSIX_REQUEST_PWRITE
            | posix::POSIX_REQUEST_PWRITE_DIRECT
            | vfs::VFS_REQUEST_READ_GRANTED
            | vfs::VFS_REQUEST_READ_VERSIONED_GRANTED
            | vfs::VFS_REQUEST_READ_CURSOR
            | vfs::VFS_REQUEST_READ_CURSOR_GRANTED
    );
    let synchronize = matches!(code, posix::POSIX_REQUEST_FSYNC | posix::POSIX_REQUEST_SYNC);
    assert_eq!(
        words,
        if matches!(
            code,
            vfs::VFS_REQUEST_CREATE | vfs::VFS_REQUEST_OPEN_COMPOUND | vfs::VFS_REQUEST_MKDIR
        ) {
            5
        } else if code == vfs::VFS_REQUEST_CLOSE {
            2
        } else if synchronize {
            2
        } else if matches!(
            code,
            posix::POSIX_REQUEST_FTRUNCATE | vfs::VFS_REQUEST_FTRUNCATE | vfs::VFS_REQUEST_FSTAT_GRANTED
        ) {
            3
        } else if positioned {
            5
        } else {
            4
        }
    );
    let reply = record(Call {
        code,
        handle: fd,
        buffer: if matches!(code, vfs::VFS_REQUEST_READ_GRANTED | vfs::VFS_REQUEST_READ_CURSOR_GRANTED) { offset }
            else if code == vfs::VFS_REQUEST_READ_CURSOR { offset & vfs::VFS_DELEGATE_VALUE_MASK }
            else if matches!(code, vfs::VFS_REQUEST_READ_VERSIONED_GRANTED | vfs::VFS_REQUEST_SEEK_CURSOR | vfs::VFS_REQUEST_SEEK_CURSOR_GRANTED) { 0 }
            else { buffer },
        len,
        offset: if matches!(code, vfs::VFS_REQUEST_READ_GRANTED | vfs::VFS_REQUEST_READ_VERSIONED_GRANTED
            | vfs::VFS_REQUEST_READ_CURSOR | vfs::VFS_REQUEST_READ_CURSOR_GRANTED
            | vfs::VFS_REQUEST_SEEK_CURSOR | vfs::VFS_REQUEST_SEEK_CURSOR_GRANTED) { buffer } else { offset },
        delegate: if code == vfs::VFS_REQUEST_READ_CURSOR { offset >> vfs::VFS_DELEGATE_ID_SHIFT } else { 0 },
    })
    .map(|bytes| (bytes, 0));
    if code == vfs::VFS_REQUEST_FSTAT_GRANTED {
        reply?;
        let (identity, metadata) = BACKEND.with(|b| b.borrow().stat_reply.unwrap_or((0, 0)));
        return Ok((OS_RESPONSE_OK, identity, metadata));
    }
    if code == vfs::VFS_REQUEST_READ_VERSIONED_GRANTED && reply.is_err() { return reply.map(|_| unreachable!()); }
    let (bytes, verified) = if matches!(code, posix::POSIX_REQUEST_PREAD_VERSIONED | vfs::VFS_REQUEST_READ_VERSIONED_GRANTED) {
        BACKEND.with(|b| b.borrow().verified_reply).unwrap_or(reply)
    } else { reply }?;
    Ok((OS_RESPONSE_OK, bytes, verified))
}

pub mod abi {
    pub const ALTER_IO_OFFSET: usize = 512;
    pub const ALTER_DEFAULT_SHM_BYTES: usize = 65536;
}
mod alter;
mod ext2;
#[path = "linux-sync.rs"]
mod linux_sync;
pub mod posix;
mod server;
pub mod vfs;
