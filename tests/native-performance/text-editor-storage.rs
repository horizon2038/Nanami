//! Exercise the production VFS adapter, including partial transfers and failures.
#![allow(dead_code)]
extern crate alloc;
extern crate self as nanami_client;
extern crate self as nanami_runtime;
pub use nanami_uapi::{RequestError, Word};
pub use std::println;
pub const OS_RESPONSE_INVALID_DESCRIPTOR: usize = nanami_uapi::os::OS_RESPONSE_INVALID_DESCRIPTOR;

#[path = "../../nanami/servers/apps/text-editor/src/app.rs"]
mod app;
#[path = "../../nanami/servers/apps/text-editor/src/document.rs"]
mod document;
#[path = "../../nanami/servers/apps/text-editor/src/input.rs"]
mod input;
#[path = "../../nanami/servers/apps/text-editor/src/layout.rs"]
mod layout;
#[path = "../../nanami/servers/apps/text-editor/src/storage.rs"]
mod storage;

use app::Files;
use std::cell::RefCell;

struct State {
    data: Vec<u8>,
    shm: Vec<u8>,
    size: Option<usize>,
    kind: usize,
    max_transfer: usize,
    fail_read: bool,
    zero_read: bool,
    fail_write_at: Option<usize>,
    fail_sync: bool,
    fail_truncate: bool,
    fail_close: bool,
    opened: usize,
    closed: usize,
    events: Vec<&'static str>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            data: Vec::new(),
            shm: vec![0; 1024],
            size: None,
            kind: nanami_uapi::vfs::VFS_FILE_TYPE_REGULAR,
            max_transfer: usize::MAX,
            fail_read: false,
            zero_read: false,
            fail_write_at: None,
            fail_sync: false,
            fail_truncate: false,
            fail_close: false,
            opened: 0,
            closed: 0,
            events: Vec::new(),
        }
    }
}

thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    STATE.with(|s| f(&mut s.borrow_mut()))
}
fn reset(data: &[u8]) -> storage::Storage {
    with(|s| {
        *s = State::default();
        s.data = data.to_vec();
    });
    storage::Storage::connect(24).unwrap()
}

pub mod ipc {
    pub fn process_slot_descriptor(slot: usize) -> usize {
        slot
    }
}
pub mod registry {
    pub fn connect_vfs_service(_: usize) -> Result<(), crate::RequestError> {
        Ok(())
    }
}
pub mod vfs {
    use crate::{with, RequestError};
    pub use nanami_uapi::vfs::*;

    pub fn vfs_attach_shared_memory(_: usize, _: usize) -> Result<(usize, usize), RequestError> {
        Ok(with(|s| (s.shm.as_mut_ptr() as usize, s.shm.len())))
    }
    pub fn vfs_open_compound(
        _: usize,
        _: usize,
        _: usize,
        flags: usize,
    ) -> Result<(usize, usize, usize, usize), RequestError> {
        assert_eq!(
            flags & VFS_OPEN_TRUNCATE,
            0,
            "do not truncate before writing"
        );
        Ok(with(|s| {
            s.opened += 1;
            (7, 8, s.size.unwrap_or(s.data.len()), s.kind)
        }))
    }
    pub fn vfs_close(_: usize, handle: usize) -> Result<(), RequestError> {
        assert_eq!(handle, 7);
        with(|s| {
            s.closed += 1;
            s.events.push("close");
            if s.fail_close {
                return Err(RequestError::Protocol);
            }
            Ok(())
        })
    }
    pub fn vfs_stat(_: usize, _: usize, _: usize) -> Result<(usize, usize, usize), RequestError> {
        Ok((8, 0, VFS_FILE_TYPE_REGULAR))
    }
    pub fn vfs_read(
        _: usize,
        handle: usize,
        offset: usize,
        len: usize,
        output: usize,
    ) -> Result<usize, RequestError> {
        assert_eq!(handle, 7);
        with(|s| {
            if s.fail_read {
                return Err(RequestError::Protocol);
            }
            if s.zero_read {
                return Ok(0);
            }
            let count = len
                .min(s.max_transfer)
                .min(s.data.len().saturating_sub(offset));
            let bytes = s.data[offset..offset + count].to_vec();
            s.shm[output..output + count].copy_from_slice(&bytes);
            Ok(count)
        })
    }
    pub fn vfs_write(
        _: usize,
        handle: usize,
        offset: usize,
        len: usize,
        input: usize,
    ) -> Result<usize, RequestError> {
        assert_eq!(handle, 7);
        with(|s| {
            s.events.push("write");
            if s.fail_write_at.is_some_and(|at| offset >= at) {
                return Err(RequestError::Protocol);
            }
            let count = len.min(s.max_transfer);
            s.data.resize(s.data.len().max(offset + count), 0);
            let bytes = s.shm[input..input + count].to_vec();
            s.data[offset..offset + count].copy_from_slice(&bytes);
            Ok(count)
        })
    }
    pub fn vfs_ftruncate(_: usize, _: usize, len: usize) -> Result<(), RequestError> {
        with(|s| {
            s.events.push("truncate");
            if s.fail_truncate {
                return Err(RequestError::Protocol);
            }
            s.data.resize(len, 0);
            Ok(())
        })
    }
    pub fn vfs_fsync(_: usize, _: usize) -> Result<(), RequestError> {
        with(|s| {
            s.events.push("sync");
            if s.fail_sync {
                return Err(RequestError::Protocol);
            }
            Ok(())
        })
    }
}

