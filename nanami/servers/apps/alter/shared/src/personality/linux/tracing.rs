use super::{
    read_shm_u64, read_target_memory, EmulationAction, LinuxSyscallContext, Runtime, Word, ENOSYS,
    SYS_ACCESS, SYS_ARCH_PRCTL, SYS_BRK, SYS_CHDIR, SYS_CHOWN, SYS_CLOCK_GETTIME, SYS_CLONE,
    SYS_CLOSE, SYS_CREAT, SYS_DUP, SYS_DUP2, SYS_DUP3, SYS_EXECVE, SYS_EXIT, SYS_EXIT_GROUP,
    SYS_FACCESSAT, SYS_FACCESSAT2, SYS_FCHOWN, SYS_FCHOWNAT, SYS_FCNTL, SYS_FORK, SYS_FSTAT,
    SYS_FUTEX, SYS_FUTIMESAT, SYS_GETCWD, SYS_GETDENTS64, SYS_GETEGID, SYS_GETEUID, SYS_GETGID,
    SYS_GETPGID, SYS_GETPID, SYS_GETPPID, SYS_GETRANDOM, SYS_GETRESGID, SYS_GETRESUID,
    SYS_GETRLIMIT, SYS_GETTID, SYS_GETTIMEOFDAY, SYS_GETUID, SYS_IOCTL, SYS_KILL, SYS_LCHOWN,
    SYS_LINK, SYS_LINKAT, SYS_LSEEK, SYS_LSTAT, SYS_MADVISE, SYS_MKDIR, SYS_MKDIRAT, SYS_MKNOD,
    SYS_MKNODAT, SYS_MMAP, SYS_MPROTECT, SYS_MREMAP, SYS_MSYNC, SYS_MUNMAP, SYS_NANOSLEEP,
    SYS_NEWFSTATAT, SYS_OPEN, SYS_OPENAT, SYS_PIPE, SYS_PIPE2, SYS_POLL, SYS_PPOLL, SYS_PREAD64,
    SYS_PRLIMIT64, SYS_PSELECT6, SYS_PWRITE64, SYS_READ, SYS_READLINK, SYS_READLINKAT, SYS_READV,
    SYS_RECVMSG, SYS_RENAME, SYS_RENAMEAT, SYS_RMDIR, SYS_RSEQ, SYS_RT_SIGACTION,
    SYS_RT_SIGPROCMASK, SYS_RT_SIGSUSPEND, SYS_SCHED_GETAFFINITY, SYS_SELECT, SYS_SENDMSG,
    SYS_SETITIMER, SYS_SETPGID, SYS_SET_ROBUST_LIST, SYS_SET_TID_ADDRESS, SYS_SIGALTSTACK,
    SYS_STAT, SYS_STATX, SYS_UNAME, SYS_UNLINK, SYS_UNLINKAT, SYS_UTIMENSAT, SYS_UTIMES, SYS_VFORK,
    SYS_WAIT4, SYS_WRITE, SYS_WRITEV, SYS_FSYNC, SYS_FDATASYNC, SYS_SYNC, SYS_SYNCFS, SYS_FTRUNCATE,
};

