use super::POSIX_REQUEST_FTRUNCATE;
use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

pub fn posix_ftruncate(port: Word, fd: Word, length: Word) -> Result<(), RequestError> {
    let (status, _, _) = call_port(port, POSIX_REQUEST_FTRUNCATE, fd, length, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}
