use a9n_abi::CapabilityDescriptor;
use libnanami::{request_exit_with_status, request_heap};

use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

use super::constants::*;

pub fn posix_attach_shared_memory(
    service_port: CapabilityDescriptor,
    size_bytes: Word,
) -> Result<(Word, Word), RequestError> {
    let (status, local_vaddr, mapped_size) = call_port(
        service_port,
        POSIX_REQUEST_CONTROL,
        POSIX_CONTROL_ATTACH_SHARED_MEMORY,
        size_bytes,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((local_vaddr, mapped_size))
}

pub fn posix_getpid(service_port: CapabilityDescriptor) -> Result<Word, RequestError> {
    let (status, pid, _) = call_port(service_port, POSIX_REQUEST_GETPID, 0, 0, 0, 0, 1)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(pid)
}

pub fn posix_getppid(service_port: CapabilityDescriptor) -> Result<Word, RequestError> {
    let (status, pid, _) = call_port(service_port, POSIX_REQUEST_GETPPID, 0, 0, 0, 0, 1)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(pid)
}

pub fn posix_get_native_pid(service_port: CapabilityDescriptor) -> Result<Word, RequestError> {
    let (status, pid, _) = call_port(service_port, POSIX_REQUEST_GET_NATIVE_PID, 0, 0, 0, 0, 1)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(pid)
}

pub fn posix_getuid(service_port: CapabilityDescriptor) -> Result<Word, RequestError> {
    let (status, uid, _) = call_port(service_port, POSIX_REQUEST_GETUID, 0, 0, 0, 0, 1)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(uid)
}

pub fn posix_geteuid(service_port: CapabilityDescriptor) -> Result<Word, RequestError> {
    let (status, uid, _) = call_port(service_port, POSIX_REQUEST_GETEUID, 0, 0, 0, 0, 1)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(uid)
}

pub fn posix_getgid(service_port: CapabilityDescriptor) -> Result<Word, RequestError> {
    let (status, gid, _) = call_port(service_port, POSIX_REQUEST_GETGID, 0, 0, 0, 0, 1)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(gid)
}

pub fn posix_getegid(service_port: CapabilityDescriptor) -> Result<Word, RequestError> {
    let (status, gid, _) = call_port(service_port, POSIX_REQUEST_GETEGID, 0, 0, 0, 0, 1)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(gid)
}

pub fn posix_getpgid(service_port: CapabilityDescriptor, pid: Word) -> Result<Word, RequestError> {
    let (status, pgid, _) = call_port(service_port, POSIX_REQUEST_GETPGID, pid, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(pgid)
}

pub fn posix_getsid(service_port: CapabilityDescriptor, pid: Word) -> Result<Word, RequestError> {
    let (status, sid, _) = call_port(service_port, POSIX_REQUEST_GETSID, pid, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(sid)
}

pub fn posix_setpgid(
    service_port: CapabilityDescriptor,
    pid: Word,
    pgid: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(service_port, POSIX_REQUEST_SETPGID, pid, pgid, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn posix_setsid(service_port: CapabilityDescriptor) -> Result<Word, RequestError> {
    let (status, sid, _) = call_port(service_port, POSIX_REQUEST_SETSID, 0, 0, 0, 0, 1)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(sid)
}

pub fn posix_fork(service_port: CapabilityDescriptor) -> Result<Word, RequestError> {
    let (status, child_pid, _) = call_port(service_port, POSIX_REQUEST_FORK, 0, 0, 0, 0, 1)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(child_pid)
}

pub fn posix_exec(
    service_port: CapabilityDescriptor,
    path_offset: Word,
    path_len: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(service_port, POSIX_REQUEST_EXEC, path_offset, path_len, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn posix_waitpid(
    service_port: CapabilityDescriptor,
    pid: Word,
    options: Word,
) -> Result<(Word, Word), RequestError> {
    let (status, waited_pid, exit_status) = call_port(service_port, POSIX_REQUEST_WAITPID, pid, options, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((waited_pid, exit_status))
}

pub fn posix_kill(
    service_port: CapabilityDescriptor,
    pid: Word,
    signal: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(service_port, POSIX_REQUEST_KILL, pid, signal, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn posix_spawn(
    service_port: CapabilityDescriptor,
    path_offset: Word,
    path_len: Word,
) -> Result<Word, RequestError> {
    let (status, child_pid, _) = call_port(service_port, POSIX_REQUEST_SPAWN, path_offset, path_len, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(child_pid)
}

pub fn posix_exit(status: Word) -> ! {
    let is_ok = if status == 0 { 1 } else { 0 };
    let _ = request_exit_with_status(is_ok, status);
    loop {
        core::hint::spin_loop();
    }
}

pub fn posix_mmap_anonymous(size_bytes: Word) -> Result<(Word, Word), RequestError> {
    request_heap(size_bytes)
}

pub fn posix_getpagesize() -> Word {
    POSIX_PAGE_SIZE
}

pub fn posix_mmap(
    size_bytes: Word,
    protection: Word,
    flags: Word,
) -> Result<(Word, Word), RequestError> {
    let supported_protection = protection & !(POSIX_PROT_READ | POSIX_PROT_WRITE);
    if supported_protection != 0 || (flags & POSIX_MAP_ANONYMOUS) == 0 {
        return Err(RequestError::Unsupported);
    }
    request_heap(size_bytes)
}

pub fn posix_sbrk(increment_bytes: Word) -> Result<(Word, Word), RequestError> {
    request_heap(increment_bytes)
}

pub fn posix_brk(_address: Word) -> Result<(), RequestError> {
    Err(RequestError::Unsupported)
}

pub fn posix_munmap(base: Word, size_bytes: Word) -> Result<(), RequestError> {
    libnanami::request_mapping_release(base, size_bytes)
}

pub fn posix_mprotect(
    _base: Word,
    _size_bytes: Word,
    _protection: Word,
) -> Result<(), RequestError> {
    Err(RequestError::Unsupported)
}

pub fn posix_getcwd(
    service_port: CapabilityDescriptor,
    out_offset: Word,
    max_len: Word,
) -> Result<Word, RequestError> {
    let (status, len, _) = call_port(service_port, POSIX_REQUEST_GETCWD, out_offset, max_len, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(len)
}

pub fn posix_chdir(
    service_port: CapabilityDescriptor,
    path_offset: Word,
    path_len: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(service_port, POSIX_REQUEST_CHDIR, path_offset, path_len, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn posix_open(
    service_port: CapabilityDescriptor,
    path_offset: Word,
    path_len: Word,
    flags: Word,
) -> Result<Word, RequestError> {
    let (status, fd, _) = call_port(service_port, POSIX_REQUEST_OPEN, path_offset, path_len, flags, 0, 4)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(fd)
}

pub fn posix_close(service_port: CapabilityDescriptor, fd: Word) -> Result<(), RequestError> {
    let (status, _, _) = call_port(service_port, POSIX_REQUEST_CLOSE, fd, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn posix_dup(service_port: CapabilityDescriptor, old_fd: Word) -> Result<Word, RequestError> {
    let (status, new_fd, _) = call_port(service_port, POSIX_REQUEST_DUP, old_fd, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(new_fd)
}

pub fn posix_dup2(
    service_port: CapabilityDescriptor,
    old_fd: Word,
    new_fd: Word,
) -> Result<Word, RequestError> {
    let (status, fd, _) = call_port(service_port, POSIX_REQUEST_DUP2, old_fd, new_fd, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(fd)
}

pub fn posix_fcntl_getfd(
    service_port: CapabilityDescriptor,
    fd: Word,
) -> Result<Word, RequestError> {
    let (status, flags, _) = call_port(service_port, POSIX_REQUEST_FCNTL, fd, POSIX_F_GETFD, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(flags)
}

pub fn posix_fcntl_setfd(
    service_port: CapabilityDescriptor,
    fd: Word,
    flags: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(service_port, POSIX_REQUEST_FCNTL, fd, POSIX_F_SETFD, flags, 0, 4)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn posix_getenv(
    service_port: CapabilityDescriptor,
    name_offset: Word,
    name_len: Word,
    out_offset: Word,
    max_len: Word,
) -> Result<Word, RequestError> {
    let (status, value_len, _) = call_port(
        service_port,
        POSIX_REQUEST_GETENV,
        name_offset,
        name_len,
        out_offset,
        max_len,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(value_len)
}

pub fn posix_setenv(
    service_port: CapabilityDescriptor,
    name_offset: Word,
    name_len: Word,
    value_offset: Word,
    value_len: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(
        service_port,
        POSIX_REQUEST_SETENV,
        name_offset,
        name_len,
        value_offset,
        value_len,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn posix_unsetenv(
    service_port: CapabilityDescriptor,
    name_offset: Word,
    name_len: Word,
) -> Result<(), RequestError> {
    let (status, _, _) =
        call_port(service_port, POSIX_REQUEST_UNSETENV, name_offset, name_len, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn posix_env_count(service_port: CapabilityDescriptor) -> Result<Word, RequestError> {
    let (status, count, _) = call_port(service_port, POSIX_REQUEST_ENV_COUNT, 0, 0, 0, 0, 1)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(count)
}

pub fn posix_env_at(
    service_port: CapabilityDescriptor,
    index: Word,
    out_offset: Word,
    max_len: Word,
) -> Result<(Word, Word), RequestError> {
    let (status, name_len, value_len) = call_port(
        service_port,
        POSIX_REQUEST_ENV_AT,
        index,
        out_offset,
        max_len,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((name_len, value_len))
}

pub fn posix_read(
    service_port: CapabilityDescriptor,
    fd: Word,
    out_offset: Word,
    len: Word,
) -> Result<Word, RequestError> {
    let (status, bytes, _) = call_port(service_port, POSIX_REQUEST_READ, fd, out_offset, len, 0, 4)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(bytes)
}

pub fn posix_write(
    service_port: CapabilityDescriptor,
    fd: Word,
    input_offset: Word,
    len: Word,
) -> Result<Word, RequestError> {
    let (status, bytes, _) = call_port(service_port, POSIX_REQUEST_WRITE, fd, input_offset, len, 0, 4)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(bytes)
}

pub fn posix_stat(
    service_port: CapabilityDescriptor,
    path_offset: Word,
    path_len: Word,
) -> Result<(Word, Word, Word, Word, Word), RequestError> {
    let (status, inode, metadata) = call_port(service_port, POSIX_REQUEST_STAT, path_offset, path_len, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    let size = metadata & POSIX_STAT_SIZE_MASK;
    let kind = (metadata >> POSIX_STAT_TYPE_SHIFT) & 0xff;
    let major = (metadata >> POSIX_STAT_MAJOR_SHIFT) & 0xff;
    let minor = (metadata >> POSIX_STAT_MINOR_SHIFT) & 0xffff;
    Ok((inode, size, kind, major, minor))
}

pub fn posix_fstat(
    service_port: CapabilityDescriptor,
    fd: Word,
) -> Result<(Word, Word, Word, Word, Word), RequestError> {
    let (status, inode, metadata) = call_port(service_port, POSIX_REQUEST_FSTAT, fd, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    let size = metadata & POSIX_STAT_SIZE_MASK;
    let kind = (metadata >> POSIX_STAT_TYPE_SHIFT) & 0xff;
    let major = (metadata >> POSIX_STAT_MAJOR_SHIFT) & 0xff;
    let minor = (metadata >> POSIX_STAT_MINOR_SHIFT) & 0xffff;
    Ok((inode, size, kind, major, minor))
}

pub fn posix_read_dir(
    service_port: CapabilityDescriptor,
    fd: Word,
    max_entries: Word,
    out_offset: Word,
) -> Result<(Word, Word), RequestError> {
    let (status, entries, next_index) = call_port(
        service_port,
        POSIX_REQUEST_READ_DIR,
        fd,
        max_entries,
        out_offset,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((entries, next_index))
}

pub fn posix_seek(
    service_port: CapabilityDescriptor,
    fd: Word,
    offset: Word,
    whence: Word,
) -> Result<Word, RequestError> {
    let (status, new_offset, _) = call_port(service_port, POSIX_REQUEST_SEEK, fd, offset, whence, 0, 4)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(new_offset)
}

pub fn posix_mkdir(service_port: CapabilityDescriptor, path_offset: Word, path_len: Word) -> Result<(), RequestError> {
    let (status, _, _) = call_port(service_port, POSIX_REQUEST_MKDIR, path_offset, path_len, 0, 0, 3)?;
    if status != OS_RESPONSE_OK { return Err(RequestError::Status(status)); }
    Ok(())
}

pub fn posix_unlink(service_port: CapabilityDescriptor, path_offset: Word, path_len: Word) -> Result<(), RequestError> {
    let (status, _, _) = call_port(service_port, POSIX_REQUEST_UNLINK, path_offset, path_len, 0, 0, 3)?;
    if status != OS_RESPONSE_OK { return Err(RequestError::Status(status)); }
    Ok(())
}

pub fn posix_rmdir(service_port: CapabilityDescriptor, path_offset: Word, path_len: Word) -> Result<(), RequestError> {
    let (status, _, _) = call_port(service_port, POSIX_REQUEST_RMDIR, path_offset, path_len, 0, 0, 3)?;
    if status != OS_RESPONSE_OK { return Err(RequestError::Status(status)); }
    Ok(())
}

pub fn posix_rename(
    service_port: CapabilityDescriptor,
    old_path_offset: Word,
    old_path_len: Word,
    new_path_offset: Word,
    new_path_len: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(service_port, POSIX_REQUEST_RENAME, old_path_offset, old_path_len, new_path_offset, new_path_len, 5)?;
    if status != OS_RESPONSE_OK { return Err(RequestError::Status(status)); }
    Ok(())
}
