//! Reuse the kind returned by this request's rooted lookup, never a saved stat.
use super::*;

pub(super) fn chdir(runtime: &mut Runtime, pid: Word, input_bytes: Word) -> Result<Word, i32> {
    let resolved = namespace::resolve_for_check(runtime, pid, LINUX_AT_FDCWD, input_bytes, true)?;
    let len = resolved.bytes;
    if len >= LINUX_CWD_MAX {
        return Err(ENAMETOOLONG);
    }
    if let Some(node) = current_virtual_node(runtime, pid, len) {
        if !node.is_directory() {
            return Err(ENOTDIR);
        }
    } else {
        if is_linux_virtual_path(runtime.posix_shm, len) {
            return Err(ENOENT);
        }
        let directory = match resolved.file_type {
            vfs::VFS_FILE_TYPE_DIRECTORY => true,
            vfs::VFS_FILE_TYPE_REGULAR | vfs::VFS_FILE_TYPE_SOCKET | vfs::VFS_FILE_TYPE_SYMLINK => false,
            _ => return chdir_with_stat(runtime, pid, len),
        };
        if !directory {
            return Err(ENOTDIR);
        }
    }
    // The checked canonical path is still in SHM on the known-kind and virtual
    // paths. set_cwd copies it synchronously without changing that buffer.
    let path = unsafe { core::slice::from_raw_parts(runtime.posix_shm as *const u8, len) };
    if runtime.set_cwd(pid, path) {
        Ok(0)
    } else {
        Err(EINVAL)
    }
}

fn chdir_with_stat(runtime: &mut Runtime, pid: Word, len: Word) -> Result<Word, i32> {
    // Legacy translation/stat may overwrite SHM. Preserve the guest path only
    // for this fallback, then commit it after the directory check succeeds.
    let mut guest_path = [0u8; LINUX_CWD_MAX];
    unsafe {
        core::ptr::copy_nonoverlapping(runtime.posix_shm as *const u8, guest_path.as_mut_ptr(), len);
    }
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    let kind = posix::posix_stat(runtime.posix_port, 0, vfs_len).map_err(map_path_request_error)?.2;
    if kind != posix::POSIX_FILE_TYPE_DIRECTORY {
        return Err(ENOTDIR);
    }
    if runtime.set_cwd(pid, &guest_path[..len]) {
        Ok(0)
    } else {
        Err(EINVAL)
    }
}

pub(super) fn check_times(
    runtime: &mut Runtime, pid: Word, dirfd: Word, input_bytes: Word, follow: bool,
) -> Result<Word, i32> {
    let resolved = namespace::resolve_for_check(runtime, pid, dirfd, input_bytes, follow)?;
    let len = resolved.bytes;
    if current_virtual_node(runtime, pid, len).is_some() {
        return Ok(0);
    }
    if is_linux_virtual_path(runtime.posix_shm, len) {
        return Err(ENOENT);
    }
    match resolved.file_type {
        vfs::VFS_FILE_TYPE_REGULAR | vfs::VFS_FILE_TYPE_DIRECTORY
        | vfs::VFS_FILE_TYPE_SOCKET => Ok(0),
        _ => {
            let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
            posix::posix_stat(runtime.posix_port, 0, vfs_len)
                .map(|_| 0).map_err(map_path_request_error)
        }
    }
}
