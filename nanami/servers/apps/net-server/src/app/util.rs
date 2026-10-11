use super::*;

pub(crate) fn log_request_error(prefix: &str, err: RequestError) {
    nanami_runtime::println!("{}{}", prefix, err);
}

pub(crate) fn map_request_error_to_status(err: RequestError) -> Word {
    match err {
        RequestError::InvalidArgument => nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Transport => nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Protocol => nanami_runtime::OS_RESPONSE_FATAL,
        RequestError::Status(status) => status,
    }
}
