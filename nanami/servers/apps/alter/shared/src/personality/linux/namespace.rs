//! Rooted path resolution and directory-FD operations, including real symlinks.
use super::*;
use nanami_client::vfs::*;

const DESCRIPTOR: Word = 0xc000;
const ROOT: Word = DESCRIPTOR + 128;
const PATH: Word = DESCRIPTOR + 512;
const TARGET: Word = DESCRIPTOR + 1024;
const OUTPUT: Word = DESCRIPTOR + 8192;

pub(super) fn path_error(error: PathError) -> i32 {
    match error {
        PathError::Transport(error) => map_request_error(error),
        PathError::Filesystem(code) => match code {
            VFS_PATH_ERROR_MISSING => ENOENT,
            VFS_PATH_ERROR_NOT_DIRECTORY => ENOTDIR,
            VFS_PATH_ERROR_LOOP => ELOOP,
            VFS_PATH_ERROR_TOO_LONG => ENAMETOOLONG,
            VFS_PATH_ERROR_EXISTS => EEXIST,
            VFS_PATH_ERROR_IO => EIO,
            VFS_PATH_ERROR_NO_SPACE => ENOSPC,
            VFS_PATH_ERROR_READ_ONLY => EROFS,
            VFS_PATH_ERROR_UNSUPPORTED => EOPNOTSUPP,
            _ => EINVAL,
        },
    }
}

pub(super) fn request_path(
    runtime: &mut Runtime,
    pid: Word,
    operation: Word,
    directory: Word,
    path: &[u8],
    target: &[u8],
    flags: Word,
    output_bytes: Word,
) -> Result<(Word, Word), i32> {
    if path.len() >= LINUX_CWD_MAX
        || target.len() > 4096
        || output_bytes > 4096
        || OUTPUT
            .checked_add(output_bytes)
            .is_none_or(|end| end > runtime.posix_shm_size)
    {
        return Err(ENAMETOOLONG);
    }
    let root = guest_root_for_process(runtime, pid)?;
    if root.len() > PATH - ROOT {
        return Err(ENAMETOOLONG);
    }
    let request = PathRequest {
        operation,
        directory,
        root_offset: ROOT,
        root_bytes: root.len(),
        path_offset: PATH,
        path_bytes: path.len(),
        target_offset: TARGET,
        target_bytes: target.len(),
        output_offset: OUTPUT,
        output_bytes,
        flags,
    };
    unsafe {
        core::ptr::copy_nonoverlapping(
            root.as_ptr(),
            (runtime.posix_shm + ROOT) as *mut u8,
            root.len(),
        );
        core::ptr::copy_nonoverlapping(
            path.as_ptr(),
            (runtime.posix_shm + PATH) as *mut u8,
            path.len(),
        );
        core::ptr::copy_nonoverlapping(
            target.as_ptr(),
            (runtime.posix_shm + TARGET) as *mut u8,
            target.len(),
        );
        core::ptr::write_unaligned(
            (runtime.posix_shm + DESCRIPTOR) as *mut PathRequest,
            request,
        );
    }
    let details = posix_path_request(runtime.posix_port, DESCRIPTOR).map_err(path_error)?;
    if matches!(operation, VFS_PATH_RESOLVE | VFS_PATH_READLINK) {
        if details.0 > output_bytes {
            return Err(EIO);
        }
        unsafe {
            core::ptr::copy(
                (runtime.posix_shm + OUTPUT) as *const u8,
                runtime.posix_shm as *mut u8,
                details.0,
            );
        }
    }
    Ok(details)
}

fn virtual_directory(node: VirtualNode) -> Option<&'static [u8]> {
    match node {
        VirtualNode::Root => Some(b"/"),
        VirtualNode::Dev => Some(b"/dev"),
        VirtualNode::DevInput => Some(b"/dev/input"),
        VirtualNode::DevPts => Some(b"/dev/pts"),
        VirtualNode::Proc => Some(b"/proc"),
        VirtualNode::ProcSelf => Some(b"/proc/self"),
        VirtualNode::ProcNet => Some(b"/proc/net"),
        VirtualNode::Sys => Some(b"/sys"),
        VirtualNode::SysClass => Some(b"/sys/class"),
        VirtualNode::SysClassInput => Some(b"/sys/class/input"),
        VirtualNode::SysClassGraphics => Some(b"/sys/class/graphics"),
        _ => None,
    }
}

