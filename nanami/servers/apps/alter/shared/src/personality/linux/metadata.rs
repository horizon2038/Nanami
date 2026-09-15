use super::{
    align_up_word, arch, graphics_enabled, is_at_fdcwd, is_linux_virtual_path,
    map_path_request_error, map_request_error, path_is_absolute, posix, read_c_string,
    resolve_current_shm_path, resolve_path, translate_guest_path_for_vfs, virtual_fs,
    write_target_memory, write_u16, write_u32, write_u64, LinuxFile, LinuxFileKind, Runtime,
    VirtualNode, Word, ALTER_FB_BYTES, EBADF, EFAULT, EINVAL, ENOENT, ENOSYS, EOPNOTSUPP, ESRCH,
    LINUX_AT_EMPTY_PATH, LINUX_STATX_BASIC_STATS, LINUX_STATX_SIZE, LINUX_S_IFIFO, LINUX_S_IFSOCK,
    POSIX_FILE_TYPE_PIPE, POSIX_FILE_TYPE_SOCKET, STAT_SIZE,
};

pub(super) fn sys_stat(
    runtime: &mut Runtime,
    pid: Word,
    path_ptr: Word,
    stat_ptr: Word,
) -> Result<Word, i32> {
    let len = resolve_path(runtime, pid, path_ptr)?;
    let stat = stat_current_path(runtime, pid, len)?;
    write_linux_stat(runtime, pid, stat_ptr, stat)?;
    Ok(0)
}

pub(super) fn sys_newfstatat(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    path_ptr: Word,
    stat_ptr: Word,
    flags: Word,
) -> Result<Word, i32> {
    // AT_SYMLINK_NOFOLLOW and AT_NO_AUTOMOUNT are accepted by Linux stat.
    if flags & !(LINUX_AT_EMPTY_PATH | 0x100 | 0x800) != 0 {
        return Err(EINVAL);
    }
    let raw_len = read_c_string(runtime, pid, path_ptr)?;
    if raw_len == 0 {
        if flags & LINUX_AT_EMPTY_PATH == 0 {
            return Err(ENOENT);
        }
        if !is_at_fdcwd(dirfd) {
            return sys_fstat(runtime, pid, dirfd, stat_ptr);
        }
        // An empty path with AT_FDCWD designates the current directory.
        unsafe {
            *(runtime.posix_shm as *mut u8) = b'.';
        }
    } else if !path_is_absolute(runtime.posix_shm, raw_len) && !is_at_fdcwd(dirfd) {
        return Err(EOPNOTSUPP);
    }
    let len = resolve_current_shm_path(runtime, pid, raw_len.max(1))?;
    let stat = stat_current_path(runtime, pid, len)?;
    write_linux_stat(runtime, pid, stat_ptr, stat)?;
    Ok(0)
}

pub(super) fn sys_chown(runtime: &mut Runtime, pid: Word, path_ptr: Word) -> Result<Word, i32> {
    let len = resolve_path(runtime, pid, path_ptr)?;
    let _ = stat_current_path(runtime, pid, len)?;
    Ok(0)
}

pub(super) fn sys_fchown(runtime: &Runtime, pid: Word, fd: Word) -> Result<Word, i32> {
    runtime.linux_file(pid, fd).ok_or(EBADF)?;
    Ok(0)
}

pub(super) fn sys_fchownat(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    path_ptr: Word,
    flags: Word,
) -> Result<Word, i32> {
    let raw_len = read_c_string(runtime, pid, path_ptr)?;
    if raw_len == 0 && (flags & LINUX_AT_EMPTY_PATH) != 0 {
        return sys_fchown(runtime, pid, dirfd);
    }
    if !path_is_absolute(runtime.posix_shm, raw_len) && !is_at_fdcwd(dirfd) {
        return Err(ENOSYS);
    }
    let len = resolve_current_shm_path(runtime, pid, raw_len)?;
    let _ = stat_current_path(runtime, pid, len)?;
    Ok(0)
}

