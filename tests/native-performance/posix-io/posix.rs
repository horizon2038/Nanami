#[path = "../../../nanami/servers/uapi/src/posix.rs"]
pub mod constants;
#[path = "../../../nanami/servers/libs/nanami-client/src/posix/io.rs"]
mod io;
#[path = "../../../nanami/servers/libs/nanami-client/src/posix/sync.rs"]
mod sync;
pub use constants::*;
pub use io::*;
pub use sync::*;
#[path = "../../../nanami/servers/libs/nanami-client/src/posix/truncate.rs"]
mod truncate;
pub use truncate::*;

pub fn posix_seek(_: super::Word, fd: super::Word, offset: super::Word, whence: super::Word) -> Result<super::Word, super::RequestError> {
    super::record(super::Call { code: POSIX_REQUEST_SEEK, handle: fd, offset, len: whence, buffer: 0, delegate: 0 })
}

pub fn posix_fstat_flags(_: super::Word, fd: super::Word, flags: super::Word)
    -> Result<(super::Word, super::Word, super::Word, super::Word, super::Word), super::RequestError> {
    super::record(super::Call { code: POSIX_REQUEST_FSTAT, handle: fd, buffer: flags, len: 0, offset: 0, delegate: 0 })?;
    Ok((91, 13, POSIX_FILE_TYPE_REGULAR, 0, 0))
}
