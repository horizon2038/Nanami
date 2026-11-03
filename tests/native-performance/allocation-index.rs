//! Compare indexed ownership bookkeeping to the original full-scan rules.
use super::*;
use nanami_core::process::ProcessPhysicalAllocation;

#[derive(Clone, Copy)]
struct Allocation {
    pid: usize,
    value: ProcessPhysicalAllocation,
}

fn key(value: ProcessPhysicalAllocation) -> (usize, usize) {
    (value.base_page, value.page_count)
}
fn snapshot(
    values: impl IntoIterator<Item = ProcessPhysicalAllocation>,
) -> Vec<(usize, usize, usize, usize)> {
    let mut out: Vec<_> = values
        .into_iter()
        .map(|v| (v.base_va, v.start_slot, v.base_page, v.page_count))
        .collect();
    out.sort_unstable();
    out
}

fn retirement_model(
    local: &[Allocation],
    other: &[Allocation],
    pid: usize,
) -> Vec<ProcessPhysicalAllocation> {
    let mut ranges = std::collections::BTreeMap::<_, Vec<_>>::new();
    for entry in local.iter().filter(|entry| entry.pid == pid) {
        ranges
            .entry(key(entry.value))
            .or_default()
            .push(entry.value);
    }
    ranges
        .into_iter()
        .filter_map(|(range, aliases)| {
            let total = local
                .iter()
                .chain(other)
                .filter(|entry| key(entry.value) == range)
                .count();
            if total != aliases.len() {
                return None;
            }
            aliases
                .into_iter()
                .min_by_key(|allocation| (allocation.base_va, allocation.start_slot))
        })
        .collect()
}

#[test]
fn indexed_counts_match_full_scan_with_aliases_defer_reuse_drop_and_exec() {
    let mut manager = manager();
    for pid in 1..=8 {
        install(&mut manager, pid, pid + 200);
    }
    let mut active: Vec<Allocation> = Vec::new();
    let mut deferred: Vec<Allocation> = Vec::new();
    let mut seed = 39u64;
    for step in 1..=4000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let pid = 1 + (seed >> 8) as usize % 8;
        match seed % 13 {
            0..=6 => {
                let value = ProcessPhysicalAllocation {
                    base_va: step * 4096,
                    start_slot: step,
                    base_page: 1 + (seed >> 17) as usize % 32,
                    page_count: 1 + (seed >> 24) as usize % 3,
                };
                manager
                    .register_physical_allocation(
                        pid,
                        value.base_va,
                        value.start_slot,
                        value.base_page,
                        value.page_count,
                    )
                    .unwrap();
                active.push(Allocation { pid, value });
            }
            7 | 8 if !active.is_empty() => {
                let index = (seed >> 17) as usize % active.len();
                let entry = active[index];
                let count = active
                    .iter()
                    .chain(&deferred)
                    .filter(|a| key(a.value) == key(entry.value))
                    .count();
                if seed % 13 == 8 {
                    let moved = manager
                        .defer_active_physical_allocation(
                            entry.pid,
                            entry.value.base_va,
                            entry.value.page_count,
                        )
                        .unwrap();
                    assert_eq!(snapshot([moved]), snapshot([entry.value]));
                    deferred.push(entry);
                } else {
                    let (removed, last) = manager
                        .release_physical_allocation_reference(
                            entry.pid,
                            entry.value.base_va,
                            entry.value.page_count,
                        )
                        .unwrap();
                    assert_eq!(snapshot([removed]), snapshot([entry.value]));
                    assert_eq!(last, count == 1);
                }
                active.swap_remove(index);
            }
            9 if !deferred.is_empty() => {
                let index = (seed >> 17) as usize % deferred.len();
                let entry = deferred[index];
                let unique = active
                    .iter()
                    .chain(&deferred)
                    .filter(|a| key(a.value) == key(entry.value))
                    .count()
                    == 1;
                let removed = manager.take_deferred_physical_allocation(
                    entry.pid,
                    entry.value.base_va,
                    entry.value.page_count,
                );
                if unique {
                    assert_eq!(snapshot([removed.unwrap()]), snapshot([entry.value]));
                    deferred.swap_remove(index);
                } else {
                    assert!(removed.is_none());
                }
            }
            10 => {
                manager.drop_physical_allocations_for_pid(pid);
                active.retain(|a| a.pid != pid);
                deferred.retain(|a| a.pid != pid);
            }
            11 => {
                let owned: Vec<_> = active
                    .iter()
                    .filter(|a| a.pid == pid)
                    .map(|a| a.value)
                    .collect();
                let removed = manager
                    .reset_runtime_memory_for_exec(pid, 0, 0x100000, 0x70000000)
                    .unwrap();
                assert_eq!(snapshot(removed.iter().map(|(v, _)| *v)), snapshot(owned));
                active.retain(|a| a.pid != pid);
                let mut freed = std::collections::BTreeSet::new();
                for (value, last) in removed {
                    if last {
                        assert!(freed.insert(key(value)), "double free");
                        assert!(
                            !active
                                .iter()
                                .chain(&deferred)
                                .any(|a| key(a.value) == key(value)),
                            "freed a shared range"
                        );
                    }
                }
            }
            12 => {
                manager.drop_deferred_physical_allocations_for_pid(pid);
                deferred.retain(|a| a.pid != pid);
            }
            _ => {}
        }
        for pid in 1..=8 {
            let expected = retirement_model(&active, &deferred, pid);
            assert_eq!(
                snapshot(manager.releasable_physical_allocations_for_pid(pid)),
                snapshot(expected),
                "active step={step}"
            );
            let expected = retirement_model(&deferred, &active, pid);
            assert_eq!(
                snapshot(manager.releasable_deferred_physical_allocations_for_pid(pid)),
                snapshot(expected),
                "deferred step={step}"
            );
        }
    }
}

