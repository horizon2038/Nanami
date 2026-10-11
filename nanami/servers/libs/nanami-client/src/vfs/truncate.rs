use super::VFS_REQUEST_FTRUNCATE;
use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

pub fn vfs_ftruncate(port: Word, handle: Word, length: Word) -> Result<(), RequestError> {
    let (status, _, _) = call_port(port, VFS_REQUEST_FTRUNCATE, handle, length, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}
