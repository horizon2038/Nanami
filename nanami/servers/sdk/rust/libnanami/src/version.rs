use crate::{call_os_port, RequestError, Word, OS_REQUEST_NANAMI_INFO, OS_RESPONSE_OK};

const NANAMI_INFO_KERNEL_VERSION: Word = 6;
const NANAMI_INFO_OS_VERSION: Word = 7;

/// Version of the running A9N kernel, including prerelease and build metadata.
/// Returns Protocol if the caller's buffer is too small or the reply is invalid.
pub fn request_kernel_version(buffer: &mut [u8]) -> Result<&str, RequestError> {
    request_version(NANAMI_INFO_KERNEL_VERSION, buffer)
}

/// Version of the running Nanami core, not the SDK or calling application.
pub fn request_nanami_version(buffer: &mut [u8]) -> Result<&str, RequestError> {
    request_version(NANAMI_INFO_OS_VERSION, buffer)
}

fn request_version(kind: Word, buffer: &mut [u8]) -> Result<&str, RequestError> {
    const CHUNK_BYTES: usize = 2 * core::mem::size_of::<Word>();
    let mut len = 0;
    for index in 0..=buffer.len() / CHUNK_BYTES {
        let (status, first, second) = call_os_port(OS_REQUEST_NANAMI_INFO, kind, index, 0, 0, 3)?;
        if status != OS_RESPONSE_OK {
            return Err(RequestError::Status(status));
        }
        for byte in first.to_le_bytes().into_iter().chain(second.to_le_bytes()) {
            if byte == 0 {
                if len == 0 {
                    return Err(RequestError::Protocol);
                }
                return core::str::from_utf8(&buffer[..len]).map_err(|_| RequestError::Protocol);
            }
            *buffer.get_mut(len).ok_or(RequestError::Protocol)? = byte;
            len += 1;
        }
    }
    Err(RequestError::Protocol)
}
