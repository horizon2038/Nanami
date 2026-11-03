//! An ambiguous or post-commit exec failure must never resume the old image.
use super::{map_request_error, RequestError};

pub(super) enum ExecError {
    Errno(i32),
    ImageReplaced,
}

impl From<i32> for ExecError {
    fn from(errno: i32) -> Self {
        Self::Errno(errno)
    }
}

impl From<RequestError> for ExecError {
    fn from(error: RequestError) -> Self {
        match error {
            RequestError::Status(nanami_runtime::OS_RESPONSE_FATAL)
            | RequestError::Transport
            | RequestError::Protocol => Self::ImageReplaced,
            other => Self::Errno(map_request_error(other)),
        }
    }
}
