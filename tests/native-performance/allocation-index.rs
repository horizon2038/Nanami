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
                    .filter(|a| key(a.value) == key(entry.value))
                    .count();
                let (removed, last) = manager
                    .release_physical_allocation_reference(
                        entry.pid,
                        entry.value.base_va,
                        entry.value.page_count,
                    )
                    .unwrap();
                assert_eq!(snapshot([removed]), snapshot([entry.value]));
                assert_eq!(last, count == 1);
                active.swap_remove(index);
                if seed % 13 == 8 {
                    manager.defer_physical_allocation_for_pid(entry.pid, entry.value);
                    deferred.push(entry);
                }
            }
            9 if !deferred.is_empty() => {
                let index = (seed >> 17) as usize % deferred.len();
                let entry = deferred.swap_remove(index);
                let removed = manager
                    .take_deferred_physical_allocation(
                        entry.pid,
                        entry.value.base_va,
                        entry.value.page_count,
                    )
                    .unwrap();
                assert_eq!(snapshot([removed]), snapshot([entry.value]));
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
                            !active.iter().any(|a| key(a.value) == key(value)),
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
            let expected = active
                .iter()
                .filter(|a| {
                    a.pid == pid
                        && active
                            .iter()
                            .filter(|other| key(a.value) == key(other.value))
                            .count()
                            == 1
                })
                .map(|a| a.value);
            assert_eq!(
                snapshot(manager.releasable_physical_allocations_for_pid(pid)),
                snapshot(expected),
                "active step={step}"
            );
            let expected = deferred
                .iter()
                .filter(|a| {
                    a.pid == pid
                        && active
                            .iter()
                            .chain(&deferred)
                            .filter(|other| key(a.value) == key(other.value))
                            .count()
                            == 1
                })
                .map(|a| a.value);
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
