#![allow(dead_code)]
extern crate self as libnanami;
extern crate self as nanami_services;
use std::cell::RefCell;

pub type Word = usize;
pub const PROCESS_SLOT_NOTIFICATION: Word = 18;
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
    pub const TIMER_NOTIFICATION_IDENTIFIER_BIT: usize = 0x100;
}
#[derive(Default)]
struct Fake {
    notified: Vec<Word>,
    failed_descriptor: Option<Word>,
    copies: usize,
    starts: usize,
}
thread_local! { static FAKE: RefCell<Fake> = RefCell::new(Fake::default()); }
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
    _pid: Word,
    _source: Word,
    _dest: Word,
    _id: Word,
) -> Result<(), RequestError> {
    FAKE.with(|state| state.borrow_mut().copies += 1);
    Ok(())
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
            self.ticks
        }
    }
}
fn log_request_error(_message: &str, _error: RequestError) {}
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
