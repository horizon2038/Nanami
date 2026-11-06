//! Actual native adapter: unsupported personalities never issue view requests.
#![allow(dead_code)]
extern crate self as nanami_runtime;
use std::cell::Cell;
pub type Word = usize;
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OsPersonality {
    Linux,
    FreeBsd,
}
#[derive(Clone, Copy)]
pub enum RequestError {
    Status(Word),
}
pub const OS_RESPONSE_INVALID_ARGUMENT: Word = 1;
pub const OS_RESPONSE_PERMISSION_DENIED: Word = 2;
pub const OS_RESPONSE_INVALID_DESCRIPTOR: Word = 4;
pub const OS_RESPONSE_ILLEGAL_OPERATION: Word = 5;
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/page_window.rs"]
pub mod page_window;
mod state {
    pub use super::{page_window, OsPersonality};
}
struct Mappings;
impl Mappings {
    fn generation(&self) -> Word {
        1
    }
    fn invalidate_views(&self) {}
}
struct Process {
    personality: OsPersonality,
    exited: bool,
    vm_group: Word,
    mappings: Mappings,
}
struct Runtime {
    posix_shm: Word,
    posix_shm_size: Word,
    read_window: page_window::ReadWindow,
    write_window: page_window::WriteWindow,
    process: Process,
}
impl Runtime {
    fn managed_process(&self, pid: Word) -> Option<&Process> {
        (pid == 7).then_some(&self.process)
    }
}
thread_local! { static REQUESTS: Cell<usize> = const { Cell::new(0) }; }
fn request_process_write_view(_: Word, _: Word, _: Word) -> Result<(), RequestError> {
    REQUESTS.with(|count| count.set(count.get() + 1));
    // Refuse installation, so no synthetic fixed-VA mapping is dereferenced.
    Err(RequestError::Status(OS_RESPONSE_PERMISSION_DENIED))
}
fn request_process_read_view(_: Word, _: Word, _: Word) -> Result<(), RequestError> {
    panic!("write adapter must not request a read view");
}
fn request_mapping_release(_: Word, _: Word) -> Result<(), RequestError> {
    panic!("a denied installation owns no mapping to release");
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/guest_views.rs"]
mod production;

fn runtime(scratch: &[u8], personality: OsPersonality) -> Runtime {
    REQUESTS.with(|count| count.set(0));
    Runtime {
        posix_shm: scratch.as_ptr() as Word,
        posix_shm_size: scratch.len(),
        read_window: page_window::ReadWindow::new(),
        write_window: page_window::WriteWindow::new(),
        process: Process {
            personality,
            exited: false,
            vm_group: 7,
            mappings: Mappings,
        },
    }
}

#[test]
fn freebsd_translation_never_attempts_a_writable_linux_view() {
    let scratch = [17u8; 32];
    let runtime = runtime(&scratch, OsPersonality::FreeBsd);
    assert!(!production::copy_from_owned(&runtime, 7, 4096, &[1; 8]));
    assert!(!production::copy_from_owned_many(
        &runtime,
        7,
        &[(4096, &[1; 8])]
    ));
    assert!(!production::copy_from_scratch(
        &runtime,
        7,
        4096,
        runtime.posix_shm,
        8
    ));
    REQUESTS.with(|count| assert_eq!(count.get(), 0));
}

#[test]
fn live_linux_can_attempt_the_optional_request_and_decline_without_storing() {
    let scratch = [17u8; 32];
    let runtime = runtime(&scratch, OsPersonality::Linux);
    assert!(!production::copy_from_owned(&runtime, 7, 4096, &[1; 8]));
    let enabled = match option_env!("NANAMI_GUEST_WRITE_VIEWS") {
        Some(value) => value == "1",
        None => cfg!(target_arch = "x86_64"),
    };
    REQUESTS.with(|count| assert_eq!(count.get(), usize::from(enabled)));
    assert!(!production::copy_from_owned_many(
        &runtime,
        7,
        &[(4096, &[1; 8])]
    ));
    REQUESTS.with(|count| assert_eq!(count.get(), 2 * usize::from(enabled)));
    assert_eq!(scratch, [17; 32]);
}

#[test]
fn bounds_exited_and_unknown_tasks_are_rejected_before_any_native_request() {
    let scratch = [17u8; 32];
    let mut runtime = runtime(&scratch, OsPersonality::Linux);
    assert!(!production::copy_from_scratch(&runtime, 7, 4096, 0, 8));
    assert!(!production::copy_from_scratch(
        &runtime,
        7,
        4096,
        runtime.posix_shm + 25,
        8
    ));
    assert!(!production::copy_from_scratch(
        &runtime,
        7,
        4096,
        Word::MAX - 3,
        8
    ));
    assert!(!production::copy_from_owned(&runtime, 99, 4096, &[1; 8]));
    assert!(!production::copy_from_owned(&runtime, 7, 4096, &[1; 4097]));
    runtime.process.exited = true;
    assert!(!production::copy_from_owned(&runtime, 7, 4096, &[1; 8]));
    REQUESTS.with(|count| assert_eq!(count.get(), 0));
}
