use super::{
    bounded_len, current_virtual_node, is_at_fdcwd, is_linux_virtual_path,
    map_create_request_error, map_link_request_error, map_path_request_error, map_request_error,
    map_unit, move_shm_bytes,
    path_equals, path_is_absolute, posix, read_c_string, reject_virtual_fs_mutation,
    resolve_current_shm_path, resolve_path, translate_guest_path_at,
    translate_guest_path_for_vfs, write_target_memory,
    Runtime, Word, EFAULT, EINVAL, ENAMETOOLONG, ENOENT, ENOSYS,
    ENOTDIR, ERANGE, ESRCH, LINUX_AT_REMOVEDIR, LINUX_AT_SYMLINK_FOLLOW, LINUX_CWD_MAX,
    LINUX_SECOND_PATH_OFFSET, LINUX_S_IFBLK, LINUX_S_IFCHR, LINUX_S_IFMT,
};

pub(super) fn sys_access(runtime: &mut Runtime, pid: Word, path_ptr: Word) -> Result<Word, i32> {
    let len = resolve_path(runtime, pid, path_ptr)?;
    super::stat_current_path(runtime, pid, len).map(|_| 0)
}

pub(super) fn sys_faccessat(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    path_ptr: Word,
    _mode: Word,
    _flags: Word,
) -> Result<Word, i32> {
    let raw_len = read_c_string(runtime, pid, path_ptr)?;
    if !path_is_absolute(runtime.posix_shm, raw_len) && !is_at_fdcwd(dirfd) {
        return Err(ENOSYS);
    }
    let len = resolve_current_shm_path(runtime, pid, raw_len)?;
    super::stat_current_path(runtime, pid, len).map(|_| 0)
}

pub(super) fn sys_chdir(runtime: &mut Runtime, pid: Word, path_ptr: Word) -> Result<Word, i32> {
    let len = resolve_path(runtime, pid, path_ptr)?;
    let mut guest_path = [0u8; LINUX_CWD_MAX];
    if len as usize >= guest_path.len() {
        return Err(ENAMETOOLONG);
    }
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            runtime.posix_shm as *const u8,
            guest_path.as_mut_ptr(),
            len as usize,
        );
    }
    if let Some(node) = current_virtual_node(runtime, pid, len) {
        if !node.is_directory() {
            return Err(ENOTDIR);
        }
        return if runtime.set_cwd(pid, &guest_path[..len as usize]) {
            Ok(0)
        } else {
            Err(EINVAL)
        };
    }
    if is_linux_virtual_path(runtime.posix_shm, len) {
        return Err(ENOENT);
    }
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    let stat = posix::posix_stat(runtime.posix_port, 0, vfs_len).map_err(map_path_request_error)?;
    if stat.2 != posix::POSIX_FILE_TYPE_DIRECTORY {
        return Err(ENOTDIR);
    }
    let path = &guest_path[..len as usize];
    if runtime.set_cwd(pid, path) {
        Ok(0)
    } else {
        Err(EINVAL)
    }
}

pub(super) fn sys_mkdir(runtime: &mut Runtime, pid: Word, path_ptr: Word) -> Result<Word, i32> {
    let len = resolve_path(runtime, pid, path_ptr)?;
    reject_virtual_fs_mutation(runtime.posix_shm, len)?;
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    posix::posix_mkdir(runtime.posix_port, 0, vfs_len)
        .map(|_| 0)
        .map_err(map_create_request_error)
}

pub(super) fn sys_mkdirat(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    path_ptr: Word,
) -> Result<Word, i32> {
    let raw_len = read_c_string(runtime, pid, path_ptr)?;
    if !path_is_absolute(runtime.posix_shm, raw_len) && !is_at_fdcwd(dirfd) {
        return Err(ENOSYS);
    }
    let len = resolve_current_shm_path(runtime, pid, raw_len)?;
    reject_virtual_fs_mutation(runtime.posix_shm, len)?;
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    posix::posix_mkdir(runtime.posix_port, 0, vfs_len)
        .map(|_| 0)
        .map_err(map_create_request_error)
}

