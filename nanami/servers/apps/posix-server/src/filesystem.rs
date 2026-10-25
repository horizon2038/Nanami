use core::ptr;
use nanami_runtime::{ipc::ServiceRequest, Word};
use nanami_client::posix::*;

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
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let session = &runtime.sessions[index];
    let out_offset = request.arg0 as usize;
    let max_len = request.arg1 as usize;
    if out_offset.saturating_add(session.cwd_len) > session.shm_size as usize
        || session.cwd_len > max_len
    {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    unsafe {
        ptr::copy_nonoverlapping(
            session.cwd.as_ptr(),
            (session.shm_local as usize + out_offset) as *mut u8,
            session.cwd_len,
        );
    }
    (nanami_runtime::OS_RESPONSE_OK, session.cwd_len as Word, 0)
}

pub(crate) fn handle_chdir(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((path, len)) =
        resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    match nanami_client::vfs::vfs_stat(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok((_, _, kind)) if kind == nanami_client::vfs::VFS_FILE_TYPE_DIRECTORY => {
            runtime.sessions[index].cwd[..len].copy_from_slice(&path[..len]);
            runtime.sessions[index].cwd_len = len;
            (nanami_runtime::OS_RESPONSE_OK, 0, 0)
        }
        Ok(_) => (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

pub(crate) fn handle_open(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((path, len)) =
        resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let kind = special_device_kind(&path[..len]);
    if kind != FdKind::Empty {
        if (request.arg2 & POSIX_O_DIRECTORY) != 0 {
            return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
        }
        return alloc_open_file_and_fd(runtime, index, kind, 0, request.arg2);
    }
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    let mut flags = 0;
    if (request.arg2 & POSIX_O_CREAT) != 0 {
        flags |= nanami_client::vfs::VFS_OPEN_CREATE;
    }
    if (request.arg2 & POSIX_O_TRUNC) != 0 {
        flags |= nanami_client::vfs::VFS_OPEN_TRUNCATE;
    }
    if (request.arg2 & POSIX_O_DIRECTORY) != 0 {
        flags |= nanami_client::vfs::VFS_OPEN_DIRECTORY;
    }
    match nanami_client::vfs::vfs_open_compound(
        runtime.vfs_port,
        VFS_PATH_OFFSET as Word,
        len as Word,
        flags,
    ) {
        Ok((vfs_handle, _, _, kind)) => {
            if kind == nanami_client::vfs::VFS_FILE_TYPE_SOCKET {
                let _ = nanami_client::vfs::vfs_close(runtime.vfs_port, vfs_handle);
                return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
            }
            let fd_kind = if kind == nanami_client::vfs::VFS_FILE_TYPE_DIRECTORY {
                FdKind::Directory
            } else {
                FdKind::Regular
            };
            alloc_open_file_and_fd(runtime, index, fd_kind, vfs_handle, request.arg2)
        }
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

pub(crate) fn handle_create_socket_node(
    runtime: &mut Runtime,
    request: ServiceRequest,
) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((path, len)) =
        resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    if special_device_kind(&path[..len]) != FdKind::Empty {
        return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    }
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    match nanami_client::vfs::vfs_create_node(
        runtime.vfs_port,
        VFS_PATH_OFFSET,
        len,
        nanami_client::vfs::VFS_FILE_TYPE_SOCKET,
    ) {
        Ok(inode) => {
            let result = nanami_client::vfs::vfs_open(runtime.vfs_port, VFS_PATH_OFFSET, len);
            let result = match result {
                Ok(handle) => alloc_open_file_and_fd(runtime, index, FdKind::Regular, handle, 0),
                Err(error) => (map_request_error_to_status(error), 0, 0),
            };
            if result.0 == nanami_runtime::OS_RESPONSE_OK {
                (result.0, inode, result.1)
            } else {
                let _ = nanami_client::vfs::vfs_remove(runtime.vfs_port, VFS_PATH_OFFSET, len);
                result
            }
        }
        Err(error) => (map_request_error_to_status(error), 0, 0),
    }
}

pub(crate) fn handle_stat(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((path, len)) =
        resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let dev_kind = special_device_kind(&path[..len]);
    if dev_kind == FdKind::DevNull {
        return (
            nanami_runtime::OS_RESPONSE_OK,
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
            nanami_runtime::OS_RESPONSE_OK,
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
    match nanami_client::vfs::vfs_stat(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok((inode, size, kind)) => (
            nanami_runtime::OS_RESPONSE_OK,
            inode,
            pack_stat(size, vfs_kind_to_posix(kind), 0, 0),
        ),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

pub(crate) fn handle_mkdir(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((path, len)) =
        resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    match nanami_client::vfs::vfs_mkdir(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok(_) => (nanami_runtime::OS_RESPONSE_OK, 0, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

pub(crate) fn handle_unlink(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((path, len)) =
        resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    match nanami_client::vfs::vfs_stat(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok((_, _, kind)) if kind == nanami_client::vfs::VFS_FILE_TYPE_DIRECTORY => {
            return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
        }
        Ok(_) => {}
        Err(e) => return (map_request_error_to_status(e), 0, 0),
    }
    match nanami_client::vfs::vfs_remove(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok(()) => (nanami_runtime::OS_RESPONSE_OK, 0, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

pub(crate) fn handle_link(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((old_path, old_len)) =
        resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((new_path, new_len)) =
        resolve_client_path(runtime, index, request.arg2 as usize, request.arg3 as usize)
    else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    write_vfs_path(runtime, VFS_PATH_OFFSET, &old_path[..old_len]);
    write_vfs_path(runtime, VFS_PATH2_OFFSET, &new_path[..new_len]);
    match nanami_client::vfs::vfs_link(
        runtime.vfs_port,
        VFS_PATH_OFFSET as Word,
        old_len as Word,
        VFS_PATH2_OFFSET as Word,
        new_len as Word,
    ) {
        Ok(()) => (nanami_runtime::OS_RESPONSE_OK, 0, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

pub(crate) fn handle_rmdir(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((path, len)) =
        resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    match nanami_client::vfs::vfs_stat(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok((_, _, kind)) if kind == nanami_client::vfs::VFS_FILE_TYPE_DIRECTORY => {}
        Ok(_) => return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
        Err(e) => return (map_request_error_to_status(e), 0, 0),
    }
    match nanami_client::vfs::vfs_remove(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok(()) => (nanami_runtime::OS_RESPONSE_OK, 0, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

pub(crate) fn handle_rename(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((old_path, old_len)) =
        resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((new_path, new_len)) =
        resolve_client_path(runtime, index, request.arg2 as usize, request.arg3 as usize)
    else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    write_vfs_path(runtime, VFS_PATH_OFFSET, &old_path[..old_len]);
    write_vfs_path(runtime, VFS_PATH2_OFFSET, &new_path[..new_len]);
    match nanami_client::vfs::vfs_rename(
        runtime.vfs_port,
        VFS_PATH_OFFSET as Word,
        old_len as Word,
        VFS_PATH2_OFFSET as Word,
        new_len as Word,
    ) {
        Ok(()) => (nanami_runtime::OS_RESPONSE_OK, 0, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

pub(crate) fn handle_fstat(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let fd = request.arg0 as usize;
    if fd >= MAX_FDS || !runtime.sessions[index].fds[fd].active {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let open_index = runtime.sessions[index].fds[fd].open_file;
    if open_index >= runtime.open_files.len() || !runtime.open_files[open_index].active {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let entry = runtime.open_files[open_index];
    match entry.kind {
        FdKind::DevNull => (
            nanami_runtime::OS_RESPONSE_OK,
            0,
            pack_stat(
                0,
                POSIX_FILE_TYPE_CHAR_DEVICE,
                POSIX_DEV_NULL_MAJOR,
                POSIX_DEV_NULL_MINOR,
            ),
        ),
        FdKind::DevZero => (
            nanami_runtime::OS_RESPONSE_OK,
            0,
            pack_stat(
                0,
                POSIX_FILE_TYPE_CHAR_DEVICE,
                POSIX_DEV_ZERO_MAJOR,
                POSIX_DEV_ZERO_MINOR,
            ),
        ),
        FdKind::Regular | FdKind::Directory => {
            match nanami_client::vfs::vfs_fstat(runtime.vfs_port, entry.vfs_handle) {
                Ok((inode, size, kind)) => (
                    nanami_runtime::OS_RESPONSE_OK,
                    inode,
                    pack_stat(size, vfs_kind_to_posix(kind), 0, 0),
                ),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        FdKind::Empty => (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0),
    }
}
