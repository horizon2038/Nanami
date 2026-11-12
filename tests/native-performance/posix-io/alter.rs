use super::*;

struct Runtime {
    file: Option<LinuxFile>,
    file_reader: file_reader::Reader,
    rooted_channel: Channel,
    posix_port: Word,
    posix_shm: Word,
    posix_shm_size: Word,
    posix_direct_shm: Word,
    posix_direct_shm_size: Word,
}

#[path = "../../../nanami/servers/apps/alter/shared/src/common/state/io.rs"]
mod io;
#[path = "../../../nanami/servers/apps/alter/shared/src/common/file_reader.rs"]
mod file_reader;

#[derive(Clone, Copy, PartialEq)]
enum LinuxFileKind { Posix, VirtualDirectory, MemoryFile }
#[derive(Clone, Copy)]
struct LinuxFile {
    kind: LinuxFileKind,
    posix_fd: Word,
    flags: Word,
    local_port: u16,
    peer_port: u16,
    peer_ip: u32,
    offset: Word,
    resource: Word,
}
impl LinuxFile {
    fn readonly(posix_fd: Word, grant: Word) -> Self {
        Self { posix_fd, kind: LinuxFileKind::Posix, resource: 0, flags: 0,
            local_port: 0, peer_port: 0, peer_ip: 0, offset: 0 }.with_posix_read_grant(grant)
    }
}
#[path = "../../../nanami/servers/apps/alter/shared/src/common/state/read_grant.rs"]
mod read_grant;
struct Channel {
    port: Option<Word>,
    connections: usize,
}
impl Channel {
    fn port(&mut self, _: Word) -> Option<Word> {
        self.connections += 1;
        self.port
    }
}
mod state {
    use super::*;
    mod file_reader {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../nanami/servers/apps/alter/shared/src/common/state/file_reader.rs"));
    }
}

fn runtime() -> Runtime {
    reset();
    Runtime {
        file: None,
        file_reader: file_reader::Reader::EMPTY,
        rooted_channel: Channel { port: Some(22), connections: 0 },
        posix_port: 7,
        posix_shm: 0x1000,
        posix_shm_size: 65536,
        posix_direct_shm: 0x20000,
        posix_direct_shm_size: 65536,
    }
}

#[test]
fn synchronous_reader_scopes_restore_the_outer_file_after_nested_errors() {
    let mut r = runtime();
    r.file_reader = file_reader::Reader::new(5, 88, 22);
    BACKEND.with(|b| b.borrow_mut().stat_reply = Some((42,
        13 | (vfs::VFS_FILE_TYPE_REGULAR << vfs::VFS_STAT_TYPE_SHIFT))));
    let result = r.with_file_reader(LinuxFile::readonly(3, 71), |r| {
        r.file_reader.fstat(7, 3, 0).unwrap();
        let inner = r.with_file_reader(LinuxFile::readonly(4, 72), |r| {
            r.file_reader.fstat(7, 4, 0).unwrap();
            Err::<(), _>(5)
        });
        assert_eq!(inner, Err(5));
        r.file_reader.fstat(7, 3, 0).unwrap();
        Err::<(), _>(9)
    });
    assert_eq!(result, Err(9));
    r.file_reader.fstat(7, 5, 0).unwrap();
    BACKEND.with(|b| assert_eq!(b.borrow().calls.iter().map(|c| c.handle).collect::<Vec<_>>(),
        [71, 72, 71, 88]));
}

#[test]
fn a_reader_needs_both_a_live_file_grant_and_a_delegated_buffer_and_channel() {
    let mut r = runtime();
    for (grant, buffer) in [(0, 0x20000), (71, 0)] {
        r.posix_direct_shm = buffer;
        r.with_file_reader(LinuxFile::readonly(3, grant), |r| {
            r.file_reader.fstat(7, 3, 0).unwrap();
        });
    }
    assert_eq!(r.rooted_channel.connections, 0);
    r.posix_direct_shm = 0x20000;
    r.rooted_channel.port = None;
    r.with_file_reader(LinuxFile::readonly(3, 71), |r| {
        r.file_reader.fstat(7, 3, 0).unwrap();
    });
    assert_eq!(r.rooted_channel.connections, 1);
    BACKEND.with(|b| assert!(b.borrow().calls.iter().all(|c| c.code == posix::POSIX_REQUEST_FSTAT)));
}

#[path = "positioned.rs"]
mod positioned;
#[path = "cursor_client.rs"]
mod cursor_client;

#[test]
fn file_reader_uses_only_its_bound_fd_and_preserves_positioned_wire_arguments() {
    let mut r = runtime(); r.file_reader = file_reader::Reader::new(3, 71, 22);
    assert_eq!(r.pread_posix(3, 9, 13, 123), Ok((13, 0x20009)));
    BACKEND.with(|b| assert_eq!(b.borrow().calls[0], Call { code: vfs::VFS_REQUEST_READ_GRANTED,
        handle: 71, offset: 123, len: 13, delegate: 0, buffer: 9 }));
    assert_eq!(r.pread_posix(4, 9, 13, 123), Ok((13, 0x20009)));
    BACKEND.with(|b| assert_eq!(b.borrow().calls[1].code, posix::POSIX_REQUEST_PREAD_DIRECT));
}

