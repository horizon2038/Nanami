//! Production mapping-release transaction; only capability calls are mocked.
use super::*;
use nanami_core::process::{ProcessEntry, SharedMemoryReservation};
use std::cell::RefCell;
const PAGE_SIZE: usize = 4096;
#[derive(Clone, Copy)]
struct OsRequestEvent {
    identifier: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
}
#[derive(Default)]
struct Calls {
    unmaps: Vec<(usize, usize, usize)>,
    fail_at: Option<usize>,
}
thread_local! { static CALLS: RefCell<Calls> = RefCell::new(Calls::default()); }
mod arch {
    pub mod address_space {
        pub fn unmap(space: usize, frame: usize, va: usize) -> Result<(), crate::CapabilityError> {
            super::super::CALLS.with(|calls| {
                let mut calls = calls.borrow_mut();
                let index = calls.unmaps.len();
                calls.unmaps.push((space, frame, va));
                if calls.fail_at == Some(index) {
                    return Err(crate::CapabilityError::Fatal);
                }
                Ok(())
            })
        }
    }
}
fn process_frame_descriptor(root: usize, slot: usize) -> usize {
    root * 100000 + slot
}
struct Memory;
impl Memory {
    fn physical_frame_descriptor_from_index(&self, page: usize) -> Option<usize> { Some(1_000_000_000 + page) }
}
struct Alpha {
    memory: Memory,
    processes: ProcessManager,
    invalidations: usize,
    fail_invalidation: bool,
    shared_releases: usize,
}
impl Alpha {
    fn new() -> Self {
        CALLS.with(|calls| *calls.borrow_mut() = Calls::default());
        let mut processes = manager();
        install(&mut processes, 1, 201);
        install(&mut processes, 2, 202);
        processes.ensure_vm_space_for_pid(1).unwrap();
        Self {
            memory: Memory,
            processes,
            invalidations: 0,
            fail_invalidation: false,
            shared_releases: 0,
        }
    }
    fn page(&mut self, va: usize, slot: usize, permissions: usize) {
        self.processes
            .register_physical_allocation(1, va, slot, slot + 100, 1)
            .unwrap();
        let root = self.processes.memory_entry(1).unwrap().root_node;
        let vm = self.processes.vm_space_mut(1).unwrap();
        vm.record_frame(va, process_frame_descriptor(root, slot))
            .unwrap();
        vm.set_permissions(va, permissions);
    }
    fn invalidate_process_copy_mappings(&mut self, _: usize) -> Result<(), CapabilityError> {
        self.invalidations += 1;
        if self.fail_invalidation {
            return Err(CapabilityError::Fatal);
        }
        Ok(())
    }
    fn release_shared_memory(
        &mut self,
        _: ProcessEntry,
        _: SharedMemoryReservation,
    ) -> Result<(), CapabilityError> {
        self.shared_releases += 1;
        Ok(())
    }
    fn release(&mut self, base: usize, bytes: usize, sparse: bool) -> Result<(), CapabilityError> {
        self.handle_mapping_release_request(request(base, bytes, sparse))
    }
}
#[path = "../../nanami/src/nanami_core/alpha/mapping_release.rs"]
mod production;
fn request(base: usize, bytes: usize, sparse: bool) -> OsRequestEvent {
    OsRequestEvent {
        identifier: 1,
        arg0: base,
        arg1: bytes,
        arg2: 1,
        arg3: if sparse {
            nanami_uapi::os::OS_MAPPING_RELEASE_ALLOW_HOLES
        } else {
            0
        },
    }
}

