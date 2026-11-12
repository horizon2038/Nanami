//! Production descriptor tagging, synchronous reader, SDK and I/O selection.
use super::*;
const EIO: i32 = 5;
const EFAULT: i32 = 14;
#[path = "../../../nanami/servers/apps/alter/shared/src/personality/linux/file_read.rs"]
mod read_flow;

fn setup() -> (Runtime, Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut r = runtime();
    let mut control = vec![0xa5; 65536];
    let mut direct = control.clone();
    let data: Vec<_> = (0..200).map(|i| i as u8).collect();
    r.posix_shm = control.as_mut_ptr() as Word;
    r.posix_direct_shm = direct.as_mut_ptr() as Word;
    r.file = Some(LinuxFile::readonly(3, 71).with_posix_read_cursor(true));
    BACKEND.with(|b| {
        let mut b = b.borrow_mut(); b.cursor_position = 7;
        b.read_payload = Some(ReadPayload { bytes: data.clone(), control: r.posix_shm, direct: r.posix_direct_shm });
    });
    (r, control, direct, data)
}

#[test]
fn ordinary_read_and_seek_use_the_provider_cursor_while_pread_keeps_its_explicit_offset() {
    let (mut r, _control, direct, data) = setup();
    let file = r.file.unwrap();
    r.with_file_reader_operation(file, 1, "read", |r| {
        assert_eq!(r.read_posix(3, 2, 5), Ok((5, r.posix_direct_shm + 2)));
        assert_eq!(&direct[2..7], &data[7..12]);
        assert_eq!(r.seek_posix(3, 0, vfs::VFS_CURSOR_SEEK_CUR), Ok(12));
        assert_eq!(r.seek_posix(3, 21, vfs::VFS_CURSOR_SEEK_SET), Ok(21));
        assert_eq!(r.read_posix(3, 0, 3), Ok((3, r.posix_direct_shm)));
        assert_eq!(&direct[..3], &data[21..24]);
        assert_eq!(r.pread_posix(3, 0, 4, 2), Ok((4, r.posix_direct_shm)));
        assert_eq!(&direct[..4], &data[2..6]);
    });
    BACKEND.with(|b| {
        let b = b.borrow(); assert_eq!(b.cursor_position, 24);
        assert_eq!(b.calls.iter().map(|c| c.code).collect::<Vec<_>>(), [
            vfs::VFS_REQUEST_READ_CURSOR_GRANTED, vfs::VFS_REQUEST_SEEK_CURSOR_GRANTED,
            vfs::VFS_REQUEST_SEEK_CURSOR_GRANTED, vfs::VFS_REQUEST_READ_CURSOR_GRANTED, vfs::VFS_REQUEST_READ_GRANTED]);
        assert!(b.calls.iter().all(|c| c.handle == 71));
    });
    assert_eq!(r.rooted_channel.connections, 1);
    assert!(!r.file_reader.cursor_for(3));
}

#[test]
fn lost_or_invalid_success_replies_are_never_replayed_after_a_committed_read_or_seek() {
    for error in [RequestError::Transport, RequestError::Protocol, RequestError::Status(OS_RESPONSE_FATAL)] {
        let (mut r, _control, _direct, _) = setup(); let file = r.file.unwrap();
        BACKEND.with(|b| b.borrow_mut().cursor_committed_error = Some(error));
        assert_eq!(r.with_cursor_file_reader(file, 1, "read", |r| r.read_posix(3, 0, 4)), Err(error));
        BACKEND.with(|b| {
            let b = b.borrow(); assert_eq!(b.cursor_position, 11); assert_eq!(b.calls.len(), 1);
            assert_eq!(b.calls[0].code, vfs::VFS_REQUEST_READ_CURSOR_GRANTED);
        });
        let (mut r, _control, _direct, _) = setup(); let file = r.file.unwrap();
        BACKEND.with(|b| b.borrow_mut().cursor_committed_error = Some(error));
        assert_eq!(r.with_cursor_file_reader(file, 1, "seek", |r| r.seek_posix(3, 19, 0)), Err(error));
        BACKEND.with(|b| { let b = b.borrow(); assert_eq!(b.cursor_position, 19); assert_eq!(b.calls.len(), 1); });
    }
}

#[test]
fn a_missing_direct_port_still_prevents_replay_of_an_uncertain_proxy_read() {
    for buffer in [true, false] {
        let (mut r, _control, _direct, _) = setup(); let file = r.file.unwrap();
        r.rooted_channel.port = None;
        if !buffer { r.posix_direct_shm = 0; }
        BACKEND.with(|b| b.borrow_mut().cursor_committed_error = Some(RequestError::Transport));
        assert_eq!(r.with_cursor_file_reader(file, 1, "read", |r| {
            assert!(r.file_reader.cursor_for(3));
            r.read_posix(3, 0, 4)
        }), Err(RequestError::Transport));
        BACKEND.with(|b| {
            let b = b.borrow(); assert_eq!((b.cursor_position, b.calls.len()), (11, 1));
            assert_eq!(b.calls[0].code, if buffer { posix::POSIX_REQUEST_READ_DIRECT } else { posix::POSIX_REQUEST_READ });
        });
    }
}