pub(super) fn trace_syscall_action(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
    action: EmulationAction,
) {
    if !runtime.trace_ever_enabled {
        return;
    }
    let Some(process) = runtime.managed_process(pid) else {
        return;
    };
    if !process.trace_enabled
        || runtime.terminal_port == 0
        || runtime.terminal_shm == 0
        || process.terminal_id == 0
    {
        return;
    }

    let mut line = [0u8; 192];
    let mut pos = 0usize;
    pos = trace_append_bytes(&mut line, pos, b"[strace ");
    pos = trace_append_decimal(&mut line, pos, pid);
    pos = trace_append_bytes(&mut line, pos, b"] ");
    pos = trace_append_bytes(&mut line, pos, syscall_name(context.number));
    pos = trace_append_bytes(&mut line, pos, b"(");
    let argc = syscall_arg_count(context.number);
    let mut i = 0usize;
    while i < argc {
        if i != 0 {
            pos = trace_append_bytes(&mut line, pos, b", ");
        }
        pos = trace_append_hex(&mut line, pos, context.args[i]);
        i += 1;
    }
    pos = trace_append_bytes(&mut line, pos, b") = ");
    match action {
        EmulationAction::Return(value) => {
            pos = trace_append_isize(&mut line, pos, value);
        }
        EmulationAction::Resume => {
            pos = trace_append_bytes(&mut line, pos, b"0 <resume>");
        }
        EmulationAction::Park => {
            pos = trace_append_bytes(&mut line, pos, b"? <park>");
        }
        EmulationAction::Exit(status) => {
            pos = trace_append_bytes(&mut line, pos, b"? <exit ");
            pos = trace_append_decimal(&mut line, pos, status);
            pos = trace_append_bytes(&mut line, pos, b">");
        }
        EmulationAction::Unsupported(_) => {
            pos = trace_append_bytes(&mut line, pos, b"-38 <ENOSYS>");
        }
    }
    pos = trace_append_bytes(&mut line, pos, b"\n");
    unsafe {
        ::core::ptr::copy_nonoverlapping(line.as_ptr(), runtime.terminal_shm as *mut u8, pos);
    }
    let _ = nanami_services::terminal::terminal_write_output(
        runtime.terminal_port,
        process.terminal_id,
        0,
        pos as Word,
    );
}

pub(super) fn record_syscall_result(runtime: &mut Runtime, pid: Word, syscall: Word, value: isize) {
    if !runtime.trace_ever_enabled {
        return;
    }
    if let Some(process) = runtime.managed_process_mut(pid) {
        if !process.trace_enabled {
            return;
        }
        process.last_syscall = syscall;
        process.last_syscall_return = value;
    }
}

pub(super) fn record_action_result(
    runtime: &mut Runtime,
    pid: Word,
    syscall: Word,
    action: EmulationAction,
) {
    let value = match action {
        EmulationAction::Return(value) => value,
        EmulationAction::Resume => 0,
        EmulationAction::Park => isize::MIN,
        EmulationAction::Exit(status) => status as isize,
        EmulationAction::Unsupported(_) => -(ENOSYS as isize),
    };
    record_syscall_result(runtime, pid, syscall, value);
}

pub(super) fn process_trace_enabled(runtime: &Runtime, pid: Word) -> bool {
    if !runtime.trace_ever_enabled {
        return false;
    }
    runtime
        .managed_process(pid)
        .map(|process| process.trace_enabled)
        .unwrap_or(false)
}

pub(super) fn trace_critical_syscall(
    runtime: &Runtime,
    pid: Word,
    context: LinuxSyscallContext,
    value: isize,
) {
    if !process_trace_enabled(runtime, pid) {
        return;
    }
    match context.number {
        SYS_READ if context.args[0] == 0 => {
            libnanami::println!(
                "[alter/linux] read stdin pid={} buf={:#x} len={} ret={}",
                pid,
                context.args[1],
                context.args[2],
                value
            );
        }
        SYS_WAIT4 | SYS_RT_SIGSUSPEND | SYS_VFORK | SYS_FORK | SYS_CLONE => {
            libnanami::println!(
                "[alter/linux] sync syscall pid={} nr={} a0={:#x} a1={:#x} ret={}",
                pid,
                context.number,
                context.args[0],
                context.args[1],
                value
            );
        }
        _ => {}
    }
}

pub(super) fn trace_critical_action(
    runtime: &Runtime,
    pid: Word,
    context: LinuxSyscallContext,
    action: EmulationAction,
) {
    let value = match action {
        EmulationAction::Return(value) => value,
        EmulationAction::Resume => 0,
        EmulationAction::Park => isize::MIN,
        EmulationAction::Exit(status) => status as isize,
        EmulationAction::Unsupported(_) => -(ENOSYS as isize),
    };
    trace_critical_syscall(runtime, pid, context, value);
}