#[test]
fn releasing_an_old_large_mapping_keeps_newer_backing_and_defers_in_scalar_order() {
    let mut alpha = Alpha::new();
    for page in 0..513 { alpha.page(0x1000 + page * PAGE_SIZE, page, 3); }
    for page in 0..512 { alpha.page(0x10000000 + page * PAGE_SIZE, 1000 + page, 3); }
    alpha.processes.take_mutable_vm_lookups();
    alpha.release(0x1000, 513 * PAGE_SIZE, false).unwrap();
    let lookups = alpha.processes.take_mutable_vm_lookups();
    println!("RELEASE-VM strict-pages=513 mutable-lookups={lookups}");
    assert_eq!(lookups, 1);
    let deferred: Vec<_> = alpha.processes.deferred_physical_allocations_for_pid(1).collect();
    assert_eq!(deferred.len(), 513);
    for (i, allocation) in deferred.iter().enumerate() {
        assert_eq!(allocation.base_va, 0x1000 + i * PAGE_SIZE);
        assert_eq!(alpha.processes.physical_reference_count(*allocation), 1);
    }
    assert_eq!(alpha.processes.releasable_physical_allocations_for_pid(1).len(), 512);
    let vm = alpha.processes.vm_space_mut(1).unwrap();
    assert!(vm.find_frame(0x1000).is_none());
    assert!(vm.find_frame(0x10000000).is_some());
    CALLS.with(|calls| assert_eq!(calls.borrow().unmaps.len(), 513));
}

#[test]
fn sparse_release_walks_backing_not_a_terabyte_of_unpopulated_addresses() {
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 1, 3);
    alpha.page(0x10_0000_0000, 2, 0);
    alpha.processes.take_mutable_vm_lookups();
    assert_eq!(alpha.release(0x1000, 0x100_0000_0000, true), Ok(()));
    let lookups = alpha.processes.take_mutable_vm_lookups();
    println!("RELEASE-VM sparse-pages=2 mutable-lookups={lookups}");
    assert_eq!(lookups, 2);
    CALLS.with(|calls| assert_eq!(calls.borrow().unmaps.len(), 1));
    assert_eq!(
        alpha
            .processes
            .releasable_physical_allocations_for_pid(1)
            .len(),
        0
    );
    assert_eq!(
        alpha
            .processes
            .releasable_deferred_physical_allocations_for_pid(1)
            .len(),
        2
    );
    assert!(alpha
        .processes
        .vm_space_mut(1)
        .unwrap()
        .backed_pages(0, usize::MAX)
        .is_empty());
    alpha.processes.take_mutable_vm_lookups();
    assert_eq!(alpha.release(0x1000, 0x100_0000_0000, true), Ok(()));
    let lookups = alpha.processes.take_mutable_vm_lookups();
    assert_eq!(lookups, 1); // Existing sparse validation only, no update lookup.
    assert_eq!(alpha.invalidations, 2);
    CALLS.with(|calls| assert_eq!(calls.borrow().unmaps.len(), 1));
    println!("RELEASE-VM empty-sparse mutable-lookups={lookups}");
}

#[test]
fn canonical_ram_reservations_reject_partial_and_crossing_releases_before_unmap() {
    for canonical in [false, true] {
        let mut alpha = Alpha::new();
        let r = alpha
            .processes
            .reserve_shared_memory_at(1, 3, 1024, 0x2000)
            .unwrap();
        alpha
            .processes
            .register_physical_allocation(1, r.base_va, r.start_slot, 100, 3)
            .unwrap();
        if canonical {
            alpha.processes.canonicalize_shared_memory(1, r).unwrap();
        }
        let root = alpha.processes.memory_entry(1).unwrap().root_node;
        for page in 0..3 {
            alpha
                .processes
                .vm_space_mut(1)
                .unwrap()
                .record_frame(
                    r.base_va + page * PAGE_SIZE,
                    process_frame_descriptor(root, r.start_slot + page),
                )
                .unwrap();
        }
        for sparse in [false, true] {
            for (base, bytes) in [
                (0x2000, PAGE_SIZE),
                (0x3000, PAGE_SIZE),
                (0x1000, 2 * PAGE_SIZE),
                (0x4000, 2 * PAGE_SIZE),
                (0x1000, 5 * PAGE_SIZE),
            ] {
                assert_eq!(
                    alpha.release(base, bytes, sparse),
                    Err(CapabilityError::InvalidArgument)
                );
                assert_eq!(alpha.invalidations, 0);
                assert_eq!(alpha.shared_releases, 0);
                CALLS.with(|calls| assert!(calls.borrow().unmaps.is_empty()));
                assert_eq!(
                    alpha
                        .processes
                        .vm_space_mut(1)
                        .unwrap()
                        .backed_pages(0, usize::MAX)
                        .len(),
                    3
                );
            }
        }
        assert_eq!(alpha.release(r.base_va, 3 * PAGE_SIZE, false), Ok(()));
        assert_eq!(alpha.shared_releases, 1);
    }
}

