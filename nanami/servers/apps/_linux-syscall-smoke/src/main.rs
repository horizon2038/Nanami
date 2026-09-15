#![no_std]
#![no_main]

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".global _start",
    ".type _start,@function",
    "_start:",
    // Linux enters with a 16-byte-aligned stack. Make a real call so the
    // Rust function sees the SysV AMD64 function-entry alignment it expects.
    "call linux_smoke_main",
    "ud2",
);

mod arch;

use arch::{
    linux_clock_gettime, linux_close, linux_exit_group, linux_getpid, linux_link, linux_nanosleep,
    linux_open, linux_readv, linux_unlink, linux_write, linux_writev, LinuxIoVec, LinuxTimespec,
};

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    linux_exit_group(125)
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    linux_smoke_main()
}

#[no_mangle]
pub extern "C" fn linux_smoke_main() -> ! {
    let pid = linux_getpid();
    if pid == 0 {
        linux_exit_group(77);
    }
    verify_readv();
    verify_writev();
    verify_link();
    verify_nanosleep();
    println("Hello from Linux syscall smoke test!\n");
    println("\x1b[32mLinux syscall smoke test passed!\x1b[0m\n");

    linux_exit_group(0)
}

fn verify_link() {
    let source = b"/bin/linux-syscall-smoke\0";
    let target = b"/tmp/linux-syscall-smoke-link\0";
    let _ = linux_unlink(target.as_ptr());
    if linux_link(source.as_ptr(), target.as_ptr()) != 0 {
        linux_exit_group(83);
    }

    let fd = linux_open(target.as_ptr(), 0);
    if fd < 0 {
        linux_exit_group(84);
    }
    let mut magic = [0u8; 4];
    let iov = [LinuxIoVec {
        base: magic.as_mut_ptr(),
        len: magic.len(),
    }];
    let read = linux_readv(fd as usize, iov.as_ptr(), iov.len());
    let _ = linux_close(fd as usize);
    let unlinked = linux_unlink(target.as_ptr());
    if read != 4 || magic != [0x7f, b'E', b'L', b'F'] || unlinked != 0 {
        linux_exit_group(85);
    }
    println("Linux link smoke test passed!\n");
}

