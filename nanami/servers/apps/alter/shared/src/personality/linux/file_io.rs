use super::{
    ensure_standard_terminal_fd, map_request_error, network_result_action, pump_input_events,
    read_linux_iovecs, result_to_linux_return, sys_evdev_read, sys_framebuffer_read,
    sys_framebuffer_write, sys_netlink_send, sys_pipe_read, sys_pipe_write, sys_socket_recv,
    sys_socket_send, sys_terminal_read, sys_terminal_read_now, sys_terminal_write,
    sys_virtual_read, sys_virtual_write, terminal_bounded_len, terminal_id_for_pid, vectored,
    write_target_memory_from, EmulationAction, LinuxFileKind, LinuxSyscallContext, Runtime, Word,
    ALTER_IO_OFFSET, EAGAIN, EBADF, EFAULT, EINVAL, EIO, EISDIR, ENOTCONN, EOPNOTSUPP, ESPIPE,
    ESRCH, LINUX_IOV_MAX, LINUX_O_ACCMODE, LINUX_O_NONBLOCK, LINUX_O_RDONLY, LINUX_O_WRONLY,
    LINUX_SOCK_NONBLOCK,
};

pub(super) fn sys_read(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    if user_buffer == 0 && len != 0 {
        return Err(EFAULT);
    }
    ensure_standard_terminal_fd(runtime, pid, fd);
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    match file.kind {
        LinuxFileKind::Terminal => return sys_terminal_read(runtime, pid, user_buffer, len),
        LinuxFileKind::PipeRead => {
            return sys_pipe_read(runtime, pid, file.posix_fd, user_buffer, len);
        }
        LinuxFileKind::PipeWrite => return Err(EBADF),
        LinuxFileKind::SocketUdp | LinuxFileKind::SocketTcp | LinuxFileKind::SocketIcmp => {
            return sys_socket_recv(runtime, pid, fd, user_buffer, len, 0, 0);
        }
        LinuxFileKind::SocketNetlink => return Err(EOPNOTSUPP),
        LinuxFileKind::SocketTcpListener => return Err(ENOTCONN),
        LinuxFileKind::VirtualDirectory => return Err(EISDIR),
        LinuxFileKind::VirtualFile => return sys_virtual_read(runtime, pid, fd, user_buffer, len),
        LinuxFileKind::EvdevKeyboard | LinuxFileKind::EvdevMouse => {
            return sys_evdev_read(runtime, pid, fd, user_buffer, len);
        }
        LinuxFileKind::Framebuffer => {
            return sys_framebuffer_read(runtime, pid, fd, user_buffer, len);
        }
        LinuxFileKind::Posix => {
            if file.resource & LINUX_O_ACCMODE == LINUX_O_WRONLY {
                return Err(EBADF);
            }
        }
        LinuxFileKind::Empty => return Err(EBADF),
    }
    let mut done = 0;
    while done < len {
        let chunk = ::core::cmp::min(len - done, runtime.posix_read_buffer_size().max(1));
        let (bytes, source) = runtime
            .read_posix(file.posix_fd, 0, chunk)
            .map_err(map_request_error)?;
        if bytes > chunk {
            return Err(EIO);
        }
        if bytes == 0 {
            break;
        }
        write_target_memory_from(pid, user_buffer + done, source, bytes)?;
        done += bytes;
    }
    Ok(done)
}

pub(super) fn sys_positioned_io(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    buffer: Word,
    len: Word,
    offset: Word,
    write: bool,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind == LinuxFileKind::VirtualDirectory {
        return Err(EISDIR);
    }
    if file.kind != LinuxFileKind::Posix {
        return Err(ESPIPE);
    }
    let access = file.resource & LINUX_O_ACCMODE;
    if (write && access == LINUX_O_RDONLY) || (!write && access == LINUX_O_WRONLY) {
        return Err(EBADF);
    }
    if offset > isize::MAX as Word {
        return Err(EINVAL);
    }
    if buffer == 0 && len != 0 {
        return Err(EFAULT);
    }
    let len = len.min(isize::MAX as Word);
    buffer.checked_add(len).ok_or(EFAULT)?;
    offset
        .checked_add(len)
        .filter(|end| *end <= isize::MAX as Word)
        .ok_or(EINVAL)?;
    let mut done = 0;
    while done < len {
        let chunk = (len - done).min(if write {
            runtime.posix_write_buffer().1
        } else {
            runtime.posix_read_buffer_size()
        });
        if chunk == 0 {
            return if done != 0 { Ok(done) } else { Err(EIO) };
        }
        let result = if write {
            libnanami::request_process_memory_read(
                pid,
                buffer + done,
                runtime.posix_write_buffer().0,
                chunk,
            )
            .map_err(map_request_error)
            .and_then(|_| {
                runtime
                    .write_posix(file.posix_fd, 0, chunk, Some(offset + done))
                    .map_err(map_request_error)
            })
        } else {
            runtime
                .pread_posix(file.posix_fd, 0, chunk, offset + done)
                .map_err(map_request_error)
                .and_then(|(bytes, source)| {
                    if bytes > chunk {
                        return Err(EIO);
                    }
                    if bytes != 0 {
                        write_target_memory_from(pid, buffer + done, source, bytes)?;
                    }
                    Ok(bytes)
                })
        };
        match result {
            Ok(bytes) if bytes <= chunk => {
                done += bytes;
                if bytes < chunk {
                    break;
                }
            }
            Ok(_) => return if done != 0 { Ok(done) } else { Err(EIO) },
            Err(_) if done != 0 => break,
            Err(errno) => return Err(errno),
        }
    }
    Ok(done)
}

