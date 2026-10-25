//! Production display selection with fake process/framebuffer startup.
#![allow(dead_code)]
extern crate self as nanami_runtime;
use std::cell::RefCell;
pub type Word = usize;
pub const OS_RESPONSE_PERMISSION_DENIED: Word = 5;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    InvalidArgument,
    Unsupported,
    Transport,
    Status(Word),
}
impl std::fmt::Display for RequestError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{self:?}")
    }
}
#[macro_export]
macro_rules! println { ($($arg:tt)*) => {{ let _ = format_args!($($arg)*); }}; }
#[path = "../../nanami/servers/core-services/driver-manager/src/display.rs"]
mod display;

#[derive(Default)]
struct Startup {
    native_error: Option<RequestError>,
    framebuffer_error: Option<RequestError>,
    images: Vec<String>,
    framebuffer_attempts: usize,
}
thread_local! { static STARTUP: RefCell<Startup> = RefCell::new(Startup::default()); }
fn reset() {
    STARTUP.with(|state| *state.borrow_mut() = Startup::default());
}
fn spawn(image: &str) -> Result<Word, RequestError> {
    STARTUP.with(|state| {
        let mut state = state.borrow_mut();
        state.images.push(image.into());
        match state.native_error {
            Some(error) => Err(error),
            None => Ok(12),
        }
    })
}
fn start_framebuffer() -> Result<(), RequestError> {
    STARTUP.with(|state| {
        let mut state = state.borrow_mut();
        state.framebuffer_attempts += 1;
        match state.framebuffer_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    })
}
fn fallback_attempts() -> usize {
    STARTUP.with(|state| state.borrow().framebuffer_attempts)
}

#[test]
fn unknown_gpu_or_platform_without_a_native_backend_starts_framebuffer_directly() {
    reset();
    let mut display = display::Selection::start(None);
    assert_eq!(fallback_attempts(), 1);
    STARTUP.with(|state| assert!(state.borrow().images.is_empty()));
    assert_eq!(display.report(0, 0), Err(RequestError::Status(5)));
    assert_eq!(display.report(12, 0), Err(RequestError::Status(5)));
    assert_eq!(fallback_attempts(), 1);
}

#[test]
fn failed_native_driver_spawn_uses_the_same_framebuffer_fallback() {
    for error in [
        RequestError::Unsupported,
        RequestError::Transport,
        RequestError::Status(1),
    ] {
        reset();
        STARTUP.with(|state| state.borrow_mut().native_error = Some(error));
        let _ = display::Selection::start(Some("./bin/intel-graphics-server"));
        assert_eq!(fallback_attempts(), 1);
        STARTUP.with(|state| assert_eq!(state.borrow().images, ["./bin/intel-graphics-server"]));
    }
}

#[test]
fn inapplicable_native_backend_declines_before_activation_and_starts_only_one_fallback() {
    reset();
    let mut display = display::Selection::start(Some("./bin/intel-graphics-server"));
    assert_eq!(fallback_attempts(), 0);
    assert_eq!(display.report(12, 0), Ok(()));
    assert_eq!(fallback_attempts(), 1);
    assert_eq!(display.report(12, 0), Err(RequestError::InvalidArgument));
    assert_eq!(display.report(12, 1), Err(RequestError::InvalidArgument));
    assert_eq!(fallback_attempts(), 1);
}

#[test]
fn accepted_native_backend_cannot_start_a_second_display_owner_later() {
    reset();
    let mut display = display::Selection::start(Some("./bin/intel-graphics-server"));
    assert_eq!(display.report(12, 1), Ok(()));
    assert_eq!(display.report(12, 0), Err(RequestError::InvalidArgument));
    assert_eq!(display.report(12, 1), Err(RequestError::InvalidArgument));
    assert_eq!(fallback_attempts(), 0);
}

#[test]
fn only_the_selected_driver_can_report_and_malformed_reports_do_not_select_a_backend() {
    reset();
    let mut display = display::Selection::start(Some("./bin/intel-graphics-server"));
    for caller in [0, 1, 11, 13] {
        assert_eq!(display.report(caller, 0), Err(RequestError::Status(5)));
        assert_eq!(display.report(caller, 1), Err(RequestError::Status(5)));
    }
    assert_eq!(display.report(12, 2), Err(RequestError::InvalidArgument));
    assert_eq!(fallback_attempts(), 0);
    assert_eq!(display.report(12, 0), Ok(()));
    assert_eq!(fallback_attempts(), 1);
}

#[test]
fn failed_fallback_start_is_not_misreported_as_success_and_can_be_retried() {
    reset();
    let mut display = display::Selection::start(Some("./bin/intel-graphics-server"));
    STARTUP.with(|state| state.borrow_mut().framebuffer_error = Some(RequestError::Transport));
    assert_eq!(display.report(12, 0), Err(RequestError::Transport));
    STARTUP.with(|state| state.borrow_mut().framebuffer_error = None);
    assert_eq!(display.report(12, 0), Ok(()));
    assert_eq!(display.report(12, 0), Err(RequestError::InvalidArgument));
    assert_eq!(fallback_attempts(), 2);
}

#[test]
fn absent_boot_framebuffer_is_handled_without_crashing_the_manager() {
    reset();
    STARTUP.with(|state| state.borrow_mut().framebuffer_error = Some(RequestError::Unsupported));
    let mut display = display::Selection::start(None);
    assert_eq!(fallback_attempts(), 1);
    assert_eq!(display.report(12, 1), Err(RequestError::Status(5)));
}
