use super::*;
use ipc::ServiceRequest;
use posix::*;
#[path = "../../../nanami/servers/apps/posix-server/src/state.rs"]
mod state;
use process::find_session;
use state::*;
fn map_request_error_to_status(error: RequestError) -> Word {
    if let RequestError::Status(status) = error {
        status
    } else {
        OS_RESPONSE_FATAL
    }
}
#[path = "../../../nanami/servers/apps/posix-server/src/sync.rs"]
mod sync;

// Session lookup is outside the I/O handlers under test.
mod process {
    use super::*;
    pub(crate) fn find_session(runtime: &mut Runtime, owner: Word) -> Option<usize> {
        runtime
            .sessions
            .iter()
            .position(|s| s.active && s.owner_pid == owner)
    }
}
#[path = "../../../nanami/servers/apps/posix-server/src/io.rs"]
mod io;

fn runtime() -> Runtime {
    reset();
    let mut runtime = Runtime {
        vfs_port: 23,
        vfs_shm: 0,
        vfs_shm_size: 0,
        sessions: [Session::EMPTY; MAX_SESSIONS],
        open_files: [OpenFile::EMPTY; MAX_OPEN_FILES],
        next_posix_pid: 100,
        cached_owner_pid: 0,
        cached_session_index: INVALID_SESSION_INDEX,
    };
    let session = &mut runtime.sessions[0];
    session.active = true;
    session.owner_pid = 16;
    session.direct_vfs_delegate = 5;
    session.direct_io_size = 65536;
    session.fds[3] = FileDescriptor {
        active: true,
        open_file: 0,
        flags: 0,
    };
    session.fds[4] = session.fds[3]; // dup: both descriptors refer to the same offset.
    runtime.open_files[0] = OpenFile {
        active: true,
        kind: FdKind::Regular,
        offset: 123,
        status_flags: 0,
        vfs_handle: 9,
        ref_count: 2,
    };
    runtime
}

fn request(code: Word, fd: Word, buffer: Word, len: Word) -> ServiceRequest {
    ServiceRequest {
        identifier: 16,
        code,
        arg0: fd,
        arg1: buffer,
        arg2: len,
        arg3: 77,
    }
}

#[test]
fn fsync_forwards_shared_file_handles_and_rejects_invalid_fds() {
    let mut runtime = runtime();
    for fd in [3, 4] {
        assert_eq!(
            sync::handle_sync(&mut runtime, request(POSIX_REQUEST_FSYNC, fd, 0, 0)).0,
            OS_RESPONSE_OK
        );
    }
    assert_eq!(
        sync::handle_sync(&mut runtime, request(POSIX_REQUEST_FSYNC, usize::MAX, 0, 0)).0,
        OS_RESPONSE_INVALID_DESCRIPTOR
    );
    runtime.open_files[0].kind = FdKind::Directory;
    assert_eq!(
        sync::handle_sync(&mut runtime, request(POSIX_REQUEST_FSYNC, 3, 0, 0)).0,
        OS_RESPONSE_OK
    );
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        assert_eq!(backend.calls.len(), 3);
        assert!(backend
            .calls
            .iter()
            .all(|call| call.code == vfs::VFS_REQUEST_FSYNC && call.handle == 9));
    });
    BACKEND.with(|backend| backend.borrow_mut().result = Some(Err(RequestError::Transport)));
    assert_eq!(
        sync::handle_sync(&mut runtime, request(POSIX_REQUEST_SYNC, 0, 0, 0)).0,
        OS_RESPONSE_FATAL
    );
}

#[test]
fn sync_write_waits_for_flush_on_normal_and_positioned_direct_paths() {
    for code in [POSIX_REQUEST_WRITE_DIRECT, POSIX_REQUEST_PWRITE_DIRECT] {
        let mut runtime = runtime();
        runtime.open_files[0].status_flags = POSIX_O_SYNC;
        assert_eq!(
            io::handle_write_direct(&mut runtime, request(code, 3, 0, 100)).0,
            OS_RESPONSE_OK
        );
        BACKEND.with(|backend| {
            let backend = backend.borrow();
            assert_eq!(backend.calls.len(), 2);
            assert_eq!(backend.calls[0].code, vfs::VFS_REQUEST_WRITE_DELEGATED);
            assert_eq!(backend.calls[1].code, vfs::VFS_REQUEST_FSYNC);
        });
    }
}

