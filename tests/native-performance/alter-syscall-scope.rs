//! Production ledger/controller models, not a runtime performance claim.
extern crate alloc;
#[path = "../../nanami/servers/apps/alter/shared/src/common/syscall_scope.rs"]
mod scope;
use scope::*;
use scope as syscall_scope;
#[path = "../../nanami/servers/apps/alter/shared/src/common/scope_controller.rs"]
mod controller;
use controller::{Command, Controller};
#[path = "../negoto/guest/src/scope_packet.rs"]
mod scope_packet;

fn fixture_packet(mode: u64) -> Vec<u8> {
    let mut words = vec![0u64; 1034];
    words[..8].copy_from_slice(&[controller::REPORT_MAGIC, mode + 1, mode, 10, 100, 1, 513, 1]);
    if mode == 0 {
        for (number, calls) in [(39, 7), (57, 1), (60, 1), (61, 1), (157, 2), (512, 1)] {
            words[8 + number * 2] = calls;
        }
    } else {
        words[8 + 39 * 2] = 5;
        words[9 + 39 * 2] = 40;
    }
    words.into_iter().flat_map(u64::to_le_bytes).collect()
}

#[test]
fn native_fixture_decoder_rejects_corrupt_or_unverified_protocol_fields() {
    scope_packet::validate(&fixture_packet(0), 0);
    scope_packet::validate(&fixture_packet(1), 1);
    for (mode, word, value) in [(0, 0, 0), (0, 1, 9), (0, 2, 1), (0, 4, 0),
                              (0, 5, 0), (0, 6, 512), (0, 7, 0),
                              (0, 8 + 39 * 2, 8), (0, 9 + 39 * 2, 1),
                              (1, 9 + 39 * 2, 0), (1, 9 + 39 * 2, 1000)] {
        let mut packet = fixture_packet(mode);
        packet[word * 8..word * 8 + 8].copy_from_slice(&u64::to_le_bytes(value));
        assert!(std::panic::catch_unwind(|| scope_packet::validate(&packet, mode)).is_err());
    }
    assert!(std::panic::catch_unwind(|| scope_packet::validate(&[0; 16], 0)).is_err());
}

#[test]
fn unarmed_and_count_control_never_read_per_handler_clocks() {
    let mut controller = Controller::new();
    assert_eq!(controller.command(1, b"ordinary", || panic!("ignored clock")), Ok(Command::Ignored));
    assert!(controller.sample(|| panic!("unarmed clock")).is_none());
    assert_eq!(controller.command(1, b"nscope.count", || Some(10)),
               Ok(Command::Started { epoch: 1, mode: Mode::Count }));
    let sample = controller.sample(|| panic!("count clock"));
    controller.record(sample, 1, || panic!("count clock")).unwrap();
    assert_eq!(controller.totals().unwrap()[1], Total { calls: 1, ticks: 0 });
    let Command::Stopped(summary) = controller.command(1, b"nscope.stop", || Some(20)).unwrap() else {
        panic!("missing stop");
    };
    assert!(summary.valid);
    assert!(controller.sample(|| panic!("stopped clock")).is_none());
}

#[test]
fn controller_retirement_and_stop_discard_pending_records() {
    let mut controller = Controller::new();
    controller.command(42, b"nscope.time", || Some(10)).unwrap();
    let pending = controller.sample(|| Some(11));
    assert!(!controller.retire(99));
    assert!(controller.retire(42));
    assert!(!controller.retire(42));
    controller.command(42, b"nscope.time", || Some(100)).unwrap();
    assert_eq!(controller.record(pending, 1, || Some(101)), Err(Error::StaleSample));
    let pending = controller.sample(|| Some(102));
    let Command::Stopped(summary) = controller.command(42, b"nscope.stop", || Some(103)).unwrap() else {
        panic!("missing stop");
    };
    assert!(summary.valid);
    assert_eq!(summary.epoch, 2);
    assert_eq!(controller.record(pending, 1, || panic!("closed clock")), Err(Error::StaleSample));
    assert_eq!(controller.totals().unwrap()[1], Total::default());
}

#[test]
fn controller_rejects_missing_clocks_and_does_not_reset_a_busy_scope() {
    let mut controller = Controller::new();
    assert_eq!(controller.command(42, b"nscope.time", || None), Err(Error::MissingClock));
    assert!(controller.sample(|| panic!("unarmed clock")).is_none());
    controller.command(42, b"nscope.time", || Some(10)).unwrap();
    assert_eq!(controller.command(99, b"nscope.count", || Some(11)), Err(Error::Busy));
    assert_eq!(controller.command(99, b"nscope.stop", || Some(12)), Err(Error::NotOwner));
    let sample = controller.sample(|| Some(13));
    controller.record(sample, 2, || Some(15)).unwrap();
    assert_eq!(controller.totals().unwrap()[2], Total { calls: 1, ticks: 2 });
    let Command::Stopped(summary) = controller.command(42, b"nscope.stop", || Some(20)).unwrap() else {
        panic!("missing stop");
    };
    assert!(summary.valid);
    assert_eq!(summary.epoch, 1);
}

