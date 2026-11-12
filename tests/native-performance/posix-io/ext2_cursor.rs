use super::*;

fn fixture() -> (Ext2Runtime, Vec<u8>, Vec<u8>, Word) {
    let mut control = vec![0xa5; 128];
    let mut direct = control.clone();
    let mut r = runtime(&[]);
    r.delegated_sessions[0].shm_local = direct.as_mut_ptr() as Word;
    r.delegated_sessions[0].shm_size = direct.len();
    r.session = Some(ClientSession { active: true, pid: 10,
        shm_local: control.as_mut_ptr() as Word, shm_size: control.len() });
    r.snapshot_bytes = Some((0..40).collect());
    r.handles[0].size = 40;
    r.handles[0].cursor = Some(0);
    let token = read_grants::issue(&mut r, 0, 10, 1);
    assert_ne!(token, 0);
    (r, control, direct, token)
}

fn peer(token: Word, len: Word, output: Word) -> ServiceRequest {
    ServiceRequest { identifier: 16, arg0: token, arg2: len, arg3: output, ..ServiceRequest::default() }
}
fn owner(len: Word, output: Word) -> ServiceRequest {
    ServiceRequest { identifier: 10, arg0: 0, arg2: len, arg3: output, ..ServiceRequest::default() }
}
fn seek(token: Word, offset: Word, whence: Word) -> ServiceRequest {
    ServiceRequest { identifier: 16, arg0: token, arg1: offset, arg2: whence, ..ServiceRequest::default() }
}

#[test]
fn owner_control_peer_and_owner_delegated_reads_share_one_position_and_exact_buffers() {
    let (mut r, control, direct, token) = fixture();
    assert_eq!(file_cursor::read_owned(owner(3, 4), &mut r), (OS_RESPONSE_OK, 3, 3));
    assert_eq!(&control[4..7], &[0, 1, 2]);
    assert!(direct.iter().all(|&b| b == 0xa5));
    assert_eq!(file_cursor::read_granted(peer(token, 5, 6), &mut r), (OS_RESPONSE_OK, 5, 8));
    assert_eq!(&direct[6..11], &[3, 4, 5, 6, 7]);
    let destination = (1 << vfs::VFS_DELEGATE_ID_SHIFT) | 80;
    assert_eq!(file_cursor::read_owned(owner(2, destination), &mut r), (OS_RESPONSE_OK, 2, 10));
    assert_eq!(&direct[80..82], &[8, 9]);
    assert_eq!(&control[..4], &[0xa5; 4]);
    assert!(control[7..].iter().all(|&b| b == 0xa5));
    assert_eq!(r.handles[0].cursor, Some(10));
    assert_eq!(r.snapshot_reads, 3);
}

#[test]
fn independent_opens_of_the_same_inode_never_share_their_cursors_or_tokens() {
    let (mut r, _control, direct, first) = fixture();
    r.handles[1] = FileHandle { read_grant: read_grants::Grant::EMPTY, ..r.handles[0] };
    let second = read_grants::issue(&mut r, 1, 10, 1);
    assert_ne!(first, second);
    assert_eq!(file_cursor::read_granted(peer(first, 3, 0), &mut r).1, 3);
    assert_eq!(file_cursor::read_granted(peer(second, 5, 8), &mut r).1, 5);
    assert_eq!(&direct[8..13], &[0, 1, 2, 3, 4]);
    assert_eq!((r.handles[0].cursor, r.handles[1].cursor), (Some(3), Some(5)));
    assert_eq!(file_cursor::seek_granted(seek(first, 19, 0), &mut r).1, 19);
    assert_eq!(file_cursor::read_granted(peer(second, 1, 0), &mut r).1, 1);
    assert_eq!(direct[0], 5);
    assert_eq!((r.handles[0].cursor, r.handles[1].cursor), (Some(19), Some(6)));
}