#[test]
fn file_reader_fstat_is_fresh_and_bad_grant_or_type_keeps_the_posix_route() {
    let reader = file_reader::Reader::new(3, 71, 22); reset();
    for size in [13, 99] {
        BACKEND.with(|b| b.borrow_mut().stat_reply = Some((42, size | (vfs::VFS_FILE_TYPE_REGULAR << vfs::VFS_STAT_TYPE_SHIFT))));
        assert_eq!(reader.fstat(7, 3, 2), Ok((42, size, posix::POSIX_FILE_TYPE_REGULAR, 0, 0)));
    }
    BACKEND.with(|b| b.borrow_mut().stat_reply = Some((42, 0)));
    assert_eq!(reader.fstat(7, 3, 2), Ok((91, 13, posix::POSIX_FILE_TYPE_REGULAR, 0, 0)));
    BACKEND.with(|b| assert_eq!(b.borrow().calls.last().unwrap().code, posix::POSIX_REQUEST_FSTAT));
}

#[test]
fn expired_read_grants_retry_safely_and_versioned_replies_still_require_exact_identity() {
    let identity = (12usize << 32) | 42;
    let mut r = runtime(); r.file_reader = file_reader::Reader::new(3, 71, 22);
    BACKEND.with(|b| { let mut b=b.borrow_mut(); b.granted_result=Some(Err(RequestError::Status(OS_RESPONSE_INVALID_DESCRIPTOR)));
        b.verified_reply=Some(Ok((4096, identity))); });
    assert_eq!(r.pread_versioned_image(3,4096,identity),Ok(Some(0x20000)));
    BACKEND.with(|b| assert_eq!(b.borrow().calls.iter().map(|c|c.code).collect::<Vec<_>>(),
        [vfs::VFS_REQUEST_READ_VERSIONED_GRANTED,posix::POSIX_REQUEST_PREAD_VERSIONED]));
    reset();
    BACKEND.with(|b| b.borrow_mut().verified_reply=Some(Ok((4095,identity))));
    assert_eq!(r.pread_versioned_image(3,4096,identity),Err(RequestError::Protocol));
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

#[test]
fn large_file_window_preserves_normal_io_quantum_and_uses_full_snapshot_capacity() {
    let mut r=runtime();r.posix_direct_shm_size=1024*1024;
    assert_eq!(r.posix_read_buffer_size(),65536);
    assert_eq!(r.posix_write_buffer(),(0x20000,65536));
    assert_eq!(r.posix_image_read_buffer_size(),1024*1024);
    assert_eq!(r.pread_posix(3,0,1024*1024,17),Ok((1024*1024,0x20000)));
    assert_eq!(r.write_posix(3,0,65537,None),Err(RequestError::InvalidArgument));
    r.posix_direct_shm=0;
    assert_eq!(r.posix_image_read_buffer_size(),65536);
    assert_eq!(r.posix_read_buffer_size(),65536);
}

#[test]
fn versioned_image_reads_require_a_full_reply_with_the_expected_identity() {
    let runtime = runtime();
    let identity = 4 << 32 | 7;
    for (reply, expected) in [
        (Ok((4096, identity)), Ok(Some(0x20000))),
        (Ok((4096, identity + 1)), Err(RequestError::Protocol)),
        (Ok((4095, identity)), Err(RequestError::Protocol)),
        (Ok((0, 0)), Err(RequestError::Protocol)),
        (Err(RequestError::Status(OS_RESPONSE_INVALID_ARGUMENT)), Ok(None)),
    ] {
        reset();
        BACKEND.with(|b| b.borrow_mut().verified_reply = Some(reply));
        assert_eq!(runtime.pread_versioned_image(3, 4096, identity), expected);
        BACKEND.with(|b| assert_eq!(b.borrow().calls.as_slice(), &[Call {
            code: posix::POSIX_REQUEST_PREAD_VERSIONED, handle: 3, offset: identity,
            len: 4096, delegate: 0, buffer: 0,
        }]));
    }
    reset();
    assert_eq!(runtime.pread_versioned_image(3, 65537, identity), Ok(None));
    assert_eq!(runtime.pread_versioned_image(3, 4096, 7), Ok(None));
    BACKEND.with(|b| assert!(b.borrow().calls.is_empty()));
}

#[test]
fn failed_large_positioned_read_falls_back_with_bounded_length_and_same_offset() {
    for error in [RequestError::Transport,RequestError::Protocol,RequestError::Status(99)] {
        for out in [0,512,65536,65537] {
            let mut r=runtime();r.posix_direct_shm_size=1024*1024;
            BACKEND.with(|b|b.borrow_mut().read_direct_result=Some(Err(error)));
            let result=r.pread_posix(3,out,1024*1024-out,123);
            if out>=65536 {
                assert_eq!(result,Err(RequestError::InvalidArgument));
                BACKEND.with(|b|assert_eq!(b.borrow().calls.len(),1));
            } else {
                assert_eq!(result,Ok((65536-out,0x1000+out)));
                BACKEND.with(|b| {let b=b.borrow();assert_eq!(b.calls.len(),2);
                    assert_eq!(b.calls[0].code,posix::POSIX_REQUEST_PREAD_DIRECT);
                    assert_eq!(b.calls[1].code,posix::POSIX_REQUEST_PREAD);
                    assert_eq!((b.calls[0].offset,b.calls[1].offset),(123,123));
                    assert_eq!((b.calls[1].buffer,b.calls[1].len),(out,65536-out));});
            }
        }
    }
}