pub(super) fn syscall_arg_count(number: Word) -> usize {
    match number {
        SYS_GETPID | SYS_GETPPID | SYS_GETUID | SYS_GETEUID | SYS_GETGID | SYS_GETEGID
        | SYS_FORK | SYS_VFORK | SYS_GETTID | SYS_SYNC => 0,
        SYS_CLOSE | SYS_EXIT | SYS_EXIT_GROUP | SYS_PIPE | SYS_GETPGID
        | SYS_FSYNC | SYS_FDATASYNC | SYS_SYNCFS => 1,
        SYS_OPEN | SYS_CREAT | SYS_STAT | SYS_LSTAT | SYS_FSTAT | SYS_ACCESS | SYS_ARCH_PRCTL
        | SYS_DUP | SYS_CLOCK_GETTIME | SYS_SET_TID_ADDRESS | SYS_GETRANDOM | SYS_GETRLIMIT
        | SYS_CHDIR | SYS_MKDIR | SYS_RMDIR | SYS_LINK | SYS_UNLINK | SYS_UTIMES | SYS_DUP2
        | SYS_RENAME | SYS_RT_SIGSUSPEND | SYS_SIGALTSTACK | SYS_PIPE2 | SYS_KILL | SYS_SETPGID
        | SYS_MSYNC | SYS_NANOSLEEP | SYS_FTRUNCATE => 2,
        SYS_READ
        | SYS_WRITE
        | SYS_READV
        | SYS_WRITEV
        | SYS_SENDMSG
        | SYS_RECVMSG
        | SYS_OPENAT
        | SYS_FACCESSAT
        | SYS_IOCTL
        | SYS_LSEEK
        | SYS_POLL
        | SYS_PPOLL
        | SYS_EXECVE
        | SYS_WAIT4
        | SYS_DUP3
        | SYS_GETDENTS64
        | SYS_GETCWD
        | SYS_UNAME
        | SYS_BRK
        | SYS_FCNTL
        | SYS_FUTIMESAT
        | SYS_MKDIRAT
        | SYS_MKNOD
        | SYS_UNLINKAT
        | SYS_READLINK
        | SYS_GETRESUID
        | SYS_GETRESGID
        | SYS_MADVISE
        | SYS_CHOWN
        | SYS_FCHOWN
        | SYS_LCHOWN
        | SYS_SETITIMER
        | SYS_SCHED_GETAFFINITY => 3,
        SYS_MMAP | SYS_SELECT | SYS_PSELECT6 | SYS_PRLIMIT64 | SYS_UTIMENSAT | SYS_RENAMEAT
        | SYS_READLINKAT | SYS_MKNODAT | SYS_FACCESSAT2 | SYS_PREAD64 | SYS_PWRITE64
        | SYS_NEWFSTATAT => 4,
        SYS_CLONE | SYS_STATX | SYS_FCHOWNAT | SYS_LINKAT => 5,
        _ => 6,
    }
}

