//! Actual positioned syscall, request-local reader, I/O selector and clients.
use super::*;
#[path = "../../../nanami/servers/apps/alter/shared/src/personality/linux/constants.rs"]
mod constants;
use constants::*;

impl Runtime {
    fn linux_file(&self, pid: Word, fd: Word) -> Option<LinuxFile> {
        if pid == 1 && fd == 3 { self.file } else { None }
    }
}
mod memfd {
    use super::*;
    pub(super) fn io(_: &mut Runtime, _: Word, _: LinuxFile, _: Word, _: Word, _: Option<Word>, _: bool) -> Result<Word, i32> { Err(EOPNOTSUPP) }
}
fn map_request_error(_: RequestError) -> i32 { EIO }
fn copyout_from(_: &Runtime, _: Word, output: Word, source: Word, bytes: Word) -> Result<(), i32> {
    BACKEND.with(|b| {
        let mut b = b.borrow_mut();
        if b.copyout_fail_after == Some(b.copyouts) { return Err(EFAULT); }
        unsafe { std::ptr::copy_nonoverlapping(source as *const u8, output as *mut u8, bytes); }
        b.copyouts += 1;
        Ok(())
    })
}
mod guest_memory {
    use super::*;
    pub(super) fn copyin_posix_write(_: &mut Runtime, _: Word, source: Word, output: Word, bytes: Word) -> Result<(), i32> {
        unsafe { std::ptr::copy_nonoverlapping(source as *const u8, output as *mut u8, bytes); }
        Ok(())
    }
}
#[path = "../../../nanami/servers/apps/alter/shared/src/personality/linux/positioned_io.rs"]
mod production;

fn setup(bytes: usize) -> (Runtime, Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut r = runtime();
    let mut control = vec![0xa5; 65536];
    let mut direct = control.clone();
    r.posix_shm = control.as_mut_ptr() as Word;
    r.posix_direct_shm = direct.as_mut_ptr() as Word;
    r.file = Some(LinuxFile::readonly(9, 71));
    let data: Vec<_> = (0..bytes).map(|index| index.wrapping_mul(29) as u8).collect();
    BACKEND.with(|b| b.borrow_mut().read_payload = Some(ReadPayload {
        bytes: data.clone(), control: r.posix_shm, direct: r.posix_direct_shm,
    }));
    (r, control, direct, data)
}

#[test]
fn positioned_syscall_uses_one_granted_rpc_per_chunk_and_leaves_sequential_cursor_untouched() {
    let (mut r, _control, _direct, data) = setup(150000);
    let mut guest = vec![0xa5; 70002];
    r.file_reader = file_reader::Reader::new(88, 99, 22);
    assert_eq!(production::sys_positioned_io(&mut r, 1, 3, guest.as_mut_ptr() as Word + 1, 70000, 123, false), Ok(70000));
    assert_eq!(&guest[1..70001], &data[123..70123]);
    assert_eq!((guest[0], guest[70001]), (0xa5, 0xa5));
    assert_eq!(r.rooted_channel.connections, 1);
    BACKEND.with(|b| {
        let b = b.borrow();
        assert_eq!(b.calls.len(), 2);
        assert!(b.calls.iter().all(|c| c.code == vfs::VFS_REQUEST_READ_GRANTED && c.handle == 71));
        assert_eq!((b.calls[0].offset, b.calls[0].len), (123, 65536));
        assert_eq!((b.calls[1].offset, b.calls[1].len), (65659, 4464));
    });
    assert_eq!(r.file_reader.pread(88, 0, 1, 0), Some(Ok(1)));
    BACKEND.with(|b| assert_eq!(b.borrow().calls.last().unwrap().handle, 99));
}

