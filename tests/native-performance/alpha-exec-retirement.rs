//! Production sealed-exec retirement with real VM/global RAM ownership indices.
use super::*;
use std::{cell::RefCell, collections::BTreeSet};
const PAGE_SIZE: usize = 4096;
const PROCESS_FRAME_CHUNK_PAGES: usize = 4;
const USER_HEAP_LIMIT: usize = 0x4000_0000;
#[derive(Debug, PartialEq, Eq)]
enum Call {
    Invalidate,
    Unmap(usize),
    Revoke(usize),
    Free(usize),
}
#[derive(Default)]
struct Calls {
    events: Vec<Call>,
    leaves: BTreeSet<usize>,
    caps: BTreeSet<usize>,
    fail_unmap: Option<usize>,
    fail_revoke: Option<usize>,
    fail_free: Option<usize>,
    free_attempts: usize,
    freed: BTreeSet<usize>,
}
thread_local! { static CALLS: RefCell<Calls> = RefCell::new(Calls::default()); }
fn process_frame_directory_descriptor(root: usize) -> usize {
    root + 1000
}
fn process_frame_chunk_descriptor(root: usize, chunk: usize) -> usize {
    root + chunk
}
mod arch {
    use super::*;
    pub mod address_space {
        use super::*;
        pub fn unmap(_: usize, _: usize, va: usize) -> Result<(), CapabilityError> {
            CALLS.with(|calls| {
                let mut calls = calls.borrow_mut();
                calls.events.push(Call::Unmap(va));
                if calls.fail_unmap == Some(va) {
                    return Err(CapabilityError::Fatal);
                }
                assert!(calls.leaves.remove(&va));
                Ok(())
            })
        }
    }
    pub mod node {
        use super::*;
        pub fn revoke(directory: usize, chunk: usize) -> Result<(), CapabilityError> {
            assert_eq!(directory, process_frame_directory_descriptor(202));
            CALLS.with(|calls| {
                let mut calls = calls.borrow_mut();
                assert!(
                    calls.leaves.is_empty(),
                    "caps cleared before all old leaves were unmapped"
                );
                calls.events.push(Call::Revoke(chunk));
                if calls.fail_revoke == Some(chunk) {
                    return Err(CapabilityError::Fatal);
                }
                calls.caps.remove(&chunk);
                Ok(())
            })
        }
        pub fn remove(_: usize, _: usize) -> Result<(), CapabilityError> {
            panic!("bulk-cleared deferred Frames must not be individually removed again");
        }
    }
}
struct Memory;
impl Memory {
    fn free_physical(&mut self, address: usize, _: usize) -> Result<(), CapabilityError> {
        CALLS.with(|calls| {
            let mut calls = calls.borrow_mut();
            assert!(calls.leaves.is_empty());
            assert!(
                calls.caps.is_empty(),
                "RAM freed before all Frame aliases disappeared"
            );
            let index = calls.free_attempts;
            calls.free_attempts += 1;
            if calls.fail_free == Some(index) {
                return Err(CapabilityError::Fatal);
            }
            assert!(calls.freed.insert(address), "RAM released twice");
            calls.events.push(Call::Free(address));
            Ok(())
        })
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
        processes.ensure_vm_space_for_pid(1).unwrap();
        processes.seal_capabilities(1).unwrap();
        Self {
            processes,
            memory: Memory,
        }
    }
    fn invalidate_process_copy_mappings(&mut self, _: usize) -> Result<(), CapabilityError> {
        CALLS.with(|calls| calls.borrow_mut().events.push(Call::Invalidate));
        Ok(())
    }
    fn page(&mut self, slot: usize, physical: usize, permissions: usize) {
        let va = (slot + 1) * PAGE_SIZE;
        self.processes
            .register_physical_allocation(1, va, slot, physical, 1)
            .unwrap();
        self.processes
            .register_frame_chunk(1, slot / PROCESS_FRAME_CHUNK_PAGES)
            .unwrap();
        let vm = self.processes.vm_space_mut(1).unwrap();
        vm.record_frame(va, slot + 1000).unwrap();
        vm.set_permissions(va, permissions);
        CALLS.with(|calls| {
            let mut calls = calls.borrow_mut();
            if permissions != 0 {
                calls.leaves.insert(va);
            }
            calls.caps.insert(slot / PROCESS_FRAME_CHUNK_PAGES);
        });
    }
    fn retire(&mut self) -> Result<(), CapabilityError> {
        self.drop_sealed_process_runtime_mappings(1, self.processes.find_entry_by_pid(1).unwrap())
    }
}
#[path = "../../nanami/src/nanami_core/alpha/memory_retirement.rs"]
mod deferred;
#[path = "../../nanami/src/nanami_core/alpha/memory_exec.rs"]
mod production;

