use core::arch::asm;

const AT_FDCWD: usize = (-100isize) as usize;

#[repr(C)]
pub struct LinuxIoVec {
    pub base: *mut u8,
    pub len: usize,
}

#[repr(C)]
pub struct LinuxTimespec {
    pub seconds: i64,
    pub nanoseconds: i64,
}

unsafe fn syscall0(number: usize) -> isize {
    let result: usize;
    asm!(
        "svc 0",
        in("x8") number,
        lateout("x0") result,
        options(nostack),
    );
    result as isize
}

unsafe fn syscall1(number: usize, arg0: usize) -> isize {
    let result: usize;
    asm!(
        "svc 0",
        in("x8") number,
        inlateout("x0") arg0 => result,
        options(nostack),
    );
    result as isize
}

unsafe fn syscall2(number: usize, arg0: usize, arg1: usize) -> isize {
    let result: usize;
    asm!(
        "svc 0",
        in("x8") number,
        inlateout("x0") arg0 => result,
        in("x1") arg1,
        options(nostack),
    );
    result as isize
}

unsafe fn syscall3(number: usize, arg0: usize, arg1: usize, arg2: usize) -> isize {
    let result: usize;
    asm!(
        "svc 0",
        in("x8") number,
        inlateout("x0") arg0 => result,
        in("x1") arg1,
        in("x2") arg2,
        options(nostack),
    );
    result as isize
}

unsafe fn syscall4(number: usize, arg0: usize, arg1: usize, arg2: usize, arg3: usize) -> isize {
    let result: usize;
    asm!(
        "svc 0",
        in("x8") number,
        inlateout("x0") arg0 => result,
        in("x1") arg1,
        in("x2") arg2,
        in("x3") arg3,
        options(nostack),
    );
    result as isize
}

unsafe fn syscall5(
    number: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
) -> isize {
    let result: usize;
    asm!(
        "svc 0",
        in("x8") number,
        inlateout("x0") arg0 => result,
        in("x1") arg1,
        in("x2") arg2,
        in("x3") arg3,
        in("x4") arg4,
        options(nostack),
    );
    result as isize
}

pub fn linux_getpid() -> usize {
    unsafe { syscall0(172) as usize }
}

pub fn linux_exit_group(status: usize) -> ! {
    unsafe {
        asm!(
            "svc 0",
            in("x8") 94usize,
            in("x0") status,
            options(noreturn),
        )
    }
}

pub fn linux_write(fd: usize, buf: *const u8, count: usize) -> isize {
    unsafe { syscall3(64, fd, buf as usize, count) }
}

pub fn linux_open(path: *const u8, flags: usize) -> isize {
    unsafe { syscall4(56, AT_FDCWD, path as usize, flags, 0) }
}

pub fn linux_link(old_path: *const u8, new_path: *const u8) -> isize {
    unsafe {
        syscall5(
            37,
            AT_FDCWD,
            old_path as usize,
            AT_FDCWD,
            new_path as usize,
            0,
        )
    }
}

pub fn linux_unlink(path: *const u8) -> isize {
    unsafe { syscall3(35, AT_FDCWD, path as usize, 0) }
}

pub fn linux_readv(fd: usize, iov: *const LinuxIoVec, count: usize) -> isize {
    unsafe { syscall3(65, fd, iov as usize, count) }
}

pub fn linux_close(fd: usize) -> isize {
    unsafe { syscall1(57, fd) }
}

pub fn linux_nanosleep(request: *const LinuxTimespec) -> isize {
    unsafe { syscall2(101, request as usize, 0) }
}

pub fn linux_clock_gettime(clock_id: usize, result: *mut LinuxTimespec) -> isize {
    unsafe { syscall2(113, clock_id, result as usize) }
}
