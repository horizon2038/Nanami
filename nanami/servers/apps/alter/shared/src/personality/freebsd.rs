use libnanami::Word;

use crate::linux::{self, EmulationAction};
use crate::process::{write_register_value, LinuxSyscallContext, REG_FS_BASE};
use crate::state::Runtime;

const FREEBSD_ENOSYS: isize = 78;
const FREEBSD_EFAULT: isize = 14;
const FREEBSD_EINVAL: isize = 22;

const FREEBSD_SYS_EXIT: Word = 1;
const FREEBSD_SYS_FORK: Word = 2;
const FREEBSD_SYS_READ: Word = 3;
const FREEBSD_SYS_WRITE: Word = 4;
const FREEBSD_SYS_OPEN: Word = 5;
const FREEBSD_SYS_CLOSE: Word = 6;
const FREEBSD_SYS_WAIT4: Word = 7;
const FREEBSD_SYS_UNLINK: Word = 10;
const FREEBSD_SYS_CHDIR: Word = 12;
const FREEBSD_SYS_MKNOD: Word = 14;
const FREEBSD_SYS_BREAK: Word = 17;
const FREEBSD_SYS_GETPID: Word = 20;
const FREEBSD_SYS_GETUID: Word = 24;
const FREEBSD_SYS_GETEUID: Word = 25;
const FREEBSD_SYS_ACCESS: Word = 33;
const FREEBSD_SYS_KILL: Word = 37;
const FREEBSD_SYS_GETPPID: Word = 39;
const FREEBSD_SYS_DUP: Word = 41;
const FREEBSD_SYS_PIPE: Word = 42;
const FREEBSD_SYS_GETGID: Word = 47;
const FREEBSD_SYS_GETEGID: Word = 43;
const FREEBSD_SYS_IOCTL: Word = 54;
const FREEBSD_SYS_READLINK: Word = 58;
const FREEBSD_SYS_EXECVE: Word = 59;
const FREEBSD_SYS_UMASK: Word = 60;
const FREEBSD_SYS_MUNMAP: Word = 73;
const FREEBSD_SYS_MPROTECT: Word = 74;
const FREEBSD_SYS_DUP2: Word = 90;
const FREEBSD_SYS_FCNTL: Word = 92;
const FREEBSD_SYS_SELECT: Word = 93;
const FREEBSD_SYS_GETTIMEOFDAY: Word = 116;
const FREEBSD_SYS_READV: Word = 120;
const FREEBSD_SYS_WRITEV: Word = 121;
const FREEBSD_SYS_RENAME: Word = 128;
const FREEBSD_SYS_MKDIR: Word = 136;
const FREEBSD_SYS_RMDIR: Word = 137;
const FREEBSD_SYS_UTIMES: Word = 138;
const FREEBSD_SYS_SYSARCH: Word = 165;
const FREEBSD_SYS_STAT: Word = 188;
const FREEBSD_SYS_FSTAT: Word = 189;
const FREEBSD_SYS_LSTAT: Word = 190;
const FREEBSD_SYS_GETDIRENTRIES: Word = 196;
const FREEBSD_SYS___SYSCTL: Word = 202;
const FREEBSD_SYS_GETPGID: Word = 207;
const FREEBSD_SYS_POLL: Word = 209;
const FREEBSD_SYS_CLOCK_GETTIME: Word = 232;
const FREEBSD_SYS_ISSETUGID: Word = 253;
const FREEBSD_SYS_SIGACTION: Word = 416;
const FREEBSD_SYS_SIGPROCMASK: Word = 340;
const FREEBSD_SYS_THR_EXIT: Word = 431;
const FREEBSD_SYS_THR_SELF: Word = 432;
const FREEBSD_SYS_MMAP: Word = 477;
const FREEBSD_SYS_LSEEK: Word = 478;
const FREEBSD_SYS_FACCESSAT: Word = 489;
const FREEBSD_SYS_FSTATAT: Word = 493;
const FREEBSD_SYS_MKDIRAT: Word = 496;
const FREEBSD_SYS_MKNODAT: Word = 498;
const FREEBSD_SYS_OPENAT: Word = 499;
const FREEBSD_SYS_READLINKAT: Word = 500;
const FREEBSD_SYS_RENAMEAT: Word = 502;
const FREEBSD_SYS_UNLINKAT: Word = 503;

const AMD64_GET_FSBASE: Word = 128;
const AMD64_SET_FSBASE: Word = 129;

