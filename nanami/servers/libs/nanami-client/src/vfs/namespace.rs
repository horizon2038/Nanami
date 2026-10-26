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

fn path_request(
    port: CapabilityDescriptor,
    code: Word,
    descriptor_offset: Word,
) -> Result<(Word, Word), PathError> {
    let (status, detail0, detail1) =
        call_port(port, code, descriptor_offset, 0, 0, 0, 2).map_err(PathError::Transport)?;
    if status == VFS_PATH_RESPONSE_ERROR {
        return Err(PathError::Filesystem(detail0));
    }
    if status != OS_RESPONSE_OK {
        return Err(PathError::Transport(RequestError::Status(status)));
    }
    Ok((detail0, detail1))
}
