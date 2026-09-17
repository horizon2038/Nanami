use super::{POSIX_REQUEST_FSYNC, POSIX_REQUEST_SYNC};
use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

pub fn posix_fsync(port: Word, fd: Word) -> Result<(), RequestError> {
    synchronize(port, POSIX_REQUEST_FSYNC, fd)
}

pub fn posix_sync(port: Word) -> Result<(), RequestError> {
    synchronize(port, POSIX_REQUEST_SYNC, 0)
}

fn synchronize(port: Word, code: Word, fd: Word) -> Result<(), RequestError> {
    let (status, _, _) = call_port(port, code, fd, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}