pub(super) fn sys_read_action(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    user_buffer: Word,
    len: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    if user_buffer == 0 && len != 0 {
        return EmulationAction::Return(-(EFAULT as isize));
    }
    ensure_standard_terminal_fd(runtime, pid, fd);
    let Some(file) = runtime.linux_file(pid, fd) else {
        return EmulationAction::Return(-(EBADF as isize));
    };
    if matches!(
        file.kind,
        LinuxFileKind::SocketUdp | LinuxFileKind::SocketTcp | LinuxFileKind::SocketIcmp
    ) {
        let nonblocking = (file.flags & LINUX_SOCK_NONBLOCK) != 0;
        let result = sys_read(runtime, pid, fd, user_buffer, len);
        return network_result_action(runtime, pid, context, result, nonblocking);
    }
    if file.kind != LinuxFileKind::Terminal {
        if matches!(
            file.kind,
            LinuxFileKind::EvdevKeyboard | LinuxFileKind::EvdevMouse
        ) {
            pump_input_events(runtime);
            let result = sys_evdev_read(runtime, pid, fd, user_buffer, len);
            if result != Err(EAGAIN) || (file.flags & LINUX_O_NONBLOCK) != 0 {
                return EmulationAction::Return(result_to_linux_return(result));
            }
            if runtime.park_device_reader(pid, fd, user_buffer, len, context) {
                return EmulationAction::Park;
            }
            return EmulationAction::Return(-(ESRCH as isize));
        }
        return EmulationAction::Return(result_to_linux_return(sys_read(
            runtime,
            pid,
            fd,
            user_buffer,
            len,
        )));
    }

    match sys_terminal_read_now(runtime, pid, user_buffer, len) {
        Ok(Some(bytes)) => EmulationAction::Return(bytes as isize),
        Ok(None) => {
            if runtime.park_terminal_reader(pid, user_buffer, len, context) {
                EmulationAction::Park
            } else {
                EmulationAction::Return(-(ESRCH as isize))
            }
        }
        Err(errno) => EmulationAction::Return(-(errno as isize)),
    }
}

pub(super) fn sys_write(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    if user_buffer == 0 && len != 0 {
        return Err(EFAULT);
    }
    ensure_standard_terminal_fd(runtime, pid, fd);
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    match file.kind {
        LinuxFileKind::Terminal => return sys_terminal_write(runtime, pid, user_buffer, len),
        LinuxFileKind::PipeWrite => {
            return sys_pipe_write(runtime, pid, file.posix_fd, user_buffer, len);
        }
        LinuxFileKind::PipeRead => return Err(EBADF),
        LinuxFileKind::SocketUdp | LinuxFileKind::SocketTcp | LinuxFileKind::SocketIcmp => {
            return sys_socket_send(runtime, pid, fd, user_buffer, len, 0, 0);
        }
        LinuxFileKind::SocketNetlink => {
            return sys_netlink_send(runtime, pid, fd, user_buffer, len);
        }
        LinuxFileKind::SocketTcpListener => return Err(ENOTCONN),
        LinuxFileKind::VirtualDirectory => return Err(EISDIR),
        LinuxFileKind::VirtualFile => return sys_virtual_write(runtime, pid, fd, len),
        LinuxFileKind::EvdevKeyboard | LinuxFileKind::EvdevMouse => return Err(EBADF),
        LinuxFileKind::Framebuffer => {
            return sys_framebuffer_write(runtime, pid, fd, user_buffer, len);
        }
        LinuxFileKind::Posix => {
            if file.resource & LINUX_O_ACCMODE == LINUX_O_RDONLY {
                return Err(EBADF);
            }
        }
        LinuxFileKind::Empty => return Err(EBADF),
    }
    let mut done = 0;
    while done < len {
        let (destination, capacity) = runtime.posix_write_buffer();
        let chunk = (len - done).min(capacity);
        if chunk == 0 {
            return Err(EIO);
        }
        libnanami::request_process_memory_read(pid, user_buffer + done, destination, chunk)
            .map_err(map_request_error)?;
        let written = runtime
            .write_posix(file.posix_fd, 0, chunk, None)
            .map_err(map_request_error)?;
        done += written;
        if written == 0 || written < chunk {
            break;
        }
    }
    Ok(done)
}