#[test]
fn same_pid_aliases_have_exactly_one_last_reference_during_exec() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    for offset in 0..4096 {
        manager
            .register_physical_allocation(1, offset * 4096, offset, 1, 1)
            .unwrap();
    }
    let removed = manager
        .reset_runtime_memory_for_exec(1, 0, 0x100000, 0x70000000)
        .unwrap();
    assert_eq!(removed.len(), 4096);
    assert_eq!(removed.iter().filter(|(_, last)| *last).count(), 1);
    assert!(manager
        .releasable_physical_allocations_for_pid(1)
        .is_empty());
    assert!(manager
        .release_physical_allocation_reference(1, 0, 1)
        .is_err());
}

#[test]
fn retirement_snapshot_releases_same_pid_active_aliases_once_without_mutating_references() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    for slot in 1..=3 {
        manager
            .register_physical_allocation(1, slot * 4096, slot, 77, 1)
            .unwrap();
    }
    let ready = manager.releasable_physical_allocations_for_pid(1);
    assert_eq!(ready.len(), 1, "the final local alias must release its RAM");
    assert_eq!(key(ready[0]), (77, 1));
    assert_eq!(manager.physical_pages_in_range(1, 0, usize::MAX).len(), 3);
    assert_eq!(manager.releasable_physical_allocations_for_pid(1).len(), 1);
    for slot in (1..=3).rev() {
        let (_, last) = manager
            .release_physical_allocation_reference(1, slot * 4096, 1)
            .unwrap();
        assert_eq!(last, slot == 1);
    }
}

#[test]
fn retirement_snapshot_releases_same_pid_deferred_aliases_once() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    for slot in 1..=3 {
        manager
            .register_physical_allocation(1, slot * 4096, slot, 77, 1)
            .unwrap();
        manager
            .defer_active_physical_allocation(1, slot * 4096, 1)
            .unwrap();
    }
    let ready = manager.releasable_deferred_physical_allocations_for_pid(1);
    assert_eq!(
        ready.len(),
        1,
        "deferred aliases also own the final reference"
    );
    assert_eq!(key(ready[0]), (77, 1));
    for slot in 1..=3 {
        assert!(manager
            .take_deferred_physical_allocation(1, slot * 4096, 1)
            .is_none());
    }
}

