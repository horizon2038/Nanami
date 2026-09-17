//! Exercise the real opt-in compositor profiler, not an alternate implementation.
extern crate self as libnanami;
extern crate self as nanami_services;
use std::cell::RefCell;
type Word = usize;

#[derive(Default)]
struct Fake {
    clock: Option<(Word, Word)>,
    reads: usize,
    logs: Vec<String>,
}
thread_local! { static FAKE: RefCell<Fake> = RefCell::new(Fake::default()); }
#[macro_export]
macro_rules! println {
    ($($arg:tt)*) => { $crate::FAKE.with(|fake| fake.borrow_mut().logs.push(format!($($arg)*))) };
}
pub mod timer {
    pub fn timer_service_monotonic_ticks(_: usize) -> Result<(usize, usize), ()> {
        super::FAKE.with(|fake| {
            let mut fake = fake.borrow_mut();
            fake.reads += 1;
            fake.clock.ok_or(())
        })
    }
}
#[path = "../../nanami/servers/apps/honoka/src/app/profile.rs"]
mod profile;

fn enabled() -> bool {
    option_env!("NANAMI_FB_PROFILE") == Some("1")
}
fn clock(ticks: Word, hz: Word) {
    FAKE.with(|fake| fake.borrow_mut().clock = Some((ticks, hz)));
}

#[test]
fn disabled_profile_or_missing_port_never_queries_timer() {
    let mut profile = profile::Profile::new(if enabled() { 0 } else { 42 });
    assert_eq!(profile.now(), None);
    profile.record(None, None, 1);
    FAKE.with(|fake| {
        assert_eq!(fake.borrow().reads, 0);
        assert!(fake.borrow().logs.is_empty());
    });
}

#[test]
fn conversion_supports_hpet_and_non_nanosecond_clock_sources() {
    let profile = profile::Profile::new(42);
    for (ticks, hz, ns) in [
        (123, 100, 1_230_000_000),
        (7, 3, 2_333_333_333),
        (123, 1_000_000_000, 123),
    ] {
        clock(ticks, hz);
        assert_eq!(profile.now(), enabled().then_some(ns));
    }
}

#[test]
fn batches_separate_composition_from_presentation_and_preserve_idle_time() {
    if !enabled() {
        return;
    }
    let mut profile = profile::Profile::new(42);
    // The first render is well after boot. Do not count uptime before it.
    clock(10_000_005_000, 1_000_000_000);
    profile.record(Some(10_000_000_000), Some(10_000_002_000), 2);
    clock(12_000_009_000, 1_000_000_000);
    profile.record(Some(12_000_000_000), Some(12_000_005_000), 1);
    FAKE.with(|fake| assert_eq!(fake.borrow().logs, [
        "[honoka.perf] interval-ms=2000 renders=2 rects=3 compose-us=7 present-us=7 max-compose-us=5 max-present-us=4"
    ]));
    clock(16_000_003_000, 1_000_000_000);
    profile.record(Some(16_000_000_000), Some(16_000_001_000), 1);
    FAKE.with(|fake| assert_eq!(fake.borrow().logs[1],
        "[honoka.perf] interval-ms=3999 renders=1 rects=1 compose-us=1 present-us=2 max-compose-us=1 max-present-us=2"
    ));
}

#[test]
fn failed_or_zero_frequency_samples_do_not_produce_measurements() {
    if !enabled() {
        return;
    }
    let mut profile = profile::Profile::new(42);
    assert_eq!(profile.now(), None);
    profile.record(Some(0), Some(1), 1);
    clock(4_000_000_000, 0);
    assert_eq!(profile.now(), None);
    profile.record(Some(0), Some(1), 1);
    clock(4_000_000_000, 1_000_000_000);
    profile.record(None, Some(1), 1);
    profile.record(Some(0), None, 1);
    FAKE.with(|fake| assert!(fake.borrow().logs.is_empty()));
    profile.record(Some(1_000_000_000), Some(1_000_001_000), 1);
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.logs.len(), 1);
        assert!(fake.logs[0].contains("renders=1 rects=1 compose-us=1 "));
    });
}
