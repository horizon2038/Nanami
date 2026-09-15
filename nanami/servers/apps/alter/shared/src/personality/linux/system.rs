use super::{
    bounded_len, map_request_error, map_unit, map_word, posix, read_register_value,
    write_guest_u32, write_guest_u64, write_register_value, write_target_memory, write_u64,
    LinuxSyscallContext, Runtime, Word, ARCH_GET_FS, ARCH_SET_FS, EFAULT, EINVAL, EIO, ESRCH,
    REG_FS_BASE, SYS_PRLIMIT64, UNAME_MACHINE,
};

pub(super) fn sys_uname(runtime: &mut Runtime, pid: Word, user_buffer: Word) -> Result<Word, i32> {
    if user_buffer == 0 {
        return Err(EFAULT);
    }
    unsafe {
        ::core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, 390);
    }
    write_uts_field(runtime.posix_shm, 0, b"Nanami Alter/Linux");
    write_uts_field(runtime.posix_shm, 65, b"nanami");
    write_uts_field(runtime.posix_shm, 130, b"0.1.0");
    write_uts_field(runtime.posix_shm, 195, b"#1 Nanami/A9N");
    write_uts_field(runtime.posix_shm, 260, UNAME_MACHINE);
    write_uts_field(runtime.posix_shm, 325, b"(none)");
    write_target_memory(runtime, pid, user_buffer, 390)?;
    Ok(0)
}

pub(super) fn sys_arch_prctl(
    runtime: &mut Runtime,
    pid: Word,
    code: Word,
    value: Word,
) -> Result<Word, i32> {
    match code {
        ARCH_SET_FS => {
            let pcb = runtime
                .managed_process(pid)
                .map(|process| process.pcb)
                .ok_or(ESRCH)?;
            write_register_value(pcb, REG_FS_BASE, value).map_err(|_| EIO)?;
            if !runtime.set_fs_base(pid, value) {
                return Err(ESRCH);
            }
            Ok(0)
        }
        ARCH_GET_FS => {
            let pcb = runtime
                .managed_process(pid)
                .map(|process| process.pcb)
                .ok_or(ESRCH)?;
            let fs_base = read_register_value(pcb, REG_FS_BASE).unwrap_or_else(|_| {
                runtime
                    .managed_process(pid)
                    .map(|process| process.fs_base)
                    .unwrap_or(0)
            });
            write_guest_u64(runtime, pid, value, fs_base)?;
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}

pub(super) fn sys_getrandom(
    runtime: &mut Runtime,
    pid: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    if user_buffer == 0 && len != 0 {
        return Err(EFAULT);
    }
    let bytes = bounded_len(runtime, len)?;
    let mut i = 0;
    while i < bytes {
        unsafe {
            ::core::ptr::write(
                (runtime.posix_shm + i) as *mut u8,
                (i as u8).wrapping_mul(37),
            );
        }
        i += 1;
    }
    if bytes != 0 {
        write_target_memory(runtime, pid, user_buffer, bytes)?;
    }
    Ok(bytes)
}

pub(super) fn sys_getrlimit(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> Result<Word, i32> {
    let out = if context.number == SYS_PRLIMIT64 {
        context.args[3]
    } else {
        context.args[1]
    };
    if out == 0 {
        return Err(EFAULT);
    }
    unsafe {
        write_u64(runtime.posix_shm, Word::MAX);
        write_u64(runtime.posix_shm + 8, Word::MAX);
    }
    write_target_memory(runtime, pid, out, 16)?;
    Ok(0)
}

pub(super) fn sys_getresid(
    runtime: &mut Runtime,
    pid: Word,
    real_ptr: Word,
    effective_ptr: Word,
    saved_ptr: Word,
    is_user: bool,
) -> Result<Word, i32> {
    let id = if is_user {
        map_word(posix::posix_getuid(runtime.posix_port))?
    } else {
        map_word(posix::posix_getgid(runtime.posix_port))?
    } as u32;
    write_guest_u32(runtime, pid, real_ptr, id)?;
    write_guest_u32(runtime, pid, effective_ptr, id)?;
    write_guest_u32(runtime, pid, saved_ptr, id)?;
    Ok(0)
}

pub(super) fn sys_getpgid(
    runtime: &Runtime,
    current_pid: Word,
    target_pid: Word,
) -> Result<Word, i32> {
    if target_pid == 0 {
        return Ok(current_pid);
    }
    if runtime.managed_process(target_pid).is_some() {
        return Ok(target_pid);
    }
    map_word(posix::posix_getpgid(runtime.posix_port, target_pid))
}

pub(super) fn sys_kill(
    runtime: &mut Runtime,
    _current_pid: Word,
    target_pid: Word,
    signal: Word,
) -> Result<Word, i32> {
    if target_pid == 0 || (target_pid as isize) < 0 {
        return Ok(0);
    }
    if signal == 0 {
        return if runtime.managed_process(target_pid).is_some() {
            Ok(0)
        } else {
            Err(ESRCH)
        };
    }
    if runtime.managed_process(target_pid).is_some() {
        libnanami::request_process_kill(target_pid, signal).map_err(map_request_error)?;
        return Ok(0);
    }
    map_unit(posix::posix_kill(runtime.posix_port, target_pid, signal), 0)
}

pub(super) fn write_uts_field(base: Word, offset: Word, value: &[u8]) {
    let mut i = 0usize;
    while i < value.len() && i < 64 {
        unsafe {
            ::core::ptr::write((base + offset + i as Word) as *mut u8, value[i]);
        }
        i += 1;
    }
}
