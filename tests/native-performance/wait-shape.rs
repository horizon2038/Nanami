#![allow(dead_code)]
extern crate alloc;
#[path = "../../nanami/servers/apps/alter/shared/src/common/wait_shape.rs"]
mod wait_shape;
use wait_shape::{Identity, Row, Trace, LIMIT, OWNER_LIMIT};

fn in_gap() -> Trace {
    let mut trace = Trace::new();
    for _ in 0..14 { trace.record(row(41, 0)); }
    trace.record(row(41, 3));
    trace
}

#[test]
fn paths_own_exact_bytes_and_only_match_selected_current_dispatch_results() {
    let mut trace = in_gap();
    trace.syscall(41, 257);
    let mut name = *b"/etc/fonts/fonts.conf";
    trace.path(42, &name);
    trace.path(41, &name);
    name.fill(0);
    trace.path_result(42, 257, Some(-2));
    trace.path_result(41, 21, Some(-2));
    trace.path_result(41, 257, Some(4));
    trace.path_result(41, 257, Some(4));
    trace.syscall(41, 21);
    trace.path(41, b"/etc/fonts/fonts.conf");
    trace.path_result(41, 21, Some(-2));
    assert_eq!(trace.paths.len(), 1);
    let entry = &trace.paths[0];
    assert_eq!(&entry.bytes[..entry.len], b"/etc/fonts/fonts.conf");
    assert_eq!((entry.first_call, entry.hits, entry.ok, entry.errors, entry.last), (257, 2, 1, 1, Some(-2)));
    trace.record(row(41, 0));
    trace.path(41, b"outside");
    assert_eq!(trace.path_total, 2);
}

#[test]
fn truncated_paths_never_merge_and_unique_capacity_preserves_known_path_hits() {
    let mut trace = in_gap();
    trace.syscall(41, 21);
    for _ in 0..2 { trace.path(41, &[b'x'; 193]); }
    trace.path(41, &[b'x'; 192]);
    assert_eq!(trace.paths.len(), 3);
    assert!(trace.paths[0].truncated && trace.paths[1].truncated && !trace.paths[2].truncated);
    for index in 3..wait_shape::PATH_LIMIT + 2 { trace.path(41, &index.to_le_bytes()); }
    assert_eq!(trace.paths.len(), wait_shape::PATH_LIMIT);
    assert_eq!(trace.path_dropped, 2);
    trace.path(41, &[b'x'; 192]);
    assert_eq!(trace.paths[2].hits, 2);
    assert_eq!(trace.paths.iter().map(|row| row.hits).sum::<usize>() + trace.path_dropped, trace.path_total);
}

#[test]
fn failed_or_unassociated_path_results_remain_unknown_and_overflow_is_explicit() {
    let mut trace = in_gap();
    trace.path(41, b"no-dispatch");
    assert_eq!(trace.path_total, 0);
    trace.syscall(41, 21);
    trace.path(41, b"first");
    trace.path(41, b"second");
    trace.path_result(41, 21, None);
    assert!(trace.paths.iter().all(|row| row.last.is_none()));
    trace.path_total = usize::MAX;
    trace.path(41, b"third");
    assert!(trace.path_overflow);
    assert_eq!(trace.path_total, usize::MAX);
}

#[test]
fn gap_counts_only_selected_pid_between_fixed_result_and_next_poll_record() {
    let mut trace = Trace::new();
    for _ in 0..14 { trace.record(row(41, 0)); }
    trace.syscall(41, 1);
    assert_eq!(trace.gap_calls[1], 0);
    trace.record(row(41, 3));
    assert_eq!(trace.gap_start, Some(14));
    trace.syscall(42, 1);
    trace.syscall(41, 9);
    trace.syscall(41, 7);
    trace.syscall(41, usize::MAX);
    trace.record(row(41, 0));
    assert_eq!(trace.gap_end, Some(15));
    trace.syscall(41, 1);
    assert_eq!(trace.gap_calls[1], 0);
    assert_eq!(trace.gap_calls[7], 1);
    assert_eq!(trace.gap_calls[9], 1);
    assert_eq!(trace.gap_calls[512], 1);
}

