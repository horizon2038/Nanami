//! Production deferred retirement with only capability and RAM release mocked.
use super::*;
use std::cell::RefCell;
const PAGE_SIZE: usize = 4096;
const PROCESS_FRAME_CHUNK_PAGES: usize = 4;

#[derive(Default)]
struct Calls {
    removed: Vec<(usize, usize)>,
    fail_remove: Option<usize>,
}
thread_local! { static CALLS: RefCell<Calls> = RefCell::new(Calls::default()); }
mod arch {
    pub mod node {
        pub fn remove(node: usize, slot: usize) -> Result<(), crate::CapabilityError> {
            super::super::CALLS.with(|calls| {
                let mut calls = calls.borrow_mut();
                let index = calls.removed.len();
                calls.removed.push((node, slot));
                if calls.fail_remove == Some(index) {
                    return Err(crate::CapabilityError::Fatal);
                }
                Ok(())
            })
        }
    }
}
fn process_frame_chunk_descriptor(root: usize, chunk: usize) -> usize {
    root + chunk
}
#[derive(Default)]
struct Memory {
    freed: Vec<(usize, usize)>,
    fail_free: bool,
    fail_at: Option<usize>,
    attempts: usize,
}
impl Memory {
    fn free_physical(&mut self, address: usize, size: usize) -> Result<(), CapabilityError> {
        let index = self.attempts;
        self.attempts += 1;
        if self.fail_free || self.fail_at == Some(index) {
            return Err(CapabilityError::Fatal);
        }
        assert!(!self.freed.contains(&(address, size)), "double RAM release");
        self.freed.push((address, size));
        Ok(())
    }
}
struct Alpha {
    processes: ProcessManager,
    memory: Memory,
}
impl Alpha {
    fn new() -> Self {
        CALLS.with(|calls| *calls.borrow_mut() = Calls::default());
        let mut processes = manager();
        install(&mut processes, 1, 201);
        install(&mut processes, 2, 202);
        Self {
            processes,
            memory: Memory::default(),
        }
    }
    fn deferred(&mut self, pid: usize, slot: usize, physical: usize, pages: usize) {
        self.processes
            .register_physical_allocation(pid, slot * PAGE_SIZE, slot, physical, pages)
            .unwrap();
        self.processes
            .defer_active_physical_allocation(pid, slot * PAGE_SIZE, pages)
            .unwrap();
    }
}
#[path = "../../nanami/src/nanami_core/alpha/memory_retirement.rs"]
mod production;

#[test]
fn exec_removes_every_alias_before_releasing_the_shared_physical_extent_once() {
    let mut alpha = Alpha::new();
    for slot in [1, 3, 5] {
        alpha.deferred(1, slot, 77, 2);
    }
    assert_eq!(
        alpha.free_deferred_process_allocations(1, Some(100)),
        Ok(())
    );
    CALLS.with(|calls| {
        assert_eq!(
            calls.borrow().removed,
            [(101, 1), (101, 2), (100, 3), (101, 0), (100, 1), (100, 2)]
        );
    });
    assert_eq!(alpha.memory.freed, [(77 * PAGE_SIZE, 2 * PAGE_SIZE)]);
    assert_eq!(
        alpha
            .processes
            .deferred_physical_allocations_for_pid(1)
            .count(),
        0
    );
}

#[test]
fn exec_removes_local_caps_even_when_a_different_process_retains_ram() {
    let mut alpha = Alpha::new();
    alpha.deferred(1, 3, 77, 2);
    alpha
        .processes
        .register_physical_allocation(2, 0x1000, 1, 77, 2)
        .unwrap();
    assert_eq!(
        alpha.free_deferred_process_allocations(1, Some(100)),
        Ok(())
    );
    CALLS.with(|calls| assert_eq!(calls.borrow().removed, [(100, 3), (101, 0)]));
    assert!(alpha.memory.freed.is_empty());
    assert_eq!(
        alpha
            .processes
            .releasable_physical_allocations_for_pid(2)
            .len(),
        1
    );
}

#[test]
fn reap_after_root_revocation_needs_no_individual_capability_removal() {
    let mut alpha = Alpha::new();
    for slot in [1, 2, 3] {
        alpha.deferred(1, slot, 77, 1);
    }
    assert_eq!(alpha.free_deferred_process_allocations(1, None), Ok(()));
    CALLS.with(|calls| assert!(calls.borrow().removed.is_empty()));
    assert_eq!(alpha.memory.freed, [(77 * PAGE_SIZE, PAGE_SIZE)]);
    assert_eq!(alpha.free_deferred_process_allocations(1, None), Ok(()));
    assert_eq!(alpha.memory.freed.len(), 1);
}