const FREEBSD_O_NONBLOCK: Word = 0x0004;
const FREEBSD_O_APPEND: Word = 0x0008;
const FREEBSD_O_CREAT: Word = 0x0200;
const FREEBSD_O_TRUNC: Word = 0x0400;
const FREEBSD_O_DIRECTORY: Word = 0x0002_0000;
const FREEBSD_O_CLOEXEC: Word = 0x0010_0000;

const LINUX_O_NONBLOCK: Word = 0o4000;
const LINUX_O_APPEND: Word = 0o2000;
const LINUX_O_CREAT: Word = 0o100;
const LINUX_O_TRUNC: Word = 0o1000;
const LINUX_O_DIRECTORY: Word = 0o200000;
const LINUX_O_CLOEXEC: Word = 0o2000000;

const FREEBSD_CTL_KERN: u32 = 1;
const FREEBSD_CTL_HW: u32 = 6;
const FREEBSD_CTL_USER: u32 = 8;

const FREEBSD_KERN_OSTYPE: u32 = 1;
const FREEBSD_KERN_OSRELEASE: u32 = 2;
const FREEBSD_KERN_OSREV: u32 = 3;
const FREEBSD_KERN_VERSION: u32 = 4;
const FREEBSD_KERN_HOSTNAME: u32 = 10;
const FREEBSD_KERN_OSRELDATE: u32 = 24;
const FREEBSD_KERN_USRSTACK: u32 = 33;
const FREEBSD_KERN_ARND: u32 = 37;

const FREEBSD_HW_MACHINE: u32 = 1;
const FREEBSD_HW_MODEL: u32 = 2;
const FREEBSD_HW_NCPU: u32 = 3;
const FREEBSD_HW_BYTEORDER: u32 = 4;
const FREEBSD_HW_PHYSMEM: u32 = 5;
const FREEBSD_HW_USERMEM: u32 = 6;
const FREEBSD_HW_PAGESIZE: u32 = 7;

const FREEBSD_USER_CS_PATH: u32 = 1;

const FREEBSD_USRSTACK: Word = 0x4040000;

pub fn dispatch_syscall(
    runtime: &mut Runtime,
    native_pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    if context.number == FREEBSD_SYS_SYSARCH {
        return EmulationAction::Return(sys_sysarch(runtime, native_pid, context));
    }
    if context.number == FREEBSD_SYS_THR_SELF {
        return EmulationAction::Return(write_user_word(
            runtime,
            native_pid,
            context.args[0],
            native_pid,
        ));
    }
    if context.number == FREEBSD_SYS_THR_EXIT {
        return EmulationAction::Exit(0);
    }
    if context.number == FREEBSD_SYS___SYSCTL {
        return EmulationAction::Return(sys_sysctl(runtime, native_pid, context));
    }
    if context.number == FREEBSD_SYS_UMASK
        || context.number == FREEBSD_SYS_ISSETUGID
        || context.number == FREEBSD_SYS_SIGACTION
        || context.number == FREEBSD_SYS_SIGPROCMASK
    {
        return EmulationAction::Return(0);
    }

    let Some(mut translated) = translate_syscall_context(context) else {
        libnanami::println!(
            "[alter/freebsd] unsupported syscall {}, returning -ENOSYS",
            context.number
        );
        return EmulationAction::Return(-FREEBSD_ENOSYS);
    };
    translate_open_flags(&mut translated);
    linux::dispatch_syscall(runtime, native_pid, translated)
}

