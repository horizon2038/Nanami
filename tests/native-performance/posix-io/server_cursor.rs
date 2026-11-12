//! Actual POSIX read/seek/dup/close/truncate with the provider cursor mocked.
use super::*;

fn setup() -> (Runtime, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut r = runtime();
    let mut control = vec![0xa5; 65536];
    let mut direct = control.clone(); let mut vfs_window = control.clone();
    let data: Vec<_> = (0..200).map(|i| i as u8).collect();
    r.vfs_shm = vfs_window.as_mut_ptr() as Word; r.vfs_shm_size = vfs_window.len();
    r.sessions[0].shm_local = control.as_mut_ptr() as Word; r.sessions[0].shm_size = control.len();
    r.open_files[0].vfs_cursor = true;
    BACKEND.with(|b| {
        let mut b = b.borrow_mut(); b.cursor_position = 7; b.vfs_control_target = r.vfs_shm;
        b.read_payload = Some(ReadPayload { bytes: data.clone(), control: control.as_mut_ptr() as Word, direct: direct.as_mut_ptr() as Word });
    });
    (r, control, direct, vfs_window, data)
}

#[test]
fn proxy_and_direct_reads_and_both_dup_fds_observe_one_provider_position() {
    let (mut r, control, direct, _vfs, data) = setup();
    assert_eq!(io::handle_read_direct(&mut r, request(POSIX_REQUEST_READ_DIRECT, 3, 2, 5)), (OS_RESPONSE_OK, 5, 0));
    assert_eq!(&direct[2..7], &data[7..12]);
    // Interleave a peer's direct request, which POSIX must never overwrite.
    assert_eq!(vfs::vfs_read_cursor_granted(22, 71, 3, 8), Ok(3));
    assert_eq!(io::handle_seek(&mut r, request(POSIX_REQUEST_SEEK, 4, 0, POSIX_SEEK_CUR)), (OS_RESPONSE_OK, 15, 0));
    assert_eq!(io::handle_seek(&mut r, request(POSIX_REQUEST_SEEK, 3, 21, POSIX_SEEK_SET)), (OS_RESPONSE_OK, 21, 0));
    assert_eq!(io::handle_read(&mut r, request(POSIX_REQUEST_READ, 4, 4, 3)), (OS_RESPONSE_OK, 3, 0));
    assert_eq!(&control[4..7], &data[21..24]);
    let mut positioned = request(POSIX_REQUEST_PREAD_DIRECT, 3, 16, 4); positioned.arg3 = 2;
    assert_eq!(io::handle_read_direct(&mut r, positioned), (OS_RESPONSE_OK, 4, 0));
    assert_eq!(&direct[16..20], &data[2..6]);
    assert_eq!(r.open_files[0].offset, 123); // Inert legacy field, never a mirror.
    BACKEND.with(|b| {
        let b = b.borrow(); assert_eq!(b.cursor_position, 24);
        assert_eq!(b.calls.iter().map(|c| c.code).collect::<Vec<_>>(), [
            vfs::VFS_REQUEST_READ_CURSOR, vfs::VFS_REQUEST_READ_CURSOR_GRANTED,
            vfs::VFS_REQUEST_SEEK_CURSOR, vfs::VFS_REQUEST_SEEK_CURSOR,
            vfs::VFS_REQUEST_READ_CURSOR, vfs::VFS_REQUEST_READ_DELEGATED]);
    });
}

#[test]
fn cloned_sessions_and_dup_descriptors_share_the_cursor_without_sharing_output_buffers() {
    let (mut r, _control, direct, _vfs, data) = setup();
    let mut other = vec![0xa5; 65536];
    r.sessions[1] = Session { active: true, owner_pid: 17,
        shm_local: other.as_mut_ptr() as Word, shm_size: other.len(), ..Session::EMPTY };
    fd::inherit_fds(&mut r, 0, 1);
    assert_eq!(io::handle_read_direct(&mut r, request(POSIX_REQUEST_READ_DIRECT, 3, 0, 5)).1, 5);
    let mut read = request(POSIX_REQUEST_READ, 4, 1, 3); read.identifier = 17;
    assert_eq!(io::handle_read(&mut r, read), (OS_RESPONSE_OK, 3, 0));
    assert_eq!(&other[1..4], &data[12..15]);
    assert_eq!(&direct[..5], &data[7..12]);
    let mut query = request(POSIX_REQUEST_SEEK, 3, 0, POSIX_SEEK_CUR); query.identifier = 17;
    assert_eq!(io::handle_seek(&mut r, query), (OS_RESPONSE_OK, 15, 0));
    assert_eq!(fd::handle_close(&mut r, request(POSIX_REQUEST_CLOSE, 3, 0, 0)).0, OS_RESPONSE_OK);
    assert_eq!(r.open_files[0].ref_count, 3);
    assert_eq!(io::handle_read(&mut r, read).1, 3);
    assert_eq!(&other[1..4], &data[15..18]);
    assert!(other[..1].iter().chain(other[4..].iter()).all(|&b| b == 0xa5));
    assert_eq!(r.open_files[0].offset, 123);
}

#[test]
fn only_the_negotiated_readonly_description_rejects_content_writes_and_truncation() {
    let (mut r, _control, _direct, _vfs, _) = setup();
    for code in [POSIX_REQUEST_WRITE_DIRECT, POSIX_REQUEST_PWRITE_DIRECT] {
        assert_eq!(io::handle_write_direct(&mut r, request(code, 3, 0, 4)).0, OS_RESPONSE_ILLEGAL_OPERATION);
    }
    assert_eq!(truncate::handle_ftruncate(&mut r, request(POSIX_REQUEST_FTRUNCATE, 3, 9, 0)).0, OS_RESPONSE_ILLEGAL_OPERATION);
    BACKEND.with(|b| assert!(b.borrow().calls.is_empty()));
    // Existing nonnegotiated native descriptions keep their old behavior.
    r.open_files[0].vfs_cursor = false;
    assert_eq!(io::handle_write_direct(&mut r, request(POSIX_REQUEST_WRITE_DIRECT, 3, 0, 4)), (OS_RESPONSE_OK, 4, 0));
    assert_eq!(r.open_files[0].offset, 127);
}

#[test]
fn uncertain_provider_replies_do_not_replay_or_replace_the_provider_position() {
    let (mut r, _control, _direct, _vfs, _) = setup();
    BACKEND.with(|b| b.borrow_mut().cursor_committed_error = Some(RequestError::Transport));
    assert_eq!(io::handle_read_direct(&mut r, request(POSIX_REQUEST_READ_DIRECT, 3, 0, 4)), (OS_RESPONSE_FATAL, 0, 0));
    BACKEND.with(|b| { let b = b.borrow(); assert_eq!((b.cursor_position,b.calls.len()),(11,1)); });
    assert_eq!(r.open_files[0].offset, 123);
    BACKEND.with(|b| b.borrow_mut().cursor_committed_error = None);
    assert_eq!(io::handle_seek(&mut r, request(POSIX_REQUEST_SEEK, 4, 0, POSIX_SEEK_CUR)), (OS_RESPONSE_OK,11,0));
    assert_eq!(r.open_files[0].offset, 123);
}

#[test]
fn cursor_flag_fits_the_existing_open_description_layout() {
    assert_eq!(std::mem::size_of::<OpenFile>(), 7 * std::mem::size_of::<Word>());
}
