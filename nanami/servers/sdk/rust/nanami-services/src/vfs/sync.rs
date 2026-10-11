use super::{VFS_REQUEST_FSYNC, VFS_REQUEST_SYNC};
use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

pub fn vfs_fsync(port: Word, handle: Word) -> Result<(), RequestError> {
    synchronize(port, VFS_REQUEST_FSYNC, handle)
}

pub fn vfs_sync(port: Word) -> Result<(), RequestError> {
    synchronize(port, VFS_REQUEST_SYNC, 0)
}

/// Power-manager only: synchronize the device and reject further mutations.
pub fn vfs_freeze(port: Word) -> Result<(), RequestError> {
    synchronize(port, super::VFS_REQUEST_FREEZE, 0)
}

/// Undo a freeze when a power transition fails.
pub fn vfs_thaw(port: Word) -> Result<(), RequestError> {
    synchronize(port, super::VFS_REQUEST_THAW, 0)
}

fn synchronize(port: Word, code: Word, handle: Word) -> Result<(), RequestError> {
    let (status, _, _) = call_port(port, code, handle, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}