#[test]
fn retirement_stages_free_mixed_aliases_only_after_the_final_local_stage() {
    for active_count in 0..=3 {
        for deferred_count in 0..=3 {
            if active_count + deferred_count == 0 {
                continue;
            }
            for peer_active in 0..=2 {
                for peer_deferred in 0..=2 {
                    let mut manager = manager();
                    install(&mut manager, 1, 201);
                    install(&mut manager, 2, 202);
                    for (pid, active, deferred) in [
                        (1, active_count, deferred_count),
                        (2, peer_active, peer_deferred),
                    ] {
                        for slot in 1..=active + deferred {
                            manager
                                .register_physical_allocation(pid, slot * 4096, slot, 77, 1)
                                .unwrap();
                            if slot > active {
                                manager
                                    .defer_active_physical_allocation(pid, slot * 4096, 1)
                                    .unwrap();
                            }
                        }
                    }
                    let mut freed = manager.releasable_physical_allocations_for_pid(1);
                    manager.drop_active_physical_allocations_for_pid(1);
                    manager
                        .retire_deferred_physical_allocations(1, |allocation, last| {
                            if last {
                                freed.push(allocation);
                            }
                            Ok(())
                        })
                        .unwrap();
                    assert_eq!(freed.len(), usize::from(peer_active + peer_deferred == 0));
                    for allocation in freed {
                        assert_eq!(key(allocation), (77, 1));
                    }
                    if peer_active + peer_deferred != 0 {
                        let mut peer_freed = manager.releasable_physical_allocations_for_pid(2);
                        manager.drop_active_physical_allocations_for_pid(2);
                        manager
                            .retire_deferred_physical_allocations(2, |allocation, last| {
                                if last {
                                    peer_freed.push(allocation);
                                }
                                Ok(())
                            })
                            .unwrap();
                        assert_eq!(
                            peer_freed.len(),
                            1,
                            "the remaining peer eventually frees RAM"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn retirement_snapshot_keeps_different_extent_sizes_and_shared_vm_owner_separate() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    install(&mut manager, 2, 202);
    manager.share_address_space(2, 1).unwrap();
    for (va, physical, pages) in [(4096, 77, 1), (8192, 77, 1), (16384, 99, 2)] {
        manager
            .register_physical_allocation(1, va, va / 4096, physical, pages)
            .unwrap();
    }
    // Raw child retirement must not resolve to the shared address-space owner.
    assert!(manager
        .releasable_physical_allocations_for_pid(2)
        .is_empty());
    manager.drop_active_physical_allocations_for_pid(2);
    let mut ranges: Vec<_> = manager
        .releasable_physical_allocations_for_pid(1)
        .into_iter()
        .map(key)
        .collect();
    ranges.sort_unstable();
    assert_eq!(ranges, [(77, 1), (99, 2)]);
}

fn shared_page_with_deferred_parent() -> ProcessManager {
    let mut manager = manager();
    for pid in 1..=2 {
        install(&mut manager, pid, pid + 200);
        manager
            .register_physical_allocation(pid, 0x1000, pid + 10, 700, 1)
            .unwrap();
    }
    let (allocation, last) = manager
        .release_physical_allocation_reference(1, 0x1000, 1)
        .unwrap();
    assert!(!last);
    manager.defer_physical_allocation_for_pid(1, allocation);
    manager
}

#[test]
fn deferred_parent_backing_is_not_reused_while_a_peer_still_maps_it() {
    let mut manager = shared_page_with_deferred_parent();
    assert!(manager
        .take_deferred_physical_allocation(1, 0x1000, 1)
        .is_none());
    assert!(manager
        .releasable_deferred_physical_allocations_for_pid(1)
        .is_empty());
    assert!(manager
        .find_active_physical_allocation_reference(2, 0x1000, 1)
        .is_some());
}

#[test]
fn active_reap_and_release_include_deferred_peers_in_last_reference_checks() {
    let mut manager = shared_page_with_deferred_parent();
    assert!(manager
        .releasable_physical_allocations_for_pid(2)
        .is_empty());
    let (_, last) = manager
        .release_physical_allocation_reference(2, 0x1000, 1)
        .unwrap();
    assert!(!last, "the deferred parent still owns this physical page");
    assert_eq!(
        manager
            .releasable_deferred_physical_allocations_for_pid(1)
            .len(),
        1
    );
    assert_eq!(
        manager
            .take_deferred_physical_allocation(1, 0x1000, 1)
            .unwrap()
            .base_page,
        700
    );
}

#[test]
fn exec_must_not_free_a_page_owned_by_another_processes_deferred_reference() {
    let mut manager = shared_page_with_deferred_parent();
    let removed = manager
        .reset_runtime_memory_for_exec(2, 0, 4096, 8192)
        .unwrap();
    assert_eq!(removed.len(), 1);
    assert!(!removed[0].1);
    assert_eq!(
        manager
            .releasable_deferred_physical_allocations_for_pid(1)
            .len(),
        1
    );
}

#[test]
fn two_deferred_owners_cannot_reuse_the_same_physical_page() {
    let mut manager = shared_page_with_deferred_parent();
    let (allocation, _) = manager
        .release_physical_allocation_reference(2, 0x1000, 1)
        .unwrap();
    manager.defer_physical_allocation_for_pid(2, allocation);
    assert!(manager
        .take_deferred_physical_allocation(1, 0x1000, 1)
        .is_none());
    assert!(manager
        .take_deferred_physical_allocation(2, 0x1000, 1)
        .is_none());
    manager.drop_deferred_physical_allocations_for_pid(1);
    assert_eq!(
        manager
            .take_deferred_physical_allocation(2, 0x1000, 1)
            .unwrap()
            .base_page,
        700
    );
}

#[test]
fn range_snapshot_matches_per_page_lookup_with_duplicates_extents_and_misaligned_entries() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    install(&mut manager, 2, 202);
    for sequence in 1..=600 {
        let pid = 1 + sequence % 2;
        let va = (1 + sequence % 79) * 4096 + if sequence % 11 == 0 { 1 } else { 0 };
        manager
            .register_physical_allocation(
                pid,
                va,
                sequence,
                100 + sequence,
                if sequence % 7 == 0 { 2 } else { 1 },
            )
            .unwrap();
    }
    for pid in 1..=2 {
        for first in 0..79 {
            let end = (first + 11) * 4096;
            let expected: Vec<_> = (first * 4096..end)
                .step_by(4096)
                .filter_map(|va| manager.find_active_physical_allocation_reference(pid, va, 1))
                .collect();
            assert_eq!(
                snapshot(manager.physical_pages_in_range(pid, first * 4096, end)),
                snapshot(expected)
            );
        }
    }
}

#[test]
#[ignore = "manual release-mode range bookkeeping benchmark; no map/unmap IPC"]
fn range_release_bookkeeping_cost() {
    use std::{hint::black_box, time::Instant};
    fn populated(pages: usize) -> ProcessManager {
        let mut manager = manager();
        install(&mut manager, 1, 201);
        for page in 0..pages {
            manager
                .register_physical_allocation(1, 0x1000 + page * 4096, page, 100 + page, 1)
                .unwrap();
        }
        manager
    }
    for pages in [128, 1024, 4096] {
        for sample in 0..3 {
            let mut old = populated(pages);
            let before = Instant::now();
            let mut allocations = Vec::new();
            for page in 0..pages {
                allocations.push(
                    old.find_active_physical_allocation_reference(1, 0x1000 + page * 4096, 1)
                        .unwrap(),
                );
            }
            for page in 0..pages {
                black_box(
                    old.find_active_physical_allocation_reference(1, 0x1000 + page * 4096, 1)
                        .unwrap(),
                );
            }
            for allocation in allocations {
                let (allocation, _) = old
                    .release_physical_allocation_reference(1, allocation.base_va, 1)
                    .unwrap();
                old.defer_physical_allocation_for_pid(1, allocation);
            }
            let old_time = before.elapsed();
            let mut current = populated(pages);
            let before = Instant::now();
            let allocations = current.physical_pages_in_range(1, 0x1000, 0x1000 + pages * 4096);
            assert_eq!(allocations.len(), pages);
            for allocation in &allocations {
                black_box(allocation);
            }
            for allocation in allocations.into_iter().rev() {
                let (allocation, _) = current
                    .release_physical_allocation_reference(1, allocation.base_va, 1)
                    .unwrap();
                current.defer_physical_allocation_for_pid(1, allocation);
            }
            println!(
                "RANGE-BOOKKEEPING pages={pages} sample={sample} old={old_time:?} current={:?}",
                before.elapsed()
            );
        }
    }
}
