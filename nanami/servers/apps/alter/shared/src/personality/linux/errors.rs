use super::{
    RequestError, Word, EACCES, EADDRINUSE, EAGAIN, EEXIST, EINVAL, EIO, ENETDOWN, ENOENT, ENOSYS,
};

pub(super) fn map_word(result: Result<Word, RequestError>) -> Result<Word, i32> {
    result.map_err(map_request_error)
}

pub(super) fn map_unit(result: Result<(), RequestError>, value: Word) -> Result<Word, i32> {
    result.map(|_| value).map_err(map_request_error)
}

pub(super) fn map_request_error(error: RequestError) -> i32 {
    match error {
        RequestError::InvalidArgument => EINVAL,
        RequestError::Unsupported => ENOSYS,
        RequestError::Status(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT) => EINVAL,
        RequestError::Status(nanami_runtime::OS_RESPONSE_PERMISSION_DENIED) => EACCES,
        RequestError::Status(nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION) => EIO,
        RequestError::Status(_) | RequestError::Transport | RequestError::Protocol => EIO,
    }
}

pub(super) fn map_path_request_error(error: RequestError) -> i32 {
    match error {
        RequestError::InvalidArgument
        | RequestError::Status(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT)
        | RequestError::Status(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)
        | RequestError::Status(nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION) => ENOENT,
        other => map_request_error(other),
    }
}

pub(super) fn map_network_error(error: RequestError) -> i32 {
    match error {
        RequestError::InvalidArgument
        | RequestError::Status(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT) => EINVAL,
        RequestError::Status(nanami_runtime::OS_RESPONSE_PERMISSION_DENIED) => EACCES,
        RequestError::Status(nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION)
        | RequestError::Status(nanami_client::net::NET_SERVICE_RESPONSE_WOULD_BLOCK) => EAGAIN,
        _ => ENETDOWN,
    }
}

pub(super) fn map_link_request_error(error: RequestError) -> i32 {
    match error {
        RequestError::Status(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR) => ENOENT,
        RequestError::Status(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT) => EEXIST,
        other => map_request_error(other),
    }
}

pub(super) fn map_network_bind_error(error: RequestError) -> i32 {
    match error {
        RequestError::Status(nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION) => EADDRINUSE,
        other => map_network_error(other),
    }
}

pub(super) fn map_create_request_error(error: RequestError) -> i32 {
    match error {
        RequestError::Status(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT) => EEXIST,
        other => map_path_request_error(other),
    }
}

pub(super) fn result_to_linux_return(result: Result<Word, i32>) -> isize {
    match result {
        Ok(value) => value as isize,
        Err(errno) => -(errno as isize),
    }
}
