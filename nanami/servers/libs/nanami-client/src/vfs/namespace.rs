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
