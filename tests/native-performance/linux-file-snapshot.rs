//! Production split publisher; storage admission and kernel effects modeled.
extern crate self as nanami_runtime;
use std::cell::RefCell;
type Word = usize;
const LINUX_PAGE_SIZE: Word = 4096;
const EINVAL: i32 = 22;
const EIO: i32 = 5;
#[derive(Debug)]
pub struct RequestError;
fn map_request_error(_: RequestError) -> i32 { EIO }
struct Cache { start: Word, bytes: Word }
impl Cache {
    fn immutable_range(&self, source: Word, bytes: Word) -> bool {
        source >= self.start && source.checked_add(bytes)
            .is_some_and(|end| end <= self.start + self.bytes)
    }
}
struct Runtime { image_cache: Cache, shared: bool }
mod private_memory {
    pub fn mark_shared(runtime: &mut super::Runtime, _: usize) { runtime.shared = true; }
}
#[derive(Debug, PartialEq)]
struct Call { kind: u8, base: Word, bytes: Word, source: Word, initialized: Word, prot: Word }
thread_local! {
    static CALLS: RefCell<Vec<Call>> = const { RefCell::new(Vec::new()) };
    static FAIL: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}
fn record(call: Call) -> Result<(), RequestError> {
    CALLS.with(|calls| {
        let mut calls = calls.borrow_mut();
        let index = calls.len();
        calls.push(call);
        if FAIL.with(|fail| fail.get()) == Some(index) { Err(RequestError) } else { Ok(()) }
    })
}
pub fn request_process_map_private_snapshot(_: Word, base: Word, bytes: Word, source: Word, prot: Word) -> Result<(), RequestError> {
    record(Call { kind: 0, base, bytes, source, initialized: bytes, prot })
}
pub fn request_process_map_initialized_protected_at(_: Word, base: Word, bytes: Word, source: Word, initialized: Word, prot: Word) -> Result<(), RequestError> {
    record(Call { kind: 1, base, bytes, source, initialized, prot })
}
pub fn request_process_map_initialized_at(_: Word, base: Word, bytes: Word, source: Word, initialized: Word) -> Result<(), RequestError> {
    record(Call { kind: 2, base, bytes, source, initialized, prot: 7 })
}
pub fn request_process_mapping_protect(_: Word, base: Word, bytes: Word, prot: Word) -> Result<(), RequestError> {
    record(Call { kind: 3, base, bytes, source: 0, initialized: 0, prot })
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/file_snapshot.rs"]
mod file_snapshot;
fn fixture() -> Runtime {
    CALLS.with(|calls| calls.borrow_mut().clear()); FAIL.with(|fail| fail.set(None));
    Runtime { image_cache: Cache { start: 0x1000000, bytes: 3514368 }, shared: false }
}
#[test]
fn actual_large_mapping_aliases_file_pages_and_initializes_only_four_zero_pages() {
    let mut runtime = fixture();
    assert_eq!(file_snapshot::map_readonly(&mut runtime, 2, 0x2000000, 3530752,
        0x1000000, 3514168, 5, true), Ok(true));
    assert!(runtime.shared);
    CALLS.with(|calls| assert_eq!(*calls.borrow(), vec![
        Call { kind: 0, base: 0x2000000, bytes: 3514368, source: 0x1000000, initialized: 3514368, prot: 5 },
        Call { kind: 1, base: 0x2000000 + 3514368, bytes: 16384, source: 0, initialized: 0, prot: 5 },
    ]));
}
#[test]
fn no_zero_suffix_needs_only_the_alias_request() {
    let mut runtime = fixture();
    assert_eq!(file_snapshot::map_readonly(&mut runtime, 2, 0x2000000, 4096, 0x1001000, 4096, 1, true), Ok(true));
    CALLS.with(|calls| assert_eq!(calls.borrow().len(), 1));
}
#[test]
fn unsupported_rights_unaligned_sources_and_outside_ranges_decline_before_publication() {
    for (source, bytes, mapped, prot) in [
        (0x1000000, 4096, 4096, 3), (0x1000001, 4096, 4096, 1),
        (0x1000000, 3514369, 3530752, 1), (0x1000000, 8192, 4096, 1),
        (0x1000000, 0, 4096, 1),
    ] {
        let mut runtime = fixture();
        assert_eq!(file_snapshot::map_readonly(&mut runtime, 2, 0x2000000, mapped, source, bytes, prot, true), Ok(false));
        assert!(!runtime.shared);
        CALLS.with(|calls| assert!(calls.borrow().is_empty()));
    }
}
#[test]
fn zero_tail_keeps_the_legacy_constructor_and_protect_route_when_requested() {
    let mut runtime = fixture();
    assert_eq!(file_snapshot::map_readonly(&mut runtime, 2, 0x2000000, 8192, 0x1000000, 4096, 1, false), Ok(true));
    CALLS.with(|calls| assert_eq!(calls.borrow().iter().map(|call| call.kind).collect::<Vec<_>>(), [0, 2, 3]));
}
#[test]
fn failures_never_retry_or_copy_after_a_possible_alias_prefix() {
    for failure in 0..3 {
        let mut runtime = fixture(); FAIL.with(|fail| fail.set(Some(failure)));
        assert_eq!(file_snapshot::map_readonly(&mut runtime, 2, 0x2000000, 8192, 0x1000000, 4096, 1, false), Err(EIO));
        CALLS.with(|calls| assert_eq!(calls.borrow().len(), failure + 1));
    }
}
