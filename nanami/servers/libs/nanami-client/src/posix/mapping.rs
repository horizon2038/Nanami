use super::constants::*;
use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

pub fn posix_fstatfs(
    port: Word,
    fd: Word,
    shm: Word,
) -> Result<nanami_uapi::vfs::FilesystemStats, RequestError> {
    let (status, _, _) = call_port(port, POSIX_REQUEST_FSTATFS, fd, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(unsafe { core::ptr::read_unaligned(shm as *const nanami_uapi::vfs::FilesystemStats) })
}

pub fn posix_map_read_page(port: Word, fd: Word, offset: Word) -> Result<Word, RequestError> {
    let (status, address, _) = call_port(port, POSIX_REQUEST_MAP_READ_PAGE, fd, offset, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if address == 0 || address & (POSIX_PAGE_SIZE - 1) != 0 {
        return Err(RequestError::Protocol);
    }
    Ok(address)
}

pub fn posix_unmap_read_page(port: Word, fd: Word, offset: Word) -> Result<(), RequestError> {
    let (status, _, _) = call_port(port, POSIX_REQUEST_UNMAP_READ_PAGE, fd, offset, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}
