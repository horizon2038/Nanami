use super::*;
#[path = "../../../nanami/servers/sdk/rust/nanami-services/src/vfs/constants.rs"]
mod constants;
pub use constants::*;
#[path = "../../../nanami/servers/sdk/rust/nanami-services/src/vfs/truncate.rs"]
mod truncate;
pub use truncate::*;

pub fn vfs_fsync(_port: Word, handle: Word) -> Result<(), RequestError> {
    record(Call {
        code: VFS_REQUEST_FSYNC,
        handle,
        offset: 0,
        len: 0,
        delegate: 0,
        buffer: 0,
    })
    .and_then(|_| BACKEND.with(|backend| backend.borrow().sync_error.map_or(Ok(()), Err)))
}

pub fn vfs_sync(_port: Word) -> Result<(), RequestError> {
    record(Call {
        code: VFS_REQUEST_SYNC,
        handle: 0,
        offset: 0,
        len: 0,
        delegate: 0,
        buffer: 0,
    })
    .map(|_| ())
}

pub fn vfs_write_delegated(
    _port: Word,
    handle: Word,
    offset: Word,
    len: Word,
    delegate: Word,
    buffer: Word,
) -> Result<Word, RequestError> {
    record(Call {
        code: VFS_REQUEST_WRITE_DELEGATED,
        handle,
        offset,
        len,
        delegate,
        buffer,
    })
}

pub fn vfs_write(
    _port: Word,
    handle: Word,
    offset: Word,
    len: Word,
    buffer: Word,
) -> Result<Word, RequestError> {
    record(Call {
        code: VFS_REQUEST_WRITE,
        handle,
        offset,
        len,
        delegate: 0,
        buffer,
    })
}

pub fn vfs_read_delegated(
    _port: Word,
    handle: Word,
    offset: Word,
    len: Word,
    delegate: Word,
    buffer: Word,
) -> Result<Word, RequestError> {
    record(Call {
        code: VFS_REQUEST_READ_DELEGATED,
        handle,
        offset,
        len,
        delegate,
        buffer,
    })
}

pub fn vfs_read(
    _port: Word,
    handle: Word,
    offset: Word,
    len: Word,
    buffer: Word,
) -> Result<Word, RequestError> {
    record(Call {
        code: VFS_REQUEST_READ,
        handle,
        offset,
        len,
        delegate: 0,
        buffer,
    })
}

pub fn vfs_fstat(_port: Word, _handle: Word) -> Result<(Word, Word, Word), RequestError> {
    BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        backend.stats += 1;
        Ok((1, backend.size, VFS_FILE_TYPE_REGULAR))
    })
}

pub fn vfs_read_dir(
    _port: Word,
    _handle: Word,
    _offset: Word,
    _len: Word,
    _buffer: Word,
) -> Result<(Word, Word), RequestError> {
    panic!("unexpected directory I/O")
}
