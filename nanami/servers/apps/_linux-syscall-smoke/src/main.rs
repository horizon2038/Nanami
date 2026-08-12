#![no_std]
#![no_main]

mod arch;

use arch::{linux_exit_group, linux_getpid, linux_write};

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

fn println(s: &str) {
    let _ = linux_write(1, s.as_ptr(), s.len());
}