#[test]
fn owned_report_is_bounded_le_and_does_not_survive_owner_retirement() {
    let mut controller = Controller::new();
    assert_eq!(controller.report(42), Err(Error::Inactive));
    controller.command(42, b"nscope.count", || Some(10)).unwrap();
    assert_eq!(controller.report(42), Err(Error::Inactive));
    controller.record(controller.sample(|| None), 39, || None).unwrap();
    controller.command(42, b"nscope.stop", || Some(20)).unwrap();
    assert_eq!(controller.report(99), Err(Error::NotOwner));
    let packet = controller.report(42).unwrap();
    assert_eq!(packet.len(), controller::REPORT_BYTES);
    let word = |index: usize| u64::from_le_bytes(packet[index * 8..index * 8 + 8].try_into().unwrap());
    assert_eq!(word(0), controller::REPORT_MAGIC);
    assert_eq!((word(1), word(2), word(5), word(6), word(7)), (1, 0, 1, 513, 1));
    assert_eq!((word(8 + 39 * 2), word(9 + 39 * 2)), (1, 0));
    assert_eq!(controller.report(42).unwrap(), packet);
    assert!(!controller.retire(99));
    assert_eq!(controller.report(42).unwrap(), packet);
    assert!(!controller.retire(42)); // closed scope, but its report must be gone
    assert_eq!(controller.report(42), Err(Error::Inactive));
    controller.command(42, b"nscope.count", || Some(30)).unwrap();
    assert_eq!(controller.report(42), Err(Error::Inactive));
}

#[test]
fn timed_intervals_are_serial_and_outside_numbers_are_not_dropped() {
    let mut ledger = Ledger::new();
    assert!(ledger.sample(Some(1)).is_none());
    ledger.start(42, Mode::Time, 10).unwrap();
    assert!(ledger.needs_clock());
    let sample = ledger.sample(Some(12)).unwrap();
    ledger.record(sample, 9, Some(17)).unwrap();
    let sample = ledger.sample(Some(20)).unwrap();
    ledger.record(sample, usize::MAX, Some(23)).unwrap();
    assert_eq!(ledger.totals()[9], Total { calls: 1, ticks: 5 });
    assert_eq!(ledger.totals()[OUTSIDE], Total { calls: 1, ticks: 3 });
    let summary = ledger.stop(42, 30).unwrap();
    assert!(summary.valid);
    assert_eq!((summary.start, summary.end), (10, 30));
    assert!(!ledger.needs_clock());
}

#[test]
fn count_mode_does_not_require_per_handler_clocks() {
    let mut ledger = Ledger::new();
    ledger.start(42, Mode::Count, 10).unwrap();
    assert!(!ledger.needs_clock());
    for _ in 0..20 {
        ledger.record(ledger.sample(None).unwrap(), 1, None).unwrap();
    }
    assert_eq!(ledger.totals()[1], Total { calls: 20, ticks: 0 });
    assert!(ledger.stop(42, 20).unwrap().valid);
}

#[test]
fn stale_samples_and_wrong_owners_do_not_corrupt_another_phase() {
    let mut ledger = Ledger::new();
    ledger.start(42, Mode::Time, 10).unwrap();
    let old = ledger.sample(Some(11)).unwrap();
    assert_eq!(ledger.start(99, Mode::Count, 12), Err(Error::Busy));
    assert_eq!(ledger.stop(99, 13), Err(Error::NotOwner));
    assert_eq!(ledger.cancel(99), Err(Error::NotOwner));
    ledger.cancel(42).unwrap();
    assert_eq!(ledger.stop(42, 14), Err(Error::Inactive));
    ledger.start(42, Mode::Time, 100).unwrap();
    assert_eq!(ledger.record(old, 1, Some(120)), Err(Error::StaleSample));
    assert_eq!(ledger.totals()[1], Total::default());
    assert!(ledger.stop(42, 130).unwrap().valid);
}

#[test]
fn missing_reversed_or_overlapping_clocks_invalidate_the_phase() {
    for (before, end, error) in [(None, Some(12), Error::MissingClock),
                               (Some(12), None, Error::MissingClock),
                               (Some(9), Some(12), Error::NonMonotonic),
                               (Some(12), Some(11), Error::NonMonotonic)] {
        let mut ledger = Ledger::new();
        ledger.start(42, Mode::Time, 10).unwrap();
        assert_eq!(ledger.record(ledger.sample(before).unwrap(), 1, end), Err(error));
        assert!(!ledger.stop(42, 20).unwrap().valid);
    }
    let mut ledger = Ledger::new();
    ledger.start(42, Mode::Time, 10).unwrap();
    let pending = ledger.sample(Some(12)).unwrap();
    ledger.record(ledger.sample(Some(11)).unwrap(), 1, Some(15)).unwrap();
    assert_eq!(ledger.record(pending, 1, Some(20)), Err(Error::Overlap));
    assert!(!ledger.stop(42, 21).unwrap().valid);
}

#[test]
fn stop_cannot_claim_a_valid_interval_before_recorded_work() {
    let mut ledger = Ledger::new();
    ledger.start(42, Mode::Time, 10).unwrap();
    ledger.record(ledger.sample(Some(12)).unwrap(), 1, Some(30)).unwrap();
    assert!(!ledger.stop(42, 20).unwrap().valid);
}
