use super::{
    keyboard_event_ready, mouse_event_ready, pump_input_events, read_target_memory,
    write_target_memory, write_u64, LinuxFileKind, Runtime, Word, EFAULT, EINVAL, LINUX_FD_MAX,
    LINUX_PIPE_BYTES, LINUX_POLLFD_BYTES, LINUX_POLLFD_MAX, LINUX_POLLIN, LINUX_POLLNVAL,
    LINUX_POLLOUT,
};

pub(super) fn sys_poll(
    runtime: &mut Runtime,
    pid: Word,
    pollfds: Word,
    nfds: Word,
) -> Result<Word, i32> {
    if pollfds == 0 && nfds != 0 {
        return Err(EFAULT);
    }
    let count = ::core::cmp::min(nfds, LINUX_POLLFD_MAX);
    let bytes = count.checked_mul(LINUX_POLLFD_BYTES).ok_or(EINVAL)?;
    read_target_memory(runtime, pid, pollfds, bytes)?;
    let mut ready = 0;
    let mut index = 0;
    while index < count {
        let entry = runtime.posix_shm + index * LINUX_POLLFD_BYTES;
        let fd = unsafe { ::core::ptr::read_unaligned(entry as *const i32) };
        let events = unsafe { ::core::ptr::read_unaligned((entry + 4) as *const i16) };
        let revents = linux_poll_revents(runtime, pid, fd, events);
        if revents != 0 {
            ready += 1;
        }
        unsafe {
            ::core::ptr::write_unaligned((entry + 6) as *mut i16, revents);
        }
        index += 1;
    }
    write_target_memory(runtime, pid, pollfds, bytes)?;
    Ok(ready)
}

pub(super) fn linux_poll_revents(runtime: &mut Runtime, pid: Word, fd: i32, events: i16) -> i16 {
    if fd < 0 {
        return LINUX_POLLNVAL;
    }
    let Some(file) = runtime.linux_file(pid, fd as Word) else {
        return LINUX_POLLNVAL;
    };
    match file.kind {
        LinuxFileKind::Terminal => {
            let mut revents = 0;
            if (events & LINUX_POLLIN) != 0 {
                revents |= LINUX_POLLIN;
            }
            if (events & LINUX_POLLOUT) != 0 {
                revents |= LINUX_POLLOUT;
            }
            revents
        }
        LinuxFileKind::Posix => {
            if (events & LINUX_POLLIN) != 0 {
                LINUX_POLLIN
            } else if (events & LINUX_POLLOUT) != 0 {
                LINUX_POLLOUT
            } else {
                0
            }
        }
        LinuxFileKind::PipeRead => {
            let pipe = runtime.pipe(file.posix_fd);
            if (events & LINUX_POLLIN) != 0
                && pipe
                    .map(|pipe| pipe.len != 0 || pipe.writers == 0)
                    .unwrap_or(false)
            {
                LINUX_POLLIN
            } else {
                0
            }
        }
        LinuxFileKind::PipeWrite => {
            let pipe = runtime.pipe(file.posix_fd);
            if (events & LINUX_POLLOUT) != 0
                && pipe
                    .map(|pipe| pipe.readers != 0 && pipe.len < LINUX_PIPE_BYTES)
                    .unwrap_or(false)
            {
                LINUX_POLLOUT
            } else {
                0
            }
        }
        LinuxFileKind::SocketUdp
        | LinuxFileKind::SocketTcp
        | LinuxFileKind::SocketTcpListener
        | LinuxFileKind::SocketIcmp
        | LinuxFileKind::SocketNetlink => {
            let mut revents = 0;
            if (events & LINUX_POLLIN) != 0 {
                revents |= LINUX_POLLIN;
            }
            if (events & LINUX_POLLOUT) != 0 && file.kind != LinuxFileKind::SocketTcpListener {
                revents |= LINUX_POLLOUT;
            }
            revents
        }
        LinuxFileKind::VirtualDirectory | LinuxFileKind::VirtualFile => {
            let mut revents = 0;
            if (events & LINUX_POLLIN) != 0 {
                revents |= LINUX_POLLIN;
            }
            if (events & LINUX_POLLOUT) != 0 && file.kind == LinuxFileKind::VirtualFile {
                revents |= LINUX_POLLOUT;
            }
            revents
        }
        LinuxFileKind::EvdevKeyboard | LinuxFileKind::EvdevMouse => {
            pump_input_events(runtime);
            let session_id = file.resource >> 32;
            let ready = match file.kind {
                LinuxFileKind::EvdevKeyboard => keyboard_event_ready(runtime, session_id),
                LinuxFileKind::EvdevMouse => mouse_event_ready(runtime, session_id),
                _ => false,
            };
            if ready && (events & LINUX_POLLIN) != 0 {
                LINUX_POLLIN
            } else {
                0
            }
        }
        LinuxFileKind::Framebuffer => events & (LINUX_POLLIN | LINUX_POLLOUT),
        LinuxFileKind::Empty => LINUX_POLLNVAL,
    }
}