#[test]
fn read_retries_short_chunks_and_preserves_utf8_crlf() {
    let text = "Hello 日本語\r\n".repeat(400);
    let mut files = reset(text.as_bytes());
    with(|s| {
        s.max_transfer = 7;
    });
    assert_eq!(files.read("/note").unwrap(), text);
    with(|s| {
        assert_eq!(s.opened, 1);
        assert_eq!(s.closed, 1);
    });
}

#[test]
fn read_rejects_binary_invalid_utf8_and_oversize_and_closes() {
    for bytes in [b"binary\0file".as_slice(), &[0xff, 0xfe]] {
        let mut files = reset(bytes);
        assert!(files.read("/note").is_err());
        assert_eq!(with(|s| s.closed), 1);
    }
    let mut files = reset(b"");
    with(|s| {
        s.size = Some(document::MAX_BYTES + 1);
    });
    assert!(files.read("/note").is_err());
    assert_eq!(with(|s| s.closed), 1);
}

#[test]
fn read_errors_and_unexpected_eof_release_handle() {
    for fail in [true, false] {
        let mut files = reset(b"hello");
        with(|s| {
            s.fail_read = fail;
            s.zero_read = !fail;
        });
        assert!(files.read("/note").is_err());
        assert_eq!(with(|s| s.closed), 1);
    }
}

#[test]
fn write_retries_short_chunks_then_truncates_and_syncs() {
    let mut files = reset(&vec![b'!'; 18000]);
    with(|s| {
        s.max_transfer = 17;
    });
    let text = "saved UTF-8: é\r\n".repeat(300);
    files.write("/note", &text).unwrap();
    with(|s| {
        assert_eq!(s.data, text.as_bytes());
        assert_eq!(s.closed, 1);
        assert_eq!(
            &s.events[s.events.len() - 3..],
            &["truncate", "sync", "close"]
        );
    });
}

#[test]
fn empty_save_removes_old_contents_and_syncs() {
    let mut files = reset(b"old tail");
    files.write("/note", "").unwrap();
    with(|s| {
        assert!(s.data.is_empty());
        assert_eq!(s.events, ["truncate", "sync", "close"]);
    });
}

#[test]
fn write_failure_preserves_old_tail_and_reports_partial_file() {
    let mut files = reset(b"original");
    with(|s| {
        s.max_transfer = 3;
        s.fail_write_at = Some(3);
    });
    let error = files.write("/note", "replacement").unwrap_err();
    assert!(error.contains("partial"));
    with(|s| {
        assert_eq!(s.data, b"repginal");
        assert!(!s.events.contains(&"truncate"));
        assert!(!s.events.contains(&"sync"));
        assert_eq!(s.closed, 1);
    });
}

#[test]
fn truncate_sync_and_close_failures_are_never_success() {
    for failure in 0..3 {
        let mut files = reset(b"old");
        with(|s| {
            s.fail_truncate = failure == 0;
            s.fail_sync = failure == 1;
            s.fail_close = failure == 2;
        });
        assert!(files.write("/note", "new").is_err());
        assert_eq!(with(|s| s.closed), 1);
    }
}

#[test]
fn non_regular_files_are_rejected_and_closed_without_writes() {
    for writing in [true, false] {
        let mut files = reset(b"");
        with(|s| {
            s.kind = vfs::VFS_FILE_TYPE_DIRECTORY;
        });
        if writing {
            assert!(files.write("/dir", "x").is_err());
        } else {
            assert!(files.read("/dir").is_err());
        }
        with(|s| {
            assert_eq!(s.closed, 1);
            assert_eq!(s.events, ["close"]);
        });
    }
}
