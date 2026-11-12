use super::constants::*;
use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};
use a9n_abi::CapabilityDescriptor;

#[derive(Clone, Copy, Debug)]
pub enum PathError {
    Transport(RequestError),
    Filesystem(Word),
}

pub fn vfs_path_request(
    port: CapabilityDescriptor,
    descriptor_offset: Word,
) -> Result<(Word, Word), PathError> {
    path_request(port, VFS_REQUEST_PATH, descriptor_offset)
}

pub fn posix_path_request(
    port: CapabilityDescriptor,
    descriptor_offset: Word,
) -> Result<(Word, Word), PathError> {
    path_request(
        port,
        crate::uapi::posix::POSIX_REQUEST_PATH,
        descriptor_offset,
    )
}

pub fn posix_rooted_open_request(
    port: CapabilityDescriptor, descriptor_offset: Word,
) -> Result<(Word, Word), PathError> {
    match path_request(port, crate::uapi::posix::POSIX_REQUEST_OPEN_ROOTED, descriptor_offset) {
        // Legacy POSIX dispatchers reject this unknown request before forwarding
        // to VFS, hence no handle was acquired. Do not map other failures.
        Err(PathError::Transport(RequestError::Status(crate::uapi::os::OS_RESPONSE_INVALID_ARGUMENT))) => {
            Err(PathError::Filesystem(VFS_PATH_ERROR_UNSUPPORTED))
        }
        other => other,
    }
}

pub fn posix_rooted_open_granted_request(
    port: CapabilityDescriptor, descriptor_offset: Word,
) -> Result<(Word, Word), PathError> {
    granted_open(port, crate::uapi::posix::POSIX_REQUEST_OPEN_ROOTED_GRANTED, descriptor_offset)
}

pub fn posix_rooted_open_cursor_granted_request(
    port: CapabilityDescriptor, descriptor_offset: Word,
) -> Result<(Word, Word), PathError> {
    granted_open(port, crate::uapi::posix::POSIX_REQUEST_OPEN_ROOTED_CURSOR_GRANTED, descriptor_offset)
}

pub fn vfs_rooted_open_cursor_granted_request(
    port: CapabilityDescriptor, descriptor_offset: Word,
) -> Result<(Word, Word), PathError> {
    granted_open(port, VFS_REQUEST_OPEN_ROOTED_CURSOR_GRANTED, descriptor_offset)
}

pub fn vfs_rooted_open_granted_request(
    port: CapabilityDescriptor, descriptor_offset: Word,
) -> Result<(Word, Word), PathError> {
    granted_open(port, VFS_REQUEST_OPEN_ROOTED_GRANTED, descriptor_offset)
}

fn granted_open(port: CapabilityDescriptor, code: Word, offset: Word) -> Result<(Word, Word), PathError> {
    match path_request(port, code, offset) {
        // Old dispatchers reject the distinct entry before acquiring a handle.
        Err(PathError::Transport(RequestError::Status(crate::uapi::os::OS_RESPONSE_INVALID_ARGUMENT))) =>
            Err(PathError::Filesystem(VFS_PATH_ERROR_UNSUPPORTED)),
        result => result,
    }
}

pub fn vfs_fchmod(port: CapabilityDescriptor, handle: Word, mode: Word) -> Result<(), PathError> {
    fchmod(port, VFS_REQUEST_FCHMOD, handle, mode)
}

pub fn posix_fchmod(port: CapabilityDescriptor, fd: Word, mode: Word) -> Result<(), PathError> {
    fchmod(port, crate::uapi::posix::POSIX_REQUEST_FCHMOD, fd, mode)
}

fn fchmod(port: CapabilityDescriptor, code: Word, fd: Word, mode: Word) -> Result<(), PathError> {
    let reply = call_port(port, code, fd, mode, 0, 0, 3).map_err(PathError::Transport)?;
    decode_reply(reply).map(|_| ())
}

fn path_request(
    port: CapabilityDescriptor,
    code: Word,
    descriptor_offset: Word,
) -> Result<(Word, Word), PathError> {
    let reply =
        call_port(port, code, descriptor_offset, 0, 0, 0, 2).map_err(PathError::Transport)?;
    decode_reply(reply)
}

fn decode_reply((status, detail0, detail1): (Word, Word, Word)) -> Result<(Word, Word), PathError> {
    if status == VFS_PATH_RESPONSE_ERROR {
        return Err(PathError::Filesystem(detail0));
    }
    if status != OS_RESPONSE_OK {
        return Err(PathError::Transport(RequestError::Status(status)));
    }
    Ok((detail0, detail1))
}
