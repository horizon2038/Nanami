use a9n_abi::CapabilityDescriptor;
use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};
use super::constants::*;

pub fn posix_read_dir_biased(
    service_port: CapabilityDescriptor, fd: Word, max_entries: Word,
    out_offset: Word, bias: Word,
) -> Result<(Word, Word), RequestError> {
    let (status, first, second) = call_port(
        service_port, POSIX_REQUEST_READ_DIR_BIASED,
        fd, max_entries, out_offset, bias, 5,
    )?;
    match status {
        OS_RESPONSE_OK => Ok((first, second)),
        POSIX_READ_DIR_BIASED_ERROR if first != OS_RESPONSE_OK => Err(RequestError::Status(first)),
        POSIX_READ_DIR_BIASED_ERROR => Err(RequestError::Protocol),
        // An old POSIX server rejects this entry before doing directory I/O.
        crate::uapi::os::OS_RESPONSE_INVALID_ARGUMENT => Err(RequestError::Unsupported),
        _ => Err(RequestError::Status(status)),
    }
}