#[test]
fn gap_requires_planned_result_phase_and_reports_counter_overflow() {
    let mut trace = Trace::new();
    for _ in 0..16 { trace.record(row(41, 0)); }
    trace.syscall(41, 1);
    assert!(trace.gap_start.is_none());
    assert_eq!(trace.gap_calls[1], 0);
    let mut trace = Trace::new();
    for _ in 0..14 { trace.record(row(41, 0)); }
    trace.record(row(41, 3));
    trace.gap_calls[1] = usize::MAX;
    trace.syscall(41, 1);
    assert!(trace.gap_overflow);
    assert_eq!(trace.gap_calls[1], usize::MAX);
    assert!(!trace.due(1, 1));
    assert!(trace.due(3, 1));
    trace.syscall(41, 2);
    assert_eq!(trace.gap_calls[2], 0);
}

fn row(pid: usize, phase: u8) -> Row {
    Row { pid, syscall: 23, phase, cached_ticks: 7, hz: 1000,
          deadline: Some(100), timeout_ns: Some(10), ready: Some(0),
          interests: 64, trigger: 0, expired: false, result: 0, count: 1, first_fd: 4,
          socket: 19, peer: 20, lower: None, upper: None }
}

fn timed(pid: usize, phase: u8, ticks: usize, hz: usize) -> Row {
    Row { cached_ticks: ticks, hz, ..row(pid, phase) }
}

#[test]
fn first_later_sample_closes_each_event_once_and_does_not_absorb_later_work() {
    let mut trace = Trace::new();
    trace.sample(100, 1000);
    trace.record(timed(41, 1, 100, 1000));
    trace.sample(105, 1000);
    trace.sample(110, 1000);
    trace.record(timed(41, 3, 110, 1000));
    trace.sample(112, 1000);
    trace.sample(1000, 1000);
    assert!(trace.clock_consistent);
    assert_eq!((trace.rows[0].lower, trace.rows[0].upper), (Some(100), Some(105)));
    assert_eq!((trace.rows[1].lower, trace.rows[1].upper), (Some(110), Some(112)));
}

#[test]
fn equal_samples_are_valid_and_missing_samples_do_not_fabricate_bounds() {
    let mut trace = Trace::new();
    trace.record(row(41, 1));
    assert_eq!((trace.rows[0].lower, trace.rows[0].upper), (None, None));
    trace.sample(10, 1000);
    assert!(!trace.clock_consistent);
    assert_eq!(trace.rows[0].upper, None);
    let mut trace = Trace::new();
    trace.sample(10, 1000);
    trace.record(timed(41, 1, 10, 1000));
    trace.sample(10, 1000);
    assert_eq!((trace.rows[0].lower, trace.rows[0].upper), (Some(10), Some(10)));
    assert!(trace.clock_consistent);
}

#[test]
fn regression_rate_change_or_unobserved_cache_assignment_rejects_the_window() {
    for (ticks, hz) in [(9, 1000), (11, 2000), (11, 0)] {
        let mut trace = Trace::new();
        trace.sample(10, 1000);
        trace.record(timed(41, 1, 10, 1000));
        trace.sample(ticks, hz);
        assert!(!trace.clock_consistent);
        trace.sample(100, 1000);
        assert!(!trace.clock_consistent);
    }
    let mut trace = Trace::new();
    trace.sample(10, 1000);
    trace.record(timed(41, 1, 11, 1000));
    assert!(!trace.clock_consistent);
    assert_eq!(trace.rows[0].lower, None);
}