#[test]
fn expired_grant_and_legacy_file_retry_at_the_same_position_without_new_authority() {
    for grant in [0, 71] {
        let (mut r, _control, _direct, data) = setup(200);
        r.file = Some(r.file.unwrap().with_posix_read_grant(grant));
        BACKEND.with(|b| b.borrow_mut().granted_result = Some(Err(RequestError::Status(OS_RESPONSE_INVALID_DESCRIPTOR))));
        let mut guest = [0xa5; 11];
        assert_eq!(production::sys_positioned_io(&mut r, 1, 3, guest.as_mut_ptr() as Word, 11, 37, false), Ok(11));
        assert_eq!(&guest, &data[37..48]);
        BACKEND.with(|b| {
            let b = b.borrow();
            assert_eq!(b.calls.len(), if grant == 0 { 1 } else { 2 });
            assert!(b.calls.iter().all(|c| c.offset == 37));
            assert_eq!(b.calls.last().unwrap().code, posix::POSIX_REQUEST_PREAD_DIRECT);
        });
        assert_eq!(r.rooted_channel.connections, usize::from(grant != 0));
    }
}

#[test]
fn short_reads_and_later_copyout_failure_preserve_the_existing_prefix_rules() {
    let (mut r, _control, _direct, data) = setup(19);
    let mut guest = vec![0xa5; 70000];
    assert_eq!(production::sys_positioned_io(&mut r, 1, 3, guest.as_mut_ptr() as Word, 100, 7, false), Ok(12));
    assert_eq!(&guest[..12], &data[7..]);
    assert!(guest[12..].iter().all(|&b| b == 0xa5));
    let (mut r, _control, _direct, data) = setup(80000);
    BACKEND.with(|b| b.borrow_mut().copyout_fail_after = Some(1));
    assert_eq!(production::sys_positioned_io(&mut r, 1, 3, guest.as_mut_ptr() as Word, 70000, 0, false), Ok(65536));
    assert_eq!(&guest[..65536], &data[..65536]);
    assert!(guest[65536..].iter().all(|&b| b == 0xa5));
    assert!(r.file_reader.pread(9, 0, 1, 0).is_none());
}

#[test]
fn invalid_positioned_io_never_connects_or_calls_the_provider() {
    let (mut r, _control, _direct, _) = setup(200);
    assert_eq!(production::sys_positioned_io(&mut r, 1, 3, 0, 0, isize::MAX as Word, false), Ok(0));
    for (fd, buffer, bytes, offset, write, error) in [
        (4, 4096, 1, 0, false, EBADF),
        (3, 0, 1, 0, false, EFAULT),
        (3, Word::MAX, 1, 0, false, EFAULT),
        (3, 4096, 1, Word::MAX, false, EINVAL),
        (3, 4096, 1, isize::MAX as Word, false, EINVAL),
        (3, 4096, 1, 0, true, EBADF),
    ] {
        assert_eq!(production::sys_positioned_io(&mut r, 1, fd, buffer, bytes, offset, write), Err(error));
    }
    assert_eq!(r.rooted_channel.connections, 0);
    BACKEND.with(|b| assert!(b.borrow().calls.is_empty()));
}

#[test]
fn pwrite_retains_the_mutating_posix_route_and_never_uses_a_read_grant() {
    let (mut r, _control, direct, _) = setup(200);
    r.file.as_mut().unwrap().resource = LINUX_O_RDWR;
    let guest = [7u8; 13];
    assert_eq!(production::sys_positioned_io(&mut r, 1, 3, guest.as_ptr() as Word, guest.len(), 29, true), Ok(13));
    assert_eq!(&direct[..13], &guest);
    assert_eq!(r.rooted_channel.connections, 0);
    BACKEND.with(|b| {
        let b = b.borrow();
        assert_eq!(b.calls.len(), 1);
        assert_eq!((b.calls[0].code, b.calls[0].handle, b.calls[0].offset), (posix::POSIX_REQUEST_PWRITE_DIRECT, 9, 29));
    });
}

#[test]
fn an_oversized_granted_read_never_reaches_the_guest_copy() {
    let (mut r, _control, _direct, _) = setup(200);
    BACKEND.with(|b| b.borrow_mut().read_results.push_back(Ok(18)));
    let mut guest = [0xa5; 17];
    assert_eq!(production::sys_positioned_io(&mut r, 1, 3, guest.as_mut_ptr() as Word, 17, 0, false), Err(EIO));
    assert_eq!(guest, [0xa5; 17]);
    BACKEND.with(|b| assert_eq!(b.borrow().copyouts, 0));
}