#[test]
fn direct_write_uses_full_buffer_without_accessing_posix_scratch() {
    let mut runtime = runtime(); // Both intermediate pointers are deliberately null.
    assert_eq!(
        io::handle_write_direct(
            &mut runtime,
            request(POSIX_REQUEST_WRITE_DIRECT, 3, 0, 65536)
        ),
        (OS_RESPONSE_OK, 65536, 0)
    );
    assert_eq!(runtime.open_files[0].offset, 123 + 65536);
    BACKEND.with(|backend| {
        assert_eq!(
            backend.borrow().calls,
            [Call {
                code: vfs::VFS_REQUEST_WRITE_DELEGATED,
                handle: 9,
                offset: 123,
                len: 65536,
                delegate: 5,
                buffer: 0,
            }]
        )
    });
}

#[test]
fn shared_offsets_positioned_offsets_and_short_writes_are_preserved() {
    let mut runtime = runtime();
    BACKEND.with(|backend| backend.borrow_mut().result = Some(Ok(3)));
    assert_eq!(
        io::handle_write_direct(&mut runtime, request(POSIX_REQUEST_WRITE_DIRECT, 4, 11, 20)),
        (OS_RESPONSE_OK, 3, 0)
    );
    assert_eq!(runtime.open_files[0].offset, 126);
    io::handle_write_direct(&mut runtime, request(POSIX_REQUEST_PWRITE_DIRECT, 3, 2, 20));
    assert_eq!(runtime.open_files[0].offset, 126);
    io::handle_read_direct(&mut runtime, request(POSIX_REQUEST_READ_DIRECT, 3, 0, 20));
    assert_eq!(runtime.open_files[0].offset, 129);
    io::handle_read_direct(&mut runtime, request(POSIX_REQUEST_PREAD_DIRECT, 4, 0, 20));
    assert_eq!(runtime.open_files[0].offset, 129);
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        assert_eq!(
            backend.calls.iter().map(|c| c.offset).collect::<Vec<_>>(),
            [123, 77, 126, 77]
        );
        assert_eq!(backend.calls[0].buffer, 11);
    });
}

#[test]
fn append_uses_current_size_including_positioned_writes() {
    let mut runtime = runtime();
    runtime.open_files[0].status_flags = POSIX_O_APPEND;
    BACKEND.with(|backend| backend.borrow_mut().size = 1000);
    io::handle_write_direct(&mut runtime, request(POSIX_REQUEST_WRITE_DIRECT, 3, 0, 20));
    assert_eq!(runtime.open_files[0].offset, 1020);
    io::handle_write_direct(&mut runtime, request(POSIX_REQUEST_PWRITE_DIRECT, 4, 0, 20));
    assert_eq!(runtime.open_files[0].offset, 1020);
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        assert_eq!(backend.stats, 2);
        assert!(backend.calls.iter().all(|c| c.offset == 1000));
    });
}

#[test]
fn conventional_write_keeps_the_copy_path_and_capacity_limit() {
    let mut runtime = runtime();
    let input: Vec<u8> = (0..80).collect();
    let mut scratch = [0xcc; VFS_IO_OFFSET + 32];
    runtime.sessions[0].shm_local = input.as_ptr() as Word;
    runtime.sessions[0].shm_size = input.len();
    runtime.vfs_shm = scratch.as_mut_ptr() as Word;
    runtime.vfs_shm_size = scratch.len();
    assert_eq!(
        io::handle_write(&mut runtime, request(POSIX_REQUEST_WRITE, 3, 4, 40)),
        (OS_RESPONSE_OK, 32, 0)
    );
    assert_eq!(&scratch[VFS_IO_OFFSET..], &input[4..36]);
    assert!(scratch[..VFS_IO_OFFSET].iter().all(|b| *b == 0xcc));
    BACKEND.with(|backend| assert_eq!(backend.borrow().calls[0].code, vfs::VFS_REQUEST_WRITE));
}