fn translate_syscall_context(context: LinuxSyscallContext) -> Option<LinuxSyscallContext> {
    let number = match context.number {
        FREEBSD_SYS_READ => linux::SYS_READ,
        FREEBSD_SYS_WRITE => linux::SYS_WRITE,
        FREEBSD_SYS_OPEN => linux::SYS_OPEN,
        FREEBSD_SYS_CLOSE => linux::SYS_CLOSE,
        FREEBSD_SYS_WAIT4 => linux::SYS_WAIT4,
        FREEBSD_SYS_UNLINK => linux::SYS_UNLINK,
        FREEBSD_SYS_CHDIR => linux::SYS_CHDIR,
        FREEBSD_SYS_MKNOD => linux::SYS_MKNOD,
        FREEBSD_SYS_BREAK => linux::SYS_BRK,
        FREEBSD_SYS_GETPID => linux::SYS_GETPID,
        FREEBSD_SYS_GETUID => linux::SYS_GETUID,
        FREEBSD_SYS_GETEUID => linux::SYS_GETEUID,
        FREEBSD_SYS_ACCESS => linux::SYS_ACCESS,
        FREEBSD_SYS_KILL => linux::SYS_KILL,
        FREEBSD_SYS_GETPPID => linux::SYS_GETPPID,
        FREEBSD_SYS_DUP => linux::SYS_DUP,
        FREEBSD_SYS_PIPE => linux::SYS_PIPE,
        FREEBSD_SYS_GETGID => linux::SYS_GETGID,
        FREEBSD_SYS_GETEGID => linux::SYS_GETEGID,
        FREEBSD_SYS_IOCTL => linux::SYS_IOCTL,
        FREEBSD_SYS_READLINK => linux::SYS_READLINK,
        FREEBSD_SYS_EXECVE => linux::SYS_EXECVE,
        FREEBSD_SYS_MUNMAP => linux::SYS_MUNMAP,
        FREEBSD_SYS_MPROTECT => linux::SYS_MPROTECT,
        FREEBSD_SYS_DUP2 => linux::SYS_DUP2,
        FREEBSD_SYS_FCNTL => linux::SYS_FCNTL,
        FREEBSD_SYS_SELECT => linux::SYS_SELECT,
        FREEBSD_SYS_GETTIMEOFDAY => linux::SYS_GETTIMEOFDAY,
        FREEBSD_SYS_READV => return None,
        FREEBSD_SYS_WRITEV => linux::SYS_WRITEV,
        FREEBSD_SYS_RENAME => linux::SYS_RENAME,
        FREEBSD_SYS_MKDIR => linux::SYS_MKDIR,
        FREEBSD_SYS_RMDIR => linux::SYS_RMDIR,
        FREEBSD_SYS_UTIMES => linux::SYS_UTIMES,
        FREEBSD_SYS_STAT => linux::SYS_STAT,
        FREEBSD_SYS_FSTAT => linux::SYS_FSTAT,
        FREEBSD_SYS_LSTAT => linux::SYS_LSTAT,
        FREEBSD_SYS_GETDIRENTRIES => linux::SYS_GETDENTS64,
        FREEBSD_SYS_GETPGID => linux::SYS_GETPGID,
        FREEBSD_SYS_POLL => linux::SYS_POLL,
        FREEBSD_SYS_CLOCK_GETTIME => linux::SYS_CLOCK_GETTIME,
        FREEBSD_SYS_MMAP => linux::SYS_MMAP,
        FREEBSD_SYS_LSEEK => linux::SYS_LSEEK,
        FREEBSD_SYS_FACCESSAT => linux::SYS_FACCESSAT,
        FREEBSD_SYS_FSTATAT => linux::SYS_NEWFSTATAT,
        FREEBSD_SYS_MKDIRAT => linux::SYS_MKDIRAT,
        FREEBSD_SYS_MKNODAT => linux::SYS_MKNODAT,
        FREEBSD_SYS_OPENAT => linux::SYS_OPENAT,
        FREEBSD_SYS_READLINKAT => linux::SYS_READLINKAT,
        FREEBSD_SYS_RENAMEAT => linux::SYS_RENAMEAT,
        FREEBSD_SYS_UNLINKAT => linux::SYS_UNLINKAT,
        FREEBSD_SYS_FORK => linux::SYS_FORK,
        FREEBSD_SYS_EXIT => linux::SYS_EXIT,
        _ => return None,
    };

    Some(LinuxSyscallContext { number, ..context })
}

