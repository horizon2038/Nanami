use core::arch::asm;

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

pub fn linux_getpid() -> usize {
    let pid: usize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") 39usize => pid,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    pid
}

pub fn linux_exit_group(status: usize) -> ! {
    unsafe {
        asm!(
            "syscall",
            in("rax") 231usize,
            in("rdi") status,
            options(noreturn),
        )
    }
}

pub fn linux_write(fd: usize, buf: *const u8, count: usize) -> isize {
    let ret: isize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") 1usize => ret,
            in("rdi") fd,
            in("rsi") buf,
            in("rdx") count,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    ret
}

pub fn linux_open(path: *const u8, flags: usize) -> isize {
    let ret: isize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") 2usize => ret,
            in("rdi") path,
            in("rsi") flags,
            in("rdx") 0usize,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    ret
}

pub fn linux_readv(fd: usize, iov: *const LinuxIoVec, count: usize) -> isize {
    let ret: isize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") 19usize => ret,
            in("rdi") fd,
            in("rsi") iov,
            in("rdx") count,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    ret
}

pub fn linux_close(fd: usize) -> isize {
    let ret: isize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") 3usize => ret,
            in("rdi") fd,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    ret
}

pub fn linux_nanosleep(request: *const LinuxTimespec) -> isize {
    let ret: isize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") 35usize => ret,
            in("rdi") request,
            in("rsi") 0usize,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    ret
}

pub fn linux_clock_gettime(clock_id: usize, result: *mut LinuxTimespec) -> isize {
    let ret: isize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") 228usize => ret,
            in("rdi") clock_id,
            in("rsi") result,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    ret
}