#[test]
fn errors_do_not_advance_offset_or_retry_write() {
    let mut runtime = runtime();
    BACKEND.with(|backend| backend.borrow_mut().result = Some(Err(RequestError::Status(99))));
    assert_eq!(
        io::handle_write_direct(&mut runtime, request(POSIX_REQUEST_WRITE_DIRECT, 3, 0, 20)),
        (99, 0, 0)
    );
    assert_eq!(runtime.open_files[0].offset, 123);
    BACKEND.with(|backend| assert_eq!(backend.borrow().calls.len(), 1));
}

#[test]
fn sync_write_returns_flush_failure_without_replaying_the_accepted_write() {
    let mut runtime = runtime();
    runtime.open_files[0].status_flags = POSIX_O_SYNC;
    BACKEND.with(|backend| backend.borrow_mut().sync_error = Some(RequestError::Transport));
    assert_eq!(
        io::handle_write_direct(&mut runtime, request(POSIX_REQUEST_WRITE_DIRECT, 3, 0, 20)),
        (OS_RESPONSE_FATAL, 0, 0)
    );
    assert_eq!(runtime.open_files[0].offset, 123);
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        assert_eq!(backend.calls.len(), 2);
        assert_eq!(backend.calls[0].code, vfs::VFS_REQUEST_WRITE_DELEGATED);
        assert_eq!(backend.calls[1].code, vfs::VFS_REQUEST_FSYNC);
    });
}

#[test]
fn conventional_sync_write_also_waits_for_the_barrier() {
    let mut runtime = runtime();
    let input = [0x22; 20];
    let mut scratch = [0; VFS_IO_OFFSET + 32];
    runtime.sessions[0].shm_local = input.as_ptr() as Word;
    runtime.sessions[0].shm_size = input.len();
    runtime.vfs_shm = scratch.as_mut_ptr() as Word;
    runtime.vfs_shm_size = scratch.len();
    runtime.open_files[0].status_flags = POSIX_O_SYNC;
    assert_eq!(
        io::handle_write(&mut runtime, request(POSIX_REQUEST_WRITE, 3, 0, 20)).0,
        OS_RESPONSE_OK
    );
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        assert_eq!(backend.calls.len(), 2);
        assert_eq!(backend.calls[0].code, vfs::VFS_REQUEST_WRITE);
        assert_eq!(backend.calls[1].code, vfs::VFS_REQUEST_FSYNC);
    });
}

#[test]
fn bad_ranges_descriptors_and_missing_delegates_never_reach_vfs() {
    let mut runtime = runtime();
    for (buffer, len) in [(65536, 1), (usize::MAX, 2)] {
        assert_eq!(
            io::handle_write_direct(
                &mut runtime,
                request(POSIX_REQUEST_WRITE_DIRECT, 3, buffer, len)
            )
            .0,
            OS_RESPONSE_INVALID_ARGUMENT
        );
    }
    assert_eq!(
        io::handle_write_direct(
            &mut runtime,
            request(POSIX_REQUEST_WRITE_DIRECT, MAX_FDS, 0, 1)
        )
        .0,
        OS_RESPONSE_INVALID_DESCRIPTOR
    );
    assert_eq!(
        io::handle_write_direct(&mut runtime, request(POSIX_REQUEST_WRITE_DIRECT, 1, 0, 1)).0,
        OS_RESPONSE_ILLEGAL_OPERATION
    );
    // Validate stdout's conventional buffer before constructing a raw slice.
    assert_eq!(
        io::handle_write(&mut runtime, request(POSIX_REQUEST_WRITE, 1, usize::MAX, 2)).0,
        OS_RESPONSE_INVALID_ARGUMENT
    );
    runtime.sessions[0].direct_vfs_delegate = 0;
    assert_eq!(
        io::handle_write_direct(&mut runtime, request(POSIX_REQUEST_WRITE_DIRECT, 3, 0, 1)).0,
        OS_RESPONSE_ILLEGAL_OPERATION
    );
    BACKEND.with(|backend| assert!(backend.borrow().calls.is_empty()));
}
