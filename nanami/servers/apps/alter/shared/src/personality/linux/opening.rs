//! Read-only rooted open fusion and legacy/virtual open fallbacks.
use super::*;

pub(super) fn sys_open(
    runtime: &mut Runtime,
    pid: Word,
    path_ptr: Word,
    linux_flags: Word,
    mode: Word,
) -> Result<Word, i32> {
    sys_openat(runtime, pid, super::LINUX_AT_FDCWD, path_ptr, linux_flags, mode)
}

fn open_current_path(
    runtime: &mut Runtime,
    pid: Word,
    len: Word,
    linux_flags: Word,
    mode: Word,
) -> Result<Word, i32> {
    if linux_flags & (super::LINUX_O_ACCMODE | LINUX_O_CREAT | LINUX_O_TRUNC) != 0
        && runtime
            .managed_process(pid)
            .is_some_and(|process| process.rootfs.readonly)
        && !super::is_linux_virtual_path(runtime.posix_shm, len)
    {
        return Err(super::EROFS);
    }
    if let Some(fd) = open_virtual_path(runtime, pid, len, linux_flags)? {
        return Ok(fd);
    }
    let path = unsafe { core::slice::from_raw_parts(runtime.posix_shm as *const u8, len) };
    let fallback = super::virtual_fs::fallback_file(path);
    let flags = translate_open_flags(linux_flags);
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    let mode = super::filesystem::creation_mode(runtime, pid, mode)?;
    let fd = match (
        posix::posix_open_mode(runtime.posix_port, 0, vfs_len, flags, mode),
        fallback,
    ) {
        (
            Err(nanami_runtime::RequestError::Status(
                nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR,
            )),
            Some(node),
        ) if linux_flags & (super::LINUX_O_ACCMODE | LINUX_O_CREAT | LINUX_O_TRUNC) == 0 => {
            return super::open_virtual_node(runtime, pid, node, linux_flags);
        }
        (result, _) => result.map_err(map_path_request_error)?,
    };
    allocate_posix_file(runtime, pid, fd, linux_flags, 0, false)
}

fn allocate_posix_file(runtime: &mut Runtime, pid: Word, fd: Word, linux_flags: Word, grant: Word, cursor: bool) -> Result<Word, i32> {
    let fd_flags = if (linux_flags & LINUX_O_CLOEXEC) != 0 {
        LINUX_FD_CLOEXEC
    } else {
        0
    };
    match runtime.allocate_linux_file(pid, LinuxFile::posix(fd, fd_flags, linux_flags)
        .with_posix_read_grant(grant).with_posix_read_cursor(cursor), 0) {
        Some(local) => Ok(local),
        None => {
            let _ = posix::posix_close(runtime.posix_port, fd);
            Err(EMFILE)
        }
    }
}

pub(super) fn sys_openat(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    path_ptr: Word,
    linux_flags: Word,
    mode: Word,
) -> Result<Word, i32> {
    let raw_len = read_c_string(runtime, pid, path_ptr)?;
    // Fusion preserves the Linux flags on the local descriptor. It may omit
    // only flags that the legacy backend translation already omits. Status or
    // mutating backend flags still require the ordinary open request.
    let eligible = option_env!("NANAMI_ROOTED_READONLY_OPEN") != Some("0")
        && linux_flags & LINUX_O_ACCMODE == 0
        && translate_open_flags(linux_flags) & !posix::POSIX_O_DIRECTORY == 0;
    let follow = linux_flags & super::LINUX_O_NOFOLLOW == 0;
    let len = if eligible {
        let flags = if linux_flags & LINUX_O_DIRECTORY != 0 {
            nanami_client::vfs::VFS_OPEN_DIRECTORY
        } else { 0 };
        let resolved = super::namespace::resolve_for_open(runtime, pid, dirfd, raw_len, follow, flags)?;
        if let Some(fd) = resolved.opened {
            // Personality-owned mounts take precedence even when a disk path
            // (including a symlink) resolves into one. Release before delegating.
            if super::current_virtual_node(runtime, pid, resolved.bytes).is_some()
                || super::is_linux_virtual_path(runtime.posix_shm, resolved.bytes)
            {
                posix::posix_close(runtime.posix_port, fd).map_err(map_request_error)?;
                return open_current_path(runtime, pid, resolved.bytes, linux_flags, mode);
            }
            #[cfg(target_os = "none")]
            if option_env!("NANAMI_ROOTED_OPEN_DIAG") == Some("1") {
                use core::sync::atomic::{AtomicBool, Ordering};
                static FIRST: AtomicBool = AtomicBool::new(false);
                if !FIRST.swap(true, Ordering::Relaxed) {
                    nanami_runtime::println!("[linux.open] rooted-readonly pid={}", pid);
                }
                static FILES: AtomicBool = AtomicBool::new(false);
                if runtime.managed_process(pid).is_some_and(|process| {
                    process.image_name[..process.image_name_len].rsplit(|byte| *byte == b'/').next()
                        == Some(&b"xfe"[..])
                }) && !FILES.swap(true, Ordering::Relaxed) {
                    nanami_runtime::println!("[linux.open] rooted-readonly-files pid={}", pid);
                }
            }
            return allocate_posix_file(runtime, pid, fd, linux_flags, resolved.read_grant, resolved.read_cursor);
        }
        resolved.bytes
    } else {
        super::resolve_at(
        runtime,
        pid,
        dirfd,
        raw_len,
        follow,
        )?
    };
    if linux_flags & super::LINUX_O_NOFOLLOW != 0 {
        check_no_follow(runtime, pid, len)?;
    }
    open_current_path(runtime, pid, len, linux_flags, mode)
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

fn check_no_follow(runtime: &mut Runtime, pid: Word, bytes: Word) -> Result<(), i32> {
    // Both rooted and lexical resolution limit canonical paths to CWD_MAX-1.
    // stat may translate/overwrite SHM, so retain the path until it returns.
    let mut path = [0u8; super::LINUX_CWD_MAX];
    path[..bytes].copy_from_slice(unsafe {
        core::slice::from_raw_parts(runtime.posix_shm as *const u8, bytes)
    });
    let stat = super::stat_current_path(runtime, pid, bytes);
    unsafe {
        core::ptr::copy_nonoverlapping(path.as_ptr(), runtime.posix_shm as *mut u8, bytes);
    }
    if stat.is_ok_and(|stat| stat.2 == posix::POSIX_FILE_TYPE_SYMLINK) {
        return Err(super::ELOOP);
    }
    Ok(())
}

