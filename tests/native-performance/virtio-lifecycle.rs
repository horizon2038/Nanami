//! Device teardown must never touch I/O before a device has been discovered.
#![allow(dead_code)]
extern crate self as nanami_runtime;
use std::cell::RefCell;
pub use std::println;
pub type Word = usize;
const VIRTIO_STATUS_FAILED: u8 = 128;
#[derive(Clone, Copy, Debug)]
pub enum RequestError {
    Transport,
}
impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}
#[derive(Default)]
struct State {
    reads: Vec<(Word, Word)>,
    writes: Vec<(Word, Word, u8)>,
    fail_read: bool,
}
thread_local! { static STATE: RefCell<State> = RefCell::default(); }
fn read_device_status(io: Word, base: Word) -> Result<u8, RequestError> {
    STATE.with_borrow_mut(|state| {
        state.reads.push((io, base));
        if state.fail_read {
            Err(RequestError::Transport)
        } else {
            Ok(7)
        }
    })
}
fn write_device_status(io: Word, base: Word, status: u8) -> Result<(), RequestError> {
    STATE.with_borrow_mut(|state| state.writes.push((io, base, status)));
    Ok(())
}
#[path = "../../nanami/servers/apps/virtio-net/src/app/util.rs"]
mod util;

#[test]
fn absent_device_does_not_read_or_write_legacy_ports() {
    STATE.with_borrow_mut(|state| *state = State::default());
    util::fail_device(16, 0); // PCI I/O capability may cover every port.
    STATE.with_borrow(|state| {
        assert!(
            state.reads.is_empty(),
            "no BAR was discovered; status read would hit port 0x12"
        );
        assert!(state.writes.is_empty());
    });
}

#[test]
fn discovered_device_sets_failed_preserving_other_status_bits() {
    STATE.with_borrow_mut(|state| *state = State::default());
    util::fail_device(17, 0xc000);
    STATE.with_borrow(|state| {
        assert_eq!(state.reads, [(17, 0xc000)]);
        assert_eq!(state.writes, [(17, 0xc000, 0x87)]);
    });
}

#[test]
fn failed_status_read_does_not_issue_a_write() {
    STATE.with_borrow_mut(|state| {
        *state = State {
            fail_read: true,
            ..State::default()
        }
    });
    util::fail_device(17, 0xc000);
    STATE.with_borrow(|state| assert!(state.writes.is_empty()));
}
