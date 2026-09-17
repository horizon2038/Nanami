use super::{
    cleanup_graphics_for_process, close_socket_file, is_at_fdcwd, map_path_request_error,
    map_request_error, map_word, open_virtual_path, path_is_absolute, posix, read_c_string,
    release_pipe_file, resolve_current_shm_path, resolve_path, translate_guest_path_for_vfs,
    virtual_node_stat, LinuxFile, LinuxFileKind, Runtime, Word, EBADF, EINVAL, EMFILE, ENOSYS,
    ESPIPE, ESRCH, LINUX_FD_CLOEXEC, LINUX_FD_MAX, LINUX_F_DUPFD, LINUX_F_DUPFD_CLOEXEC,
    LINUX_F_GETFD, LINUX_F_GETFL, LINUX_F_SETFD, LINUX_F_SETFL, LINUX_O_APPEND, LINUX_O_CLOEXEC,
    LINUX_O_CREAT, LINUX_O_DIRECTORY, LINUX_O_LARGEFILE, LINUX_O_NONBLOCK, LINUX_O_TRUNC,
    LINUX_SOCK_NONBLOCK, LINUX_O_DSYNC, LINUX_O_SYNC,
};

pub(super) fn sys_open(
    runtime: &mut Runtime,
    pid: Word,
    path_ptr: Word,
    linux_flags: Word,
) -> Result<Word, i32> {
    let len = resolve_path(runtime, pid, path_ptr)?;
    if let Some(fd) = open_virtual_path(runtime, pid, len, linux_flags)? {
        return Ok(fd);
    }
    let flags = translate_open_flags(linux_flags);
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    let fd =
        posix::posix_open(runtime.posix_port, 0, vfs_len, flags).map_err(map_path_request_error)?;
    let fd_flags = if (linux_flags & LINUX_O_CLOEXEC) != 0 {
        LINUX_FD_CLOEXEC
    } else {
        0
    };
    runtime
        .allocate_linux_file(pid, LinuxFile::posix(fd, fd_flags, linux_flags), 0)
        .ok_or(EMFILE)
}

pub(super) fn sys_openat(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    path_ptr: Word,
    linux_flags: Word,
) -> Result<Word, i32> {
    let raw_len = read_c_string(runtime, pid, path_ptr)?;
    if !path_is_absolute(runtime.posix_shm, raw_len) && !is_at_fdcwd(dirfd) {
        return Err(ENOSYS);
    }
    let len = resolve_current_shm_path(runtime, pid, raw_len)?;
    if let Some(fd) = open_virtual_path(runtime, pid, len, linux_flags)? {
        return Ok(fd);
    }
    let flags = translate_open_flags(linux_flags);
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    let fd =
        posix::posix_open(runtime.posix_port, 0, vfs_len, flags).map_err(map_path_request_error)?;
    let fd_flags = if (linux_flags & LINUX_O_CLOEXEC) != 0 {
        LINUX_FD_CLOEXEC
    } else {
        0
    };
    runtime
        .allocate_linux_file(pid, LinuxFile::posix(fd, fd_flags, linux_flags), 0)
        .ok_or(EMFILE)
}

pub(super) fn sys_close(runtime: &mut Runtime, pid: Word, fd: Word) -> Result<Word, i32> {
    let file = runtime.clear_linux_file(pid, fd).ok_or(EBADF)?;
    if fd <= 2 {
        libnanami::println!(
            "[alter/linux] close stdio pid={} fd={} kind={}",
            pid,
            fd,
            linux_file_kind_name(file.kind)
        );
    }
    match file.kind {
        LinuxFileKind::Posix => {
            posix::posix_close(runtime.posix_port, file.posix_fd).map_err(map_request_error)?;
        }
        LinuxFileKind::PipeRead | LinuxFileKind::PipeWrite => release_pipe_file(runtime, file),
        LinuxFileKind::SocketUdp
        | LinuxFileKind::SocketTcp
        | LinuxFileKind::SocketTcpListener
        | LinuxFileKind::SocketIcmp
        | LinuxFileKind::SocketNetlink => close_socket_file(runtime, file),
        LinuxFileKind::Terminal
        | LinuxFileKind::VirtualDirectory
        | LinuxFileKind::VirtualFile
        | LinuxFileKind::EvdevKeyboard
        | LinuxFileKind::EvdevMouse
        | LinuxFileKind::Framebuffer
        | LinuxFileKind::Empty => {}
    }
    Ok(0)
}