#[test]
fn sparse_validation_rejects_unreferenced_keys_between_and_after_owned_pages() {
    for missing in [0x1001, 0x2000, 0x3fff, 0x5000] {
        let mut alpha = Alpha::new();
        for (va, slot) in [(0x1000, 1), (0x3000, 3), (0x4000, 4)] {
            alpha.page(va, slot, 0);
        }
        let vm = alpha.processes.vm_space_mut(1).unwrap();
        vm.record_frame(missing, 999).unwrap();
        vm.set_permissions(missing, 0);
        vm.mark_mapping_uncertain(missing);
        assert_eq!(alpha.release(0x1000, 5 * PAGE_SIZE, true), Err(CapabilityError::InvalidArgument));
        assert_eq!(alpha.invalidations, 0);
        CALLS.with(|calls| assert!(calls.borrow().unmaps.is_empty()));
        assert_eq!(alpha.processes.releasable_physical_allocations_for_pid(1).len(), 3);
        assert_eq!(alpha.processes.deferred_physical_allocations_for_pid(1).count(), 0);
        assert!(alpha.processes.vm_space_mut(1).unwrap().mapping_is_uncertain(missing));
    }
}

#[test]
fn strict_holes_and_unreferenced_prot_none_leaves_fail_before_any_mutation() {
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 1, 3);
    alpha.page(0x3000, 3, 3);
    assert_eq!(
        alpha.release(0x1000, 3 * PAGE_SIZE, false),
        Err(CapabilityError::InvalidArgument)
    );
    let vm = alpha.processes.vm_space_mut(1).unwrap();
    vm.record_frame(0x2000, 999).unwrap();
    vm.set_permissions(0x2000, 0);
    assert_eq!(
        alpha.release(0x1000, 3 * PAGE_SIZE, true),
        Err(CapabilityError::InvalidArgument)
    );
    assert_eq!(alpha.invalidations, 0);
    CALLS.with(|calls| assert!(calls.borrow().unmaps.is_empty()));
    assert_eq!(
        alpha
            .processes
            .releasable_physical_allocations_for_pid(1)
            .len(),
        2
    );
}

#[test]
fn failed_unmap_keeps_all_references_active_until_retry_finishes() {
    let mut alpha = Alpha::new();
    for page in 1..=3 {
        alpha.page(page * PAGE_SIZE, page, 3);
    }
    CALLS.with(|calls| calls.borrow_mut().fail_at = Some(1));
    assert_eq!(
        alpha.release(PAGE_SIZE, 3 * PAGE_SIZE, false),
        Err(CapabilityError::Fatal)
    );
    assert!(alpha
        .processes
        .releasable_deferred_physical_allocations_for_pid(1)
        .is_empty());
    assert_eq!(
        alpha
            .processes
            .releasable_physical_allocations_for_pid(1)
            .len(),
        3
    );
    assert!(alpha
        .processes
        .vm_space_mut(1)
        .unwrap()
        .find_frame(PAGE_SIZE)
        .is_none());
    assert!(alpha
        .processes
        .vm_space_mut(1)
        .unwrap()
        .find_frame(2 * PAGE_SIZE)
        .is_some());
    CALLS.with(|calls| calls.borrow_mut().fail_at = None);
    assert_eq!(alpha.release(PAGE_SIZE, 3 * PAGE_SIZE, false), Ok(()));
    assert_eq!(
        alpha
            .processes
            .releasable_deferred_physical_allocations_for_pid(1)
            .len(),
        3
    );
}

