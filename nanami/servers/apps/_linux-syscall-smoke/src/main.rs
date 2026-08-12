#![no_std]
#![no_main]

mod arch;

use arch::{
    linux_clock_gettime, linux_close, linux_exit_group, linux_getpid, linux_nanosleep, linux_open,
    linux_readv, linux_write, LinuxIoVec, LinuxTimespec,
};

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
    verify_readv();
    verify_nanosleep();
    println("Hello from Linux syscall smoke test!\n");
    println("\x1b[32mLinux syscall smoke test passed!\x1b[0m\n");

    linux_exit_group(0)
}

fn verify_nanosleep() {
    let mut before = LinuxTimespec {
        seconds: 0,
        nanoseconds: 0,
    };
    if linux_clock_gettime(1, &mut before) != 0 {
        linux_exit_group(81);
    }
    let request = LinuxTimespec {
        seconds: 0,
        nanoseconds: 10_000_000,
    };
    if linux_nanosleep(&request) != 0 {
        linux_exit_group(80);
    }
    let mut after = LinuxTimespec {
        seconds: 0,
        nanoseconds: 0,
    };
    if linux_clock_gettime(1, &mut after) != 0
        || (after.seconds, after.nanoseconds) <= (before.seconds, before.nanoseconds)
    {
        linux_exit_group(82);
    }
    println("Linux nanosleep smoke test passed!\n");
}

fn verify_readv() {
    let path = b"/bin/linux-syscall-smoke\0";
    let fd = linux_open(path.as_ptr(), 0);
    if fd < 0 {
        linux_exit_group(78);
    }

    let mut first = [0u8; 2];
    let mut second = [0u8; 2];
    let iov = [
        LinuxIoVec {
            base: first.as_mut_ptr(),
            len: first.len(),
        },
        LinuxIoVec {
            base: second.as_mut_ptr(),
            len: second.len(),
        },
    ];
    let read = linux_readv(fd as usize, iov.as_ptr(), iov.len());
    let _ = linux_close(fd as usize);
    if read != 4 || first != [0x7f, b'E'] || second != [b'L', b'F'] {
        linux_exit_group(79);
    }
    println("Linux readv smoke test passed!\n");
}

fn println(s: &str) {
    let _ = linux_write(1, s.as_ptr(), s.len());
}