pub(super) fn sys_dup(runtime: &mut Runtime, pid: Word, old_fd: Word) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, old_fd).ok_or(EBADF)?;
    let mut new_file = duplicate_linux_file(runtime, file)?;
    new_file.flags &= !LINUX_FD_CLOEXEC;
    runtime.allocate_linux_file(pid, new_file, 0).ok_or(EMFILE)
}

pub(super) fn sys_dup2(
    runtime: &mut Runtime,
    pid: Word,
    old_fd: Word,
    new_fd: Word,
) -> Result<Word, i32> {
    if new_fd as usize >= LINUX_FD_MAX {
        return Err(EBADF);
    }
    let file = runtime.linux_file(pid, old_fd).ok_or(EBADF)?;
    if old_fd == new_fd {
        return Ok(new_fd);
    }
    close_linux_fd(runtime, pid, new_fd)?;
    let mut new_file = duplicate_linux_file(runtime, file)?;
    new_file.flags &= !LINUX_FD_CLOEXEC;
    if runtime.set_linux_file(pid, new_fd, new_file) {
        if new_fd <= 2 {
            libnanami::println!(
                "[alter/linux] dup2 stdio pid={} old_fd={} new_fd={} kind={}",
                pid,
                old_fd,
                new_fd,
                linux_file_kind_name(new_file.kind)
            );
        }
        Ok(new_fd)
    } else {
        if new_file.kind == LinuxFileKind::Posix {
            let _ = posix::posix_close(runtime.posix_port, new_file.posix_fd);
        }
        Err(EBADF)
    }
}

