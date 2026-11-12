#[path = "../../nanami/servers/apps/alter/shared/src/common/state/process_index.rs"]
mod process_index;
use process_index::ProcessIndex;

#[test]
fn alternating_hot_processes_keep_one_identity_check_each() {
    let entries: Vec<_> = (1..=16).collect(); let index = ProcessIndex::new();
    for pid in 1..=16 { assert_eq!(index.find(&entries, pid, |p| Some(*p)), Some(pid - 1)); }
    let mut checks = 0;
    for _ in 0..1000 {
        for pid in 1..=16 {
            assert_eq!(index.find(&entries, pid, |p| { checks += 1; Some(*p) }), Some(pid - 1));
        }
    }
    assert_eq!(checks, 16000);
    // The old single hint checks its stale slot, then 1..16 entries each round.
    assert_eq!(1000 * (16 + (1..=16).sum::<usize>()), 152000);
    assert_eq!(core::mem::size_of::<ProcessIndex>(), 16 * core::mem::size_of::<usize>());
}

#[test]
fn bucket_collisions_and_failed_colliding_lookups_never_confer_identity() {
    let mut entries = [Some(1), Some(17), Some(33), None]; let index = ProcessIndex::new();
    for _ in 0..100 {
        for pid in [1, 17, 33] { assert_eq!(index.find(&entries, pid, |p| *p), entries.iter().position(|p| *p == Some(pid))); }
    }
    assert_eq!(index.find(&entries, 49, |p| *p), None);
    let mut checks = 0;
    assert_eq!(index.find(&entries, 33, |p| { checks += 1; *p }), Some(2)); assert_eq!(checks, 1);
    entries[2] = Some(49); entries[0] = None;
    assert_eq!(index.find(&entries, 33, |p| *p), None);
    assert_eq!(index.find(&entries, 49, |p| *p), Some(2));
}

#[test]
fn bucket_hints_match_linear_lookup_through_reuse_movement_and_truncation() {
    let mut entries: Vec<_> = (1..=16).map(Some).collect(); let index = ProcessIndex::new();
    let mut seed = 0x7ae5683u64; let mut next = 100usize;
    for step in 0..4000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        match step % 5 {
            0 if !entries.is_empty() => { let i = seed as usize % entries.len(); entries[i] = None; },
            1 if !entries.is_empty() => { let i = seed as usize % entries.len(); entries[i] = Some(next); next += 1; },
            2 if entries.len() > 1 => { let i = seed as usize % entries.len(); entries.swap(0, i); },
            3 => entries.truncate(seed as usize % 17),
            _ => entries.resize(16, None),
        }
        for pid in [0, 1, 16, next, next - 1, next.saturating_sub(16), usize::MAX] {
            assert_eq!(index.find(&entries, pid, |p| *p), entries.iter().position(|p| *p == Some(pid)));
        }
        for pid in entries.iter().flatten() { assert_eq!(index.find(&entries, *pid, |p| *p), entries.iter().position(|p| *p == Some(*pid))); }
    }
}

#[test]
fn warm_lookup_checks_one_slot_instead_of_rescanning_the_table() {
    let entries: Vec<_> = (1..=64).collect();
    let index = ProcessIndex::new();
    assert_eq!(index.find(&entries, 64, |p| Some(*p)), Some(63));
    let mut checks = 0;
    for _ in 0..10000 {
        assert_eq!(
            index.find(&entries, 64, |p| {
                checks += 1;
                Some(*p)
            }),
            Some(63)
        );
    }
    assert_eq!(checks, 10000);
}
#[test]
fn reaped_reused_or_moved_slots_cannot_return_another_process() {
    let mut entries = [Some(7), Some(9), None];
    let index = ProcessIndex::new();
    assert_eq!(index.find(&entries, 9, |p| *p), Some(1));
    entries[1] = Some(17);
    assert_eq!(index.find(&entries, 9, |p| *p), None);
    entries[2] = Some(9);
    assert_eq!(index.find(&entries, 9, |p| *p), Some(2));
    assert_eq!(index.find(&entries[..1], 9, |p| *p), None);
    assert_eq!(index.find(&entries, 17, |p| *p), Some(1));
}
#[test]
fn failed_lookup_does_not_poison_a_previous_valid_hint() {
    let entries = [Some(1), None, Some(8)];
    let index = ProcessIndex::new();
    assert_eq!(index.find(&entries, 8, |p| *p), Some(2));
    assert_eq!(index.find(&entries, 0, |p| *p), None);
    let mut checks = 0;
    assert_eq!(
        index.find(&entries, 8, |p| {
            checks += 1;
            *p
        }),
        Some(2)
    );
    assert_eq!(checks, 1);
}
