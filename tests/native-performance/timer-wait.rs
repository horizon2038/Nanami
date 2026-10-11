//! Timer SDK regression tests: badges are coalesced hints, not timer IDs.
#![allow(dead_code)]
extern crate self as a9n_abi;
use std::{cell::RefCell, collections::VecDeque};
pub type Word = usize;
pub type CapabilityDescriptor = usize;
pub const OS_RESPONSE_OK: Word = 0;
pub const PROCESS_SLOT_NOTIFICATION: Word = 21;
#[derive(Debug, PartialEq)]
pub enum RequestError {
    Status(Word),
    Protocol,
    Exhausted,
}
#[derive(Default)]
struct State {
    now: Word,
    requested: Word,
    deferred: Word,
    notifications: VecDeque<(Word, Word)>,
}
thread_local! { static STATE: RefCell<State> = RefCell::default(); }
pub fn call_port(
    _: Word,
    code: Word,
    delay: Word,
    _: Word,
    _: Word,
    _: Word,
    _: u8,
) -> Result<(Word, Word, Word), RequestError> {
    STATE.with_borrow_mut(|s| {
        if code == timer::TIMER_SERVICE_REQUEST_MONOTONIC_TICKS {
            return Ok((0, s.now, 1000));
        }
        s.requested = delay;
        Ok((0, 0, 0))
    })
}
pub mod ipc {
    use super::*;
    pub fn process_slot_descriptor(slot: Word) -> Word {
        slot
    }
    pub fn preserve_interrupted_notification(badge: Word) {
        STATE.with_borrow_mut(|s| s.deferred |= badge);
    }
    pub fn notification_wait(slot: Word) -> Result<Word, RequestError> {
        STATE.with_borrow_mut(|s| {
            if slot == 21 && s.deferred != 0 {
                return Ok(std::mem::take(&mut s.deferred));
            }
            let (time, badge) = s.notifications.pop_front().ok_or(RequestError::Exhausted)?;
            s.now = time;
            Ok(badge)
        })
    }
}
#[path = "../../nanami/servers/libs/nanami-client/src/timer.rs"]
mod timer;
const TIMER: Word = timer::TIMER_NOTIFICATION_IDENTIFIER_BIT;
const IRQ: Word = 1 << 16;
fn setup(events: &[(Word, Word)]) {
    STATE.with_borrow_mut(|s| {
        *s = State {
            notifications: events.iter().copied().collect(),
            ..State::default()
        }
    });
}
#[test]
fn sleep_preserves_unrelated_and_combined_irq_badges() {
    setup(&[(1, IRQ), (100, TIMER | 2)]);
    timer::timer_service_sleep_milliseconds(24, 100).unwrap();
    assert_eq!(ipc::notification_wait(21), Ok(IRQ | 2));
}
#[test]
fn periodic_notification_does_not_complete_a_longer_sleep() {
    setup(&[(10, TIMER), (20, TIMER), (100, TIMER)]);
    timer::timer_service_sleep_milliseconds(24, 100).unwrap();
    STATE.with_borrow(|s| assert_eq!(s.now, 100));
}
#[test]
fn failed_wait_restores_consumed_irq() {
    setup(&[(1, IRQ)]);
    assert_eq!(
        timer::timer_service_sleep_milliseconds(24, 100),
        Err(RequestError::Exhausted)
    );
    assert_eq!(ipc::notification_wait(21), Ok(IRQ));
}
#[test]
fn private_delay_does_not_consume_bound_notification() {
    setup(&[(100, TIMER)]);
    ipc::preserve_interrupted_notification(IRQ);
    timer::timer_service_sleep_on_notification_milliseconds(24, 100, 26).unwrap();
    assert_eq!(ipc::notification_wait(21), Ok(IRQ));
}
#[test]
fn blocking_sleep_waits_for_expiry_not_just_server_ack() {
    setup(&[(100, TIMER)]);
    timer::timer_service_sleep_blocking_server_milliseconds(24, 100).unwrap();
    STATE.with_borrow(|s| assert_eq!(s.now, 100));
}

#[test]
fn explicit_default_slot_uses_the_shared_notification_rules() {
    setup(&[(10, TIMER | IRQ), (100, TIMER)]);
    timer::timer_service_sleep_on_notification_milliseconds(24, 100, 21).unwrap();
    STATE.with_borrow(|s| assert_eq!(s.now, 100));
    assert_eq!(ipc::notification_wait(21), Ok(IRQ));
}