#[test]
fn owner_and_peer_seek_share_the_cursor_while_positioned_and_versioned_reads_do_not_mutate_it() {
    let (mut r, _control, direct, token) = fixture();
    assert_eq!(file_cursor::seek_granted(seek(token, 11, vfs::VFS_CURSOR_SEEK_SET), &mut r), (OS_RESPONSE_OK, 11, 0));
    let request = ServiceRequest { identifier: 10, arg0: 0, arg1: 2, arg2: vfs::VFS_CURSOR_SEEK_CUR, ..ServiceRequest::default() };
    assert_eq!(file_cursor::seek_owned(request, &mut r), (OS_RESPONSE_OK, 13, 0));
    assert_eq!(file_cursor::read_granted(peer(token, 2, 0), &mut r), (OS_RESPONSE_OK, 2, 15));
    assert_eq!(&direct[..2], &[13, 14]);
    let mut positioned = peer(token, 3, 5); positioned.arg1 = 1;
    assert_eq!(file_io::handle_read_granted(positioned, &mut r), (OS_RESPONSE_OK, 3, 0));
    assert_eq!(&direct[5..8], &[1, 2, 3]);
    let file = r.handles[0];
    let identity = file_io::versioned_file_identity(&mut r, file);
    let mut versioned = peer(token, 40, 64); versioned.arg1 = identity;
    assert_eq!(file_io::handle_read_versioned_granted(versioned, &mut r), (OS_RESPONSE_OK, 40, identity));
    assert_eq!(&direct[64..104], &(0..40).collect::<Vec<_>>());
    assert_eq!(r.handles[0].cursor, Some(15));
    // SEEK_END observes the same current handle size as FSTAT.
    r.handles[0].size = 47;
    assert_eq!(file_cursor::seek_granted(seek(token, 0, vfs::VFS_CURSOR_SEEK_END), &mut r), (OS_RESPONSE_OK, 47, 0));
}

#[test]
fn cursor_mutation_requires_the_live_owner_or_exact_peer_delegate_and_serial() {
    let (mut r, _control, direct, token) = fixture();
    for caller in [0, 10, 17] {
        let mut request = peer(token, 3, 0); request.identifier = caller;
        assert_eq!(file_cursor::read_granted(request, &mut r).0, OS_RESPONSE_INVALID_DESCRIPTOR);
        request.arg2 = vfs::VFS_CURSOR_SEEK_SET; request.arg1 = 9;
        assert_eq!(file_cursor::seek_granted(request, &mut r).0, OS_RESPONSE_INVALID_DESCRIPTOR);
    }
    for bad in [0, token ^ 64, Word::MAX] {
        assert_eq!(file_cursor::read_granted(peer(bad, 3, 0), &mut r).0, OS_RESPONSE_INVALID_DESCRIPTOR);
    }
    let mut wrong_owner = owner(3, 0); wrong_owner.identifier = 16;
    assert_eq!(file_cursor::read_owned(wrong_owner, &mut r).0, OS_RESPONSE_INVALID_DESCRIPTOR);
    r.delegated_sessions[0].active = false;
    assert_eq!(file_cursor::read_granted(peer(token, 3, 0), &mut r).0, OS_RESPONSE_INVALID_DESCRIPTOR);
    assert_eq!(file_cursor::seek_granted(seek(token, 9, 0), &mut r).0, OS_RESPONSE_INVALID_DESCRIPTOR);
    assert_eq!(r.handles[0].cursor, Some(0));
    assert_eq!(r.snapshot_reads, 0);
    assert!(direct.iter().all(|&b| b == 0xa5));
}

#[test]
fn a_positioned_only_grant_never_acquires_cursor_authority_from_the_new_opcode() {
    let (mut r, _control, direct, token) = fixture();
    r.handles[0].cursor = None;
    assert_eq!(file_cursor::read_granted(peer(token, 3, 0), &mut r).0, OS_RESPONSE_ILLEGAL_OPERATION);
    assert_eq!(file_cursor::seek_granted(seek(token, 9, 0), &mut r).0, OS_RESPONSE_ILLEGAL_OPERATION);
    assert_eq!(file_cursor::read_owned(owner(3, 0), &mut r).0, OS_RESPONSE_ILLEGAL_OPERATION);
    let mut positioned = peer(token, 3, 0); positioned.arg1 = 4;
    assert_eq!(file_io::handle_read_granted(positioned, &mut r), (OS_RESPONSE_OK, 3, 0));
    assert_eq!(&direct[..3], &[4, 5, 6]);
    assert_eq!(r.handles[0].cursor, None);
}

