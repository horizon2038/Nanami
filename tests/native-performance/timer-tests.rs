#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_runtime;
use std::cell::RefCell;

#[macro_export]
macro_rules! println {
    ($($arg:tt)*) => { $crate::capture_log(format_args!($($arg)*)); };
}

pub type Word = usize;
pub const PROCESS_SLOT_NOTIFICATION: Word = nanami_uapi::os::PROCESS_SLOT_NOTIFICATION;
const SLOT_CLIENT_NOTIFICATION_BASE: Word = 32;
const MAX_CLIENT_NOTIFICATIONS: usize = 128;
const MAX_PENDING_ASYNC_TIMERS: usize = 512;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestError {
    InvalidArgument,
    Unsupported,
    Transport,
}

pub mod timer {
    pub use nanami_uapi::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT;
}
#[derive(Default)]
struct Fake {
    trace: Vec<String>,
    copy_requests: Vec<(Word, Word, Word, Word)>,
    copy_fails: bool,
    arm_fails: bool,
    notified: Vec<Word>,
    failed_descriptor: Option<Word>,
    copies: usize,
    starts: usize,
    clock_reads: usize,
    arms: Vec<Option<u64>>,
}
thread_local! { static FAKE: RefCell<Fake> = RefCell::new(Fake::default()); }
fn capture_log(args: std::fmt::Arguments<'_>) {
    let line = args.to_string();
    FAKE.with(|state| state.borrow_mut().trace.push(line));
}
pub mod ipc {
    use super::*;
    pub fn process_slot_descriptor(slot: Word) -> Word {
        slot
    }
    pub fn interrupt_ack(_descriptor: Word) -> Result<(), RequestError> {
        Ok(())
    }
    pub fn notification_notify(descriptor: Word) -> Result<(), RequestError> {
        FAKE.with(|state| {
            let mut state = state.borrow_mut();
            state.notified.push(descriptor);
            if state.failed_descriptor == Some(descriptor) {
                Err(RequestError::Transport)
            } else {
                Ok(())
            }
        })
    }
}
pub fn request_notification_port_copy(
    pid: Word,
    source: Word,
    dest: Word,
    id: Word,
) -> Result<(), RequestError> {
    FAKE.with(|state| {
        let mut f = state.borrow_mut();
        f.copies += 1;
        f.copy_requests.push((pid, source, dest, id));
        if f.copy_fails {
            Err(RequestError::Transport)
        } else {
            Ok(())
        }
    })
}
mod arch {
    use super::*;
    pub const TICK_HZ: u64 = 1000;
    pub struct PreparedTimer {
        pub ticks: u64,
    }
    impl PreparedTimer {
        pub fn start(&mut self) -> Result<(), RequestError> {
            FAKE.with(|state| state.borrow_mut().starts += 1);
            Ok(())
        }
        pub fn now(&mut self) -> u64 {
            FAKE.with(|state| state.borrow_mut().clock_reads += 1);
            self.ticks
        }
        pub fn arm(&mut self, deadline: Option<u64>) -> Result<(), RequestError> {
            FAKE.with(|state| {
                let mut f = state.borrow_mut();
                f.arms.push(deadline);
                if f.arm_fails {
                    Err(RequestError::Transport)
                } else {
                    Ok(())
                }
            })
        }
    }
}
fn log_request_error(_message: &str, _error: RequestError) {}
#[path = "../../nanami/servers/core-services/timer-server/src/init_trace.rs"]
mod init_trace;
#[path = "../../nanami/servers/core-services/timer-server/src/state.rs"]
mod state;
use state::*;
#[path = "../../nanami/servers/core-services/timer-server/src/deadlines.rs"]
mod deadlines;
#[path = "../../nanami/servers/core-services/timer-server/src/arch/hpet_clock.rs"]
mod hpet_clock;
mod timers {
    include!(env!("NANAMI_TIMER_SOURCE"));
}
use timers::*;

fn reset() -> TimerState {
    FAKE.with(|state| *state.borrow_mut() = Fake::default());
    TimerState::new()
}

