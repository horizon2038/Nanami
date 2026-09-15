use super::*;

struct Runtime {
    posix_port: Word,
    posix_shm: Word,
    posix_shm_size: Word,
    posix_direct_shm: Word,
    posix_direct_shm_size: Word,
}

#[path = "../../../nanami/servers/apps/alter/shared/src/common/state/io.rs"]
mod io;

fn runtime() -> Runtime {
    reset();
    Runtime {
        posix_port: 7,
        posix_shm: 0x1000,
        posix_shm_size: 65536,
        posix_direct_shm: 0x20000,
        posix_direct_shm_size: 65536,
    }
}

#[test]
fn negotiated_buffer_selects_direct_write_and_pwrite() {
    let runtime = runtime();
    assert_eq!(runtime.posix_write_buffer(), (0x20000, 65536));
    assert_eq!(runtime.write_posix(3, 0, 65536, None), Ok(65536));
    assert_eq!(runtime.write_posix(3, 4, 13, Some(123)), Ok(13));
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        assert_eq!(backend.calls[0].code, posix::POSIX_REQUEST_WRITE_DIRECT);
        assert_eq!(
            backend.calls[1],
            Call {
                code: posix::POSIX_REQUEST_PWRITE_DIRECT,
                handle: 3,
                offset: 123,
                len: 13,
                delegate: 0,
                buffer: 4
            }
        );
    });
}

#[test]
fn missing_buffer_uses_conventional_route_with_existing_capacity() {
    let mut runtime = runtime();
    for (address, size) in [(0, 0), (0, 65536), (0x20000, 0)] {
        reset();
        runtime.posix_direct_shm = address;
        runtime.posix_direct_shm_size = size;
        assert_eq!(runtime.posix_write_buffer(), (0x1000, 65536 - 512));
        assert_eq!(runtime.write_posix(3, 0, 27, None), Ok(27));
        assert_eq!(runtime.write_posix(3, 1, 28, Some(5)), Ok(28));
        BACKEND.with(|backend| {
            let backend = backend.borrow();
            assert_eq!(backend.calls[0].code, posix::POSIX_REQUEST_WRITE);
            assert_eq!(backend.calls[1].code, posix::POSIX_REQUEST_PWRITE);
        });
    }
}

#[test]
fn failed_direct_write_is_not_retried_or_replayed() {
    let runtime = runtime();
    for error in [
        RequestError::Transport,
        RequestError::Protocol,
        RequestError::Status(99),
    ] {
        reset();
        BACKEND.with(|backend| backend.borrow_mut().result = Some(Err(error)));
        assert_eq!(runtime.write_posix(3, 0, 32, None), Err(error));
        BACKEND.with(|backend| assert_eq!(backend.borrow().calls.len(), 1));
    }
}

#[test]
fn write_range_is_checked_before_ipc() {
    let runtime = runtime();
    for (offset, len) in [(65536, 1), (usize::MAX, 2)] {
        assert_eq!(
            runtime.write_posix(3, offset, len, None),
            Err(RequestError::InvalidArgument)
        );
    }
    BACKEND.with(|backend| assert!(backend.borrow().calls.is_empty()));
}