pub(super) fn syscall_name(number: Word) -> &'static [u8] {
    match number {
        SYS_READ => b"read",
        SYS_PREAD64 => b"pread64",
        SYS_PWRITE64 => b"pwrite64",
        SYS_WRITE => b"write",
        SYS_READV => b"readv",
        SYS_SENDMSG => b"sendmsg",
        SYS_RECVMSG => b"recvmsg",
        SYS_OPEN => b"open",
        SYS_DUP => b"dup",
        SYS_DUP2 => b"dup2",
        SYS_DUP3 => b"dup3",
        SYS_PIPE => b"pipe",
        SYS_PIPE2 => b"pipe2",
        SYS_CREAT => b"creat",
        SYS_CLOSE => b"close",
        SYS_FSYNC => b"fsync",
        SYS_FDATASYNC => b"fdatasync",
        SYS_FTRUNCATE => b"ftruncate",
        SYS_SYNC => b"sync",
        SYS_SYNCFS => b"syncfs",
        SYS_STAT => b"stat",
        SYS_LSTAT => b"lstat",
        SYS_STATX => b"statx",
        SYS_FSTAT => b"fstat",
        SYS_CHOWN => b"chown",
        SYS_FCHOWN => b"fchown",
        SYS_LCHOWN => b"lchown",
        SYS_FCHOWNAT => b"fchownat",
        SYS_POLL => b"poll",
        SYS_LSEEK => b"lseek",
        SYS_MMAP => b"mmap",
        SYS_MPROTECT => b"mprotect",
        SYS_MUNMAP => b"munmap",
        SYS_MSYNC => b"msync",
        SYS_MADVISE => b"madvise",
        SYS_MREMAP => b"mremap",
        SYS_BRK => b"brk",
        SYS_RT_SIGACTION => b"rt_sigaction",
        SYS_RT_SIGPROCMASK => b"rt_sigprocmask",
        SYS_RT_SIGSUSPEND => b"rt_sigsuspend",
        SYS_SIGALTSTACK => b"sigaltstack",
        SYS_IOCTL => b"ioctl",
        SYS_WRITEV => b"writev",
        SYS_NANOSLEEP => b"nanosleep",
        SYS_ACCESS => b"access",
        SYS_SELECT => b"select",
        SYS_GETPID => b"getpid",
        SYS_CLONE => b"clone",
        SYS_FORK => b"fork",
        SYS_VFORK => b"vfork",
        SYS_EXECVE => b"execve",
        SYS_EXIT => b"exit",
        SYS_WAIT4 => b"wait4",
        SYS_UNAME => b"uname",
        SYS_GETCWD => b"getcwd",
        SYS_CHDIR => b"chdir",
        SYS_RENAME => b"rename",
        SYS_LINK => b"link",
        SYS_MKDIR => b"mkdir",
        SYS_RMDIR => b"rmdir",
        SYS_UNLINK => b"unlink",
        SYS_READLINK => b"readlink",
        SYS_GETTIMEOFDAY => b"gettimeofday",
        SYS_SETITIMER => b"setitimer",
        SYS_GETRLIMIT => b"getrlimit",
        SYS_GETUID => b"getuid",
        SYS_GETGID => b"getgid",
        SYS_GETEUID => b"geteuid",
        SYS_GETEGID => b"getegid",
        SYS_GETPPID => b"getppid",
        SYS_SETPGID => b"setpgid",
        SYS_GETPGID => b"getpgid",
        SYS_KILL => b"kill",
        SYS_ARCH_PRCTL => b"arch_prctl",
        SYS_GETTID => b"gettid",
        SYS_SCHED_GETAFFINITY => b"sched_getaffinity",
        SYS_FUTEX => b"futex",
        SYS_GETDENTS64 => b"getdents64",
        SYS_SET_TID_ADDRESS => b"set_tid_address",
        SYS_CLOCK_GETTIME => b"clock_gettime",
        SYS_UTIMES => b"utimes",
        SYS_EXIT_GROUP => b"exit_group",
        SYS_OPENAT => b"openat",
        SYS_MKDIRAT => b"mkdirat",
        SYS_MKNOD => b"mknod",
        SYS_MKNODAT => b"mknodat",
        SYS_FUTIMESAT => b"futimesat",
        SYS_NEWFSTATAT => b"newfstatat",
        SYS_UNLINKAT => b"unlinkat",
        SYS_LINKAT => b"linkat",
        SYS_RENAMEAT => b"renameat",
        SYS_READLINKAT => b"readlinkat",
        SYS_FACCESSAT => b"faccessat",
        SYS_FACCESSAT2 => b"faccessat2",
        SYS_UTIMENSAT => b"utimensat",
        SYS_PSELECT6 => b"pselect6",
        SYS_PPOLL => b"ppoll",
        SYS_SET_ROBUST_LIST => b"set_robust_list",
        SYS_PRLIMIT64 => b"prlimit64",
        SYS_GETRANDOM => b"getrandom",
        SYS_GETRESUID => b"getresuid",
        SYS_GETRESGID => b"getresgid",
        SYS_RSEQ => b"rseq",
        _ => b"syscall",
    }
}

