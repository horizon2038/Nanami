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
    pub fn start(_resource: Word) -> Result<(), RequestError> {
        FAKE.with(|state| state.borrow_mut().starts += 1);
        Ok(())
    }
}
fn log_request_error(_message: &str, _error: RequestError) {}
#[path = "../../nanami/servers/core-services/timer-server/src/state.rs"]
mod state;
use state::*;
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
    schedule_timer(state, 16, 19, pid, 18, delay, interval)
}
fn tick(state: &mut TimerState, now: u64) {
    state.ticks = now;
    fire_expired_async_timers(state);
}

#[test]
fn idle_and_future_ticks_do_not_fire_and_earlier_insert_wakes_on_time() {
    let mut state = reset();
    tick(&mut state, 50);
    assert_eq!(state.next_deadline, None);
    schedule(&mut state, 1, 100, 0).unwrap();
    schedule(&mut state, 2, 10, 0).unwrap();
    assert_eq!(state.next_deadline, Some(60));
    tick(&mut state, 59);
    assert_eq!(state.fire_count, 0);
    tick(&mut state, 60);
    assert_eq!(state.fire_count, 1);
    assert_eq!(state.next_deadline, Some(150));
    tick(&mut state, 149);
    assert_eq!(state.fire_count, 1);
    tick(&mut state, 150);
    assert_eq!(state.next_deadline, None);
    FAKE.with(|fake| assert_eq!(fake.borrow().notified, [33, 32]));
}

#[test]
fn periodic_timer_skips_missed_periods_without_drifting() {
    let mut state = reset();
    schedule(&mut state, 1, 10, 10).unwrap();
    tick(&mut state, 35);
    assert_eq!(state.next_deadline, Some(40));
    assert_eq!(state.fire_count, 1);
    tick(&mut state, 40);
    assert_eq!(state.next_deadline, Some(50));
    tick(&mut state, 1_000_000_000_001);
    assert_eq!(state.next_deadline, Some(1_000_000_000_010));
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
    assert_eq!(state.next_deadline, Some(50));
    tick(&mut state, 49);
    assert_eq!(state.fire_count, 1);
    tick(&mut state, 50);
    assert_eq!(state.next_deadline, None);
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
    assert_eq!(state.next_deadline, Some(50));
    tick(&mut state, 50);
    assert_eq!(state.fire_count, MAX_PENDING_ASYNC_TIMERS);
    assert_eq!(state.next_deadline, None);
    schedule(&mut state, 1, 10, 0).unwrap();
    assert_eq!(state.next_deadline, Some(60));
    FAKE.with(|fake| {
        assert_eq!(fake.borrow().starts, 1);
        assert_eq!(fake.borrow().copies, 1);
    });
}

#[test]
fn immediate_timer_does_not_change_deadlines_or_start_hardware() {
    let mut state = reset();
    schedule(&mut state, 1, 0, 0).unwrap();
    assert_eq!(state.next_deadline, None);
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
    assert_eq!(state.next_deadline, Some(u64::MAX));
    tick(&mut state, u64::MAX);
    assert_eq!(state.next_deadline, None);
    tick(&mut state, u64::MAX);
    assert_eq!(state.fire_count, 2);
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