pub(super) fn sys_writev(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    iov_ptr: Word,
    iov_count: Word,
) -> Result<Word, i32> {
    let mut bases = [0 as Word; LINUX_IOV_MAX as usize];
    let mut lens = [0 as Word; LINUX_IOV_MAX as usize];
    read_linux_iovecs(runtime, pid, iov_ptr, iov_count, &mut bases, &mut lens)?;

    // Datagram, pipe and device writes retain their individual-write semantics.
    // Only byte streams can share one service write across iovec boundaries.
    if lens[..iov_count as usize].iter().any(|&len| len != 0) {
        ensure_standard_terminal_fd(runtime, pid, fd);
        let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
        let iovecs = bases[..iov_count as usize]
            .iter()
            .copied()
            .zip(lens.iter().copied());
        if file.kind == LinuxFileKind::Posix {
            if file.resource & LINUX_O_ACCMODE == LINUX_O_RDONLY {
                return Err(EBADF);
            }
            let (destination, capacity) = runtime.posix_write_buffer();
            return vectored::writev(
                iovecs,
                capacity,
                |source, offset, len| {
                    libnanami::request_process_memory_read(pid, source, destination + offset, len)
                        .map_err(map_request_error)
                },
                |len| {
                    runtime
                        .write_posix(file.posix_fd, 0, len, None)
                        .map_err(map_request_error)
                },
            );
        }
        if file.kind == LinuxFileKind::Terminal {
            let terminal_id = terminal_id_for_pid(runtime, pid)?;
            let capacity = terminal_bounded_len(runtime, Word::MAX)?;
            return vectored::writev(
                iovecs,
                capacity,
                |source, offset, len| {
                    libnanami::request_process_memory_read(
                        pid,
                        source,
                        runtime.terminal_shm + offset,
                        len,
                    )
                    .map_err(map_request_error)
                },
                |len| {
                    let written = nanami_services::terminal::terminal_write_output(
                        runtime.terminal_port,
                        terminal_id,
                        0,
                        len,
                    )
                    .map_err(map_request_error)?;
                    if written == 0 {
                        Err(EIO)
                    } else {
                        Ok(written)
                    }
                },
            );
        }
    }

    let mut total = 0 as Word;
    let mut i = 0usize;
    while i < iov_count as usize {
        let base = bases[i];
        let len = lens[i];
        if len != 0 {
            match sys_write(runtime, pid, fd, base, len) {
                Ok(written) => {
                    total = total.checked_add(written).ok_or(EINVAL)?;
                    if written < len {
                        break;
                    }
                }
                Err(_) if total != 0 => break,
                Err(errno) => return Err(errno),
            }
        }
        i += 1;
    }
    Ok(total)
}

pub(super) fn sys_readv(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    iov_ptr: Word,
    iov_count: Word,
) -> Result<Word, i32> {
    let mut bases = [0 as Word; LINUX_IOV_MAX as usize];
    let mut lens = [0 as Word; LINUX_IOV_MAX as usize];
    read_linux_iovecs(runtime, pid, iov_ptr, iov_count, &mut bases, &mut lens)?;

    let mut total = 0 as Word;
    let mut i = 0usize;
    while i < iov_count as usize {
        let len = lens[i];
        if len != 0 {
            match sys_read(runtime, pid, fd, bases[i], len) {
                Ok(read) => {
                    total = total.checked_add(read).ok_or(EINVAL)?;
                    if read < len {
                        break;
                    }
                }
                Err(_) if total != 0 => break,
                Err(errno) => return Err(errno),
            }
        }
        i += 1;
    }
    Ok(total)
}

pub(super) fn bounded_len(runtime: &Runtime, len: Word) -> Result<Word, i32> {
    let limit = runtime
        .posix_shm_size
        .saturating_sub(ALTER_IO_OFFSET as Word)
        .max(1);
    Ok(::core::cmp::min(len, limit))
}
