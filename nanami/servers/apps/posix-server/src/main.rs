#![no_std]
#![no_main]

use core::ptr;
use libnanami::ipc::{ServiceEvent, ServiceRequest};
use libnanami::{self, RequestError, Word};
use nanami_services::posix::*;

mod environment;
mod fd;
mod path;
mod process;
mod state;

use environment::*;
use fd::*;
use path::*;
use process::*;
use state::*;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[posix-server] panic\n");
    loop {}
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::print!("[posix-server] bootstrap\n");
    libnanami::ipc::init_ipc_tls()
        .map_err(|e| log_error("[posix-server] ipc tls init failed: ", e))?;

    let vfs_port = connect_vfs_service();
    let (vfs_shm, vfs_shm_size) = nanami_services::vfs::vfs_attach_shared_memory(vfs_port, VFS_SHM_BYTES)
        .map_err(|e| log_error("[posix-server] vfs shm attach failed: ", e))?;

    nanami_services::registry::register_posix_service()
        .map_err(|e| log_error("[posix-server] service register failed: ", e))?;
    libnanami::print!("[posix-server] service registered: posix-service\n");

    let mut runtime = Runtime {
        vfs_port,
        vfs_shm,
        vfs_shm_size,
        sessions: [Session::EMPTY; MAX_SESSIONS],
        open_files: [OpenFile::EMPTY; MAX_OPEN_FILES],
        next_posix_pid: 100,
    };

    let service_port = libnanami::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let mut pending = (libnanami::OS_RESPONSE_OK, 0, 0);
    let mut has_reply = false;

    loop {
        let event = if has_reply {
            has_reply = false;
            match libnanami::ipc::service_reply_receive_event(service_port, pending.0, pending.1, pending.2) {
                Ok(e) => e,
                Err(e) => return Err(log_error("[posix-server] reply_receive failed: ", e)),
            }
        } else {
            match libnanami::ipc::service_receive_event(service_port) {
                Ok(e) => e,
                Err(e) => return Err(log_error("[posix-server] receive failed: ", e)),
            }
        };

        match event {
            ServiceEvent::Request(request) => {
                pending = handle_request(&mut runtime, request);
                has_reply = true;
            }
            ServiceEvent::Notification { .. } => {}
            ServiceEvent::Fault { identifier, reason, .. } => {
                libnanami::println!("[posix-server] fault id={} reason={:#x}", identifier, reason);
            }
        }
    }
}

fn connect_vfs_service() -> Word {
    let mut tries = 0usize;
    loop {
        match nanami_services::registry::connect_vfs_service(SLOT_VFS_SERVICE) {
            Ok(()) => return libnanami::ipc::process_slot_descriptor(SLOT_VFS_SERVICE),
            Err(e) => {
                if tries == 0 {
                    log_request_error("[posix-server] waiting vfs-service: ", e);
                }
                tries += 1;
                let mut spin = 0usize;
                while spin < 200_000 {
                    core::hint::spin_loop();
                    spin += 1;
                }
            }
        }
    }
}

