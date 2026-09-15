use super::{
    bounded_len, read_target_memory, write_target_memory, write_u32, LinuxFile, LinuxFileKind,
    Runtime, Word, EBADF, EFAULT, EINVAL, EMFILE, EPIPE, LINUX_FD_CLOEXEC, LINUX_O_CLOEXEC,
    LINUX_PIPE_BYTES,
};

pub(super) fn sys_pipe_read(
    runtime: &mut Runtime,
    pid: Word,
    pipe_id: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    if user_buffer == 0 && len != 0 {
        return Err(EFAULT);
    }
    let max = bounded_len(runtime, len)? as usize;
    if max == 0 {
        return Ok(0);
    }
    let shm = runtime.posix_shm;
    let pipe = runtime.pipe_mut(pipe_id).ok_or(EBADF)?;
    if pipe.len == 0 {
        return Ok(0);
    }
    let mut done = 0usize;
    while done < max && pipe.len != 0 {
        unsafe {
            ::core::ptr::write((shm + done as Word) as *mut u8, pipe.buffer[pipe.read]);
        }
        pipe.read = (pipe.read + 1) % LINUX_PIPE_BYTES;
        pipe.len -= 1;
        done += 1;
    }
    write_target_memory(runtime, pid, user_buffer, done as Word)?;
    Ok(done as Word)
}

pub(super) fn sys_pipe_write(
    runtime: &mut Runtime,
    pid: Word,
    pipe_id: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    if user_buffer == 0 && len != 0 {
        return Err(EFAULT);
    }
    let max = bounded_len(runtime, len)?;
    if max == 0 {
        return Ok(0);
    }
    read_target_memory(runtime, pid, user_buffer, max)?;
    let shm = runtime.posix_shm;
    let pipe = runtime.pipe_mut(pipe_id).ok_or(EBADF)?;
    if pipe.readers == 0 {
        return Err(EPIPE);
    }
    let mut done = 0usize;
    while done < max as usize && pipe.len < LINUX_PIPE_BYTES {
        let byte = unsafe { ::core::ptr::read((shm + done as Word) as *const u8) };
        pipe.buffer[pipe.write] = byte;
        pipe.write = (pipe.write + 1) % LINUX_PIPE_BYTES;
        pipe.len += 1;
        done += 1;
    }
    Ok(done as Word)
}

pub(super) fn sys_pipe(
    runtime: &mut Runtime,
    pid: Word,
    pipefd_ptr: Word,
    flags: Word,
) -> Result<Word, i32> {
    if pipefd_ptr == 0 {
        return Err(EFAULT);
    }
    if (flags & !LINUX_O_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    let fd_flags = if (flags & LINUX_O_CLOEXEC) != 0 {
        LINUX_FD_CLOEXEC
    } else {
        0
    };
    let pipe_id = runtime.allocate_pipe().ok_or(EMFILE)?;
    let read_fd = match runtime.allocate_linux_file(pid, LinuxFile::pipe_read(pipe_id, fd_flags), 0)
    {
        Some(fd) => fd,
        None => {
            release_pipe_id(runtime, pipe_id);
            return Err(EMFILE);
        }
    };
    let write_fd =
        match runtime.allocate_linux_file(pid, LinuxFile::pipe_write(pipe_id, fd_flags), 0) {
            Some(fd) => fd,
            None => {
                let _ = runtime.clear_linux_file(pid, read_fd);
                release_pipe_id(runtime, pipe_id);
                return Err(EMFILE);
            }
        };
    unsafe {
        write_u32(runtime.posix_shm, read_fd as u32);
        write_u32(runtime.posix_shm + 4, write_fd as u32);
    }
    write_target_memory(runtime, pid, pipefd_ptr, 8)?;
    Ok(0)
}

pub(super) fn release_pipe_file(runtime: &mut Runtime, file: LinuxFile) {
    let Some(pipe) = runtime.pipe_mut(file.posix_fd) else {
        return;
    };
    match file.kind {
        LinuxFileKind::PipeRead => {
            pipe.readers = pipe.readers.saturating_sub(1);
        }
        LinuxFileKind::PipeWrite => {
            pipe.writers = pipe.writers.saturating_sub(1);
        }
        _ => {}
    }
    if pipe.readers == 0 && pipe.writers == 0 {
        release_pipe_id(runtime, file.posix_fd);
    }
}

pub(super) fn release_pipe_id(runtime: &mut Runtime, pipe_id: Word) {
    let index = pipe_id as usize;
    if index < runtime.pipes.len() {
        runtime.pipes[index] = crate::state::LinuxPipe::EMPTY;
    }
}
