//! Actual fault routing and snapshot-fault admission; other repair paths modeled.
extern crate self as nanami_runtime;
pub type Word = usize;
use std::cell::{Cell, RefCell};

thread_local! {
    static CALLS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    static REQUESTS: RefCell<Vec<(Word, Word, Word)>> = const { RefCell::new(Vec::new()) };
    static POPULATE: Cell<Result<bool, ()>> = const { Cell::new(Ok(true)) };
}
fn record(call: &'static str) { CALLS.with_borrow_mut(|calls| calls.push(call)); }
pub fn request_process_populate_snapshot(pid: Word, page: Word, required: Word) -> Result<bool, ()> {
    record("populate");
    REQUESTS.with_borrow_mut(|rows| rows.push((pid, page, required)));
    POPULATE.with(Cell::get)
}
mod state {
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum OsPersonality { Linux, FreeBsd }
    pub struct Runtime {
        pub rights: Option<usize>,
        pub access: bool,
        pub bus: bool,
        pub cow: bool,
        pub stack: bool,
    }
    impl Runtime {
        pub fn mapping_prot(&self, pid: usize, page: usize, bytes: usize) -> Option<usize> {
            assert_eq!((pid, page, bytes), (7, 0x800000, 4096));
            self.rights
        }
        pub fn memory_access_allowed(&self, _: usize, _: usize, _: usize, _: usize) -> bool {
            self.access
        }
    }
}
use state::{OsPersonality, Runtime};
mod linux {
    use super::*;
    pub fn bus_fault(r: &Runtime, _: Word, _: Word) -> bool { record("bus"); r.bus }
    pub fn materialize_private_write_fault(r: &Runtime, _: Word, _: Word, _: Word) -> bool {
        record("cow"); r.cow
    }
    pub fn materialize_stack_fault(r: &Runtime, _: Word, _: Word, _: Word) -> bool {
        record("stack"); r.stack
    }
    pub(crate) use super::snapshot::materialize_snapshot_fault;
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/snapshot_fault.rs"]
mod snapshot;
#[path = "../../nanami/servers/apps/alter/shared/src/common/fault_memory.rs"]
mod production;
fn runtime(rights: Option<Word>) -> Runtime {
    CALLS.with_borrow_mut(Vec::clear);
    REQUESTS.with_borrow_mut(Vec::clear);
    POPULATE.with(|result| result.set(Ok(true)));
    Runtime { rights, access: true, bus: false, cow: false, stack: false }
}

#[test]
fn both_personalities_repair_missing_owned_zero_backing_with_current_rights() {
    for personality in [OsPersonality::Linux, OsPersonality::FreeBsd] {
        for (rights, code, required) in [(1, 4, 1), (3, 6, 2), (4, 20, 4), (5, 20, 4)] {
            let r = runtime(Some(rights));
            assert_eq!(production::materialize(&r, 7, personality, 0x800017, code), cfg!(target_arch = "x86_64"));
            REQUESTS.with_borrow(|rows| {
                assert_eq!(*rows, if cfg!(target_arch = "x86_64") { vec![(7, 0x800000, required)] } else { vec![] });
            });
        }
    }
}

#[test]
fn bus_faults_and_missing_or_denied_logical_rights_never_request_population() {
    for personality in [OsPersonality::Linux, OsPersonality::FreeBsd] {
        for mode in 0..6 {
            let mut r = runtime(Some(1));
            let code = match mode {
                0 => { r.bus = true; 4 },
                1 => { r.rights = None; 4 },
                2 => { r.rights = Some(0); 4 },
                3 => { r.access = false; 4 },
                4 => 6,
                _ => 5,
            };
            assert!(!production::materialize(&r, 7, personality, 0x800017, code));
            REQUESTS.with_borrow(|rows| assert!(rows.is_empty()));
            if r.bus {
                CALLS.with_borrow(|calls| assert_eq!(*calls, vec!["bus"]));
            }
        }
    }
}

#[test]
fn rejected_or_failed_authoritative_population_keeps_the_existing_fallback() {
    for personality in [OsPersonality::Linux, OsPersonality::FreeBsd] {
        for result in [Ok(false), Err(())] {
            let r = runtime(Some(1));
            POPULATE.with(|value| value.set(result));
            assert!(!production::materialize(&r, 7, personality, 0x800017, 4));
            CALLS.with_borrow(|calls| assert_eq!(calls.last(), Some(&"stack")));
        }
    }
}

#[test]
fn both_personalities_try_authoritative_cow_before_other_fault_repairs() {
    for personality in [OsPersonality::Linux, OsPersonality::FreeBsd] {
        let mut r = runtime(Some(3));
        r.cow = true;
        assert!(production::materialize(&r, 7, personality, 0x800017, 7));
        CALLS.with_borrow(|calls| assert_eq!(*calls, vec!["bus", "cow"]));
        let mut r = runtime(None);
        r.stack = true;
        assert!(production::materialize(&r, 7, personality, 0x800017, 4));
    }
}
