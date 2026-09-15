//! Exercise ext2's real startup loop with deterministic service/timer responses.
extern crate self as libnanami;
extern crate self as nanami_services;

use std::cell::RefCell;

pub type Word = usize;
pub const OS_RESPONSE_INVALID_ARGUMENT: Word = 1;
const SLOT_BLOCK_DEVICE: Word = 23;
const SLOT_TIMER_SERVICE: Word = 24;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestError {
    Status(Word),
    Transport,
    Protocol,
}

#[derive(Clone, Default)]
struct Fake {
    block_after: usize,
    timer_after: usize,
    block_error: Option<RequestError>,
    timer_error: Option<RequestError>,
    sleep_error: Option<RequestError>,
    block_calls: usize,
    timer_calls: usize,
    sleeps: usize,
    elapsed_ms: usize,
    yields: usize,
}

thread_local! {
    static FAKE: RefCell<Fake> = RefCell::new(Fake::default());
}

pub mod ipc {
    pub fn process_slot_descriptor(slot: usize) -> usize {
        slot
    }
}

pub mod registry {
    use super::*;
    pub fn connect_block_device_with_pid(slot: Word) -> Result<Word, RequestError> {
        assert_eq!(slot, SLOT_BLOCK_DEVICE);
        FAKE.with(|state| {
            let mut state = state.borrow_mut();
            state.block_calls += 1;
            assert!(state.block_calls < 10_000, "startup loop made no progress");
            if let Some(error) = state.block_error {
                return Err(error);
            }
            if state.block_calls > state.block_after {
                Ok(5)
            } else {
                Err(RequestError::Status(OS_RESPONSE_INVALID_ARGUMENT))
            }
        })
    }

    pub fn connect_timer_service(slot: Word) -> Result<(), RequestError> {
        assert_eq!(slot, SLOT_TIMER_SERVICE);
        FAKE.with(|state| {
            let mut state = state.borrow_mut();
            state.timer_calls += 1;
            if let Some(error) = state.timer_error {
                return Err(error);
            }
            if state.timer_calls > state.timer_after {
                Ok(())
            } else {
                Err(RequestError::Status(OS_RESPONSE_INVALID_ARGUMENT))
            }
        })
    }
}

pub mod timer {
    use super::*;
    pub fn timer_service_sleep_milliseconds(
        port: Word,
        milliseconds: Word,
    ) -> Result<(), RequestError> {
        assert_eq!(port, SLOT_TIMER_SERVICE);
        FAKE.with(|state| {
            let mut state = state.borrow_mut();
            state.sleeps += 1;
            if let Some(error) = state.sleep_error {
                return Err(error);
            }
            state.elapsed_ms += milliseconds;
            Ok(())
        })
    }
}

pub fn yield_now() {
    FAKE.with(|state| state.borrow_mut().yields += 1);
}

fn log_request_error(_message: &str, _error: RequestError) {}

#[path = "../../nanami/servers/core-services/ext2-server/src/block_connection.rs"]
mod block_connection;

fn run(fake: Fake) -> (Result<Word, RequestError>, Fake) {
    FAKE.with(|state| *state.borrow_mut() = fake);
    let result = block_connection::connect_block_device();
    (result, FAKE.with(|state| state.borrow().clone()))
}

#[test]
fn ready_storage_does_not_require_a_timer() {
    let (result, calls) = run(Fake::default());
    assert_eq!(result, Ok(SLOT_BLOCK_DEVICE));
    assert_eq!(
        (
            calls.block_calls,
            calls.timer_calls,
            calls.sleeps,
            calls.yields
        ),
        (1, 0, 0, 0)
    );
}

#[test]
fn timer_registering_after_more_than_64_yields_is_rediscovered() {
    let (result, calls) = run(Fake {
        block_after: 130,
        timer_after: 128,
        ..Fake::default()
    });
    assert_eq!(result, Ok(SLOT_BLOCK_DEVICE));
    assert_eq!(
        (
            calls.timer_calls,
            calls.yields,
            calls.sleeps,
            calls.elapsed_ms
        ),
        (129, 128, 2, 200)
    );
}

#[test]
fn storage_can_register_without_any_timer_after_more_than_64_yields() {
    let (result, calls) = run(Fake {
        block_after: 100,
        timer_after: usize::MAX,
        ..Fake::default()
    });
    assert_eq!(result, Ok(SLOT_BLOCK_DEVICE));
    assert_eq!((calls.yields, calls.sleeps), (100, 0));
}

#[test]
fn missing_storage_times_out_after_actual_delays() {
    let (result, calls) = run(Fake {
        block_after: usize::MAX,
        ..Fake::default()
    });
    assert_eq!(
        result,
        Err(RequestError::Status(OS_RESPONSE_INVALID_ARGUMENT))
    );
    assert_eq!(
        (
            calls.block_calls,
            calls.timer_calls,
            calls.sleeps,
            calls.elapsed_ms
        ),
        (601, 1, 600, 60000)
    );
}

#[test]
fn storage_is_checked_after_the_last_delay() {
    let (result, calls) = run(Fake {
        block_after: 600,
        ..Fake::default()
    });
    assert_eq!(result, Ok(SLOT_BLOCK_DEVICE));
    assert_eq!(calls.sleeps, 600);
}

#[test]
fn transport_errors_are_not_retried_as_missing_services() {
    let (result, calls) = run(Fake {
        block_error: Some(RequestError::Transport),
        ..Fake::default()
    });
    assert_eq!(result, Err(RequestError::Transport));
    assert_eq!((calls.block_calls, calls.timer_calls), (1, 0));
    let (result, calls) = run(Fake {
        block_after: 100,
        timer_error: Some(RequestError::Transport),
        ..Fake::default()
    });
    assert_eq!(result, Err(RequestError::Transport));
    assert_eq!((calls.timer_calls, calls.sleeps, calls.yields), (1, 0, 0));
}

#[test]
fn failed_timer_wait_is_reported_not_counted_as_elapsed_time() {
    let (result, calls) = run(Fake {
        block_after: 100,
        sleep_error: Some(RequestError::Protocol),
        ..Fake::default()
    });
    assert_eq!(result, Err(RequestError::Protocol));
    assert_eq!((calls.sleeps, calls.elapsed_ms, calls.yields), (1, 0, 0));
}