#[test]
fn exact_extents_round_lengths_and_preserve_every_frame_slot() {
    let mut alpha = Alpha::new();
    alpha
        .processes
        .register_physical_allocation(1, 0x1000, 70, 100, 3)
        .unwrap();
    let entry = alpha.processes.memory_entry(1).unwrap();
    for page in 0..3 {
        alpha
            .processes
            .vm_space_mut(1)
            .unwrap()
            .record_frame(
                0x1000 + page * PAGE_SIZE,
                process_frame_descriptor(entry.root_node, 70 + page),
            )
            .unwrap();
    }
    assert_eq!(alpha.release(0x1000, 2 * PAGE_SIZE + 1, false), Ok(()));
    CALLS.with(|calls| {
        let expected: Vec<_> = (0..3)
            .map(|page| {
                (
                    entry.address_space,
                    process_frame_descriptor(entry.root_node, 70 + page),
                    0x1000 + page * PAGE_SIZE,
                )
            })
            .collect();
        assert_eq!(calls.borrow().unmaps, expected);
    });
}

#[test]
fn authority_overflow_and_copy_window_failure_never_release_references() {
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 1, 3);
    let mut denied = request(0x1000, PAGE_SIZE, false);
    denied.identifier = 2;
    assert_eq!(
        alpha.handle_mapping_release_request(denied),
        Err(CapabilityError::PermissionDenied)
    );
    for (base, bytes) in [
        (0x1000, usize::MAX),
        (usize::MAX & !(PAGE_SIZE - 1), PAGE_SIZE),
    ] {
        assert_eq!(
            alpha.release(base, bytes, true),
            Err(CapabilityError::InvalidArgument)
        );
    }
    alpha.fail_invalidation = true;
    assert_eq!(
        alpha.release(0x1000, PAGE_SIZE, false),
        Err(CapabilityError::Fatal)
    );
    CALLS.with(|calls| assert!(calls.borrow().unmaps.is_empty()));
    assert!(alpha
        .processes
        .find_active_physical_allocation_reference(1, 0x1000, 1)
        .is_some());
}

#[test]
fn snapshot_preserves_latest_duplicate_and_shared_vm_owner_rules() {
    let mut alpha = Alpha::new();
    for slot in 1..=5 {
        alpha
            .processes
            .register_physical_allocation(1, 0x3000, slot, slot + 100, 1)
            .unwrap();
    }
    alpha.page(0x1000, 8, 7);
    alpha.processes.share_address_space(2, 1).unwrap();
    let expected = alpha
        .processes
        .find_active_physical_allocation_reference(1, 0x3000, 1)
        .unwrap();
    let pages = alpha.processes.physical_pages_in_range(2, 0x1000, 0x4000);
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].base_va, 0x1000);
    assert_eq!(pages[1].start_slot, expected.start_slot);
    let mut shared = request(0x1000, PAGE_SIZE, false);
    shared.identifier = 2;
    shared.arg2 = 2;
    assert_eq!(alpha.handle_mapping_release_request(shared), Ok(()));
    assert!(alpha
        .processes
        .vm_space_mut(1)
        .unwrap()
        .find_frame(0x1000)
        .is_none());
}

#[test]
fn whole_shared_reservation_dispatches_to_existing_shared_release_handler() {
    let mut alpha = Alpha::new();
    let reservation = alpha
        .processes
        .reserve_shared_memory(1, 2, 1 << 19)
        .unwrap();
    assert_eq!(
        alpha.release(reservation.base_va, 2 * PAGE_SIZE, true),
        Ok(())
    );
    assert_eq!(alpha.shared_releases, 1);
    assert_eq!(alpha.invalidations, 0);
}