#[test]
fn definite_rejection_falls_back_to_the_same_authoritative_cursor_only_once() {
    for error in [RequestError::Unsupported, RequestError::Status(OS_RESPONSE_INVALID_DESCRIPTOR),
        RequestError::Status(OS_RESPONSE_INVALID_ARGUMENT), RequestError::Status(OS_RESPONSE_ILLEGAL_OPERATION)] {
        let (mut r, _control, direct, data) = setup(); let file = r.file.unwrap();
        BACKEND.with(|b| b.borrow_mut().granted_result = Some(Err(error)));
        assert_eq!(r.with_cursor_file_reader(file, 1, "read", |r| r.read_posix(3, 0, 4)), Ok((4, r.posix_direct_shm)));
        assert_eq!(&direct[..4], &data[7..11]);
        BACKEND.with(|b| {
            let b = b.borrow(); assert_eq!(b.cursor_position, 11);
            assert_eq!(b.calls.iter().map(|c| c.code).collect::<Vec<_>>(),
                [vfs::VFS_REQUEST_READ_CURSOR_GRANTED, posix::POSIX_REQUEST_READ_DIRECT]);
        });
    }
}

#[test]
fn a_definitely_rejected_proxy_buffer_can_fall_back_but_a_fatal_proxy_reply_cannot() {
    for error in [RequestError::Status(OS_RESPONSE_INVALID_ARGUMENT), RequestError::Status(OS_RESPONSE_FATAL)] {
        let (mut r, control, _direct, data) = setup(); let file = r.file.unwrap();
        BACKEND.with(|b| {
            let mut b = b.borrow_mut(); b.granted_result = Some(Err(RequestError::Unsupported)); b.read_direct_result = Some(Err(error));
        });
        let result = r.with_cursor_file_reader(file, 1, "read", |r| r.read_posix(3, 0, 4));
        if error == RequestError::Status(OS_RESPONSE_FATAL) {
            assert_eq!(result, Err(error));
            BACKEND.with(|b| { let b = b.borrow(); assert_eq!((b.calls.len(), b.cursor_position), (2, 7)); });
        } else {
            assert_eq!(result, Ok((4, r.posix_shm)));
            assert_eq!(&control[..4], &data[7..11]);
            BACKEND.with(|b| { let b = b.borrow(); assert_eq!((b.calls.len(), b.cursor_position), (3, 11)); });
        }
    }
}

#[test]
fn malformed_success_count_does_not_copy_or_replay_an_uncertain_cursor_operation() {
    let (mut r, _control, direct, _) = setup(); let file = r.file.unwrap();
    BACKEND.with(|b| b.borrow_mut().read_results.push_back(Ok(9)));
    assert_eq!(r.with_cursor_file_reader(file, 1, "read", |r| r.read_posix(3, 0, 4)), Err(RequestError::Protocol));
    assert!(direct.iter().all(|&b| b == 0xa5));
    BACKEND.with(|b| { let b = b.borrow(); assert_eq!((b.cursor_position, b.calls.len()), (16, 1)); });
}

#[test]
fn positioned_only_descriptors_do_not_connect_or_inherit_an_outer_cursor_grant() {
    let (mut r, _control, _direct, _) = setup();
    let file = LinuxFile::readonly(3, 71);
    r.file_reader = file_reader::Reader::new(3, 99, 22).with_cursor(true);
    assert_eq!(r.with_cursor_file_reader(file, 1, "read", |r| r.read_posix(3, 0, 4)), Ok((4, r.posix_direct_shm)));
    assert_eq!(r.rooted_channel.connections, 0);
    assert!(r.file_reader.cursor_for(3));
    BACKEND.with(|b| { let b = b.borrow(); assert_eq!(b.calls.len(), 1); assert_eq!(b.calls[0].code, posix::POSIX_REQUEST_READ_DIRECT); });
}

#[test]
fn descriptor_cursor_tag_never_changes_the_wire_token_flags_or_real_offset_fields() {
    let token = Word::MAX >> 8;
    let mut file = LinuxFile::readonly(3, token);
    file.flags = 1; file.resource = 0x1234; file.offset = 0x5678;
    let tagged = file.with_posix_read_cursor(true);
    assert_eq!(tagged.posix_read_grant(), token);
    assert!(tagged.posix_read_cursor());
    assert_eq!((tagged.flags, tagged.resource, tagged.offset), (1, 0x1234, 0x5678));
    assert_eq!(tagged.with_posix_read_cursor(false).posix_read_grant(), token);
    let invalid = tagged.with_posix_read_grant(token + 1).with_posix_read_cursor(true);
    assert_eq!(invalid.posix_read_grant(), 0); assert!(!invalid.posix_read_cursor());
    let mut another_kind = tagged; another_kind.kind = LinuxFileKind::MemoryFile;
    assert_eq!(another_kind.posix_read_grant(), 0); assert!(!another_kind.posix_read_cursor());
    let mut r = runtime();
    r.with_file_reader(tagged, |r| { r.read_posix(3, 0, 1).unwrap(); });
    BACKEND.with(|b| assert_eq!(b.borrow().calls[0].handle, token));
    assert_eq!(std::mem::size_of::<file_reader::Reader>(), 4 * std::mem::size_of::<Word>());
}

#[test]
fn a_guest_copyout_fault_does_not_roll_back_or_replay_an_already_consumed_read() {
    let (mut r, _control, _direct, _) = setup(); let file = r.file.unwrap();
    let result = r.with_cursor_file_reader(file, 1, "read", |r| {
        read_flow::read(4, r.posix_read_buffer_size(),
            |bytes| r.read_posix(3, 0, bytes).map_err(|_| EIO), |_, _, _| Err(EFAULT))
    });
    assert_eq!(result, Err(EFAULT));
    BACKEND.with(|b| { let b = b.borrow(); assert_eq!((b.cursor_position, b.calls.len()), (11, 1)); });
}
