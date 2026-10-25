#![allow(dead_code)]
extern crate self as nanami_runtime;
extern crate self as nanami_client;
pub use std::{print, println};
pub type Word = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    Unsupported,
    Transport,
    Protocol,
}

mod mock;
pub use mock::{io, ipc, registry, request_notification_port_create, timer};

mod logging {
    pub fn log_request_error(_: &str, _: crate::RequestError) {}
}

#[path = "../../../nanami/servers/core-services/ps2-server/src/app/constants.rs"]
mod constants;
#[path = "../../../nanami/servers/core-services/ps2-server/src/app/controller.rs"]
mod controller;
#[path = "../../../nanami/servers/core-services/ps2-server/src/app/init.rs"]
mod init;
#[path = "../../../nanami/servers/core-services/ps2-server/src/app/state.rs"]
mod state;
#[path = "../../../nanami/servers/core-services/ps2-server/src/app/wait.rs"]
mod wait;

use mock::{Mode, HW};

fn initialize(mode: Mode) -> Result<(), RequestError> {
    HW.with_borrow_mut(|hw| hw.mode = mode);
    init::initialize_mouse(&mut state::Ps2Server::new(16, 0, 0))
}

#[test]
fn ready_controller_does_not_sleep_or_query_clock() {
    assert_eq!(initialize(Mode::Healthy), Ok(()));
    HW.with_borrow(|hw| {
        assert_eq!(hw.sleeps, 0);
        assert_eq!(hw.clock_reads, 0);
    });
}

#[test]
fn missing_controller_does_not_generate_fake_mouse_data_or_commands() {
    assert_eq!(initialize(Mode::Absent), Err(RequestError::Unsupported));
    HW.with_borrow(|hw| {
        assert!(hw.reads <= 2);
        assert_eq!(hw.writes, 0);
        assert_eq!(hw.sleeps, 0);
    });
}

#[test]
fn busy_input_buffer_sleeps_instead_of_starving_lower_priority_vfs() {
    assert_eq!(initialize(Mode::InputBusy), Err(RequestError::Transport));
    HW.with_borrow(|hw| {
        assert_eq!(hw.elapsed_ms, 200);
        assert_eq!(hw.sleeps, 200);
        assert!(hw.reads <= 202);
        assert_eq!(hw.writes, 0);
    });
}

#[test]
fn missing_config_reply_has_a_blocking_deadline() {
    assert_eq!(initialize(Mode::NoConfig), Err(RequestError::Transport));
    HW.with_borrow(|hw| {
        assert_eq!(hw.elapsed_ms, 500);
        assert_eq!(hw.sleeps, 500);
        assert!(hw.reads < 510);
    });
}

#[test]
fn missing_mouse_ack_has_a_blocking_deadline() {
    assert_eq!(initialize(Mode::NoAck), Err(RequestError::Transport));
    HW.with_borrow(|hw| {
        assert_eq!(hw.elapsed_ms, 500);
        assert_eq!(hw.sleeps, 500);
        assert!(hw.reads < 520);
    });
}

#[test]
fn coarse_pit_timer_uses_elapsed_time_not_a_number_of_wakeups() {
    HW.with_borrow_mut(|hw| hw.hz = 100);
    assert_eq!(initialize(Mode::InputBusy), Err(RequestError::Transport));
    HW.with_borrow(|hw| {
        assert_eq!(hw.elapsed_ms, 200);
        assert_eq!(hw.sleeps, 20);
    });
}

#[test]
fn unrelated_keyboard_bytes_do_not_extend_mouse_deadline() {
    assert_eq!(
        initialize(Mode::KeyboardFlood),
        Err(RequestError::Transport)
    );
    HW.with_borrow(|hw| {
        assert_eq!(hw.elapsed_ms, 500);
        assert_eq!(hw.sleeps, 500);
        assert!(hw.reads < 1100);
    });
}

#[test]
fn optional_mouse_id_timeout_still_allows_standard_mouse() {
    assert_eq!(initialize(Mode::NoMouseId), Ok(()));
    HW.with_borrow(|hw| assert_eq!(hw.elapsed_ms, 500));
}

#[test]
fn timer_failure_is_returned_not_replaced_with_busy_wait() {
    HW.with_borrow_mut(|hw| hw.fail_timer = true);
    assert_eq!(initialize(Mode::InputBusy), Err(RequestError::Transport));
    HW.with_borrow(|hw| assert!(hw.reads <= 2));
}

#[test]
fn sleep_uses_its_own_notification_not_irq_notification() {
    wait::initialize().unwrap();
    wait::sleep(10).unwrap();
    HW.with_borrow(|hw| assert_eq!(hw.created_notification, Some(27)));
}

#[test]
fn deadline_handles_clock_wrap() {
    HW.with_borrow_mut(|hw| hw.clock_origin = usize::MAX - 1_000_000);
    assert_eq!(initialize(Mode::InputBusy), Err(RequestError::Transport));
    HW.with_borrow(|hw| assert_eq!(hw.elapsed_ms, 200));
}