#[test]
fn private_delay_and_default_notification_have_distinct_cached_capabilities() {
    let mut state = reset();
    let mut timer = arch::PreparedTimer { ticks: 100 };
    for (pid, slot) in [(77, 26), (77, 0), (77, PROCESS_SLOT_NOTIFICATION), (91, 21)] {
        schedule_timer(&mut state, &mut timer, 19, pid, slot, 1, 0).unwrap();
    }
    FAKE.with(|fake| {
        let f = fake.borrow();
        let badge = timer::TIMER_NOTIFICATION_IDENTIFIER_BIT;
        assert_eq!(
            f.copy_requests,
            [
                (77, 26, 32, badge),
                (77, 21, 33, badge),
                (91, 21, 34, badge)
            ]
        );
        assert_eq!((f.copies, f.starts, f.clock_reads), (3, 1, 4));
        assert!(f.trace.is_empty());
    });
}

fn trace_stages() -> Vec<String> {
    FAKE.with(|fake| {
        fake.borrow()
            .trace
            .iter()
            .map(|line| {
                line.split("stage=")
                    .nth(1)
                    .unwrap()
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    })
}

fn expected_trace(stages: &[&str]) -> Vec<String> {
    if option_env!("NANAMI_USB_INIT_TRACE") == Some("1") {
        stages.iter().map(|stage| (*stage).to_owned()).collect()
    } else {
        Vec::new()
    }
}

#[test]
fn first_interval_trace_covers_copy_clock_queue_arm_and_reply_without_extra_work() {
    let mut state = reset();
    let mut timer = arch::PreparedTimer { ticks: 100 };
    schedule_timer(&mut state, &mut timer, 19, 77, 21, 10, 10).unwrap();
    complete_event(&mut state, &mut timer).unwrap();
    state.init_trace.reply_enter(0);
    assert_eq!(
        trace_stages(),
        expected_trace(&[
            "interval.receive",
            "notification-ensure.begin",
            "notification-copy.begin",
            "notification-copy.end",
            "notification-ensure.end",
            "clock.begin",
            "clock.end",
            "queue.end",
            "event-complete.begin",
            "hpet-arm.begin",
            "hpet-arm.end",
            "reply-enter",
        ])
    );
    FAKE.with(|fake| {
        let f = fake.borrow();
        assert_eq!((f.copies, f.starts, f.clock_reads), (1, 1, 1));
        assert_eq!(f.arms, [Some(110)]);
        assert!(f.notified.is_empty());
    });
    let log_count = trace_stages().len();
    for now in [110, 120, 1000] {
        timer.ticks = now;
        complete_event(&mut state, &mut timer).unwrap();
    }
    // Do not trace a later matching registration or its IRQ/expiry work.
    schedule_timer(&mut state, &mut timer, 19, 91, 21, 10, 10).unwrap();
    complete_event(&mut state, &mut timer).unwrap();
    state.init_trace.reply_enter(0);
    assert_eq!(trace_stages().len(), log_count);
}

#[test]
fn notification_copy_failure_does_not_cache_a_capability_or_schedule_a_timer() {
    let mut state = reset();
    let mut timer = arch::PreparedTimer { ticks: 100 };
    FAKE.with(|fake| fake.borrow_mut().copy_fails = true);
    assert_eq!(
        schedule_timer(&mut state, &mut timer, 19, 77, 21, 10, 10),
        Err(RequestError::Transport)
    );
    state.init_trace.reply_enter(1);
    assert_eq!(
        trace_stages(),
        expected_trace(&[
            "interval.receive",
            "notification-ensure.begin",
            "notification-copy.begin",
            "notification-copy.end",
            "notification-ensure.end",
            "reply-enter",
        ])
    );
    assert!(state.client_notifications.iter().all(|entry| !entry.used));
    assert_eq!(state.pending_timers.next(), None);
    FAKE.with(|fake| {
        let f = fake.borrow();
        assert_eq!((f.starts, f.clock_reads), (0, 0));
        assert!(f.arms.is_empty());
        if !f.trace.is_empty() {
            assert!(f
                .trace
                .iter()
                .any(|line| line.contains("notification-copy.end ok=false")));
        }
    });
    FAKE.with(|fake| fake.borrow_mut().copy_fails = false);
    schedule_timer(&mut state, &mut timer, 19, 77, 21, 10, 10).unwrap();
    FAKE.with(|fake| {
        let f = fake.borrow();
        assert_eq!(f.copy_requests.len(), 2);
        assert_eq!(f.copy_requests[0], f.copy_requests[1]);
    });
}

#[test]
fn cached_notification_trace_does_not_issue_another_copy() {
    let mut state = reset();
    let mut timer = arch::PreparedTimer { ticks: 100 };
    schedule_timer(&mut state, &mut timer, 19, 77, 21, 2, 0).unwrap();
    schedule_timer(&mut state, &mut timer, 19, 77, 21, 10, 10).unwrap();
    complete_event(&mut state, &mut timer).unwrap();
    state.init_trace.reply_enter(0);
    assert_eq!(
        trace_stages(),
        expected_trace(&[
            "interval.receive",
            "notification-ensure.begin",
            "notification.cache-hit",
            "notification-ensure.end",
            "clock.begin",
            "clock.end",
            "queue.end",
            "event-complete.begin",
            "hpet-arm.begin",
            "hpet-arm.end",
            "reply-enter",
        ])
    );
    FAKE.with(|fake| assert_eq!(fake.borrow().copies, 1));
}

#[test]
fn arm_failure_retains_the_original_error_and_does_not_claim_a_reply() {
    let mut state = reset();
    let mut timer = arch::PreparedTimer { ticks: 100 };
    schedule_timer(&mut state, &mut timer, 19, 77, 21, 10, 10).unwrap();
    FAKE.with(|fake| fake.borrow_mut().arm_fails = true);
    assert_eq!(
        complete_event(&mut state, &mut timer),
        Err(RequestError::Transport)
    );
    FAKE.with(|fake| {
        let f = fake.borrow();
        assert_eq!(f.arms, [Some(110)]);
        assert!(f.trace.iter().all(|line| !line.contains("reply-enter")));
        if !f.trace.is_empty() {
            assert!(f.trace.last().unwrap().contains("hpet-arm.end ok=false"));
        }
    });
}

#[test]
fn clock_query_samples_once_per_event_and_irqs_get_a_fresh_sample() {
    let mut state = reset();
    let mut timer = arch::PreparedTimer { ticks: 5 };
    for now in [5, 9, 42] {
        timer.ticks = now;
        refresh_clock(&mut state, &mut timer, 19).unwrap();
        assert_eq!(state.ticks, now);
        complete_event(&mut state, &mut timer).unwrap();
        assert!(!state.clock_sampled);
    }
    FAKE.with(|f| assert_eq!(f.borrow().clock_reads, 3));
    // Notification/IRQ has no request-side refresh_clock call.
    timer.ticks = 100;
    complete_event(&mut state, &mut timer).unwrap();
    assert_eq!(state.ticks, 100);
    FAKE.with(|f| {
        assert_eq!(f.borrow().clock_reads, 4);
        assert_eq!(f.borrow().starts, 1);
    });
}

#[test]
fn event_completion_fires_due_timers_even_during_repeated_clock_queries() {
    let mut state = reset();
    let mut timer = arch::PreparedTimer { ticks: 0 };
    set_alarm(&mut state, &mut timer, 19, 1, 18, Some(20)).unwrap();
    complete_event(&mut state, &mut timer).unwrap();
    timer.ticks = 20;
    refresh_clock(&mut state, &mut timer, 19).unwrap();
    complete_event(&mut state, &mut timer).unwrap();
    FAKE.with(|f| {
        let f = f.borrow();
        assert_eq!(f.notified, [32]);
        assert_eq!(f.clock_reads, 2);
        assert_eq!(f.arms, [Some(20), None]);
    });
}
fn schedule(
    state: &mut TimerState,
    pid: Word,
    delay: u64,
    interval: u64,
) -> Result<(), RequestError> {
    let mut timer = arch::PreparedTimer { ticks: state.ticks };
    schedule_timer(state, &mut timer, 19, pid, 18, delay, interval)
}
fn tick(state: &mut TimerState, now: u64) {
    state.ticks = now;
    fire_expired_async_timers(state);
}

#[test]
fn idle_and_future_ticks_do_not_fire_and_earlier_insert_wakes_on_time() {
    let mut state = reset();
    tick(&mut state, 50);
    assert_eq!(state.pending_timers.next(), None);
    schedule(&mut state, 1, 100, 0).unwrap();
    schedule(&mut state, 2, 10, 0).unwrap();
    assert_eq!(state.pending_timers.next(), Some(60));
    tick(&mut state, 59);
    assert_eq!(state.fire_count, 0);
    tick(&mut state, 60);
    assert_eq!(state.fire_count, 1);
    assert_eq!(state.pending_timers.next(), Some(150));
    tick(&mut state, 149);
    assert_eq!(state.fire_count, 1);
    tick(&mut state, 150);
    assert_eq!(state.pending_timers.next(), None);
    FAKE.with(|fake| assert_eq!(fake.borrow().notified, [33, 32]));
}

#[test]
fn periodic_timer_skips_missed_periods_without_drifting() {
    let mut state = reset();
    schedule(&mut state, 1, 10, 10).unwrap();
    tick(&mut state, 35);
    assert_eq!(state.pending_timers.next(), Some(40));
    assert_eq!(state.fire_count, 1);
    tick(&mut state, 40);
    assert_eq!(state.pending_timers.next(), Some(50));
    tick(&mut state, 1_000_000_000_001);
    assert_eq!(state.pending_timers.next(), Some(1_000_000_000_010));
    assert_eq!(state.fire_count, 3);
}

#[test]
fn notification_failure_retires_all_matching_timers_and_recomputes_deadline() {
    let mut state = reset();
    schedule(&mut state, 1, 10, 10).unwrap();
    schedule(&mut state, 1, 11, 0).unwrap();
    schedule(&mut state, 2, 50, 0).unwrap();
    FAKE.with(|fake| fake.borrow_mut().failed_descriptor = Some(32));
    tick(&mut state, 10);
    assert_eq!(state.pending_timers.next(), Some(50));
    tick(&mut state, 49);
    assert_eq!(state.fire_count, 1);
    tick(&mut state, 50);
    assert_eq!(state.pending_timers.next(), None);
    assert_eq!(state.fire_count, 2);
    FAKE.with(|fake| assert_eq!(fake.borrow().notified, [32, 33]));
}

#[test]
fn capacity_error_and_reuse_keep_deadline_correct() {
    let mut state = reset();
    for _ in 0..MAX_PENDING_ASYNC_TIMERS {
        schedule(&mut state, 1, 50, 0).unwrap();
    }
    assert_eq!(
        schedule(&mut state, 1, 1, 0),
        Err(RequestError::Unsupported)
    );
    assert_eq!(state.pending_timers.next(), Some(50));
    tick(&mut state, 50);
    assert_eq!(state.fire_count, MAX_PENDING_ASYNC_TIMERS);
    assert_eq!(state.pending_timers.next(), None);
    schedule(&mut state, 1, 10, 0).unwrap();
    assert_eq!(state.pending_timers.next(), Some(60));
    FAKE.with(|fake| {
        assert_eq!(fake.borrow().starts, 1);
        assert_eq!(fake.borrow().copies, 1);
    });
}

#[test]
fn immediate_timer_does_not_change_deadlines_or_start_hardware() {
    let mut state = reset();
    schedule(&mut state, 1, 0, 0).unwrap();
    assert_eq!(state.pending_timers.next(), None);
    FAKE.with(|fake| {
        assert_eq!(fake.borrow().starts, 0);
        assert_eq!(fake.borrow().notified, [32]);
    });
}

#[test]
fn saturated_clock_fires_once_and_does_not_loop_forever() {
    let mut state = reset();
    state.ticks = u64::MAX - 5;
    schedule(&mut state, 1, 1, 10).unwrap();
    tick(&mut state, u64::MAX - 4);
    assert_eq!(state.pending_timers.next(), Some(u64::MAX));
    tick(&mut state, u64::MAX);
    assert_eq!(state.pending_timers.next(), None);
    tick(&mut state, u64::MAX);
    assert_eq!(state.fire_count, 2);
}

#[test]
fn large_delay_saturates_after_conversion_not_before_division() {
    let mut state = reset();
    schedule(&mut state, 1, u64::MAX, 0).unwrap();
    assert_eq!(state.pending_timers.next(), Some(u64::MAX));
}

#[test]
fn each_request_samples_clock_even_without_an_interrupt() {
    let mut state = reset();
    let mut timer = arch::PreparedTimer { ticks: 10 };
    refresh_clock(&mut state, &mut timer, 19).unwrap();
    timer.ticks = 1234;
    schedule_timer(&mut state, &mut timer, 19, 1, 18, 10, 0).unwrap();
    assert_eq!(state.ticks, 1234);
    assert_eq!(state.pending_timers.next(), Some(1244));
    FAKE.with(|fake| assert_eq!(fake.borrow().starts, 1));
}

#[test]
fn replaceable_alarm_does_not_accumulate_or_cancel_independent_timers() {
    let mut state = reset();
    let mut timer = arch::PreparedTimer { ticks: 0 };
    schedule(&mut state, 1, 5000, 0).unwrap();
    for deadline in (1..2000).rev() {
        set_alarm(&mut state, &mut timer, 19, 1, 18, Some(deadline)).unwrap();
    }
    assert_eq!(state.pending_timers.next(), Some(1));
    set_alarm(&mut state, &mut timer, 19, 1, 18, None).unwrap();
    assert_eq!(state.pending_timers.next(), Some(5000));
    tick(&mut state, 6000);
    assert_eq!(state.fire_count, 1);
}

#[test]
fn alarm_replacement_works_at_capacity_and_past_deadline_notifies_immediately() {
    let mut state = reset();
    let mut timer = arch::PreparedTimer { ticks: 10 };
    set_alarm(&mut state, &mut timer, 19, 1, 18, Some(200)).unwrap();
    for _ in 1..MAX_PENDING_ASYNC_TIMERS {
        schedule(&mut state, 1, 100, 0).unwrap();
    }
    set_alarm(&mut state, &mut timer, 19, 1, 18, Some(50)).unwrap();
    assert_eq!(state.pending_timers.next(), Some(50));
    set_alarm(&mut state, &mut timer, 19, 1, 18, Some(9)).unwrap();
    assert_eq!(state.pending_timers.next(), Some(110));
    FAKE.with(|fake| assert_eq!(fake.borrow().notified, [32]));
}

#[test]
fn heap_removal_and_expiry_match_sorted_reference() {
    let mut queue = deadlines::Deadlines::new();
    let mut reference = Vec::new();
    for index in 0..512 {
        let target_tick = ((index * 293) % 512) as u64;
        let timer = PendingAsyncTimer {
            target_tick,
            interval_ticks: 0,
            notification_descriptor: index % 17,
            alarm: index % 3 == 0,
        };
        assert!(queue.push(timer));
        reference.push(timer);
    }
    queue.retire(5);
    reference.retain(|entry| entry.notification_descriptor != 5);
    // Each removal must repair the heap in either direction.
    for _ in 0..12 {
        queue.remove_alarm(7);
    }
    reference.retain(|entry| !(entry.notification_descriptor == 7 && entry.alarm));
    reference.sort_by_key(|entry| entry.target_tick);
    for entry in reference {
        assert_eq!(queue.next(), Some(entry.target_tick));
        assert_eq!(
            queue.pop_due(entry.target_tick).unwrap().target_tick,
            entry.target_tick
        );
    }
    assert_eq!(queue.next(), None);
}

#[test]
fn hpet_clock_extends_32_bit_wrap_without_time_going_backwards() {
    let mut clock = hpet_clock::Clock::new(10_000_000, false);
    assert_eq!(clock.observe(u32::MAX as u64 - 10), u32::MAX as u64 - 10);
    assert_eq!(clock.observe(20), (1u64 << 32) + 20);
    assert_eq!(clock.observe(20), (1u64 << 32) + 20);
    assert_eq!(
        clock.nanoseconds((1u64 << 32) + 20),
        ((1u64 << 32) + 20) * 10
    );
}

#[test]
fn hpet_conversion_never_programs_before_requested_nanosecond() {
    let clock = hpet_clock::Clock::new(69_841_279, true);
    for ns in [0, 1, 1000, 1_000_000, 9_000_000_000, u64::MAX] {
        let cycles = clock.cycles_ceil(ns);
        assert!(clock.nanoseconds(cycles) >= ns);
        if cycles != 0 {
            assert!(clock.nanoseconds(cycles - 1) < ns);
        }
    }
}

#[test]
#[ignore = "host microbenchmark; excludes interrupt and notification syscalls"]
fn idle_tick_benchmark() {
    use std::{hint::black_box, time::Instant};
    let mut state = reset();
    schedule(&mut state, 1, 60_000, 0).unwrap();
    let mut samples = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        for _ in 0..1_000_000 {
            fire_expired_async_timers(black_box(&mut state));
        }
        samples.push(start.elapsed().as_nanos() * 1000 / 1_000_000);
    }
    samples.sort();
    println!(
        "idle timer dispatch: {:.3} ns/tick",
        samples[2] as f64 / 1000.0
    );
}