fn sys_sysctl(runtime: &mut Runtime, pid: Word, context: LinuxSyscallContext) -> isize {
    let name_ptr = context.args[0];
    let name_len = context.args[1] as usize;
    let old_ptr = context.args[2];
    let old_len_ptr = context.args[3];

    if name_ptr == 0 || name_len == 0 || name_len > 8 {
        return -FREEBSD_EINVAL;
    }
    let mut mib = [0u32; 8];
    if read_user_bytes(runtime, pid, name_ptr, name_len * 4).is_err() {
        return -FREEBSD_EFAULT;
    }
    let mut index = 0usize;
    while index < name_len {
        mib[index] = unsafe {
            ::core::ptr::read_unaligned((runtime.posix_shm as usize + index * 4) as *const u32)
        };
        index += 1;
    }

    if name_len < 2 {
        return -FREEBSD_EINVAL;
    }
    match (mib[0], mib[1]) {
        (FREEBSD_CTL_KERN, FREEBSD_KERN_OSTYPE) => {
            write_sysctl_bytes(runtime, pid, old_ptr, old_len_ptr, b"FreeBSD\0")
        }
        (FREEBSD_CTL_KERN, FREEBSD_KERN_OSRELEASE) => {
            write_sysctl_bytes(runtime, pid, old_ptr, old_len_ptr, b"14.0-Nanami\0")
        }
        (FREEBSD_CTL_KERN, FREEBSD_KERN_OSREV) => {
            write_sysctl_u32(runtime, pid, old_ptr, old_len_ptr, 1400000)
        }
        (FREEBSD_CTL_KERN, FREEBSD_KERN_VERSION) => {
            write_sysctl_bytes(runtime, pid, old_ptr, old_len_ptr, b"FreeBSD on Nanami\0")
        }
        (FREEBSD_CTL_KERN, FREEBSD_KERN_HOSTNAME) => {
            write_sysctl_bytes(runtime, pid, old_ptr, old_len_ptr, b"nanami\0")
        }
        (FREEBSD_CTL_KERN, FREEBSD_KERN_OSRELDATE) => {
            write_sysctl_u32(runtime, pid, old_ptr, old_len_ptr, 1400000)
        }
        (FREEBSD_CTL_KERN, FREEBSD_KERN_USRSTACK) => {
            write_sysctl_word(runtime, pid, old_ptr, old_len_ptr, FREEBSD_USRSTACK)
        }
        (FREEBSD_CTL_KERN, FREEBSD_KERN_ARND) => write_sysctl_bytes(
            runtime,
            pid,
            old_ptr,
            old_len_ptr,
            b"\x39\xa5\x7c\x12\x81\xde\x44\x03\xba\x91\x20\x6f\xd7\x5e\xca\x18",
        ),
        (FREEBSD_CTL_HW, FREEBSD_HW_MACHINE) => {
            write_sysctl_bytes(runtime, pid, old_ptr, old_len_ptr, b"amd64\0")
        }
        (FREEBSD_CTL_HW, FREEBSD_HW_MODEL) => {
            write_sysctl_bytes(runtime, pid, old_ptr, old_len_ptr, b"Nanami Virtual CPU\0")
        }
        (FREEBSD_CTL_HW, FREEBSD_HW_NCPU) => {
            write_sysctl_u32(runtime, pid, old_ptr, old_len_ptr, 1)
        }
        (FREEBSD_CTL_HW, FREEBSD_HW_BYTEORDER) => {
            write_sysctl_u32(runtime, pid, old_ptr, old_len_ptr, 1234)
        }
        (FREEBSD_CTL_HW, FREEBSD_HW_PHYSMEM) | (FREEBSD_CTL_HW, FREEBSD_HW_USERMEM) => {
            write_sysctl_word(runtime, pid, old_ptr, old_len_ptr, 256 * 1024 * 1024)
        }
        (FREEBSD_CTL_HW, FREEBSD_HW_PAGESIZE) => {
            write_sysctl_u32(runtime, pid, old_ptr, old_len_ptr, 4096)
        }
        (FREEBSD_CTL_USER, FREEBSD_USER_CS_PATH) => {
            write_sysctl_bytes(runtime, pid, old_ptr, old_len_ptr, b"/bin:/usr/bin\0")
        }
        _ => {
            libnanami::println!(
                "[alter/freebsd] unsupported sysctl mib={}.{}",
                mib[0],
                mib[1]
            );
            -FREEBSD_EINVAL
        }
    }
}

fn write_sysctl_u32(
    runtime: &mut Runtime,
    pid: Word,
    old_ptr: Word,
    old_len_ptr: Word,
    value: u32,
) -> isize {
    let bytes = value.to_le_bytes();
    write_sysctl_bytes(runtime, pid, old_ptr, old_len_ptr, &bytes)
}

fn write_sysctl_word(
    runtime: &mut Runtime,
    pid: Word,
    old_ptr: Word,
    old_len_ptr: Word,
    value: Word,
) -> isize {
    let bytes = value.to_le_bytes();
    write_sysctl_bytes(runtime, pid, old_ptr, old_len_ptr, &bytes)
}