#[test]
fn mixed_order_sparse_release_preserves_newest_duplicates_and_deferred_records() {
    let mut alpha = Alpha::new();
    alpha.page(0x3000, 3, 3);
    alpha.page(0x1000, 1, 7);
    alpha.page(0x5000, 5, 0);
    alpha.page(0x3000, 30, 3);
    alpha.page(0x9000, 9, 7);
    assert_eq!(alpha.release(0x1000, 5 * PAGE_SIZE, true), Ok(()));
    let remaining = alpha
        .processes
        .find_active_physical_allocation_reference(1, 0x3000, 1)
        .unwrap();
    assert_eq!(remaining.start_slot, 3);
    assert!(alpha
        .processes
        .find_active_physical_allocation_reference(1, 0x9000, 1)
        .is_some());
    let mut deferred = alpha
        .processes
        .releasable_deferred_physical_allocations_for_pid(1);
    deferred.sort_by_key(|allocation| allocation.base_va);
    assert_eq!(
        deferred
            .iter()
            .map(|a| (a.base_va, a.start_slot))
            .collect::<Vec<_>>(),
        vec![(0x1000, 1), (0x3000, 30), (0x5000, 5)]
    );
    CALLS.with(|calls| {
        let calls = calls.borrow();
        assert_eq!(
            calls.unmaps.iter().map(|call| call.2).collect::<Vec<_>>(),
            vec![0x1000, 0x3000]
        );
    });
}

#[test]
fn pool_alias_release_uses_pool_frame_and_preserves_deferred_cap_state() {
    let mut alpha = Alpha::new();
    alpha.processes.register_pool_alias(1, PAGE_SIZE, 7, 99).unwrap();
    alpha.processes.vm_space_mut(1).unwrap().record_frame(PAGE_SIZE, 1_000_000_099).unwrap();
    assert_eq!(alpha.release(PAGE_SIZE, PAGE_SIZE, false), Ok(()));
    CALLS.with(|calls| assert_eq!(calls.borrow().unmaps[0].1, 1_000_000_099));
    let pages: Vec<_> = alpha.processes.deferred_physical_allocations_for_pid(1).collect();
    assert_eq!(pages.len(), 1);
    assert!(!pages[0].owns_frame_cap);
}

#[test]
fn every_failed_unmap_prefix_keeps_all_active_ownership_until_retry() {
    for fail in 0..4 {
        let mut alpha = Alpha::new();
        for page in [3, 1, 4, 2] {
            alpha.page(page * PAGE_SIZE, page, 3);
        }
        CALLS.with(|calls| calls.borrow_mut().fail_at = Some(fail));
        alpha.processes.take_mutable_vm_lookups();
        assert_eq!(
            alpha.release(PAGE_SIZE, 4 * PAGE_SIZE, false),
            Err(CapabilityError::Fatal)
        );
        let failed_lookups = alpha.processes.take_mutable_vm_lookups();
        {
            let vm = alpha.processes.vm_space_mut(1).unwrap();
            for page in 1..=4 {
                let va = page * PAGE_SIZE;
                assert_eq!(vm.find_frame(va).is_none(), page <= fail);
                assert_eq!(vm.mapping_is_uncertain(va), page == fail + 1);
            }
        }
        assert_eq!(
            alpha
                .processes
                .releasable_physical_allocations_for_pid(1)
                .len(),
            4
        );
        assert!(alpha
            .processes
            .releasable_deferred_physical_allocations_for_pid(1)
            .is_empty());
        CALLS.with(|calls| calls.borrow_mut().fail_at = None);
        alpha.processes.take_mutable_vm_lookups();
        assert_eq!(alpha.release(PAGE_SIZE, 4 * PAGE_SIZE, false), Ok(()));
        let retry_lookups = alpha.processes.take_mutable_vm_lookups();
        println!("RELEASE-VM fail-index={fail} failed-lookups={failed_lookups} retry-lookups={retry_lookups}");
        assert_eq!((failed_lookups, retry_lookups), (1, 1));
        assert!(alpha
            .processes
            .releasable_physical_allocations_for_pid(1)
            .is_empty());
        assert_eq!(
            alpha
                .processes
                .releasable_deferred_physical_allocations_for_pid(1)
                .len(),
            4
        );
    }
}