pub(super) fn trace_append_bytes(out: &mut [u8], mut pos: usize, bytes: &[u8]) -> usize {
    let mut i = 0usize;
    while i < bytes.len() && pos < out.len() {
        out[pos] = bytes[i];
        pos += 1;
        i += 1;
    }
    pos
}

pub(super) fn trace_append_decimal(out: &mut [u8], pos: usize, value: Word) -> usize {
    if value == 0 {
        return trace_append_bytes(out, pos, b"0");
    }
    let mut digits = [0u8; 20];
    let mut count = 0usize;
    let mut n = value;
    while n != 0 && count < digits.len() {
        digits[count] = b'0' + (n % 10) as u8;
        n /= 10;
        count += 1;
    }
    let mut out_pos = pos;
    while count != 0 {
        count -= 1;
        out_pos = trace_append_bytes(out, out_pos, &digits[count..count + 1]);
    }
    out_pos
}

pub(super) fn trace_append_isize(out: &mut [u8], pos: usize, value: isize) -> usize {
    if value < 0 {
        let pos = trace_append_bytes(out, pos, b"-");
        trace_append_decimal(out, pos, value.unsigned_abs() as Word)
    } else {
        trace_append_decimal(out, pos, value as Word)
    }
}

pub(super) fn trace_append_hex(out: &mut [u8], pos: usize, value: Word) -> usize {
    let mut pos = trace_append_bytes(out, pos, b"0x");
    let mut shift = (::core::mem::size_of::<Word>() * 8) as isize - 4;
    let mut seen = false;
    while shift >= 0 {
        let nibble = ((value >> shift) & 0xf) as u8;
        if nibble != 0 || seen || shift == 0 {
            seen = true;
            let byte = if nibble < 10 {
                b'0' + nibble
            } else {
                b'a' + (nibble - 10)
            };
            pos = trace_append_bytes(out, pos, &[byte]);
        }
        shift -= 4;
    }
    pos
}

pub(super) fn log_exec_image_entry(runtime: &mut Runtime, pid: Word, entry_point: Word) {
    if read_target_memory(runtime, pid, entry_point, 16).is_err() {
        libnanami::println!(
            "[alter/linux] execve entry read-back failed pid={} entry={:#x}",
            pid,
            entry_point
        );
        return;
    }
    let word0 = read_shm_u64(runtime, 0);
    let word1 = read_shm_u64(runtime, 8);
    libnanami::println!(
        "[alter/linux] execve entry pid={} entry={:#x} bytes={:#018x} {:#018x}",
        pid,
        entry_point,
        word0,
        word1
    );
}

pub(super) fn log_exec_stack(runtime: &mut Runtime, pid: Word, guest_sp: Word) {
    if read_target_memory(runtime, pid, guest_sp, 64).is_err() {
        libnanami::println!(
            "[alter/linux] execve stack read-back failed pid={} sp={:#x}",
            pid,
            guest_sp
        );
        return;
    }
    let argc = read_shm_u64(runtime, 0);
    let argv0 = read_shm_u64(runtime, 8);
    let argv1 = read_shm_u64(runtime, 16);
    let env0 = read_shm_u64(runtime, 24);
    libnanami::println!(
        "[alter/linux] execve stack pid={} sp={:#x} argc={} argv0={:#x} argv1={:#x} env0={:#x}",
        pid,
        guest_sp,
        argc,
        argv0,
        argv1,
        env0
    );
}

pub(super) fn process_diagnostics_enabled(runtime: &Runtime, pid: Word) -> bool {
    runtime
        .managed_process(pid)
        .map(|process| process.diagnostics_enabled)
        .unwrap_or(false)
}