fn verify_writev() {
    let path = b"/tmp/linux-writev-smoke\0";
    let _ = linux_unlink(path.as_ptr());
    let fd = linux_open(path.as_ptr(), 0o100 | 0o1000 | 1); // CREAT | TRUNC | WRONLY
    if fd < 0 {
        linux_exit_group(86);
    }
    let mut source = [0u8; 80000];
    for (i, byte) in source.iter_mut().enumerate() {
        *byte = (i.wrapping_mul(7) >> 3) as u8;
    }
    let iov = [
        LinuxIoVec {
            base: source.as_mut_ptr().wrapping_add(17),
            len: 7001,
        },
        LinuxIoVec {
            base: core::ptr::null_mut(),
            len: 0,
        },
        LinuxIoVec {
            base: source.as_mut_ptr().wrapping_add(9000),
            len: 70000,
        },
    ];
    let null_fd = linux_open(b"/dev/null\0".as_ptr(), 1);
    if null_fd < 0 || linux_writev(null_fd as usize, iov.as_ptr(), iov.len()) != 77001 {
        linux_exit_group(93);
    }
    let _ = linux_close(null_fd as usize);
    if linux_writev(fd as usize, iov.as_ptr(), iov.len()) != 77001 {
        linux_exit_group(87);
    }
    // Commit the good first vector when a later, non-null guest address is unmapped.
    let partial = [
        LinuxIoVec {
            base: source.as_mut_ptr(),
            len: 4,
        },
        LinuxIoVec {
            base: 0xdead0000 as *mut u8,
            len: 16,
        },
    ];
    if linux_writev(fd as usize, partial.as_ptr(), partial.len()) != 4 {
        linux_exit_group(88);
    }
    if linux_writev(fd as usize, partial[1..].as_ptr(), 1) >= 0 {
        linux_exit_group(89);
    }
    let _ = linux_close(fd as usize);
    let fd = linux_open(path.as_ptr(), 0);
    if fd < 0 || linux_writev(fd as usize, iov.as_ptr(), iov.len()) != -9 {
        linux_exit_group(90);
    }
    let mut output = [0u8; 77005];
    let out = [LinuxIoVec {
        base: output.as_mut_ptr(),
        len: output.len(),
    }];
    if linux_readv(fd as usize, out.as_ptr(), 1) != output.len() as isize
        || output[..7001] != source[17..7018]
        || output[7001..77001] != source[9000..79000]
        || output[77001..] != source[..4]
    {
        linux_exit_group(91);
    }
    let _ = linux_close(fd as usize);

    // The fixture uses 1-KiB ext2 blocks: 280 KiB reaches double indirection.
    let fd = linux_open(path.as_ptr(), 0o1000 | 1);
    if fd < 0 {
        linux_exit_group(94);
    }
    let blocks = [
        LinuxIoVec {
            base: source.as_mut_ptr().wrapping_add(31),
            len: 3000,
        },
        LinuxIoVec {
            base: source.as_mut_ptr().wrapping_add(7000),
            len: 1096,
        },
    ];
    for _ in 0..70 {
        if linux_writev(fd as usize, blocks.as_ptr(), blocks.len()) != 4096 {
            linux_exit_group(95);
        }
    }
    let _ = linux_close(fd as usize);
    let fd = linux_open(path.as_ptr(), 0);
    if fd < 0 {
        linux_exit_group(96);
    }
    let out = [LinuxIoVec {
        base: output.as_mut_ptr(),
        len: 4096,
    }];
    for _ in 0..70 {
        if linux_readv(fd as usize, out.as_ptr(), 1) != 4096
            || output[..3000] != source[31..3031]
            || output[3000..4096] != source[7000..8096]
        {
            linux_exit_group(97);
        }
    }
    let _ = linux_close(fd as usize);
    verify_overwrite(path, &mut source, &mut output);
    let _ = linux_unlink(path.as_ptr());
    let prefix = b"Linux writev ";
    let suffix = b"smoke test passed!\n";
    let terminal = [
        LinuxIoVec {
            base: prefix.as_ptr() as *mut u8,
            len: prefix.len(),
        },
        LinuxIoVec {
            base: core::ptr::null_mut(),
            len: 0,
        },
        LinuxIoVec {
            base: suffix.as_ptr() as *mut u8,
            len: suffix.len(),
        },
    ];
    if linux_writev(1, terminal.as_ptr(), terminal.len()) != (prefix.len() + suffix.len()) as isize
    {
        linux_exit_group(92);
    }
}

fn verify_overwrite(path: &[u8], source: &mut [u8], output: &mut [u8]) {
    // Reuse the existing 280-KiB allocation. Large writes exercise contiguous
    // batches across both indirect levels, then a partial overwrite checks RMW.
    for (i, byte) in source.iter_mut().enumerate() {
        *byte = (i.wrapping_mul(13).wrapping_add(91) >> 2) as u8;
    }
    let fd = linux_open(path.as_ptr(), 1);
    if fd < 0 {
        linux_exit_group(98);
    }
    let total = 70 * 4096;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(65536);
        if linux_write(fd as usize, source.as_ptr(), count) != count as isize {
            linux_exit_group(99);
        }
        offset += count;
    }
    let _ = linux_close(fd as usize);
    let fd = linux_open(path.as_ptr(), 1);
    if fd < 0 || linux_write(fd as usize, b"hdr".as_ptr(), 3) != 3 {
        linux_exit_group(100);
    }
    let _ = linux_close(fd as usize);
    let fd = linux_open(path.as_ptr(), 0);
    if fd < 0 {
        linux_exit_group(101);
    }
    offset = 0;
    while offset < total {
        let count = (total - offset).min(65536);
        let iov = [LinuxIoVec {
            base: output.as_mut_ptr(),
            len: count,
        }];
        if linux_readv(fd as usize, iov.as_ptr(), 1) != count as isize {
            linux_exit_group(102);
        }
        let unchanged = if offset == 0 {
            if output[..3] != *b"hdr" {
                linux_exit_group(103);
            }
            3
        } else {
            0
        };
        if output[unchanged..count] != source[unchanged..count] {
            linux_exit_group(104);
        }
        offset += count;
    }
    let _ = linux_close(fd as usize);
    println("Linux batched overwrite smoke test passed!\n");
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
