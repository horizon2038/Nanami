use super::*;
use nanami_core::process::SharedMemoryReservation;

const SLOTS: usize = 1 << 19;

fn reserve(manager: &mut ProcessManager, pid: usize, pages: usize) -> SharedMemoryReservation {
    manager.reserve_shared_memory(pid, pages, SLOTS).unwrap()
}

#[test]
fn repeated_mixed_sizes_reuse_slots_and_virtual_space() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    let peak = reserve(&mut manager, 1, 512);
    manager.recycle_shared_memory(1, peak);
    // Exceeds the old 27 frame-node failure by many orders of magnitude,
    // within a tiny live working set and without increasing any limit.
    for _ in 0..10_000 {
        for pages in [1, 250, 252, 469, 4] {
            let r = reserve(&mut manager, 1, pages);
            assert_eq!(r.base_va, 0x200000);
            assert_eq!(r.start_slot, 0);
            manager.recycle_shared_memory(1, r);
        }
    }
    let entry = manager.find_entry_by_pid(1).unwrap();
    assert!(entry.next_frame_slot <= 512);
    assert!(entry.user_heap_next_va <= 0x400000);
}

#[test]
fn differently_ordered_holes_coalesce_without_reusing_live_ranges() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    let a = reserve(&mut manager, 1, 32);
    let b = reserve(&mut manager, 1, 64);
    let c = reserve(&mut manager, 1, 32);
    manager.recycle_shared_memory(1, c);
    manager.recycle_shared_memory(1, a);
    let d = reserve(&mut manager, 1, 16);
    assert_eq!(d.base_va, a.base_va);
    assert_eq!(manager.shared_memory_reservation(1, b.base_va, 64), Some(b));
    assert!(manager.shared_memory_reservation(1, b.base_va, 1).is_none());
    manager.recycle_shared_memory(1, b);
    manager.recycle_shared_memory(1, d);
    let entire = reserve(&mut manager, 1, 128);
    assert_eq!(entire.base_va, a.base_va);
    assert_eq!(entire.start_slot, a.start_slot);
}

#[test]
fn failed_reservation_does_not_consume_either_resource() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    let r = reserve(&mut manager, 1, 8);
    manager.recycle_shared_memory(1, r);
    let before = manager.find_entry_by_pid(1).unwrap();
    for pages in [0, 513, usize::MAX] {
        assert!(manager.reserve_shared_memory(1, pages, SLOTS).is_err());
    }
    assert!(manager.reserve_shared_memory(1, 32, 16).is_err());
    assert!(manager.reserve_shared_memory(0, 1, SLOTS).is_err());
    assert!(manager.reserve_shared_memory(99, 1, SLOTS).is_err());
    let after = manager.find_entry_by_pid(1).unwrap();
    assert_eq!(before.next_frame_slot, after.next_frame_slot);
    assert_eq!(before.user_heap_next_va, after.user_heap_next_va);
    assert_eq!(reserve(&mut manager, 1, 8), r);
}

#[test]
fn fixed_anonymous_mapping_excludes_shared_virtual_holes_not_slots() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    let r = reserve(&mut manager, 1, 16);
    manager.recycle_shared_memory(1, r);
    manager
        .reserve_process_heap_at(1, r.base_va + 4 * 4096, 8, 4096, SLOTS)
        .unwrap();
    let a = reserve(&mut manager, 1, 4);
    let b = reserve(&mut manager, 1, 4);
    assert_eq!(a.base_va, r.base_va);
    assert_eq!(b.base_va, r.base_va + 12 * 4096);
    assert_eq!(a.start_slot, r.start_slot);
    assert_eq!(b.start_slot, r.start_slot + 4);
}

#[test]
fn exec_reap_and_failed_spawn_discard_reservations_and_holes() {
    let mut manager = manager();
    for pid in 1..=3 {
        install(&mut manager, pid, 200 + pid);
        let r = reserve(&mut manager, pid, 8);
        manager.recycle_shared_memory(pid, r);
    }
    manager
        .reset_runtime_memory_for_exec(1, 100, 0x300000, 0x400000)
        .unwrap();
    let r = reserve(&mut manager, 1, 8);
    assert_eq!(r.start_slot, 100);
    assert_eq!(r.base_va, 0x300000);
    manager.mark_exited(2, 1, 0).unwrap();
    manager.reap_process(2, true).unwrap();
    manager.discard_process_artifacts(3, 203);
    for pid in 2..=3 {
        install(&mut manager, pid, 200 + pid);
        let (_, _, _, slot) = manager.reserve_process_heap(pid, 10, 4096, SLOTS).unwrap();
        assert_eq!(slot, 0);
        let r = reserve(&mut manager, pid, 8);
        assert_eq!(r.start_slot, 10);
        assert_eq!(r.base_va, 0x200000 + 10 * 4096);
    }
}

#[test]
fn shared_physical_pages_survive_either_release_order_and_peer_exec_or_reap() {
    for first in [1, 2] {
        for teardown in ["release", "exec", "reap"] {
            let mut manager = manager();
            let mut reservations = Vec::new();
            for pid in 1..=2 {
                install(&mut manager, pid, 200 + pid);
                let r = reserve(&mut manager, pid, 8);
                manager
                    .register_physical_allocation(pid, r.base_va, r.start_slot, 0x800, r.page_count)
                    .unwrap();
                reservations.push(r);
            }
            let r = reservations[first - 1];
            match teardown {
                "release" => {
                    assert!(
                        !manager
                            .release_physical_allocation_reference(first, r.base_va, r.page_count)
                            .unwrap()
                            .1
                    );
                    manager.recycle_shared_memory(first, r);
                }
                "exec" => {
                    let allocations = manager
                        .reset_runtime_memory_for_exec(first, 0, 0x200000, 0x400000)
                        .unwrap();
                    assert_eq!(allocations.len(), 1);
                    assert!(!allocations[0].1);
                }
                _ => {
                    assert!(manager
                        .releasable_physical_allocations_for_pid(first)
                        .is_empty());
                    manager.mark_exited(first, 1, 0).unwrap();
                    manager.reap_process(first, true).unwrap();
                }
            }
            let last = 3 - first;
            let r = reservations[last - 1];
            let (allocation, is_last) = manager
                .release_physical_allocation_reference(last, r.base_va, r.page_count)
                .unwrap();
            assert!(is_last);
            assert_eq!(allocation.base_page, 0x800);
        }
    }
}