fn write_sysctl_bytes(
    runtime: &mut Runtime,
    pid: Word,
    old_ptr: Word,
    old_len_ptr: Word,
    bytes: &[u8],
) -> isize {
    if old_len_ptr == 0 {
        return -FREEBSD_EFAULT;
    }
    let old_len = match read_user_word(runtime, pid, old_len_ptr) {
        Ok(value) => value as usize,
        Err(()) => return -FREEBSD_EFAULT,
    };
    if write_user_word(runtime, pid, old_len_ptr, bytes.len() as Word) != 0 {
        return -FREEBSD_EFAULT;
    }
    if old_ptr == 0 {
        return 0;
    }
    let copy_len = ::core::cmp::min(old_len, bytes.len());
    unsafe {
        ::core::ptr::copy_nonoverlapping(bytes.as_ptr(), runtime.posix_shm as *mut u8, copy_len);
    }
    match libnanami::request_process_memory_write(pid, old_ptr, runtime.posix_shm, copy_len as Word)
    {
        Ok(()) => 0,
        Err(_) => -FREEBSD_EFAULT,
    }
}

fn translate_open_flags(context: &mut LinuxSyscallContext) {
    match context.number {
        linux::SYS_OPEN => context.args[1] = translate_file_flags(context.args[1]),
        linux::SYS_OPENAT => context.args[2] = translate_file_flags(context.args[2]),
        _ => {}
    }
}

fn translate_file_flags(flags: Word) -> Word {
    let mut out = flags & 0x3;
    if (flags & FREEBSD_O_NONBLOCK) != 0 {
        out |= LINUX_O_NONBLOCK;
    }
    if (flags & FREEBSD_O_APPEND) != 0 {
        out |= LINUX_O_APPEND;
    }
    if (flags & FREEBSD_O_CREAT) != 0 {
        out |= LINUX_O_CREAT;
    }
    if (flags & FREEBSD_O_TRUNC) != 0 {
        out |= LINUX_O_TRUNC;
    }
    if (flags & FREEBSD_O_DIRECTORY) != 0 {
        out |= LINUX_O_DIRECTORY;
    }
    if (flags & FREEBSD_O_CLOEXEC) != 0 {
        out |= LINUX_O_CLOEXEC;
    }
    out
}

fn sys_sysarch(runtime: &mut Runtime, pid: Word, context: LinuxSyscallContext) -> isize {
    match context.args[0] {
        AMD64_SET_FSBASE => {
            let Ok(fs_base) = read_user_word(runtime, pid, context.args[1]) else {
                return -FREEBSD_EFAULT;
            };
            let Some(process) = runtime.managed_process(pid) else {
                return -FREEBSD_EINVAL;
            };
            if write_register_value(process.pcb, REG_FS_BASE, fs_base).is_err() {
                return -FREEBSD_EFAULT;
            }
            if !runtime.set_fs_base(pid, fs_base) {
                return -FREEBSD_EINVAL;
            }
            0
        }
        AMD64_GET_FSBASE => {
            let Some(process) = runtime.managed_process(pid) else {
                return -FREEBSD_EINVAL;
            };
            write_user_word(runtime, pid, context.args[1], process.fs_base)
        }
        _ => -FREEBSD_ENOSYS,
    }
}

fn read_user_word(runtime: &mut Runtime, pid: Word, user_ptr: Word) -> Result<Word, ()> {
    if user_ptr == 0 {
        return Err(());
    }
    libnanami::request_process_memory_read(pid, user_ptr, runtime.posix_shm, 8).map_err(|_| ())?;
    Ok(unsafe { ::core::ptr::read_unaligned(runtime.posix_shm as *const Word) })
}

fn read_user_bytes(runtime: &mut Runtime, pid: Word, user_ptr: Word, len: usize) -> Result<(), ()> {
    if user_ptr == 0 {
        return Err(());
    }
    libnanami::request_process_memory_read(pid, user_ptr, runtime.posix_shm, len as Word)
        .map_err(|_| ())
}

fn write_user_word(runtime: &mut Runtime, pid: Word, user_ptr: Word, value: Word) -> isize {
    if user_ptr == 0 {
        return -FREEBSD_EFAULT;
    }
    unsafe {
        ::core::ptr::write_unaligned(runtime.posix_shm as *mut Word, value);
    }
    match libnanami::request_process_memory_write(pid, user_ptr, runtime.posix_shm, 8) {
        Ok(()) => 0,
        Err(_) => -FREEBSD_EFAULT,
    }
}
