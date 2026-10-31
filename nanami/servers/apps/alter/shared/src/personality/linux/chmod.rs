//! Persistent permission changes, using the same rooted namespace as open/stat.
use super::*;
use nanami_client::vfs::{
    posix_fchmod, VFS_PATH_CHMOD, VFS_PATH_DIRECTORY_ROOT, VFS_PATH_FOLLOW_FINAL,
};

const AT_SYMLINK_NOFOLLOW: Word = 0x100;
const MODE_BITS: Word = 0o7777;

pub(super) fn fchmod(runtime: &mut Runtime, pid: Word, fd: Word, mode: Word) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if runtime.managed_process(pid).ok_or(ESRCH)?.rootfs.readonly {
        return Err(EROFS);
    }
    if !file.has_posix_backend() {
        return Err(EOPNOTSUPP);
    }
    posix_fchmod(runtime.posix_port, file.posix_fd, mode & MODE_BITS)
        .map_err(namespace::path_error)?;
    Ok(0)
}

pub(super) fn fchmodat(
    runtime: &mut Runtime,
    pid: Word,
    directory: Word,
    path_ptr: Word,
    mode: Word,
    flags: Word,
) -> Result<Word, i32> {
    if flags & !(AT_SYMLINK_NOFOLLOW | LINUX_AT_EMPTY_PATH) != 0 {
        return Err(EINVAL);
    }
    let mut bytes = read_c_string(runtime, pid, path_ptr)?;
    if bytes == 0 && flags & LINUX_AT_EMPTY_PATH != 0 {
        if !is_at_fdcwd(directory) {
            return fchmod(runtime, pid, directory, mode);
        }
        unsafe { *(runtime.posix_shm as *mut u8) = b'.' };
        bytes = 1;
    }
    let follow = flags & AT_SYMLINK_NOFOLLOW == 0;
    let bytes = resolve_at(runtime, pid, directory, bytes, follow)?;
    reject_virtual_fs_mutation(runtime, pid, runtime.posix_shm, bytes)?;
    let mut path = [0u8; LINUX_CWD_MAX];
    let bytes = bytes as usize;
    if bytes >= path.len() {
        return Err(ENAMETOOLONG);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(runtime.posix_shm as *const u8, path.as_mut_ptr(), bytes);
    }
    namespace::request_path(
        runtime,
        pid,
        VFS_PATH_CHMOD,
        VFS_PATH_DIRECTORY_ROOT,
        &path[..bytes],
        &(mode & MODE_BITS).to_le_bytes(),
        if follow { VFS_PATH_FOLLOW_FINAL } else { 0 },
        0,
    )?;
    Ok(0)
}
