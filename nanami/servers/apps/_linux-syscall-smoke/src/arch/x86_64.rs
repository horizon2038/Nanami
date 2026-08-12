use core::arch::asm;

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
