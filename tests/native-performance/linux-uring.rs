//! Exercise the actual shared-ring implementation. Filesystem operations below
//! are unrelated to these tests; end-to-end coverage lives in nvim-syscalls.c.
use super::*;

pub(super) fn sys_fsync(_: &mut Runtime, _: Word, _: Word) -> Result<Word, i32> {
    Err(EOPNOTSUPP)
}
pub(super) fn sys_openat(
    _: &mut Runtime,
    _: Word,
    _: Word,
    _: Word,
    _: Word,
    _: Word,
) -> Result<Word, i32> {
    Err(EOPNOTSUPP)
}
pub(super) fn sys_statx(
    _: &mut Runtime,
    _: Word,
    _: Word,
    _: Word,
    _: Word,
    _: Word,
) -> Result<Word, i32> {
    Err(EOPNOTSUPP)
}
pub(super) fn sys_mkdirat(
    _: &mut Runtime,
    _: Word,
    _: Word,
    _: Word,
    _: Word,
) -> Result<Word, i32> {
    Err(EOPNOTSUPP)
}
pub(super) fn sys_unlinkat(
    _: &mut Runtime,
    _: Word,
    _: Word,
    _: Word,
    _: Word,
) -> Result<Word, i32> {
    Err(EOPNOTSUPP)
}
pub(super) fn sys_renameat(
    _: &mut Runtime,
    _: Word,
    _: Word,
    _: Word,
    _: Word,
    _: Word,
) -> Result<Word, i32> {
    Err(EOPNOTSUPP)
}
pub(super) fn sys_linkat(
    _: &mut Runtime,
    _: Word,
    _: Word,
    _: Word,
    _: Word,
    _: Word,
    _: Word,
) -> Result<Word, i32> {
    Err(EOPNOTSUPP)
}
pub(super) mod namespace {
    use super::*;
    pub(crate) fn symlink(
        _: &mut Runtime,
        _: Word,
        _: Word,
        _: Word,
        _: Word,
    ) -> Result<Word, i32> {
        Err(EOPNOTSUPP)
    }
}
pub(super) mod truncate {
    use super::*;
    pub(crate) fn sys_ftruncate(_: &mut Runtime, _: Word, _: Word, _: Word) -> Result<Word, i32> {
        Err(EOPNOTSUPP)
    }
}
pub(super) fn sys_positioned_io(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    address: Word,
    bytes: Word,
    offset: Word,
    write: bool,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    memfd::io(runtime, pid, file, address, bytes, Some(offset), write)
}
pub(super) fn sys_close(runtime: &mut Runtime, pid: Word, fd: Word) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    runtime.files[fd] = LinuxFile::EMPTY;
    runtime.managed_process_mut(pid).unwrap().files[fd] = LinuxFile::EMPTY;
    io_uring::release(runtime, file);
    Ok(0)
}

fn setup(runtime: &mut Runtime, flags: u32) -> (Word, [u32; 30], Word, Word) {
    let mut params = [0u32; 30];
    params[2] = flags;
    let fd = io_uring::setup(runtime, 1, 4, params.as_mut_ptr() as Word).unwrap();
    let file = runtime.linux_file(1, fd).unwrap();
    let (memory, sqes) = match runtime.poll_objects.get(file.resource).unwrap() {
        poll_objects::Object::Ring(ring) => (ring.memory, ring.sqes),
        _ => unreachable!(),
    };
    let memory = memfd::page(runtime, memory, 0).unwrap();
    let sqes = memfd::page(runtime, sqes, 0).unwrap();
    (fd, params, memory, sqes)
}

fn enter(runtime: &mut Runtime, fd: Word, count: Word, minimum: Word) -> EmulationAction {
    io_uring::enter(
        runtime,
        1,
        LinuxSyscallContext {
            number: SYS_IO_URING_ENTER,
            args: [fd, count, minimum, 1, 0, 0],
        },
    )
}

fn submit(memory: Word, sqes: Word, tail: u32, array: u32, opcode: u8, data: u64) {
    let index = tail & 3;
    unsafe {
        let target = sqes + index as Word * 64;
        core::ptr::write_bytes(target as *mut u8, 0, 64);
        *(target as *mut u8) = opcode;
        ((target + 32) as *mut u64).write_unaligned(data);
        if array != 0 {
            ((memory + array as Word + index as Word * 4) as *mut u32).write(index);
        }
        ((memory + 4) as *mut u32).write(tail.wrapping_add(1));
    }
}