#[test]
fn capability_failure_retains_ownership_and_never_releases_ram() {
    let mut alpha = Alpha::new();
    alpha.deferred(1, 3, 77, 2);
    CALLS.with(|calls| calls.borrow_mut().fail_remove = Some(1));
    assert_eq!(
        alpha.free_deferred_process_allocations(1, Some(100)),
        Err(CapabilityError::Fatal)
    );
    assert!(alpha.memory.freed.is_empty());
    assert_eq!(
        alpha
            .processes
            .deferred_physical_allocations_for_pid(1)
            .count(),
        1
    );
    assert_eq!(
        alpha
            .processes
            .releasable_deferred_physical_allocations_for_pid(1)
            .len(),
        1
    );
}

#[test]
fn ram_release_failure_does_not_silently_drop_deferred_ownership() {
    let mut alpha = Alpha::new();
    alpha.deferred(1, 3, 77, 2);
    alpha.memory.fail_free = true;
    assert_eq!(
        alpha.free_deferred_process_allocations(1, None),
        Err(CapabilityError::Fatal)
    );
    assert_eq!(
        alpha
            .processes
            .deferred_physical_allocations_for_pid(1)
            .count(),
        1
    );
    alpha.memory.fail_free = false;
    assert_eq!(alpha.free_deferred_process_allocations(1, None), Ok(()));
    assert_eq!(alpha.memory.freed, [(77 * PAGE_SIZE, 2 * PAGE_SIZE)]);
}

#[test]
fn raw_shared_vm_child_cleanup_never_drops_owner_references() {
    let mut alpha = Alpha::new();
    alpha.deferred(1, 3, 77, 2);
    alpha.processes.share_address_space(2, 1).unwrap();
    assert_eq!(alpha.free_deferred_process_allocations(2, None), Ok(()));
    assert!(alpha.memory.freed.is_empty());
    assert_eq!(
        alpha
            .processes
            .releasable_deferred_physical_allocations_for_pid(1)
            .len(),
        1
    );
}

#[test]
fn failed_ram_release_preserves_only_the_unreleased_suffix() {
    let mut alpha = Alpha::new();
    alpha.deferred(1, 1, 77, 1);
    alpha.deferred(1, 2, 99, 1);
    alpha.memory.fail_at = Some(1);
    assert_eq!(
        alpha.free_deferred_process_allocations(1, None),
        Err(CapabilityError::Fatal)
    );
    assert_eq!(alpha.memory.freed, [(99 * PAGE_SIZE, PAGE_SIZE)]);
    let remaining: Vec<_> = alpha
        .processes
        .deferred_physical_allocations_for_pid(1)
        .collect();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].base_page, 77);
    alpha.memory.fail_at = None;
    assert_eq!(alpha.free_deferred_process_allocations(1, None), Ok(()));
    assert_eq!(
        alpha.memory.freed,
        [(99 * PAGE_SIZE, PAGE_SIZE), (77 * PAGE_SIZE, PAGE_SIZE)]
    );
}

#[test]
fn failed_capability_removal_does_not_restore_already_freed_prefix_ownership() {
    let mut alpha = Alpha::new();
    alpha.deferred(1, 1, 77, 1);
    alpha.deferred(1, 2, 99, 1);
    CALLS.with(|calls| calls.borrow_mut().fail_remove = Some(1));
    assert_eq!(
        alpha.free_deferred_process_allocations(1, Some(100)),
        Err(CapabilityError::Fatal)
    );
    assert_eq!(alpha.memory.freed, [(99 * PAGE_SIZE, PAGE_SIZE)]);
    assert_eq!(
        alpha
            .processes
            .deferred_physical_allocations_for_pid(1)
            .count(),
        1
    );
    // The outer reaper may destroy the remaining root, then release only the
    // retained suffix. Do not retry an already removed capability slot.
    assert_eq!(alpha.free_deferred_process_allocations(1, None), Ok(()));
    assert_eq!(
        alpha.memory.freed,
        [(99 * PAGE_SIZE, PAGE_SIZE), (77 * PAGE_SIZE, PAGE_SIZE)]
    );
}