pub(super) fn sys_select(
    runtime: &mut Runtime,
    pid: Word,
    nfds: Word,
    readfds: Word,
    writefds: Word,
) -> Result<Word, i32> {
    if nfds == 0 {
        return Ok(0);
    }
    let mut ready = 0;
    if readfds != 0 {
        let mut bits = read_fdset_word(runtime, pid, readfds)?;
        let readable = readable_fdset_mask(runtime, pid, bits);
        bits = readable;
        if readable != 0 {
            ready += count_low_fd_bits(readable);
        }
        write_fdset_word(runtime, pid, readfds, bits)?;
    }
    if writefds != 0 {
        let requested = read_fdset_word(runtime, pid, writefds)?;
        let writable = writable_fdset_mask(runtime, pid, requested);
        if writable != 0 {
            ready += count_low_fd_bits(writable);
        }
        write_fdset_word(runtime, pid, writefds, writable)?;
    }
    Ok(ready)
}

pub(super) fn read_fdset_word(
    runtime: &mut Runtime,
    pid: Word,
    user_ptr: Word,
) -> Result<Word, i32> {
    read_target_memory(runtime, pid, user_ptr, 8)?;
    Ok(unsafe { ::core::ptr::read_unaligned(runtime.posix_shm as *const Word) })
}

pub(super) fn write_fdset_word(
    runtime: &mut Runtime,
    pid: Word,
    user_ptr: Word,
    value: Word,
) -> Result<(), i32> {
    unsafe {
        write_u64(runtime.posix_shm, value);
    }
    write_target_memory(runtime, pid, user_ptr, 8)
}

pub(super) fn count_low_fd_bits(bits: Word) -> Word {
    let mut count = 0;
    let mut bit = 0;
    while bit < 64 {
        if (bits & (1usize << bit)) != 0 {
            count += 1;
        }
        bit += 1;
    }
    count
}

pub(super) fn readable_fdset_mask(runtime: &mut Runtime, pid: Word, requested: Word) -> Word {
    let mut out = 0;
    let mut fd = 0usize;
    while fd < LINUX_FD_MAX && fd < 64 {
        let bit = 1usize << fd;
        if (requested & bit) != 0 {
            if let Some(file) = runtime.linux_file(pid, fd as Word) {
                match file.kind {
                    LinuxFileKind::Terminal => {
                        out |= bit;
                    }
                    LinuxFileKind::PipeRead => {
                        if runtime
                            .pipe(file.posix_fd)
                            .map(|pipe| pipe.len != 0 || pipe.writers == 0)
                            .unwrap_or(false)
                        {
                            out |= bit;
                        }
                    }
                    LinuxFileKind::Posix => {
                        out |= bit;
                    }
                    LinuxFileKind::VirtualDirectory
                    | LinuxFileKind::VirtualFile
                    | LinuxFileKind::Framebuffer => {
                        out |= bit;
                    }
                    LinuxFileKind::EvdevKeyboard => {
                        pump_input_events(runtime);
                        if keyboard_event_ready(runtime, file.resource >> 32) {
                            out |= bit;
                        }
                    }
                    LinuxFileKind::EvdevMouse => {
                        pump_input_events(runtime);
                        if mouse_event_ready(runtime, file.resource >> 32) {
                            out |= bit;
                        }
                    }
                    LinuxFileKind::SocketUdp
                    | LinuxFileKind::SocketTcp
                    | LinuxFileKind::SocketTcpListener
                    | LinuxFileKind::SocketIcmp
                    | LinuxFileKind::SocketNetlink => {
                        out |= bit;
                    }
                    LinuxFileKind::PipeWrite => {}
                    LinuxFileKind::Empty => {}
                }
            }
        }
        fd += 1;
    }
    out
}

pub(super) fn writable_fdset_mask(runtime: &Runtime, pid: Word, requested: Word) -> Word {
    let mut out = 0;
    let mut fd = 0usize;
    while fd < LINUX_FD_MAX && fd < 64 {
        let bit = 1usize << fd;
        if (requested & bit) != 0 {
            if let Some(file) = runtime.linux_file(pid, fd as Word) {
                match file.kind {
                    LinuxFileKind::PipeWrite => {
                        if runtime
                            .pipe(file.posix_fd)
                            .map(|pipe| pipe.readers != 0 && pipe.len < LINUX_PIPE_BYTES)
                            .unwrap_or(false)
                        {
                            out |= bit;
                        }
                    }
                    LinuxFileKind::Empty | LinuxFileKind::PipeRead => {}
                    LinuxFileKind::Terminal | LinuxFileKind::Posix => {
                        out |= bit;
                    }
                    LinuxFileKind::VirtualFile | LinuxFileKind::Framebuffer => {
                        out |= bit;
                    }
                    LinuxFileKind::VirtualDirectory
                    | LinuxFileKind::EvdevKeyboard
                    | LinuxFileKind::EvdevMouse => {}
                    LinuxFileKind::SocketUdp
                    | LinuxFileKind::SocketTcp
                    | LinuxFileKind::SocketIcmp
                    | LinuxFileKind::SocketNetlink => {
                        out |= bit;
                    }
                    LinuxFileKind::SocketTcpListener => {}
                }
            }
        }
        fd += 1;
    }
    out
}