#[test]
fn sealed_exec_unmaps_then_bulk_clears_chunks_before_releasing_ram() {
    let mut a = Alpha::new();
    for (slot, permissions) in [(0, 5), (1, 0), (4, 3), (9, 1)] {
        a.page(slot, 100 + slot, permissions);
    }
    a.retire().unwrap();
    CALLS.with(|calls| {
        let calls = calls.borrow();
        assert_eq!(
            &calls.events[..7],
            &[
                Call::Invalidate,
                Call::Unmap(4096),
                Call::Unmap(20480),
                Call::Unmap(40960),
                Call::Revoke(0),
                Call::Revoke(1),
                Call::Revoke(2)
            ]
        );
        assert_eq!(calls.freed.len(), 4);
    });
    assert_eq!(
        a.processes.frame_chunk_indices(1).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert!(a
        .processes
        .vm_space_mut(1)
        .unwrap()
        .backed_pages(0, usize::MAX)
        .is_empty());
    assert_eq!(
        a.processes.deferred_physical_allocations_for_pid(1).count(),
        0
    );
}

#[test]
fn sealed_exec_does_not_treat_an_uncertain_pte_as_prot_none() {
    let mut a = Alpha::new();
    a.page(0, 100, 3);
    a.processes
        .vm_space_mut(1)
        .unwrap()
        .mark_mapping_uncertain(PAGE_SIZE);
    assert_eq!(
        a.processes.vm_space_mut(1).unwrap().permissions(PAGE_SIZE),
        0
    );
    a.retire().unwrap();
    CALLS.with(|calls| {
        let calls = calls.borrow();
        assert!(calls
            .events
            .iter()
            .any(|call| *call == Call::Unmap(PAGE_SIZE)));
        assert_eq!(calls.freed.len(), 1);
    });
}

#[test]
fn active_and_deferred_aliases_release_once_but_never_free_a_peers_backing() {
    for peer in [false, true] {
        let mut a = Alpha::new();
        a.page(0, 77, 1);
        a.page(1, 77, 0);
        a.processes
            .defer_active_physical_allocation(1, 8192, 1)
            .unwrap();
        a.processes.vm_space_mut(1).unwrap().forget_frame(8192);
        if peer {
            a.processes
                .register_physical_allocation(2, 4096, 0, 77, 1)
                .unwrap();
        }
        a.retire().unwrap();
        CALLS.with(|calls| assert_eq!(calls.borrow().freed.len(), usize::from(!peer)));
        assert_eq!(
            a.processes.deferred_physical_allocations_for_pid(1).count(),
            0
        );
        if peer {
            assert_eq!(
                a.processes.releasable_physical_allocations_for_pid(2).len(),
                1
            );
        }
    }
}

#[test]
fn unmap_and_chunk_failures_keep_all_ram_owned_for_kill_reap() {
    for stage in 0..3 {
        let mut a = Alpha::new();
        a.page(0, 100, 3);
        a.page(4, 104, 3);
        CALLS.with(|calls| {
            let mut calls = calls.borrow_mut();
            if stage == 0 {
                calls.fail_unmap = Some(20480);
            } else {
                calls.fail_revoke = Some(stage - 1);
            }
        });
        assert_eq!(a.retire(), Err(CapabilityError::Fatal));
        assert_eq!(
            a.processes.releasable_physical_allocations_for_pid(1).len(),
            2
        );
        CALLS.with(|calls| assert!(calls.borrow().freed.is_empty()));
    }
}

#[test]
fn failed_ram_release_commits_prefix_and_retains_suffix_without_double_free() {
    let mut a = Alpha::new();
    for slot in 0..3 {
        a.page(slot, 100 + slot, 3);
    }
    CALLS.with(|calls| calls.borrow_mut().fail_free = Some(1));
    assert_eq!(a.retire(), Err(CapabilityError::Fatal));
    assert_eq!(
        a.processes.deferred_physical_allocations_for_pid(1).count(),
        2
    );
    assert!(a
        .processes
        .releasable_physical_allocations_for_pid(1)
        .is_empty());
    CALLS.with(|calls| calls.borrow_mut().fail_free = None);
    a.free_deferred_process_allocations(1, None).unwrap();
    CALLS.with(|calls| assert_eq!(calls.borrow().freed.len(), 3));
}

#[test]
fn only_sealed_exclusive_vms_can_use_bulk_exec_retirement() {
    let mut a = Alpha::new();
    a.processes.share_address_space(2, 1).unwrap();
    assert_eq!(a.retire(), Err(CapabilityError::IllegalOperation));
    CALLS.with(|calls| assert!(calls.borrow().events.is_empty()));
}
