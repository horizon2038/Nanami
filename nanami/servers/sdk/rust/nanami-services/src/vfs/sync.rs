use super::{VFS_REQUEST_FSYNC, VFS_REQUEST_SYNC};
use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

pub fn vfs_fsync(port: Word, handle: Word) -> Result<(), RequestError> {
    synchronize(port, VFS_REQUEST_FSYNC, handle)
}

pub fn vfs_sync(port: Word) -> Result<(), RequestError> {
    synchronize(port, VFS_REQUEST_SYNC, 0)
}

fn synchronize(port: Word, code: Word, handle: Word) -> Result<(), RequestError> {
    let (status, _, _) = call_port(port, code, handle, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}
