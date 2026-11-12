use super::{
    cleanup_graphics_for_process, close_socket_file, map_request_error,
    map_word, posix, release_pipe_file, translate_open_flags, virtual_node_stat, LinuxFile, LinuxFileKind, Runtime,
    VirtualNode, Word, EBADF, EINVAL, EMFILE, ESPIPE, ESRCH, LINUX_FD_CLOEXEC, LINUX_FD_MAX,
    LINUX_F_DUPFD, LINUX_F_DUPFD_CLOEXEC, LINUX_F_GETFD, LINUX_F_GETFL, LINUX_F_SETFD,
    LINUX_F_SETFL, LINUX_O_APPEND, LINUX_O_CLOEXEC, LINUX_O_CREAT,
    LINUX_O_NONBLOCK, LINUX_O_TRUNC,
    LINUX_SOCK_NONBLOCK,
};

pub(super) fn sys_close(runtime: &mut Runtime, pid: Word, fd: Word) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if fd <= 2 && super::process_trace_enabled(runtime, pid) {
        nanami_runtime::println!(
            "[alter/linux] close stdio pid={} fd={} kind={}",
            pid,
            fd,
            linux_file_kind_name(file.kind)
        );
    }
    close_linux_fd(runtime, pid, fd)?;
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
        if new_fd <= 2 && super::process_trace_enabled(runtime, pid) {
            nanami_runtime::println!(
                "[alter/linux] dup2 stdio pid={} old_fd={} new_fd={} kind={}",
                pid,
                old_fd,
                new_fd,
                linux_file_kind_name(new_file.kind)
            );
        }
        Ok(new_fd)
    } else {
        if new_file.has_posix_backend() {
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
    if file.kind == LinuxFileKind::MemoryFile {
        return super::memfd::seek(runtime, file, offset, whence);
    }
    if file.is_root_directory() {
        if whence > posix::POSIX_SEEK_CUR {
            return Err(EINVAL);
        }
        return map_word(posix::posix_seek(
            runtime.posix_port,
            file.posix_fd,
            offset,
            whence,
        ));
    }
    if matches!(
        file.kind,
        LinuxFileKind::EventFd
            | LinuxFileKind::TimerFd
            | LinuxFileKind::SignalFd
            | LinuxFileKind::Epoll
            | LinuxFileKind::IoUring
            | LinuxFileKind::SocketUnix
            | LinuxFileKind::PtyMaster
            | LinuxFileKind::PtySlave
    ) {
        return Err(ESPIPE);
    }
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
    if file.kind == LinuxFileKind::VirtualDirectory
        && matches!(
            super::procfs::node(file.resource),
            Some(VirtualNode::Proc | VirtualNode::ProcProcess | VirtualNode::ProcTask)
        )
    {
        super::procfs::validate(runtime, pid, file.resource)?;
        let next = match whence {
            0 if (offset as isize) >= 0 => offset,
            1 => file
                .offset
                .checked_add_signed(offset as isize)
                .ok_or(EINVAL)?,
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
    runtime.with_cursor_file_reader(file, pid, "seek", |runtime| {
        map_word(runtime.seek_posix(file.posix_fd, offset, whence))
    })
}

pub(super) fn sys_fcntl(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    command: Word,
    argument: Word,
) -> Result<Word, i32> {
    const F_ADD_SEALS: Word = 1033;
    const F_GET_SEALS: Word = 1034;
    if command == F_ADD_SEALS || command == F_GET_SEALS {
        let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
        if file.kind != LinuxFileKind::MemoryFile {
            return Err(EINVAL);
        }
        return super::memfd::seals(
            runtime,
            file,
            if command == F_ADD_SEALS {
                Some(argument)
            } else {
                None
            },
        );
    }
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
            if super::pty::is_pty(file) {
                return super::pty::flags(runtime, file, None);
            }
            if file.kind == LinuxFileKind::MemoryFile {
                return super::memfd::status_flags(runtime, file, None);
            }
            if file.kind == LinuxFileKind::SocketUnix {
                let nonblocking = if super::unix::nonblocking(runtime, file) {
                    LINUX_O_NONBLOCK
                } else {
                    0
                };
                return Ok(super::LINUX_O_RDWR | nonblocking);
            }
            if matches!(
                file.kind,
                LinuxFileKind::EventFd | LinuxFileKind::TimerFd | LinuxFileKind::SignalFd
            ) {
                let nonblocking = if super::eventfd::nonblocking(runtime, file) {
                    LINUX_O_NONBLOCK
                } else {
                    0
                };
                return Ok(super::LINUX_O_RDWR | nonblocking);
            }
            if file.kind != LinuxFileKind::Posix {
                let access = match file.kind {
                    LinuxFileKind::PipeWrite => super::LINUX_O_WRONLY,
                    // Native terminal descriptors support both directions.
                    // libuv uses this access mode to enable TUI writes too.
                    LinuxFileKind::Terminal | LinuxFileKind::IoUring => super::LINUX_O_RDWR,
                    _ => 0,
                };
                return Ok((file.flags & !LINUX_FD_CLOEXEC) | access);
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
            if super::pty::is_pty(file) {
                super::pty::flags(runtime, file, Some(argument))?;
                return Ok(0);
            }
            if file.kind == LinuxFileKind::MemoryFile {
                super::memfd::status_flags(runtime, file, Some(argument))?;
                return Ok(0);
            }
            if file.kind == LinuxFileKind::SocketUnix {
                let socket = runtime.unix_sockets.get_mut(file.resource).ok_or(EBADF)?;
                socket.nonblocking = argument & LINUX_O_NONBLOCK != 0;
                return Ok(0);
            }
            if matches!(
                file.kind,
                LinuxFileKind::EventFd | LinuxFileKind::TimerFd | LinuxFileKind::SignalFd
            ) {
                super::eventfd::set_nonblocking(runtime, file, argument & LINUX_O_NONBLOCK != 0)?;
                return Ok(0);
            }
            if file.kind == LinuxFileKind::Posix {
                return posix::posix_fcntl_status_flags(
                    runtime.posix_port,
                    file.posix_fd,
                    Some(translate_open_flags(argument)),
                )
                .map_err(map_request_error);
            }
            if matches!(
                file.kind,
                LinuxFileKind::PipeRead | LinuxFileKind::PipeWrite
            ) {
                // dup/fork share each pipe end's open-file status. The read
                // and write ends remain independent descriptions.
                for process in &mut runtime.managed {
                    for alias in process.files.iter_mut() {
                        if alias.kind == file.kind && alias.posix_fd == file.posix_fd {
                            alias.flags =
                                (alias.flags & !LINUX_O_NONBLOCK) | (argument & LINUX_O_NONBLOCK);
                        }
                    }
                }
                runtime
                    .unix_sockets
                    .update_flags(file, LINUX_O_NONBLOCK, argument);
                return Ok(0);
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

pub(super) fn linux_file_kind_name(kind: LinuxFileKind) -> &'static str {
    match kind {
        LinuxFileKind::Empty => "empty",
        LinuxFileKind::Posix => "posix",
        LinuxFileKind::Terminal => "terminal",
        LinuxFileKind::PtyMaster => "pty-master",
        LinuxFileKind::PtySlave => "pty-slave",
        LinuxFileKind::PipeRead => "pipe-read",
        LinuxFileKind::PipeWrite => "pipe-write",
        LinuxFileKind::SocketUdp => "socket-udp",
        LinuxFileKind::SocketTcp => "socket-tcp",
        LinuxFileKind::SocketTcpListener => "socket-tcp-listener",
        LinuxFileKind::SocketIcmp => "socket-icmp",
        LinuxFileKind::SocketNetlink => "socket-netlink",
        LinuxFileKind::SocketUnix => "socket-unix",
        LinuxFileKind::VirtualDirectory => "virtual-directory",
        LinuxFileKind::VirtualFile => "virtual-file",
        LinuxFileKind::EvdevKeyboard => "evdev-keyboard",
        LinuxFileKind::EvdevMouse => "evdev-mouse",
        LinuxFileKind::Framebuffer => "framebuffer",
        LinuxFileKind::MemoryFile => "memfd",
        LinuxFileKind::EventFd => "eventfd",
        LinuxFileKind::TimerFd => "timerfd",
        LinuxFileKind::SignalFd => "signalfd",
        LinuxFileKind::Epoll => "epoll",
        LinuxFileKind::IoUring => "io-uring",
    }
}

pub(super) fn duplicate_linux_file(
    runtime: &mut Runtime,
    file: LinuxFile,
) -> Result<LinuxFile, i32> {
    match file.kind {
        _ if file.has_posix_backend() => {
            let fd = duplicate_posix_backend_fd(runtime, file.posix_fd)?;
            Ok(LinuxFile {
                posix_fd: fd,
                ..file
            })
        }
        LinuxFileKind::Empty => Err(EBADF),
        LinuxFileKind::EventFd
        | LinuxFileKind::PtyMaster
        | LinuxFileKind::PtySlave
        | LinuxFileKind::TimerFd
        | LinuxFileKind::SignalFd
        | LinuxFileKind::MemoryFile
        | LinuxFileKind::Epoll
        | LinuxFileKind::IoUring
        | LinuxFileKind::SocketUnix => Ok(file),
        LinuxFileKind::Terminal
        | LinuxFileKind::VirtualDirectory
        | LinuxFileKind::VirtualFile
        | LinuxFileKind::EvdevKeyboard
        | LinuxFileKind::EvdevMouse
        | LinuxFileKind::Framebuffer
        | LinuxFileKind::Posix => Ok(file),
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
    if let Some(file) = runtime.linux_file(pid, fd) {
        if file.has_posix_backend() {
            // Retain ownership if backend close fails, allowing a later retry.
            posix::posix_close(runtime.posix_port, file.posix_fd).map_err(map_request_error)?;
        }
        // Socket last-reference checks must not count the FD being closed.
        runtime.clear_linux_file(pid, fd);
        super::epoll::forget_closed(runtime, pid, file);
        if !file.has_posix_backend() {
            release_file_description(runtime, file);
        }
    }
    Ok(())
}

// Also used for descriptions held in ancillary queues, without an FD slot.
pub(super) fn release_file_description(runtime: &mut Runtime, file: LinuxFile) {
    if file.has_posix_backend() {
        let _ = posix::posix_close(runtime.posix_port, file.posix_fd);
        return;
    }
    match file.kind {
        LinuxFileKind::SocketUnix => super::unix::collect(runtime),
        LinuxFileKind::PtyMaster | LinuxFileKind::PtySlave => super::pty::collect(runtime, file),
        LinuxFileKind::MemoryFile => super::memfd::collect(runtime, file.resource),
        LinuxFileKind::IoUring => super::io_uring::release(runtime, file),
        LinuxFileKind::EventFd
        | LinuxFileKind::TimerFd
        | LinuxFileKind::SignalFd
        | LinuxFileKind::Epoll => super::eventfd::release(runtime, file),
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
        | LinuxFileKind::Posix
        | LinuxFileKind::Empty => {}
    }
}

pub fn close_process_files(runtime: &mut Runtime, pid: Word) {
    let _ = close_process_files_checked(runtime, pid);
}

pub fn close_process_files_checked(runtime: &mut Runtime, pid: Word) -> Result<(), i32> {
    super::guest_views::invalidate(runtime, pid);
    super::pty::session_exit(runtime, pid);
    super::futex::exit(runtime, pid);
    let mut first_error = None;
    let mut fd = 0usize;
    let shared_files = runtime.files_shared_with_live_task(pid);
    while !shared_files && fd < LINUX_FD_MAX {
        if let Err(error) = close_linux_fd(runtime, pid, fd as Word) {
            first_error.get_or_insert(error);
        }
        fd += 1;
    }
    if !runtime.has_live_threads(pid) {
        cleanup_graphics_for_process(runtime, pid);
    }
    super::sysv_shm::leaving_vm(runtime, pid);
    super::memfd::forget_process(runtime, pid);
    super::sysv_shm::context_exit(runtime, pid);
    first_error.map_or(Ok(()), Err)
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
    // FileTable is already a borrowed handle in a ManagedProcess snapshot.
    // Retain that same handle and the cwd without copying unrelated task state.
    let (cwd, cwd_len, files) = runtime
        .managed_process(parent_pid)
        .map(|parent| (parent.cwd, parent.cwd_len, parent.files))
        .ok_or(ESRCH)?;
    if !runtime.set_cwd(child_pid, &cwd[..cwd_len]) {
        return Err(EINVAL);
    }
    let mut fd = 0usize;
    while fd < LINUX_FD_MAX {
        let file = files[fd];
        if file.is_open() {
            let child_file = match duplicate_linux_file(runtime, file) {
                Ok(file) => file,
                Err(error) => {
                    close_process_files(runtime, child_pid);
                    return Err(error);
                }
            };
            if !runtime.set_linux_file(child_pid, fd as Word, child_file) {
                if child_file.has_posix_backend() {
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