pub(super) fn sys_mknod(
    runtime: &mut Runtime,
    pid: Word,
    path_ptr: Word,
    mode: Word,
    dev: Word,
) -> Result<Word, i32> {
    let len = resolve_path(runtime, pid, path_ptr)?;
    mknod_current_path(runtime, pid, len, mode, dev)
}

pub(super) fn sys_mknodat(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    path_ptr: Word,
    mode: Word,
    dev: Word,
) -> Result<Word, i32> {
    let raw_len = read_c_string(runtime, pid, path_ptr)?;
    if !path_is_absolute(runtime.posix_shm, raw_len) && !is_at_fdcwd(dirfd) {
        return Err(ENOSYS);
    }
    let len = resolve_current_shm_path(runtime, pid, raw_len)?;
    mknod_current_path(runtime, pid, len, mode, dev)
}

pub(super) fn mknod_current_path(
    runtime: &mut Runtime,
    pid: Word,
    len: Word,
    mode: Word,
    _dev: Word,
) -> Result<Word, i32> {
    reject_virtual_fs_mutation(runtime.posix_shm, len)?;
    let file_type = mode & LINUX_S_IFMT;
    if file_type == LINUX_S_IFCHR || file_type == LINUX_S_IFBLK {
        return Err(ENOSYS);
    }
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    let fd = posix::posix_open(
        runtime.posix_port,
        0,
        vfs_len,
        posix::POSIX_O_CREAT | posix::POSIX_O_TRUNC,
    )
    .map_err(map_create_request_error)?;
    let _ = posix::posix_close(runtime.posix_port, fd);
    Ok(0)
}

pub(super) fn sys_unlink(runtime: &mut Runtime, pid: Word, path_ptr: Word) -> Result<Word, i32> {
    let len = resolve_path(runtime, pid, path_ptr)?;
    reject_virtual_fs_mutation(runtime.posix_shm, len)?;
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    map_unit(posix::posix_unlink(runtime.posix_port, 0, vfs_len), 0)
}

pub(super) fn sys_link(
    runtime: &mut Runtime,
    pid: Word,
    old_path_ptr: Word,
    new_path_ptr: Word,
) -> Result<Word, i32> {
    let old_len = resolve_path(runtime, pid, old_path_ptr)?;
    reject_virtual_fs_mutation(runtime.posix_shm, old_len)?;
    move_shm_bytes(runtime, 0, LINUX_SECOND_PATH_OFFSET, old_len);
    let old_vfs_len = translate_guest_path_at(runtime, pid, LINUX_SECOND_PATH_OFFSET, old_len)?;
    let new_len = resolve_path(runtime, pid, new_path_ptr)?;
    reject_virtual_fs_mutation(runtime.posix_shm, new_len)?;
    let new_vfs_len = translate_guest_path_for_vfs(runtime, pid, new_len)?;
    posix::posix_link(
        runtime.posix_port,
        LINUX_SECOND_PATH_OFFSET,
        old_vfs_len,
        0,
        new_vfs_len,
    )
    .map(|_| 0)
    .map_err(map_link_request_error)
}

pub(super) fn sys_unlinkat(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    path_ptr: Word,
    flags: Word,
) -> Result<Word, i32> {
    let raw_len = read_c_string(runtime, pid, path_ptr)?;
    if !path_is_absolute(runtime.posix_shm, raw_len) && !is_at_fdcwd(dirfd) {
        return Err(ENOSYS);
    }
    let len = resolve_current_shm_path(runtime, pid, raw_len)?;
    reject_virtual_fs_mutation(runtime.posix_shm, len)?;
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    if (flags & LINUX_AT_REMOVEDIR) != 0 {
        return map_unit(posix::posix_rmdir(runtime.posix_port, 0, vfs_len), 0);
    }
    map_unit(posix::posix_unlink(runtime.posix_port, 0, vfs_len), 0)
}

