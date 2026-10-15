//! Production Shell output parser/IPC batching with a mocked terminal service.
#![allow(dead_code)]
extern crate alloc;
extern crate self as nanami_client;
extern crate self as nanami_runtime;

pub use std::println;
use std::{cell::RefCell, collections::VecDeque};
pub type Word = usize;
const COLS: usize = 89;
const ROWS: usize = 33;
pub const PROCESS_SLOT_NOTIFICATION: Word = 21;

#[derive(Clone, Copy, Debug)]
pub enum RequestError {
    Status(Word),
    InvalidArgument,
    Unsupported,
    Transport,
    Protocol,
}
impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[path = "../../nanami/servers/apps/shell/src/foreground.rs"]
mod foreground;
#[path = "../../nanami/servers/apps/shell/src/terminal/mod.rs"]
mod terminal_model;

struct Backend {
    shm: Box<[u8]>,
    output: VecDeque<u8>,
    reads: Vec<usize>,
    writes: Vec<Vec<u8>>,
    max_read: usize,
    read_result: Option<Result<usize, RequestError>>,
}
impl Default for Backend {
    fn default() -> Self {
        Self {
            shm: vec![0; 0x4000].into_boxed_slice(),
            output: VecDeque::new(),
            reads: Vec::new(),
            writes: Vec::new(),
            max_read: usize::MAX,
            read_result: None,
        }
    }
}
thread_local! {
    static BACKEND: RefCell<Backend> = RefCell::new(Backend::default());
}

fn app(bytes: &[u8]) -> foreground::ForegroundApp {
    BACKEND.with(|backend| {
        *backend.borrow_mut() = Backend {
            output: bytes.iter().copied().collect(),
            ..Backend::default()
        };
    });
    let mut app = foreground::ForegroundApp::new();
    let mut out = foreground::CommandOutput::new();
    assert!(app.ensure_terminal(&mut out));
    assert_eq!(out.len(), 0);
    app
}

pub mod terminal {
    pub use super::terminal_model::Terminal;
    use super::*;
    pub const TERMINAL_CLEAR_INPUT: Word = 1;
    pub const TERMINAL_CLEAR_OUTPUT: Word = 2;
    pub fn terminal_attach_shared_memory(
        _: Word,
        size: Word,
    ) -> Result<(Word, Word), RequestError> {
        BACKEND.with(|backend| {
            let mut backend = backend.borrow_mut();
            assert!(size <= backend.shm.len());
            Ok((backend.shm.as_mut_ptr() as Word, backend.shm.len()))
        })
    }
    pub fn terminal_create(_: Word, _: Word, _: Word) -> Result<Word, RequestError> {
        Ok(1)
    }
    pub fn terminal_attach_output_notification(
        _: Word,
        _: Word,
        _: Word,
    ) -> Result<(), RequestError> {
        Ok(())
    }
    pub fn terminal_read_output(
        _: Word,
        _: Word,
        offset: Word,
        len: Word,
    ) -> Result<Word, RequestError> {
        BACKEND.with(|backend| {
            let mut backend = backend.borrow_mut();
            backend.reads.push(len);
            if let Some(result) = backend.read_result {
                return result;
            }
            let bytes = len.min(backend.max_read).min(backend.output.len());
            for i in 0..bytes {
                backend.shm[offset + i] = backend.output.pop_front().unwrap();
            }
            Ok(bytes)
        })
    }
    pub fn terminal_write_input(
        _: Word,
        _: Word,
        offset: Word,
        len: Word,
    ) -> Result<Word, RequestError> {
        BACKEND.with(|backend| {
            let mut backend = backend.borrow_mut();
            let bytes = backend.shm[offset..offset + len].to_vec();
            backend.writes.push(bytes);
            // Another RPC may overwrite the transfer area while parsing output.
            backend.shm.fill(0xcc);
        });
        Ok(len)
    }
    pub fn terminal_read_input(_: Word, _: Word, _: Word, _: Word) -> Result<Word, RequestError> {
        Ok(0)
    }
    pub fn terminal_clear(_: Word, _: Word, _: Word) -> Result<(), RequestError> {
        Ok(())
    }
    pub fn terminal_set_echo(_: Word, _: Word, _: bool) -> Result<(), RequestError> {
        Ok(())
    }
    pub fn terminal_set_output_crlf(_: Word, _: Word, _: bool) -> Result<(), RequestError> {
        Ok(())
    }
    pub fn terminal_set_size(_: Word, _: Word, _: Word, _: Word) -> Result<(), RequestError> {
        Ok(())
    }
}
pub mod registry {
    use super::*;
    pub fn connect_exec_service(_: Word) -> Result<(), RequestError> {
        Ok(())
    }
    pub fn connect_terminal_service(_: Word) -> Result<(), RequestError> {
        Ok(())
    }
}
pub mod exec {
    use super::*;
    pub fn exec_process_kill(_: Word, _: Word, _: Word) -> Result<(), RequestError> {
        Ok(())
    }
    pub fn exec_process_reap(_: Word, _: Word) -> Result<(), RequestError> {
        Ok(())
    }
    pub fn exec_process_status(_: Word, _: Word) -> Result<(bool, Word), RequestError> {
        Ok((false, 0))
    }
}
pub mod ipc {
    pub fn process_slot_descriptor(slot: usize) -> usize {
        slot
    }
}
pub fn connect_service_by_name(_: &str, _: Word) -> Result<(), RequestError> {
    Ok(())
}
pub fn call_service_port(
    _: Word,
    _: Word,
    _: Word,
    _: Word,
    _: Word,
    _: Word,
    _: Word,
) -> Result<(Word, Word, Word), RequestError> {
    Ok((0, 0, 0))
}
pub fn request_process_kill(_: Word, _: Word) -> Result<(), RequestError> {
    Ok(())
}
pub fn request_process_reap(_: Word) -> Result<(), RequestError> {
    Ok(())
}
fn copy_bytes(destination: &mut [u8], source: &[u8]) {
    let bytes = destination.len().min(source.len());
    destination[..bytes].copy_from_slice(&source[..bytes]);
}
fn append_bytes(destination: &mut [u8], offset: usize, source: &[u8]) -> usize {
    let bytes = (destination.len() - offset).min(source.len());
    destination[offset..offset + bytes].copy_from_slice(&source[..bytes]);
    offset + bytes
}
fn append_decimal(destination: &mut [u8], offset: usize, value: Word) -> usize {
    append_bytes(destination, offset, value.to_string().as_bytes())
}