#[test]
fn invalid_ranges_reserved_offsets_and_types_are_rejected_before_read_or_cursor_commit() {
    let (mut r, _control, direct, token) = fixture();
    for (bytes, output) in [(1, 128), (2, Word::MAX), (0, Word::MAX)] {
        assert_eq!(file_cursor::read_granted(peer(token, bytes, output), &mut r).0, OS_RESPONSE_INVALID_ARGUMENT);
    }
    let mut request = peer(token, 3, 0); request.arg1 = 1;
    assert_eq!(file_cursor::read_granted(request, &mut r).0, OS_RESPONSE_INVALID_ARGUMENT);
    r.handles[0].mode = EXT2_S_IFDIR;
    assert_eq!(file_cursor::read_granted(peer(token, 3, 0), &mut r).0, OS_RESPONSE_ILLEGAL_OPERATION);
    assert_eq!(file_cursor::seek_granted(seek(token, 9, 0), &mut r).0, OS_RESPONSE_ILLEGAL_OPERATION);
    assert_eq!(r.handles[0].cursor, Some(0));
    assert_eq!((r.inode_reads, r.snapshot_reads), (0, 0));
    assert!(direct.iter().all(|&b| b == 0xa5));
}

#[test]
fn storage_failure_does_not_advance_and_a_retry_starts_at_the_same_provider_position() {
    let (mut r, _control, direct, token) = fixture();
    r.error = Some(OS_RESPONSE_FATAL);
    assert_eq!(file_cursor::read_granted(peer(token, 3, 0), &mut r).0, OS_RESPONSE_FATAL);
    assert_eq!(r.handles[0].cursor, Some(0));
    assert!(direct.iter().all(|&b| b == 0xa5));
    r.error = None;
    assert_eq!(file_cursor::read_granted(peer(token, 3, 0), &mut r), (OS_RESPONSE_OK, 3, 3));
    assert_eq!(&direct[..3], &[0, 1, 2]);
}

#[test]
fn cursor_saturates_like_posix_seek_and_eof_never_wraps_or_reads_storage() {
    let (mut r, _control, _direct, token) = fixture();
    assert_eq!(file_cursor::seek_granted(seek(token, Word::MAX, 0), &mut r), (OS_RESPONSE_OK, Word::MAX, 0));
    assert_eq!(file_cursor::seek_granted(seek(token, 1, 1), &mut r), (OS_RESPONSE_OK, Word::MAX, 0));
    assert_eq!(file_cursor::read_granted(peer(token, 3, 0), &mut r), (OS_RESPONSE_OK, 0, Word::MAX));
    assert_eq!(file_cursor::read_granted(peer(token, 0, Word::MAX), &mut r).0, OS_RESPONSE_INVALID_ARGUMENT);
    assert_eq!(file_cursor::seek_granted(seek(token, 9, 3), &mut r).0, OS_RESPONSE_INVALID_ARGUMENT);
    assert_eq!(r.handles[0].cursor, Some(Word::MAX));
    assert_eq!(r.snapshot_reads, 0);
}

#[test]
fn frozen_close_revokes_cursor_tokens_and_reopening_a_slot_uses_an_independent_position() {
    let (mut r, _control, direct, token) = fixture();
    assert_eq!(file_cursor::read_granted(peer(token, 3, 0), &mut r).1, 3);
    r.frozen_by = Some(10);
    assert_eq!(file_io::handle_close(owner(0, 0), &mut r).0, OS_RESPONSE_OK);
    assert!(r.handles[0].close_pending);
    assert_eq!(file_cursor::read_granted(peer(token, 3, 0), &mut r).0, OS_RESPONSE_INVALID_DESCRIPTOR);
    assert_eq!(file_cursor::seek_granted(seek(token, 0, 0), &mut r).0, OS_RESPONSE_INVALID_DESCRIPTOR);
    r.frozen_by = None;
    r.handles[0] = FileHandle { active: true, owner_pid: 10, inode: 7, size: 40,
        mode: EXT2_S_IFREG, cursor: Some(0), ..FileHandle::EMPTY };
    let fresh = read_grants::issue(&mut r, 0, 10, 1);
    assert_ne!(fresh, token);
    assert_eq!(file_cursor::read_granted(peer(token, 3, 0), &mut r).0, OS_RESPONSE_INVALID_DESCRIPTOR);
    assert_eq!(file_cursor::read_granted(peer(fresh, 3, 0), &mut r), (OS_RESPONSE_OK, 3, 3));
    assert_eq!(&direct[..3], &[0, 1, 2]);
}