pub(super) fn sys_linkat(
    runtime: &mut Runtime,
    pid: Word,
    old_dirfd: Word,
    old_path_ptr: Word,
    new_dirfd: Word,
    new_path_ptr: Word,
    flags: Word,
) -> Result<Word, i32> {
    if flags & !LINUX_AT_SYMLINK_FOLLOW != 0 {
        return Err(EINVAL);
    }
    let old_raw_len = read_c_string(runtime, pid, old_path_ptr)?;
    if !path_is_absolute(runtime.posix_shm, old_raw_len) && !is_at_fdcwd(old_dirfd) {
        return Err(ENOSYS);
    }
    let old_len = resolve_current_shm_path(runtime, pid, old_raw_len)?;
    reject_virtual_fs_mutation(runtime.posix_shm, old_len)?;
    move_shm_bytes(runtime, 0, LINUX_SECOND_PATH_OFFSET, old_len);
    let old_vfs_len = translate_guest_path_at(runtime, pid, LINUX_SECOND_PATH_OFFSET, old_len)?;

    let new_raw_len = read_c_string(runtime, pid, new_path_ptr)?;
    if !path_is_absolute(runtime.posix_shm, new_raw_len) && !is_at_fdcwd(new_dirfd) {
        return Err(ENOSYS);
    }
    let new_len = resolve_current_shm_path(runtime, pid, new_raw_len)?;
    reject_virtual_fs_mutation(runtime.posix_shm, new_len)?;
    let new_vfs_len = translate_guest_path_for_vfs(runtime, pid, new_len)?;
    posix::posix_link(
        runtime.posix_port,
        LINUX_SECOND_PATH_OFFSET,
        old_vfs_len,
        0,
        new_vfs_len,
    )
    .map(|_| 0)
    .map_err(map_link_request_error)
}

pub(super) fn sys_rmdir(runtime: &mut Runtime, pid: Word, path_ptr: Word) -> Result<Word, i32> {
    let len = resolve_path(runtime, pid, path_ptr)?;
    reject_virtual_fs_mutation(runtime.posix_shm, len)?;
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    map_unit(posix::posix_rmdir(runtime.posix_port, 0, vfs_len), 0)
}

pub(super) fn sys_rename(
    runtime: &mut Runtime,
    pid: Word,
    old_path_ptr: Word,
    new_path_ptr: Word,
) -> Result<Word, i32> {
    let old_len = resolve_path(runtime, pid, old_path_ptr)?;
    reject_virtual_fs_mutation(runtime.posix_shm, old_len)?;
    move_shm_bytes(runtime, 0, LINUX_SECOND_PATH_OFFSET, old_len);
    let old_vfs_len = translate_guest_path_at(runtime, pid, LINUX_SECOND_PATH_OFFSET, old_len)?;
    let new_len = resolve_path(runtime, pid, new_path_ptr)?;
    reject_virtual_fs_mutation(runtime.posix_shm, new_len)?;
    let new_vfs_len = translate_guest_path_for_vfs(runtime, pid, new_len)?;
    posix::posix_rename(
        runtime.posix_port,
        LINUX_SECOND_PATH_OFFSET,
        old_vfs_len,
        0,
        new_vfs_len,
    )
    .map(|_| 0)
    .map_err(map_request_error)
}

pub(super) fn sys_renameat(
    runtime: &mut Runtime,
    pid: Word,
    old_dirfd: Word,
    old_path_ptr: Word,
    new_dirfd: Word,
    new_path_ptr: Word,
) -> Result<Word, i32> {
    let old_raw_len = read_c_string(runtime, pid, old_path_ptr)?;
    if !path_is_absolute(runtime.posix_shm, old_raw_len) && !is_at_fdcwd(old_dirfd) {
        return Err(ENOSYS);
    }
    let old_len = resolve_current_shm_path(runtime, pid, old_raw_len)?;
    reject_virtual_fs_mutation(runtime.posix_shm, old_len)?;
    move_shm_bytes(runtime, 0, LINUX_SECOND_PATH_OFFSET, old_len);
    let old_vfs_len = translate_guest_path_at(runtime, pid, LINUX_SECOND_PATH_OFFSET, old_len)?;
    let new_raw_len = read_c_string(runtime, pid, new_path_ptr)?;
    if !path_is_absolute(runtime.posix_shm, new_raw_len) && !is_at_fdcwd(new_dirfd) {
        return Err(ENOSYS);
    }
    let new_len = resolve_current_shm_path(runtime, pid, new_raw_len)?;
    reject_virtual_fs_mutation(runtime.posix_shm, new_len)?;
    let new_vfs_len = translate_guest_path_for_vfs(runtime, pid, new_len)?;
    posix::posix_rename(
        runtime.posix_port,
        LINUX_SECOND_PATH_OFFSET,
        old_vfs_len,
        0,
        new_vfs_len,
    )
    .map(|_| 0)
    .map_err(map_request_error)
}