pub(super) fn sys_dup3(
    runtime: &mut Runtime,
    pid: Word,
    old_fd: Word,
    new_fd: Word,
    flags: Word,
) -> Result<Word, i32> {
    if old_fd == new_fd {
        return Err(EINVAL);
    }
    if (flags & !LINUX_O_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    let fd = sys_dup2(runtime, pid, old_fd, new_fd)?;
    if (flags & LINUX_O_CLOEXEC) != 0 {
        let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
        file.flags |= LINUX_FD_CLOEXEC;
        if !runtime.set_linux_file(pid, fd, file) {
            return Err(EBADF);
        }
    }
    Ok(fd)
}

pub(super) fn sys_lseek(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    offset: Word,
    whence: Word,
) -> Result<Word, i32> {
    let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind == LinuxFileKind::PipeRead
        || file.kind == LinuxFileKind::PipeWrite
        || file.kind == LinuxFileKind::Terminal
    {
        return Err(ESPIPE);
    }
    if matches!(
        file.kind,
        LinuxFileKind::VirtualFile | LinuxFileKind::Framebuffer
    ) {
        let end = virtual_node_stat(runtime, pid, file)?.1;
        let next = match whence {
            0 => offset,
            1 => file.offset.checked_add(offset).ok_or(EINVAL)?,
            2 => end.checked_add(offset).ok_or(EINVAL)?,
            _ => return Err(EINVAL),
        };
        file.offset = next;
        if !runtime.set_linux_file(pid, fd, file) {
            return Err(EBADF);
        }
        return Ok(next);
    }
    if matches!(
        file.kind,
        LinuxFileKind::VirtualDirectory | LinuxFileKind::EvdevKeyboard | LinuxFileKind::EvdevMouse
    ) {
        return Err(ESPIPE);
    }
    if file.kind != LinuxFileKind::Posix {
        return Err(EBADF);
    }
    map_word(posix::posix_seek(
        runtime.posix_port,
        file.posix_fd,
        offset,
        whence,
    ))
}

pub(super) fn sys_fcntl(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    command: Word,
    argument: Word,
) -> Result<Word, i32> {
    match command {
        LINUX_F_DUPFD | LINUX_F_DUPFD_CLOEXEC => {
            let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
            let mut new_file = duplicate_linux_file(runtime, file)?;
            new_file.flags = if command == LINUX_F_DUPFD_CLOEXEC {
                new_file.flags | LINUX_FD_CLOEXEC
            } else {
                new_file.flags & !LINUX_FD_CLOEXEC
            };
            runtime
                .allocate_linux_file(pid, new_file, argument)
                .ok_or(EMFILE)
        }
        LINUX_F_GETFD => runtime
            .linux_file(pid, fd)
            .map(|file| file.flags & LINUX_FD_CLOEXEC)
            .ok_or(EBADF),
        LINUX_F_SETFD => {
            let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
            file.flags = (file.flags & !LINUX_FD_CLOEXEC) | (argument & LINUX_FD_CLOEXEC);
            if runtime.set_linux_file(pid, fd, file) {
                Ok(0)
            } else {
                Err(EBADF)
            }
        }
        LINUX_F_GETFL => {
            let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
            if file.kind != LinuxFileKind::Posix {
                return Ok(file.flags & !LINUX_FD_CLOEXEC);
            }
            let flags = posix::posix_fcntl_status_flags(runtime.posix_port, file.posix_fd, None)
                .map_err(map_request_error)?;
            Ok((file.resource
                & !(LINUX_O_CREAT
                    | LINUX_O_TRUNC
                    | LINUX_O_CLOEXEC
                    | LINUX_O_APPEND
                    | LINUX_O_NONBLOCK))
                | if flags & posix::POSIX_O_APPEND != 0 {
                    LINUX_O_APPEND
                } else {
                    0
                }
                | if flags & posix::POSIX_O_NONBLOCK != 0 {
                    LINUX_O_NONBLOCK
                } else {
                    0
                })
        }
        LINUX_F_SETFL => {
            let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
            if file.kind == LinuxFileKind::Posix {
                return posix::posix_fcntl_status_flags(
                    runtime.posix_port,
                    file.posix_fd,
                    Some(translate_open_flags(argument)),
                )
                .map_err(map_request_error);
            }
            file.flags = (file.flags & LINUX_FD_CLOEXEC) | (argument & LINUX_SOCK_NONBLOCK);
            if runtime.set_linux_file(pid, fd, file) {
                Ok(0)
            } else {
                Err(EBADF)
            }
        }
        _ => Ok(0),
    }
}

pub(super) fn translate_open_flags(flags: Word) -> Word {
    let mut out = 0;
    let _ignored = flags & LINUX_O_LARGEFILE;
    if (flags & LINUX_O_CREAT) != 0 {
        out |= posix::POSIX_O_CREAT;
    }
    if (flags & LINUX_O_TRUNC) != 0 {
        out |= posix::POSIX_O_TRUNC;
    }
    if (flags & LINUX_O_DIRECTORY) != 0 {
        out |= posix::POSIX_O_DIRECTORY;
    }
    if flags & LINUX_O_APPEND != 0 {
        out |= posix::POSIX_O_APPEND;
    }
    if flags & LINUX_O_NONBLOCK != 0 {
        out |= posix::POSIX_O_NONBLOCK;
    }
    if flags & (LINUX_O_SYNC | LINUX_O_DSYNC) != 0 {
        out |= posix::POSIX_O_SYNC;
    }
    out
}

pub(super) fn linux_file_kind_name(kind: LinuxFileKind) -> &'static str {
    match kind {
        LinuxFileKind::Empty => "empty",
        LinuxFileKind::Posix => "posix",
        LinuxFileKind::Terminal => "terminal",
        LinuxFileKind::PipeRead => "pipe-read",
        LinuxFileKind::PipeWrite => "pipe-write",
        LinuxFileKind::SocketUdp => "socket-udp",
        LinuxFileKind::SocketTcp => "socket-tcp",
        LinuxFileKind::SocketTcpListener => "socket-tcp-listener",
        LinuxFileKind::SocketIcmp => "socket-icmp",
        LinuxFileKind::SocketNetlink => "socket-netlink",
        LinuxFileKind::VirtualDirectory => "virtual-directory",
        LinuxFileKind::VirtualFile => "virtual-file",
        LinuxFileKind::EvdevKeyboard => "evdev-keyboard",
        LinuxFileKind::EvdevMouse => "evdev-mouse",
        LinuxFileKind::Framebuffer => "framebuffer",
    }
}

pub(super) fn duplicate_linux_file(
    runtime: &mut Runtime,
    file: LinuxFile,
) -> Result<LinuxFile, i32> {
    match file.kind {
        LinuxFileKind::Empty => Err(EBADF),
        LinuxFileKind::Terminal
        | LinuxFileKind::VirtualDirectory
        | LinuxFileKind::VirtualFile
        | LinuxFileKind::EvdevKeyboard
        | LinuxFileKind::EvdevMouse
        | LinuxFileKind::Framebuffer => Ok(file),
        LinuxFileKind::PipeRead => {
            let pipe = runtime.pipe_mut(file.posix_fd).ok_or(EBADF)?;
            pipe.readers = pipe.readers.saturating_add(1);
            Ok(file)
        }
        LinuxFileKind::PipeWrite => {
            let pipe = runtime.pipe_mut(file.posix_fd).ok_or(EBADF)?;
            pipe.writers = pipe.writers.saturating_add(1);
            Ok(file)
        }
        LinuxFileKind::SocketUdp
        | LinuxFileKind::SocketTcp
        | LinuxFileKind::SocketTcpListener
        | LinuxFileKind::SocketIcmp
        | LinuxFileKind::SocketNetlink => Ok(file),
        LinuxFileKind::Posix => {
            let fd = duplicate_posix_backend_fd(runtime, file.posix_fd)?;
            Ok(LinuxFile::posix(fd, file.flags, file.resource))
        }
    }
}