pub(super) fn sys_statx(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    path_ptr: Word,
    flags: Word,
    statx_ptr: Word,
) -> Result<Word, i32> {
    if statx_ptr == 0 {
        return Err(EFAULT);
    }
    let raw_len = read_c_string(runtime, pid, path_ptr)?;
    if raw_len == 0 && (flags & LINUX_AT_EMPTY_PATH) != 0 {
        let file = runtime.linux_file(pid, dirfd).ok_or(EBADF)?;
        let stat = match file.kind {
            LinuxFileKind::Terminal => (0, 0, posix::POSIX_FILE_TYPE_CHAR_DEVICE, 5, 0),
            LinuxFileKind::PipeRead | LinuxFileKind::PipeWrite => {
                (0, 0, POSIX_FILE_TYPE_PIPE, 0, 0)
            }
            LinuxFileKind::Posix => {
                posix::posix_fstat(runtime.posix_port, file.posix_fd).map_err(map_request_error)?
            }
            LinuxFileKind::SocketUdp
            | LinuxFileKind::SocketTcp
            | LinuxFileKind::SocketTcpListener
            | LinuxFileKind::SocketIcmp
            | LinuxFileKind::SocketNetlink => (0, 0, POSIX_FILE_TYPE_SOCKET, 0, 0),
            LinuxFileKind::VirtualDirectory
            | LinuxFileKind::VirtualFile
            | LinuxFileKind::EvdevKeyboard
            | LinuxFileKind::EvdevMouse
            | LinuxFileKind::Framebuffer => virtual_node_stat(runtime, pid, file)?,
            LinuxFileKind::Empty => return Err(EBADF),
        };
        write_linux_statx(runtime, pid, statx_ptr, stat)?;
        return Ok(0);
    }
    if !path_is_absolute(runtime.posix_shm, raw_len) && !is_at_fdcwd(dirfd) {
        return Err(EINVAL);
    }
    let len = resolve_current_shm_path(runtime, pid, raw_len)?;
    let stat = stat_current_path(runtime, pid, len)?;
    write_linux_statx(runtime, pid, statx_ptr, stat)?;
    Ok(0)
}

pub(super) fn stat_current_path(
    runtime: &mut Runtime,
    pid: Word,
    len: Word,
) -> Result<(Word, Word, Word, Word, Word), i32> {
    let path =
        unsafe { ::core::slice::from_raw_parts(runtime.posix_shm as *const u8, len as usize) };
    if let Some(node) = virtual_fs::lookup(path, graphics_enabled(runtime, pid)) {
        return virtual_node_stat_from_node(runtime, pid, node);
    }
    if is_linux_virtual_path(runtime.posix_shm, len) {
        return Err(ENOENT);
    }
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    posix::posix_stat(runtime.posix_port, 0, vfs_len).map_err(map_path_request_error)
}

pub(super) fn sys_fstat(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    stat_ptr: Word,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind == LinuxFileKind::Terminal {
        write_linux_stat(
            runtime,
            pid,
            stat_ptr,
            (0, 0, posix::POSIX_FILE_TYPE_CHAR_DEVICE, 5, 0),
        )?;
        return Ok(0);
    }
    if file.kind == LinuxFileKind::PipeRead || file.kind == LinuxFileKind::PipeWrite {
        write_linux_pipe_stat(runtime, pid, stat_ptr)?;
        return Ok(0);
    }
    if matches!(
        file.kind,
        LinuxFileKind::VirtualDirectory
            | LinuxFileKind::VirtualFile
            | LinuxFileKind::EvdevKeyboard
            | LinuxFileKind::EvdevMouse
            | LinuxFileKind::Framebuffer
    ) {
        let stat = virtual_node_stat(runtime, pid, file)?;
        write_linux_stat(runtime, pid, stat_ptr, stat)?;
        return Ok(0);
    }
    if file.kind != LinuxFileKind::Posix {
        return Err(EBADF);
    }
    let stat = posix::posix_fstat(runtime.posix_port, file.posix_fd).map_err(map_request_error)?;
    write_linux_stat(runtime, pid, stat_ptr, stat)?;
    Ok(0)
}

pub(super) fn virtual_node_stat(
    runtime: &Runtime,
    pid: Word,
    file: LinuxFile,
) -> Result<(Word, Word, Word, Word, Word), i32> {
    let node = if file.kind == LinuxFileKind::Framebuffer {
        VirtualNode::DevFramebuffer
    } else {
        VirtualNode::from_id(file.resource & 0xffff_ffff).ok_or(EBADF)?
    };
    virtual_node_stat_from_node(runtime, pid, node)
}