pub(super) fn sys_readlink(
    runtime: &mut Runtime,
    pid: Word,
    path_ptr: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    let path_len = resolve_path(runtime, pid, path_ptr)?;
    readlink_from_current_path(runtime, pid, path_len, user_buffer, len)
}

pub(super) fn sys_readlinkat(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    path_ptr: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    let raw_len = read_c_string(runtime, pid, path_ptr)?;
    if !path_is_absolute(runtime.posix_shm, raw_len) && !is_at_fdcwd(dirfd) {
        return Err(ENOSYS);
    }
    let path_len = resolve_current_shm_path(runtime, pid, raw_len)?;
    readlink_from_current_path(runtime, pid, path_len, user_buffer, len)
}

pub(super) fn readlink_from_current_path(
    runtime: &mut Runtime,
    pid: Word,
    path_len: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    if user_buffer == 0 && len != 0 {
        return Err(EFAULT);
    }
    if path_equals(runtime.posix_shm, path_len, b"/proc/self/exe") {
        let Some(process) = runtime.managed_process(pid) else {
            return Err(ESRCH);
        };
        // Managed rootfs executables are exposed to Linux under /bin.  A
        // basename-only target makes BusyBox --install call link(2) with a
        // nonexistent path relative to the current directory.
        const GUEST_BIN_PREFIX: &[u8] = b"/bin/";
        let name_len = process.image_name_len;
        let total = GUEST_BIN_PREFIX.len().saturating_add(name_len);
        let bytes = ::core::cmp::min(total, len as usize);
        let prefix_bytes = ::core::cmp::min(GUEST_BIN_PREFIX.len(), bytes);
        unsafe {
            ::core::ptr::copy_nonoverlapping(
                GUEST_BIN_PREFIX.as_ptr(),
                runtime.posix_shm as *mut u8,
                prefix_bytes,
            );
            if bytes > prefix_bytes {
                ::core::ptr::copy_nonoverlapping(
                    process.image_name.as_ptr(),
                    (runtime.posix_shm + prefix_bytes as Word) as *mut u8,
                    bytes - prefix_bytes,
                );
            }
        }
        write_target_memory(runtime, pid, user_buffer, bytes as Word)?;
        return Ok(bytes as Word);
    }
    Err(EINVAL)
}

pub(super) fn sys_utime_path(
    runtime: &mut Runtime,
    pid: Word,
    path_ptr: Word,
) -> Result<Word, i32> {
    if path_ptr == 0 {
        return Ok(0);
    }
    let len = resolve_path(runtime, pid, path_ptr)?;
    if current_virtual_node(runtime, pid, len).is_some() {
        return Ok(0);
    }
    if is_linux_virtual_path(runtime.posix_shm, len) {
        return Err(ENOENT);
    }
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    posix::posix_stat(runtime.posix_port, 0, vfs_len)
        .map(|_| 0)
        .map_err(map_path_request_error)
}

pub(super) fn sys_getcwd(
    runtime: &mut Runtime,
    pid: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    if user_buffer == 0 || len == 0 {
        return Err(EINVAL);
    }
    let max_len = bounded_len(runtime, len)?;
    let Some(process) = runtime.managed_process(pid) else {
        return Err(ESRCH);
    };
    let bytes = process.cwd_len as Word;
    if bytes + 1 > max_len {
        return Err(ERANGE);
    }
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            process.cwd.as_ptr(),
            runtime.posix_shm as *mut u8,
            process.cwd_len,
        );
        ::core::ptr::write((runtime.posix_shm + bytes) as *mut u8, 0);
    }
    let total = bytes + 1;
    write_target_memory(runtime, pid, user_buffer, total)?;
    Ok(total)
}
