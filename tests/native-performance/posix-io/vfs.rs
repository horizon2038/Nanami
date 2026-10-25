use super::*;
#[path = "../../../nanami/servers/uapi/src/vfs.rs"]
mod constants;
pub use constants::*;
#[path = "../../../nanami/servers/libs/nanami-client/src/vfs/truncate.rs"]
mod truncate;
pub use truncate::*;
#[path = "../../../nanami/servers/libs/nanami-client/src/vfs/service.rs"]
mod service;

#[test]
fn socket_creation_transmits_the_node_type_as_the_fourth_word() {
    reset();
    assert_eq!(
        service::vfs_create_node(8, 128, 17, VFS_FILE_TYPE_SOCKET),
        Ok(VFS_FILE_TYPE_SOCKET)
    );
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        assert_eq!(backend.calls.len(), 1);
        assert_eq!(backend.calls[0].code, VFS_REQUEST_CREATE);
        assert_eq!(backend.calls[0].handle, 128);
        assert_eq!(backend.calls[0].buffer, 17);
        assert_eq!(backend.calls[0].len, VFS_FILE_TYPE_SOCKET);
    });
}

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