#[test]
fn real_ring_completes_nops_wraps_and_reports_opcode_errors() {
    for flags in [0, 1 << 16] {
        let mut runtime = runtime();
        let (fd, params, memory, sqes) = setup(&mut runtime, flags);
        assert_eq!(params[0], 4);
        assert_eq!(params[1], 8);
        assert_eq!(params[5], 3);
        for sequence in 0..20 {
            submit(
                memory,
                sqes,
                sequence,
                params[16],
                if sequence == 7 { 255 } else { 0 },
                sequence as u64,
            );
            assert_eq!(enter(&mut runtime, fd, 1, 1), EmulationAction::Return(1));
            unsafe {
                let cqe = memory + params[25] as Word + (sequence & 7) as Word * 16;
                assert_eq!((cqe as *const u64).read(), sequence as u64);
                assert_eq!(
                    ((cqe + 8) as *const i32).read(),
                    if sequence == 7 { -EOPNOTSUPP } else { 0 }
                );
                ((memory + params[20] as Word) as *mut u32).write(sequence + 1);
            }
        }
        sys_close(&mut runtime, 1, fd).unwrap();
        assert!(runtime.poll_objects.is_empty());
    }
}

#[test]
fn ring_rejects_bad_inputs_without_leaking_backing_pages() {
    let mut runtime = runtime();
    let mut params = [0u32; 30];
    for flags in [2, 4, 64, 1 << 10, 1 << 17] {
        params[2] = flags;
        assert_eq!(
            io_uring::setup(&mut runtime, 1, 4, params.as_mut_ptr() as Word),
            Err(EINVAL)
        );
        assert!(runtime.poll_objects.is_empty());
    }
    params[2] = 0;
    assert_eq!(
        io_uring::setup(&mut runtime, 1, 0, params.as_mut_ptr() as Word),
        Err(EINVAL)
    );
    runtime.copy_error = true;
    assert_eq!(
        io_uring::setup(&mut runtime, 1, 4, params.as_mut_ptr() as Word),
        Err(EFAULT)
    );
    assert!(runtime.poll_objects.is_empty());
}

#[test]
fn ring_validates_shared_indices_and_probe_and_cancels_rejected_links() {
    let mut runtime = runtime();
    let (fd, params, memory, sqes) = setup(&mut runtime, 0);
    unsafe {
        ((memory + 4) as *mut u32).write(5);
    }
    assert_eq!(
        enter(&mut runtime, fd, 4, 0),
        EmulationAction::Return(-(EINVAL as isize))
    );
    unsafe {
        ((memory + 4) as *mut u32).write(0);
    }
    submit(memory, sqes, 0, params[16], 0, 111);
    unsafe {
        *((sqes + 1) as *mut u8) = 4;
    }
    submit(memory, sqes, 1, params[16], 0, 222);
    assert_eq!(enter(&mut runtime, fd, 2, 2), EmulationAction::Return(2));
    unsafe {
        let cqe = memory + params[25] as Word;
        assert_eq!(((cqe + 8) as *const i32).read(), -EOPNOTSUPP);
        assert_eq!(((cqe + 24) as *const i32).read(), -125);
        ((memory + params[20] as Word) as *mut u32).write(50);
    }
    assert_eq!(
        enter(&mut runtime, fd, 0, 0),
        EmulationAction::Return(-(EINVAL as isize))
    );
    let mut probe = [0u8; 16 + 64 * 8];
    assert_eq!(
        io_uring::register(&mut runtime, 1, fd, 8, probe.as_mut_ptr() as Word, 64),
        Ok(0)
    );
    assert_eq!(probe[0], 55);
    assert_eq!(probe[1], 56);
    assert_eq!(probe[16 + 29 * 8 + 2], 1);
    assert_eq!(probe[16 + 6 * 8 + 2], 0);
    assert_eq!(
        io_uring::register(&mut runtime, 1, fd, 0, probe.as_mut_ptr() as Word, 64),
        Err(EOPNOTSUPP)
    );
    assert_eq!(
        io_uring::register(&mut runtime, 1, fd, 8, probe.as_mut_ptr() as Word, 64),
        Err(EINVAL)
    );
}

#[test]
fn completion_wait_wakes_without_resubmitting_and_restores_signal_mask() {
    let mut runtime = runtime();
    let (fd, params, memory, sqes) = setup(&mut runtime, 0);
    let mask: u64 = 1 << 9;
    let context = LinuxSyscallContext {
        number: SYS_IO_URING_ENTER,
        args: [fd, 0, 1, 1, &mask as *const _ as Word, 8],
    };
    assert_eq!(
        io_uring::enter(&mut runtime, 1, context),
        EmulationAction::Park
    );
    assert_eq!(runtime.managed[0].signals().unwrap().blocked, mask);
    wake_readiness_waiters(&mut runtime, READY_OBJECT);
    assert!(runtime.managed[0].readiness_wait.is_some());
    submit(memory, sqes, 0, params[16], 0, 55);
    assert_eq!(enter(&mut runtime, fd, 1, 0), EmulationAction::Return(1));
    wake_readiness_waiters(&mut runtime, READY_OBJECT);
    assert!(runtime.managed[0].readiness_wait.is_none());
    assert_eq!(runtime.managed[0].signals().unwrap().blocked, 0);
    RETURNS.with(|values| assert_eq!(*values.borrow(), vec![(101, 0)]));
}