pub(super) fn duplicate_posix_backend_fd(runtime: &mut Runtime, old_fd: Word) -> Result<Word, i32> {
    let mut low_fds = [usize::MAX as Word; 3];
    let mut low_count = 0usize;
    loop {
        let fd = posix::posix_dup(runtime.posix_port, old_fd).map_err(map_request_error)?;
        if fd >= 3 {
            let mut i = 0usize;
            while i < low_count {
                let _ = posix::posix_close(runtime.posix_port, low_fds[i]);
                i += 1;
            }
            return Ok(fd);
        }
        if low_count >= low_fds.len() {
            let _ = posix::posix_close(runtime.posix_port, fd);
            let mut i = 0usize;
            while i < low_count {
                let _ = posix::posix_close(runtime.posix_port, low_fds[i]);
                i += 1;
            }
            return Err(EMFILE);
        }
        low_fds[low_count] = fd;
        low_count += 1;
    }
}

pub(super) fn close_linux_fd(runtime: &mut Runtime, pid: Word, fd: Word) -> Result<(), i32> {
    if let Some(file) = runtime.clear_linux_file(pid, fd) {
        match file.kind {
            LinuxFileKind::Posix => {
                posix::posix_close(runtime.posix_port, file.posix_fd).map_err(map_request_error)?;
            }
            LinuxFileKind::PipeRead | LinuxFileKind::PipeWrite => release_pipe_file(runtime, file),
            LinuxFileKind::SocketUdp
            | LinuxFileKind::SocketTcp
            | LinuxFileKind::SocketTcpListener
            | LinuxFileKind::SocketIcmp
            | LinuxFileKind::SocketNetlink => close_socket_file(runtime, file),
            LinuxFileKind::Terminal
            | LinuxFileKind::VirtualDirectory
            | LinuxFileKind::VirtualFile
            | LinuxFileKind::EvdevKeyboard
            | LinuxFileKind::EvdevMouse
            | LinuxFileKind::Framebuffer
            | LinuxFileKind::Empty => {}
        }
    }
    Ok(())
}

pub fn close_process_files(runtime: &mut Runtime, pid: Word) {
    let mut fd = 0usize;
    while fd < LINUX_FD_MAX {
        let _ = close_linux_fd(runtime, pid, fd as Word);
        fd += 1;
    }
    cleanup_graphics_for_process(runtime, pid);
}

pub(super) fn close_cloexec_files(runtime: &mut Runtime, pid: Word) {
    let mut fd = 0usize;
    while fd < LINUX_FD_MAX {
        if let Some(file) = runtime.linux_file(pid, fd as Word) {
            if (file.flags & LINUX_FD_CLOEXEC) != 0 {
                let _ = close_linux_fd(runtime, pid, fd as Word);
            }
        }
        fd += 1;
    }
}

pub(super) fn inherit_linux_files(
    runtime: &mut Runtime,
    parent_pid: Word,
    child_pid: Word,
) -> Result<(), i32> {
    let parent = runtime.managed_process(parent_pid).copied().ok_or(ESRCH)?;
    if !runtime.set_cwd(child_pid, &parent.cwd[..parent.cwd_len]) {
        return Err(EINVAL);
    }
    let mut fd = 0usize;
    while fd < LINUX_FD_MAX {
        let file = parent.files[fd];
        if file.is_open() {
            let child_file = match duplicate_linux_file(runtime, file) {
                Ok(file) => file,
                Err(error) => {
                    close_process_files(runtime, child_pid);
                    return Err(error);
                }
            };
            if !runtime.set_linux_file(child_pid, fd as Word, child_file) {
                if child_file.kind == LinuxFileKind::Posix {
                    let _ = posix::posix_close(runtime.posix_port, child_file.posix_fd);
                }
                close_process_files(runtime, child_pid);
                return Err(EMFILE);
            }
        }
        fd += 1;
    }
    Ok(())
}