#[test]
fn identity_requires_exact_first_connection_and_live_descriptor_evidence() {
    let mut trace = Trace::new();
    trace.record(row(41, 1));
    assert!(!trace.needs_identity(42, 19, 20));
    assert!(!trace.needs_identity(41, 21, 20));
    trace.capture_identity(Identity::new(41, 19, 20, 0, b"x"));
    assert!(trace.identity.is_none());
    let mut wrong = Identity::new(41, 21, 20, 0, b"y");
    wrong.add_owner(31, 8, b"/bin/Xfbdev");
    trace.capture_identity(wrong);
    assert!(trace.identity.is_none());
    let mut outside = Identity::new(41, 19, 20, LIMIT, b"z");
    outside.add_owner(31, 8, b"/bin/Xfbdev");
    trace.capture_identity(outside);
    assert!(trace.identity.is_none());
    let mut right = Identity::new(41, 19, 20, 0, b"x");
    right.add_owner(31, 8, b"/bin/Xfbdev");
    trace.capture_identity(right);
    assert!(!trace.needs_identity(41, 19, 20));
    let identity = trace.identity.as_ref().unwrap();
    assert_eq!((identity.socket, identity.peer, identity.descriptors), (19, 20, 1));
    assert_eq!(&identity.owners[0].image[..identity.owners[0].image_len], b"Xfbdev");
    assert!(!trace.due(1, 1));
    assert!(trace.due(3, 1));
    assert!(!trace.active_for(41));
}

#[test]
fn identity_owns_bytes_and_retains_total_when_saved_owners_or_names_truncate() {
    let mut address = [9u8; 112];
    let mut image = [b'a'; 33];
    let mut identity = Identity::new(41, 19, 20, 0, &address);
    for index in 0..OWNER_LIMIT + 3 { identity.add_owner(index + 1, index, &image); }
    address.fill(0);
    image.fill(0);
    assert_eq!(identity.descriptors, OWNER_LIMIT + 3);
    assert_eq!(identity.address_len, 111);
    assert!(identity.address_truncated);
    assert!(identity.address.iter().all(|byte| *byte == 9));
    for owner in identity.owners {
        assert!(owner.truncated);
        assert_eq!(owner.image_len, 32);
        assert!(owner.image.iter().all(|byte| *byte == b'a'));
    }
}

#[test]
fn reporting_starts_from_fresh_timer_not_stale_event_ticks_and_runs_once() {
    let mut trace = Trace::new();
    assert!(!trace.due(10000, 1000));
    trace.record(row(41, 1));
    assert!(!trace.due(10000, 0));
    assert!(!trace.due(10000, 1000));
    assert!(!trace.due(11999, 1000));
    assert!(trace.due(12000, 1000));
    assert!(!trace.due(13000, 1000));
    trace.record(row(41, 3));
    assert_eq!(trace.rows.len(), 1);
    assert_eq!(trace.rows[0].cached_ticks, 7);
}

#[test]
fn bounded_prefix_preserves_events_and_reports_overflow_without_wrapping() {
    let mut trace = Trace::new();
    for phase in 0..LIMIT + 13 { trace.record(row(41, phase as u8)); }
    assert_eq!(trace.rows.len(), LIMIT);
    assert_eq!(trace.dropped, 13);
    for (index, row) in trace.rows.iter().enumerate() { assert_eq!(row.phase, index as u8); }
    trace.dropped = usize::MAX;
    trace.record(row(41, 3));
    assert_eq!(trace.dropped, usize::MAX);
}

#[test]
fn another_pid_cannot_mix_with_the_selected_process_or_change_report_start() {
    let mut trace = Trace::new();
    trace.record(row(41, 1));
    trace.record(row(42, 4));
    assert_eq!(trace.pid, Some(41));
    assert_eq!(trace.rows.len(), 1);
    assert_eq!(trace.dropped, 0);
    assert!(!trace.due(100, 100));
    trace.record(row(41, 2));
    assert!(trace.due(300, 100));
    assert_eq!(trace.rows[1].phase, 2);
}