fn handle_request(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    match request.code {
        POSIX_REQUEST_CONTROL => handle_control(runtime, request),
        POSIX_REQUEST_GETPID => match session_for_pid(runtime, request.identifier) {
            Some(index) => (libnanami::OS_RESPONSE_OK, runtime.sessions[index].posix_pid, 0),
            None => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
        },
        POSIX_REQUEST_GETCWD => handle_getcwd(runtime, request),
        POSIX_REQUEST_CHDIR => handle_chdir(runtime, request),
        POSIX_REQUEST_OPEN => handle_open(runtime, request),
        POSIX_REQUEST_CLOSE => handle_close(runtime, request),
        POSIX_REQUEST_DUP => handle_dup(runtime, request),
        POSIX_REQUEST_DUP2 => handle_dup2(runtime, request),
        POSIX_REQUEST_FCNTL => handle_fcntl(runtime, request),
        POSIX_REQUEST_GETENV => handle_getenv(runtime, request),
        POSIX_REQUEST_SETENV => handle_setenv(runtime, request),
        POSIX_REQUEST_UNSETENV => handle_unsetenv(runtime, request),
        POSIX_REQUEST_ENV_COUNT => handle_env_count(runtime, request),
        POSIX_REQUEST_ENV_AT => handle_env_at(runtime, request),
        POSIX_REQUEST_READ => handle_read(runtime, request),
        POSIX_REQUEST_WRITE => handle_write(runtime, request),
        POSIX_REQUEST_STAT => handle_stat(runtime, request),
        POSIX_REQUEST_MKDIR => handle_mkdir(runtime, request),
        POSIX_REQUEST_UNLINK => handle_unlink(runtime, request),
        POSIX_REQUEST_RENAME => handle_rename(runtime, request),
        POSIX_REQUEST_FSTAT => handle_fstat(runtime, request),
        POSIX_REQUEST_READ_DIR => handle_read_dir(runtime, request),
        POSIX_REQUEST_SEEK => handle_seek(runtime, request),
        POSIX_REQUEST_RMDIR => handle_rmdir(runtime, request),
        POSIX_REQUEST_GETPPID => handle_getppid(runtime, request),
        POSIX_REQUEST_GET_NATIVE_PID => handle_get_native_pid(runtime, request),
        POSIX_REQUEST_GETUID => handle_getuid(runtime, request),
        POSIX_REQUEST_GETEUID => handle_getuid(runtime, request),
        POSIX_REQUEST_GETGID => handle_getgid(runtime, request),
        POSIX_REQUEST_GETEGID => handle_getgid(runtime, request),
        POSIX_REQUEST_GETPGID => handle_getpgid(runtime, request),
        POSIX_REQUEST_GETSID => handle_getsid(runtime, request),
        POSIX_REQUEST_SETPGID => handle_setpgid(runtime, request),
        POSIX_REQUEST_SETSID => handle_setsid(runtime, request),
        POSIX_REQUEST_SPAWN => handle_spawn(runtime, request),
        POSIX_REQUEST_WAITPID => handle_waitpid(runtime, request),
        POSIX_REQUEST_FORK
        | POSIX_REQUEST_EXEC
        | POSIX_REQUEST_KILL => handle_unsupported_process_lifecycle(runtime, request),
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_control(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    match request.arg0 {
        POSIX_CONTROL_ATTACH_SHARED_MEMORY => {
            let size = if request.arg1 == 0 { POSIX_DEFAULT_SHM_BYTES } else { request.arg1 };
            match libnanami::request_shared_memory(request.identifier, size) {
                Ok((local, peer)) => match session_for_pid(runtime, request.identifier) {
                    Some(index) => {
                        runtime.sessions[index].shm_local = local;
                        runtime.sessions[index].shm_size = size;
                        (libnanami::OS_RESPONSE_OK, peer, size)
                    }
                    None => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
                },
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_getcwd(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let session = runtime.sessions[index];
    let out_offset = request.arg0 as usize;
    let max_len = request.arg1 as usize;
    if out_offset.saturating_add(session.cwd_len) > session.shm_size as usize || session.cwd_len > max_len {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    unsafe { ptr::copy_nonoverlapping(session.cwd.as_ptr(), (session.shm_local as usize + out_offset) as *mut u8, session.cwd_len); }
    (libnanami::OS_RESPONSE_OK, session.cwd_len as Word, 0)
}

fn handle_chdir(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let Some((path, len)) = resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize) else {
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

fn handle_open(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let Some((path, len)) = resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let kind = special_device_kind(&path[..len]);
    if kind != FdKind::Empty {
        if (request.arg2 & POSIX_O_DIRECTORY) != 0 {
            return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
        }
        return alloc_open_file_and_fd(runtime, index, kind, 0);
    }
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    if (request.arg2 & POSIX_O_TRUNC) != 0 {
        match nanami_services::vfs::vfs_stat(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
            Ok((_, _, kind)) if kind == nanami_services::vfs::VFS_FILE_TYPE_DIRECTORY => {
                return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
            }
            Ok(_) => {
                let _ = nanami_services::vfs::vfs_remove(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word);
                let _ = nanami_services::vfs::vfs_create(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word);
            }
            Err(_) => {}
        }
    }
    let mut handle = nanami_services::vfs::vfs_open(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word);
    if handle.is_err() && (request.arg2 & POSIX_O_CREAT) != 0 {
        let _ = nanami_services::vfs::vfs_create(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word);
        handle = nanami_services::vfs::vfs_open(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word);
    }
    match handle {
        Ok(vfs_handle) => {
            let fd_kind = match nanami_services::vfs::vfs_fstat(runtime.vfs_port, vfs_handle) {
                Ok((_, _, kind)) if kind == nanami_services::vfs::VFS_FILE_TYPE_DIRECTORY => FdKind::Directory,
                Ok(_) => FdKind::Regular,
                Err(e) => {
                    let _ = nanami_services::vfs::vfs_close(runtime.vfs_port, vfs_handle);
                    return (map_request_error_to_status(e), 0, 0);
                }
            };
            if (request.arg2 & POSIX_O_DIRECTORY) != 0 && fd_kind != FdKind::Directory {
                let _ = nanami_services::vfs::vfs_close(runtime.vfs_port, vfs_handle);
                return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
            }
            alloc_open_file_and_fd(runtime, index, fd_kind, vfs_handle)
        }
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

fn handle_read(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let fd = request.arg0 as usize;
    let out_offset = request.arg1 as usize;
    let len = request.arg2 as usize;
    if fd >= MAX_FDS || !runtime.sessions[index].fds[fd].active { return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0); }
    if out_offset.saturating_add(len) > runtime.sessions[index].shm_size as usize { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); }
    let open_index = runtime.sessions[index].fds[fd].open_file;
    if open_index >= runtime.open_files.len() || !runtime.open_files[open_index].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let mut entry = runtime.open_files[open_index];
    match entry.kind {
        FdKind::DevNull => (libnanami::OS_RESPONSE_OK, 0, 0),
        FdKind::DevZero => {
            unsafe { ptr::write_bytes((runtime.sessions[index].shm_local as usize + out_offset) as *mut u8, 0, len); }
            (libnanami::OS_RESPONSE_OK, len as Word, 0)
        }
        FdKind::Regular => {
            let chunk = len.min(1024);
            match nanami_services::vfs::vfs_read(runtime.vfs_port, entry.vfs_handle, entry.offset, chunk as Word, VFS_IO_OFFSET as Word) {
                Ok(bytes) => {
                    unsafe {
                        ptr::copy_nonoverlapping((runtime.vfs_shm as usize + VFS_IO_OFFSET) as *const u8, (runtime.sessions[index].shm_local as usize + out_offset) as *mut u8, bytes as usize);
                    }
                    entry.offset = entry.offset.saturating_add(bytes);
                    runtime.open_files[open_index] = entry;
                    (libnanami::OS_RESPONSE_OK, bytes, 0)
                }
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        _ => (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
    }
}

fn handle_write(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let fd = request.arg0 as usize;
    let input_offset = request.arg1 as usize;
    let len = request.arg2 as usize;
    if fd >= MAX_FDS || !runtime.sessions[index].fds[fd].active { return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0); }
    if input_offset.saturating_add(len) > runtime.sessions[index].shm_size as usize { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); }
    let open_index = runtime.sessions[index].fds[fd].open_file;
    if open_index >= runtime.open_files.len() || !runtime.open_files[open_index].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let mut entry = runtime.open_files[open_index];
    match entry.kind {
        FdKind::DevNull => (libnanami::OS_RESPONSE_OK, len as Word, 0),
        FdKind::Regular => {
            let chunk = len.min(1024);
            unsafe {
                ptr::copy_nonoverlapping((runtime.sessions[index].shm_local as usize + input_offset) as *const u8, (runtime.vfs_shm as usize + VFS_IO_OFFSET) as *mut u8, chunk);
            }
            match nanami_services::vfs::vfs_write(runtime.vfs_port, entry.vfs_handle, entry.offset, chunk as Word, VFS_IO_OFFSET as Word) {
                Ok(bytes) => {
                    entry.offset = entry.offset.saturating_add(bytes);
                    runtime.open_files[open_index] = entry;
                    (libnanami::OS_RESPONSE_OK, bytes, 0)
                }
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        _ => (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
    }
}

fn handle_stat(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let Some((path, len)) = resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let dev_kind = special_device_kind(&path[..len]);
    if dev_kind == FdKind::DevNull {
        return (libnanami::OS_RESPONSE_OK, 0, pack_stat(0, POSIX_FILE_TYPE_CHAR_DEVICE, POSIX_DEV_NULL_MAJOR, POSIX_DEV_NULL_MINOR));
    }
    if dev_kind == FdKind::DevZero {
        return (libnanami::OS_RESPONSE_OK, 0, pack_stat(0, POSIX_FILE_TYPE_CHAR_DEVICE, POSIX_DEV_ZERO_MAJOR, POSIX_DEV_ZERO_MINOR));
    }
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    match nanami_services::vfs::vfs_stat(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok((inode, size, kind)) => (libnanami::OS_RESPONSE_OK, inode, pack_stat(size, vfs_kind_to_posix(kind), 0, 0)),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

fn handle_mkdir(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let Some((path, len)) = resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    match nanami_services::vfs::vfs_mkdir(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok(_) => (libnanami::OS_RESPONSE_OK, 0, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

fn handle_unlink(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let Some((path, len)) = resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
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

fn handle_rmdir(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let Some((path, len)) = resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
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

fn handle_rename(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let Some((old_path, old_len)) = resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let Some((new_path, new_len)) = resolve_client_path(runtime, index, request.arg2 as usize, request.arg3 as usize) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    write_vfs_path(runtime, VFS_PATH_OFFSET, &old_path[..old_len]);
    write_vfs_path(runtime, VFS_PATH2_OFFSET, &new_path[..new_len]);
    match nanami_services::vfs::vfs_rename(runtime.vfs_port, VFS_PATH_OFFSET as Word, old_len as Word, VFS_PATH2_OFFSET as Word, new_len as Word) {
        Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

fn handle_fstat(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let fd = request.arg0 as usize;
    if fd >= MAX_FDS || !runtime.sessions[index].fds[fd].active { return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0); }
    let open_index = runtime.sessions[index].fds[fd].open_file;
    if open_index >= runtime.open_files.len() || !runtime.open_files[open_index].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let entry = runtime.open_files[open_index];
    match entry.kind {
        FdKind::DevNull => (libnanami::OS_RESPONSE_OK, 0, pack_stat(0, POSIX_FILE_TYPE_CHAR_DEVICE, POSIX_DEV_NULL_MAJOR, POSIX_DEV_NULL_MINOR)),
        FdKind::DevZero => (libnanami::OS_RESPONSE_OK, 0, pack_stat(0, POSIX_FILE_TYPE_CHAR_DEVICE, POSIX_DEV_ZERO_MAJOR, POSIX_DEV_ZERO_MINOR)),
        FdKind::Regular | FdKind::Directory => match nanami_services::vfs::vfs_fstat(runtime.vfs_port, entry.vfs_handle) {
            Ok((inode, size, kind)) => (libnanami::OS_RESPONSE_OK, inode, pack_stat(size, vfs_kind_to_posix(kind), 0, 0)),
            Err(e) => (map_request_error_to_status(e), 0, 0),
        },
        FdKind::Empty => (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0),
    }
}

fn handle_read_dir(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let fd = request.arg0 as usize;
    let max_entries = request.arg1;
    let out_offset = request.arg2 as usize;
    if fd >= MAX_FDS || !runtime.sessions[index].fds[fd].active { return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0); }
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
    let vfs_capacity = runtime.vfs_shm_size.saturating_sub(VFS_IO_OFFSET as Word) as usize / record_bytes;
    let capped_entries = (max_entries as usize).min(client_capacity).min(vfs_capacity) as Word;
    let bytes = (capped_entries as usize).saturating_mul(record_bytes);
    if out_offset.saturating_add(bytes) > runtime.sessions[index].shm_size as usize {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    if capped_entries == 0 {
        return (libnanami::OS_RESPONSE_OK, 0, entry.offset);
    }
    match nanami_services::vfs::vfs_read_dir(runtime.vfs_port, entry.vfs_handle, entry.offset, capped_entries, VFS_IO_OFFSET as Word) {
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

fn handle_seek(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let fd = request.arg0 as usize;
    if fd >= MAX_FDS || !runtime.sessions[index].fds[fd].active { return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0); }
    let open_index = runtime.sessions[index].fds[fd].open_file;
    if open_index >= runtime.open_files.len() || !runtime.open_files[open_index].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let entry = runtime.open_files[open_index];
    let base = match request.arg2 {
        POSIX_SEEK_SET => 0,
        POSIX_SEEK_CUR => entry.offset,
        POSIX_SEEK_END => match entry.kind {
            FdKind::Regular | FdKind::Directory => match nanami_services::vfs::vfs_fstat(runtime.vfs_port, entry.vfs_handle) {
                Ok((_, size, _)) => size,
                Err(e) => return (map_request_error_to_status(e), 0, 0),
            },
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

fn log_error(prefix: &str, error: RequestError) -> libnanami::NanamiError {
    log_request_error(prefix, error);
    libnanami::NanamiError::from(error)
}

pub(crate) fn log_request_error(prefix: &str, error: RequestError) {
    libnanami::print!("{}", prefix);
    libnanami::println!("{:?}", error);
}

libnanami::nanami_entry!(nanami_main);
