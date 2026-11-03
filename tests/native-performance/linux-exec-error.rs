//! Production exec outcome policy; emulate only the transport error type.
extern crate self as nanami_runtime;
pub use nanami_uapi::os::OS_RESPONSE_FATAL;
enum RequestError {
    Status(usize),
    Transport,
    Protocol,
    InvalidArgument,
    Unsupported,
}
fn map_request_error(_: RequestError) -> i32 {
    123
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/exec_error.rs"]
mod production;
use production::ExecError;
#[test]
fn fatal_or_ambiguous_exec_cannot_return_to_old_code() {
    for error in [
        RequestError::Status(OS_RESPONSE_FATAL),
        RequestError::Transport,
        RequestError::Protocol,
    ] {
        assert!(matches!(ExecError::from(error), ExecError::ImageReplaced));
    }
}
#[test]
fn checked_rejections_preserve_the_ordinary_errno_policy() {
    for error in [
        RequestError::Status(1),
        RequestError::Status(2),
        RequestError::InvalidArgument,
        RequestError::Unsupported,
    ] {
        assert!(matches!(ExecError::from(error), ExecError::Errno(123)));
    }
    assert!(matches!(ExecError::from(8i32), ExecError::Errno(8)));
}