pub(super) fn resolve_at(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    input_bytes: Word,
    follow: bool,
) -> Result<Word, i32> {
    Ok(resolve_at_with_metadata(runtime, pid, dirfd, input_bytes, follow, false)?.bytes)
}

pub(super) struct ResolvedPath {
    pub bytes: Word,
    pub metadata: Option<PathMetadata>,
}

pub(super) fn decode_metadata(
    value: PathMetadata,
) -> Result<Option<(Word, Word, Word, Word, Word)>, i32> {
    if value.inode == 0 {
        if value.size != 0 || value.mode != 0 || value.file_type != 0 {
            return Err(EIO);
        }
        return Ok(None);
    }
    if value.inode > u32::MAX as Word || value.mode > u16::MAX as Word {
        return Err(EIO);
    }
    let kind = match value.file_type {
        VFS_FILE_TYPE_REGULAR => posix::POSIX_FILE_TYPE_REGULAR,
        VFS_FILE_TYPE_DIRECTORY => posix::POSIX_FILE_TYPE_DIRECTORY,
        VFS_FILE_TYPE_SOCKET => posix::POSIX_FILE_TYPE_SOCKET,
        VFS_FILE_TYPE_SYMLINK => posix::POSIX_FILE_TYPE_SYMLINK,
        _ => return Err(EIO),
    };
    Ok(Some((
        value.inode | (value.mode << 32),
        value.size,
        kind,
        0,
        0,
    )))
}

pub(super) fn stat_at(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    input_bytes: Word,
    follow: bool,
) -> Result<(Word, Word, Word, Word, Word), i32> {
    let resolved = resolve_at_with_metadata(runtime, pid, dirfd, input_bytes, follow, true)?;
    super::metadata::stat_resolved_path(runtime, pid, resolved)
}

