use core::ptr;
use libnanami::{ipc::ServiceRequest, Word};
use nanami_services::posix::*;

use super::{
    fd::alloc_open_file_and_fd,
    io::map_request_error_to_status,
    path::{
        pack_stat, resolve_client_path, special_device_kind, vfs_kind_to_posix, write_vfs_path,
    },
    process::find_session,
    state::*,
};

pub(crate) fn handle_getcwd(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let session = &runtime.sessions[index];
    let out_offset = request.arg0 as usize;
    let max_len = request.arg1 as usize;
    if out_offset.saturating_add(session.cwd_len) > session.shm_size as usize
        || session.cwd_len > max_len
    {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    unsafe {
        ptr::copy_nonoverlapping(
            session.cwd.as_ptr(),
            (session.shm_local as usize + out_offset) as *mut u8,
            session.cwd_len,
        );
    }
    (libnanami::OS_RESPONSE_OK, session.cwd_len as Word, 0)
}

pub(crate) fn handle_chdir(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((path, len)) =
        resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    match nanami_services::vfs::vfs_stat(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok((_, _, kind)) if kind == nanami_services::vfs::VFS_FILE_TYPE_DIRECTORY => {
            runtime.sessions[index].cwd[..len].copy_from_slice(&path[..len]);
            runtime.sessions[index].cwd_len = len;
            (libnanami::OS_RESPONSE_OK, 0, 0)
        }
        Ok(_) => (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

pub(crate) fn handle_open(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((path, len)) =
        resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let kind = special_device_kind(&path[..len]);
    if kind != FdKind::Empty {
        if (request.arg2 & POSIX_O_DIRECTORY) != 0 {
            return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
        }
        return alloc_open_file_and_fd(runtime, index, kind, 0, request.arg2);
    }
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    let mut flags = 0;
    if (request.arg2 & POSIX_O_CREAT) != 0 {
        flags |= nanami_services::vfs::VFS_OPEN_CREATE;
    }
    if (request.arg2 & POSIX_O_TRUNC) != 0 {
        flags |= nanami_services::vfs::VFS_OPEN_TRUNCATE;
    }
    if (request.arg2 & POSIX_O_DIRECTORY) != 0 {
        flags |= nanami_services::vfs::VFS_OPEN_DIRECTORY;
    }
    match nanami_services::vfs::vfs_open_compound(
        runtime.vfs_port,
        VFS_PATH_OFFSET as Word,
        len as Word,
        flags,
    ) {
        Ok((vfs_handle, _, _, kind)) => {
            let fd_kind = if kind == nanami_services::vfs::VFS_FILE_TYPE_DIRECTORY {
                FdKind::Directory
            } else {
                FdKind::Regular
            };
            alloc_open_file_and_fd(runtime, index, fd_kind, vfs_handle, request.arg2)
        }
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

pub(crate) fn handle_stat(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((path, len)) =
        resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let dev_kind = special_device_kind(&path[..len]);
    if dev_kind == FdKind::DevNull {
        return (
            libnanami::OS_RESPONSE_OK,
            0,
            pack_stat(
                0,
                POSIX_FILE_TYPE_CHAR_DEVICE,
                POSIX_DEV_NULL_MAJOR,
                POSIX_DEV_NULL_MINOR,
            ),
        );
    }
    if dev_kind == FdKind::DevZero {
        return (
            libnanami::OS_RESPONSE_OK,
            0,
            pack_stat(
                0,
                POSIX_FILE_TYPE_CHAR_DEVICE,
                POSIX_DEV_ZERO_MAJOR,
                POSIX_DEV_ZERO_MINOR,
            ),
        );
    }
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    match nanami_services::vfs::vfs_stat(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok((inode, size, kind)) => (
            libnanami::OS_RESPONSE_OK,
            inode,
            pack_stat(size, vfs_kind_to_posix(kind), 0, 0),
        ),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

pub(crate) fn handle_mkdir(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((path, len)) =
        resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    match nanami_services::vfs::vfs_mkdir(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok(_) => (libnanami::OS_RESPONSE_OK, 0, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

pub(crate) fn handle_unlink(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((path, len)) =
        resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    match nanami_services::vfs::vfs_stat(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok((_, _, kind)) if kind == nanami_services::vfs::VFS_FILE_TYPE_DIRECTORY => {
            return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
        }
        Ok(_) => {}
        Err(e) => return (map_request_error_to_status(e), 0, 0),
    }
    match nanami_services::vfs::vfs_remove(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

pub(crate) fn handle_link(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((old_path, old_len)) =
        resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((new_path, new_len)) =
        resolve_client_path(runtime, index, request.arg2 as usize, request.arg3 as usize)
    else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    write_vfs_path(runtime, VFS_PATH_OFFSET, &old_path[..old_len]);
    write_vfs_path(runtime, VFS_PATH2_OFFSET, &new_path[..new_len]);
    match nanami_services::vfs::vfs_link(
        runtime.vfs_port,
        VFS_PATH_OFFSET as Word,
        old_len as Word,
        VFS_PATH2_OFFSET as Word,
        new_len as Word,
    ) {
        Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

pub(crate) fn handle_rmdir(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((path, len)) =
        resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    match nanami_services::vfs::vfs_stat(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok((_, _, kind)) if kind == nanami_services::vfs::VFS_FILE_TYPE_DIRECTORY => {}
        Ok(_) => return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
        Err(e) => return (map_request_error_to_status(e), 0, 0),
    }
    match nanami_services::vfs::vfs_remove(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

pub(crate) fn handle_rename(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((old_path, old_len)) =
        resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((new_path, new_len)) =
        resolve_client_path(runtime, index, request.arg2 as usize, request.arg3 as usize)
    else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    write_vfs_path(runtime, VFS_PATH_OFFSET, &old_path[..old_len]);
    write_vfs_path(runtime, VFS_PATH2_OFFSET, &new_path[..new_len]);
    match nanami_services::vfs::vfs_rename(
        runtime.vfs_port,
        VFS_PATH_OFFSET as Word,
        old_len as Word,
        VFS_PATH2_OFFSET as Word,
        new_len as Word,
    ) {
        Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

pub(crate) fn handle_fstat(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let fd = request.arg0 as usize;
    if fd >= MAX_FDS || !runtime.sessions[index].fds[fd].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let open_index = runtime.sessions[index].fds[fd].open_file;
    if open_index >= runtime.open_files.len() || !runtime.open_files[open_index].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let entry = runtime.open_files[open_index];
    match entry.kind {
        FdKind::DevNull => (
            libnanami::OS_RESPONSE_OK,
            0,
            pack_stat(
                0,
                POSIX_FILE_TYPE_CHAR_DEVICE,
                POSIX_DEV_NULL_MAJOR,
                POSIX_DEV_NULL_MINOR,
            ),
        ),
        FdKind::DevZero => (
            libnanami::OS_RESPONSE_OK,
            0,
            pack_stat(
                0,
                POSIX_FILE_TYPE_CHAR_DEVICE,
                POSIX_DEV_ZERO_MAJOR,
                POSIX_DEV_ZERO_MINOR,
            ),
        ),
        FdKind::Regular | FdKind::Directory => {
            match nanami_services::vfs::vfs_fstat(runtime.vfs_port, entry.vfs_handle) {
                Ok((inode, size, kind)) => (
                    libnanami::OS_RESPONSE_OK,
                    inode,
                    pack_stat(size, vfs_kind_to_posix(kind), 0, 0),
                ),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        FdKind::Empty => (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0),
    }
}
