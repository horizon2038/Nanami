use core::ptr;
use libnanami::{ipc::ServiceRequest, RequestError, Word};
use nanami_services::posix::*;

use super::{process::find_session, state::*};

pub(crate) fn handle_read(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    handle_read_with_mode(runtime, request, false)
}

pub(crate) fn handle_read_direct(
    runtime: &mut Runtime,
    request: ServiceRequest,
) -> (Word, Word, Word) {
    handle_read_with_mode(runtime, request, true)
}

pub(crate) fn handle_read_with_mode(
    runtime: &mut Runtime,
    request: ServiceRequest,
    direct: bool,
) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let fd = request.arg0 as usize;
    let out_offset = request.arg1 as usize;
    let len = request.arg2 as usize;

    // stdin/stdout/stderr
    match fd {
        0 => {
            libnanami::println!("[posix-server] read from stdin not supported");
            return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
        }
        1 | 2 => {
            libnanami::println!("[posix-server] read from stdout/stderr not supported");
            return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
        }
        _ => {}
    }

    if fd >= MAX_FDS || !runtime.sessions[index].fds[fd].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let output_size = if direct {
        if runtime.sessions[index].direct_vfs_delegate == 0 {
            return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
        }
        runtime.sessions[index].direct_io_size
    } else {
        runtime.sessions[index].shm_size
    };
    if out_offset.saturating_add(len) > output_size as usize {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let open_index = runtime.sessions[index].fds[fd].open_file;
    if open_index >= runtime.open_files.len() || !runtime.open_files[open_index].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let entry = &runtime.open_files[open_index];
    let positioned = matches!(
        request.code,
        POSIX_REQUEST_PREAD | POSIX_REQUEST_PREAD_DIRECT
    );
    let file_offset = if positioned {
        request.arg3
    } else {
        entry.offset
    };
    match entry.kind {
        FdKind::DevNull => (libnanami::OS_RESPONSE_OK, 0, 0),
        FdKind::DevZero => {
            if direct {
                return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
            }
            unsafe {
                ptr::write_bytes(
                    (runtime.sessions[index].shm_local as usize + out_offset) as *mut u8,
                    0,
                    len,
                );
            }
            (libnanami::OS_RESPONSE_OK, len as Word, 0)
        }
        FdKind::Regular => {
            let chunk = if direct {
                len
            } else {
                len.min(runtime.vfs_shm_size.saturating_sub(VFS_IO_OFFSET as Word) as usize)
            };
            if chunk == 0 {
                return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            let read = if direct {
                nanami_services::vfs::vfs_read_delegated(
                    runtime.vfs_port,
                    entry.vfs_handle,
                    file_offset,
                    chunk as Word,
                    runtime.sessions[index].direct_vfs_delegate,
                    out_offset as Word,
                )
            } else {
                nanami_services::vfs::vfs_read(
                    runtime.vfs_port,
                    entry.vfs_handle,
                    file_offset,
                    chunk as Word,
                    VFS_IO_OFFSET as Word,
                )
            };
            match read {
                Ok(bytes) => {
                    if !direct {
                        unsafe {
                            ptr::copy_nonoverlapping(
                                (runtime.vfs_shm as usize + VFS_IO_OFFSET) as *const u8,
                                (runtime.sessions[index].shm_local as usize + out_offset)
                                    as *mut u8,
                                bytes as usize,
                            );
                        }
                    }
                    if !positioned {
                        runtime.open_files[open_index].offset = file_offset.saturating_add(bytes);
                    }
                    (libnanami::OS_RESPONSE_OK, bytes, 0)
                }
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        _ => (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
    }
}

pub(crate) fn handle_write(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    handle_write_with_mode(runtime, request, false)
}

pub(crate) fn handle_write_direct(
    runtime: &mut Runtime,
    request: ServiceRequest,
) -> (Word, Word, Word) {
    handle_write_with_mode(runtime, request, true)
}

fn handle_write_with_mode(
    runtime: &mut Runtime,
    request: ServiceRequest,
    direct: bool,
) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let fd = request.arg0 as usize;
    let input_offset = request.arg1 as usize;
    let len = request.arg2 as usize;

    let session = &runtime.sessions[index];
    let input_size = if direct {
        if session.direct_vfs_delegate == 0 {
            return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
        }
        session.direct_io_size
    } else {
        session.shm_size
    };
    // Check before constructing stdout/stderr slices as well as file transfers.
    if input_offset
        .checked_add(len)
        .filter(|end| *end <= input_size)
        .is_none()
    {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }

    match fd {
        0 => {
            libnanami::println!("[posix-server] write to stdin not supported");
            return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
        }
        1 | 2 => {
            if direct {
                return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
            }
            let input = unsafe {
                core::slice::from_raw_parts(
                    (runtime.sessions[index].shm_local as usize + input_offset) as *const u8,
                    len,
                )
            };
            let Ok(s) = core::str::from_utf8(input) else {
                return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            };
            libnanami::print!("Guest stdout/err: {}", s);
            return (libnanami::OS_RESPONSE_OK, len as Word, 0);
        }
        _ => {}
    }

    if fd >= MAX_FDS || !runtime.sessions[index].fds[fd].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let open_index = runtime.sessions[index].fds[fd].open_file;
    if open_index >= runtime.open_files.len() || !runtime.open_files[open_index].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let entry = &runtime.open_files[open_index];
    let positioned = matches!(
        request.code,
        POSIX_REQUEST_PWRITE | POSIX_REQUEST_PWRITE_DIRECT
    );
    let file_offset = if entry.kind == FdKind::Regular && entry.status_flags & POSIX_O_APPEND != 0 {
        match nanami_services::vfs::vfs_fstat(runtime.vfs_port, entry.vfs_handle) {
            Ok((_, size, _)) => size,
            Err(error) => return (map_request_error_to_status(error), 0, 0),
        }
    } else if positioned {
        request.arg3
    } else {
        entry.offset
    };
    match entry.kind {
        FdKind::DevNull => (libnanami::OS_RESPONSE_OK, len as Word, 0),
        FdKind::Regular => {
            let chunk = if direct {
                len
            } else {
                len.min(runtime.vfs_shm_size.saturating_sub(VFS_IO_OFFSET as Word) as usize)
            };
            if chunk == 0 {
                return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            let write = if direct {
                nanami_services::vfs::vfs_write_delegated(
                    runtime.vfs_port,
                    entry.vfs_handle,
                    file_offset,
                    chunk,
                    runtime.sessions[index].direct_vfs_delegate,
                    input_offset,
                )
            } else {
                unsafe {
                    ptr::copy_nonoverlapping(
                        (runtime.sessions[index].shm_local + input_offset) as *const u8,
                        (runtime.vfs_shm + VFS_IO_OFFSET) as *mut u8,
                        chunk,
                    );
                }
                nanami_services::vfs::vfs_write(
                    runtime.vfs_port,
                    entry.vfs_handle,
                    file_offset,
                    chunk,
                    VFS_IO_OFFSET,
                )
            };
            match write {
                Ok(bytes) => {
                    if entry.status_flags & POSIX_O_SYNC != 0 {
                        if let Err(error) = nanami_services::vfs::vfs_fsync(runtime.vfs_port, entry.vfs_handle) {
                            return (map_request_error_to_status(error), 0, 0);
                        }
                    }
                    if !positioned {
                        runtime.open_files[open_index].offset = file_offset.saturating_add(bytes);
                    }
                    (libnanami::OS_RESPONSE_OK, bytes, 0)
                }
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        _ => (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
    }
}

pub(crate) fn handle_read_dir(
    runtime: &mut Runtime,
    request: ServiceRequest,
) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let fd = request.arg0 as usize;
    let max_entries = request.arg1;
    let out_offset = request.arg2 as usize;
    if fd >= MAX_FDS || !runtime.sessions[index].fds[fd].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let open_index = runtime.sessions[index].fds[fd].open_file;
    if open_index >= runtime.open_files.len() || !runtime.open_files[open_index].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let entry = runtime.open_files[open_index];
    if entry.kind != FdKind::Directory {
        return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    }
    if out_offset > runtime.sessions[index].shm_size as usize {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let record_bytes = nanami_services::vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES;
    let client_capacity = runtime.sessions[index]
        .shm_size
        .saturating_sub(out_offset as Word) as usize
        / record_bytes;
    let vfs_capacity =
        runtime.vfs_shm_size.saturating_sub(VFS_IO_OFFSET as Word) as usize / record_bytes;
    let capped_entries = (max_entries as usize)
        .min(client_capacity)
        .min(vfs_capacity) as Word;
    let bytes = (capped_entries as usize).saturating_mul(record_bytes);
    if out_offset.saturating_add(bytes) > runtime.sessions[index].shm_size as usize {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    if capped_entries == 0 {
        return (libnanami::OS_RESPONSE_OK, 0, entry.offset);
    }
    match nanami_services::vfs::vfs_read_dir(
        runtime.vfs_port,
        entry.vfs_handle,
        entry.offset,
        capped_entries,
        VFS_IO_OFFSET as Word,
    ) {
        Ok((entries, next_index)) => {
            let copy_bytes = (entries as usize).saturating_mul(record_bytes);
            unsafe {
                ptr::copy_nonoverlapping(
                    (runtime.vfs_shm as usize + VFS_IO_OFFSET) as *const u8,
                    (runtime.sessions[index].shm_local as usize + out_offset) as *mut u8,
                    copy_bytes,
                );
            }
            runtime.open_files[open_index].offset = next_index;
            (libnanami::OS_RESPONSE_OK, entries, next_index)
        }
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

pub(crate) fn handle_seek(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
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
    let base = match request.arg2 {
        POSIX_SEEK_SET => 0,
        POSIX_SEEK_CUR => entry.offset,
        POSIX_SEEK_END => match entry.kind {
            FdKind::Regular | FdKind::Directory => {
                match nanami_services::vfs::vfs_fstat(runtime.vfs_port, entry.vfs_handle) {
                    Ok((_, size, _)) => size,
                    Err(e) => return (map_request_error_to_status(e), 0, 0),
                }
            }
            FdKind::DevNull | FdKind::DevZero => 0,
            FdKind::Empty => return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0),
        },
        _ => return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    };
    let new_offset = base.saturating_add(request.arg1);
    runtime.open_files[open_index].offset = new_offset;
    (libnanami::OS_RESPONSE_OK, new_offset, 0)
}

pub(crate) fn map_request_error_to_status(error: RequestError) -> Word {
    match error {
        RequestError::InvalidArgument => libnanami::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Status(status) => status,
        RequestError::Unsupported => libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Transport | RequestError::Protocol => libnanami::OS_RESPONSE_FATAL,
    }
}