pub(super) fn virtual_node_stat_from_node(
    runtime: &Runtime,
    pid: Word,
    node: VirtualNode,
) -> Result<(Word, Word, Word, Word, Word), i32> {
    let (size, major, minor) = match node {
        VirtualNode::DevNull => (0, 1, 3),
        VirtualNode::DevZero => (0, 1, 5),
        VirtualNode::DevTty => (0, 5, 0),
        VirtualNode::DevKeyboard => (0, 13, 64),
        VirtualNode::DevMouse => (0, 13, 65),
        VirtualNode::DevFramebuffer => {
            if !graphics_enabled(runtime, pid) {
                return Err(ENOENT);
            }
            (ALTER_FB_BYTES, 29, 0)
        }
        VirtualNode::ProcSelfExe => runtime
            .managed_process(pid)
            .map(|process| (process.image_name_len as Word, 0, 0))
            .ok_or(ESRCH)?,
        _ => (
            virtual_fs::static_file(node)
                .map(|bytes| bytes.len() as Word)
                .unwrap_or(0),
            0,
            0,
        ),
    };
    Ok((
        node.id(),
        size,
        if node.is_directory() {
            posix::POSIX_FILE_TYPE_DIRECTORY
        } else if node.is_regular_file() {
            posix::POSIX_FILE_TYPE_REGULAR
        } else {
            posix::POSIX_FILE_TYPE_CHAR_DEVICE
        },
        major,
        minor,
    ))
}

pub(super) fn write_linux_pipe_stat(
    runtime: &mut Runtime,
    pid: Word,
    user_ptr: Word,
) -> Result<(), i32> {
    if user_ptr == 0 {
        return Err(EFAULT);
    }
    unsafe {
        arch::write_stat_buffer(runtime.posix_shm, 0, 4096, LINUX_S_IFIFO | 0o600, 0);
    }
    write_target_memory(runtime, pid, user_ptr, STAT_SIZE as Word)
}

pub(super) fn write_linux_stat(
    runtime: &mut Runtime,
    pid: Word,
    user_ptr: Word,
    stat: (Word, Word, Word, Word, Word),
) -> Result<(), i32> {
    if user_ptr == 0 {
        return Err(EFAULT);
    }
    let (inode, size, kind, major, minor) = stat;
    let mode = linux_mode_for_kind(kind);
    let rdev = ((major & 0xfff) << 8) | (minor & 0xff);
    unsafe {
        arch::write_stat_buffer(runtime.posix_shm, inode, size, mode, rdev);
    }
    write_target_memory(runtime, pid, user_ptr, STAT_SIZE as Word)
}

pub(super) fn write_linux_statx(
    runtime: &mut Runtime,
    pid: Word,
    user_ptr: Word,
    stat: (Word, Word, Word, Word, Word),
) -> Result<(), i32> {
    let (inode, size, kind, _major, _minor) = stat;
    let mode = linux_mode_for_kind(kind);
    unsafe {
        ::core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, LINUX_STATX_SIZE);
        write_u32(runtime.posix_shm, LINUX_STATX_BASIC_STATS);
        write_u32(runtime.posix_shm + 4, 4096);
        write_u32(runtime.posix_shm + 16, 1);
        write_u32(runtime.posix_shm + 20, 0);
        write_u32(runtime.posix_shm + 24, 0);
        write_u16(runtime.posix_shm + 28, mode as u16);
        write_u64(runtime.posix_shm + 32, inode);
        write_u64(runtime.posix_shm + 40, size);
        write_u64(runtime.posix_shm + 48, align_up_word(size, 512) / 512);
        write_u64(runtime.posix_shm + 56, LINUX_STATX_BASIC_STATS as Word);
    }
    write_target_memory(runtime, pid, user_ptr, LINUX_STATX_SIZE as Word)
}

pub(super) fn linux_mode_for_kind(kind: Word) -> Word {
    match kind {
        posix::POSIX_FILE_TYPE_DIRECTORY => 0o040000 | 0o755,
        posix::POSIX_FILE_TYPE_CHAR_DEVICE => 0o020000 | 0o666,
        posix::POSIX_FILE_TYPE_BLOCK_DEVICE => 0o060000 | 0o666,
        POSIX_FILE_TYPE_PIPE => LINUX_S_IFIFO | 0o600,
        POSIX_FILE_TYPE_SOCKET => LINUX_S_IFSOCK | 0o600,
        _ => 0o100000 | 0o644,
    }
}