fn assert_terminal_matches(actual: &terminal::Terminal, expected: &terminal::Terminal) {
    assert_eq!(actual.cursor(), expected.cursor());
    assert_eq!(actual.history_len(), expected.history_len());
    for row in 0..ROWS {
        for (actual, expected) in actual.row(row).iter().zip(expected.row(row)) {
            assert_eq!(
                (actual.byte, actual.fg, actual.bg),
                (expected.byte, expected.fg, expected.bg)
            );
        }
    }
}

#[test]
fn a_full_processing_budget_needs_one_read_ipc_instead_of_sixteen() {
    let bytes = vec![b'x'; 4096];
    let mut app = app(&bytes);
    assert!(app.drain_output().is_some());
    BACKEND.with(|backend| {
        assert_eq!(backend.borrow().reads, [4096]);
        assert!(backend.borrow().output.is_empty());
    });
    assert!(app.drain_output().is_none());
}

#[test]
fn continuous_output_keeps_the_existing_per_event_budget() {
    let bytes = b"hello\r\n\x1b[31mred\x1b[0m".repeat(600);
    let mut app = app(&bytes);
    let mut expected = terminal::Terminal::new(COLS, ROWS);
    let mut offset = 0;
    while app.drain_output().is_some() {
        let end = (offset + 4096).min(bytes.len());
        for &byte in &bytes[offset..end] {
            expected.process_byte(byte);
        }
        offset = end;
        assert_terminal_matches(&app.terminal, &expected);
    }
    assert_eq!(offset, bytes.len());
    BACKEND.with(|backend| {
        assert_eq!(backend.borrow().reads.len(), bytes.len().div_ceil(4096) + 2);
    });
}

#[test]
fn cursor_reports_cannot_corrupt_pending_output_via_shared_memory() {
    let bytes = b"\x1b[2;3H\x1b[6ntext after report\r\n\x1b[31mcolored\x1b[0m".repeat(100);
    let mut app = app(&bytes);
    let mut expected = terminal::Terminal::new(COLS, ROWS);
    let mut responses = Vec::new();
    for &byte in &bytes {
        expected.process_byte(byte);
        let (response, len) = expected.take_response();
        if len != 0 {
            responses.push(response[..len].to_vec());
        }
    }
    while app.drain_output().is_some() {}
    assert_terminal_matches(&app.terminal, &expected);
    BACKEND.with(|backend| assert_eq!(backend.borrow().writes, responses));
}

#[test]
fn partial_reads_and_split_escape_sequences_preserve_output() {
    let bytes = b"abcdef\x1b[2;3HX\r\n\x1b[6nfinish".repeat(40);
    let mut app = app(&bytes);
    BACKEND.with(|backend| backend.borrow_mut().max_read = 7);
    let mut expected = terminal::Terminal::new(COLS, ROWS);
    for &byte in &bytes {
        expected.process_byte(byte);
    }
    while app.drain_output().is_some() {}
    assert_terminal_matches(&app.terminal, &expected);
}

#[test]
fn oversized_or_failed_reads_report_once_and_can_recover() {
    for result in [Ok(4097), Err(RequestError::Transport)] {
        let mut app = app(b"valid");
        BACKEND.with(|backend| backend.borrow_mut().read_result = Some(result));
        assert_eq!(app.drain_output().unwrap().len(), 1);
        assert!(app.drain_output().is_none());
        BACKEND.with(|backend| backend.borrow_mut().read_result = None);
        assert_eq!(app.drain_output().unwrap().len(), 0);
        assert_eq!(app.terminal.row(0)[0].byte, b'v');
    }
}

#[test]
fn session_cleanup_also_drains_in_full_sized_chunks() {
    let mut app = app(&vec![b'x'; 10000]);
    app.prepare_start();
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        assert_eq!(backend.reads, [4096, 4096, 4096]);
        assert!(backend.output.is_empty());
    });
}