fn resolve_at_with_metadata(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    input_bytes: Word,
    follow: bool,
    with_metadata: bool,
) -> Result<ResolvedPath, i32> {
    if input_bytes == 0 {
        return Err(ENOENT);
    }
    if input_bytes >= LINUX_CWD_MAX {
        return Err(ENAMETOOLONG);
    }
    let mut path = [0u8; LINUX_CWD_MAX];
    let input = unsafe { core::slice::from_raw_parts(runtime.posix_shm as *const u8, input_bytes) };
    let mut directory = VFS_PATH_DIRECTORY_ROOT;
    let mut proc_directory = [0u8; 64];
    let (prefix, prefix_bytes) = if input[0] == b'/' {
        (&[][..], 0)
    } else if is_at_fdcwd(dirfd) {
        let process = runtime.managed_process(pid).ok_or(ESRCH)?;
        (&process.cwd[..process.cwd_len], process.cwd_len)
    } else {
        let file = runtime.linux_file(pid, dirfd).ok_or(EBADF)?;
        if file.has_posix_backend() {
            directory = file.posix_fd;
            (&[][..], 0)
        } else {
            if file.kind != LinuxFileKind::VirtualDirectory {
                return Err(ENOTDIR);
            }
            super::procfs::validate(runtime, pid, file.resource)?;
            let node = super::procfs::node(file.resource).ok_or(ENOTDIR)?;
            let prefix = if let Some(len) =
                super::procfs::directory_path(file.resource, &mut proc_directory)
            {
                &proc_directory[..len]
            } else {
                virtual_directory(node).ok_or(ENOTDIR)?
            };
            (prefix, prefix.len())
        }
    };
    let separator = usize::from(prefix_bytes != 0 && prefix != b"/");
    let total = prefix_bytes + separator + input_bytes;
    if total >= path.len() {
        return Err(ENAMETOOLONG);
    }
    path[..prefix_bytes].copy_from_slice(prefix);
    if separator != 0 {
        path[prefix_bytes] = b'/';
    }
    path[prefix_bytes + separator..total].copy_from_slice(input);
    // /dev, /proc and /sys are personality-owned mounts, not disk directories.
    if is_linux_virtual_path(path.as_ptr() as Word, total) {
        unsafe {
            core::ptr::copy_nonoverlapping(path.as_ptr(), runtime.posix_shm as *mut u8, total);
        }
        let bytes = resolve_lexical_shm_path(runtime, pid, total)?;
        if is_linux_virtual_path(runtime.posix_shm, bytes) {
            return Ok(ResolvedPath {
                bytes,
                metadata: None,
            });
        }
        return resolve_at_with_metadata(
            runtime,
            pid,
            LINUX_AT_FDCWD,
            bytes,
            follow,
            with_metadata,
        );
    }
    let mut flags = VFS_PATH_ALLOW_MISSING;
    if follow {
        flags |= VFS_PATH_FOLLOW_FINAL;
    }
    let metadata_bytes = if with_metadata {
        core::mem::size_of::<PathMetadata>()
    } else {
        0
    };
    if with_metadata {
        flags |= VFS_PATH_WITH_METADATA;
    }
    let (output_bytes, file_type) = request_path(
        runtime,
        pid,
        VFS_PATH_RESOLVE,
        directory,
        &path[..total],
        &[],
        flags,
        metadata_bytes + LINUX_CWD_MAX - 1,
    )?;
    if output_bytes <= metadata_bytes {
        return Err(EIO);
    }
    if with_metadata && unsafe { *((runtime.posix_shm + metadata_bytes) as *const u8) } != b'/' {
        return Err(EIO);
    }
    let metadata = if with_metadata {
        let value = unsafe { core::ptr::read_unaligned(runtime.posix_shm as *const PathMetadata) };
        if value.file_type != file_type {
            return Err(EIO);
        }
        Some(value)
    } else {
        None
    };
    let bytes = output_bytes - metadata_bytes;
    unsafe {
        core::ptr::copy(
            (runtime.posix_shm + metadata_bytes) as *const u8,
            runtime.posix_shm as *mut u8,
            bytes,
        );
        *((runtime.posix_shm + bytes) as *mut u8) = 0;
    }
    Ok(ResolvedPath { bytes, metadata })
}

pub(super) fn fchdir(runtime: &mut Runtime, pid: Word, fd: Word) -> Result<Word, i32> {
    unsafe {
        *(runtime.posix_shm as *mut u8) = b'.';
    }
    let bytes = resolve_at(runtime, pid, fd, 1, true)?;
    let path =
        unsafe { core::slice::from_raw_parts(runtime.posix_shm as *const u8, bytes) }.to_vec();
    if !runtime.set_cwd(pid, &path) {
        return Err(ENAMETOOLONG);
    }
    Ok(0)
}

pub(super) fn resolve_nofollow(
    runtime: &mut Runtime,
    pid: Word,
    pointer: Word,
) -> Result<Word, i32> {
    let bytes = read_c_string(runtime, pid, pointer)?;
    resolve_at(runtime, pid, LINUX_AT_FDCWD, bytes, false)
}

pub(super) fn symlink(
    runtime: &mut Runtime,
    pid: Word,
    target: Word,
    dirfd: Word,
    path: Word,
) -> Result<Word, i32> {
    if runtime.managed_process(pid).ok_or(ESRCH)?.rootfs.readonly {
        return Err(EROFS);
    }
    let target_bytes = read_c_string(runtime, pid, target)?;
    let target =
        unsafe { core::slice::from_raw_parts(runtime.posix_shm as *const u8, target_bytes) }
            .to_vec();
    let path_bytes = read_c_string(runtime, pid, path)?;
    let bytes = resolve_at(runtime, pid, dirfd, path_bytes, false)?;
    reject_virtual_fs_mutation(runtime, pid, runtime.posix_shm, bytes)?;
    let path =
        unsafe { core::slice::from_raw_parts(runtime.posix_shm as *const u8, bytes) }.to_vec();
    request_path(
        runtime,
        pid,
        VFS_PATH_SYMLINK,
        VFS_PATH_DIRECTORY_ROOT,
        &path,
        &target,
        0,
        0,
    )?;
    Ok(0)
}
