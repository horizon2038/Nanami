#[path = "../../nanami/servers/apps/alter/shared/src/common/state/process_index.rs"]
mod process_index;
use process_index::ProcessIndex;

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
