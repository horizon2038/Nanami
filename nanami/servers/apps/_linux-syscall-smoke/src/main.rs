#![no_std]
#![no_main]

use core::arch::asm;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    linux_exit_group(125)
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let pid = linux_getpid();
    if pid == 0 {
        linux_exit_group(77);
    }
    println("Hello from Linux syscall smoke test!\n");
    println("\x1b[32mLinux syscall smoke test passed!\x1b[0m\n");

    linux_exit_group(0)
}

fn linux_getpid() -> usize {
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

fn linux_exit_group(status: usize) -> ! {
    unsafe {
        asm!(
            "syscall",
            in("rax") 231usize,
            in("rdi") status,
            options(noreturn),
        )
    }
}

fn linux_write(fd: usize, buf: *const u8, count: usize) -> isize {
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

fn println(s: &str) {
    let _ = linux_write(1, s.as_ptr(), s.len());
}
