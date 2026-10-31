use super::*;

pub(crate) fn statfs(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(session) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some(file) = runtime.sessions[session]
        .fds
        .get(request.arg0)
        .filter(|fd| fd.active)
        .and_then(|fd| runtime.open_files.get(fd.open_file))
        .filter(|file| file.active)
    else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    let bytes = core::mem::size_of::<nanami_client::vfs::FilesystemStats>();
    if runtime.sessions[session].shm_size < bytes || runtime.vfs_shm_size < bytes {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    if matches!(file.kind, FdKind::DevNull | FdKind::DevZero) {
        let stats = nanami_client::vfs::FilesystemStats {
            kind: 0x01021994,
            block_size: 4096,
            name_max: 255,
            ..nanami_client::vfs::FilesystemStats::default()
        };
        unsafe {
            core::ptr::write_unaligned(
                runtime.sessions[session].shm_local as *mut nanami_client::vfs::FilesystemStats,
                stats,
            );
        }
        return (nanami_runtime::OS_RESPONSE_OK, 0, 0);
    }
    if !matches!(file.kind, FdKind::Regular | FdKind::Directory) {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    match nanami_runtime::call_service_port(
        runtime.vfs_port,
        nanami_client::vfs::VFS_REQUEST_FSTATFS,
        file.vfs_handle,
        0,
        0,
        0,
        2,
    ) {
        Ok((status, _, _)) if status == nanami_runtime::OS_RESPONSE_OK => {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    runtime.vfs_shm as *const u8,
                    runtime.sessions[session].shm_local as *mut u8,
                    bytes,
                );
            }
            (status, 0, 0)
        }
        Ok(reply) => reply,
        Err(error) => (map_request_error_to_status(error), 0, 0),
    }
}

pub(crate) fn handle(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(session) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some(file) = runtime.sessions[session]
        .fds
        .get(request.arg0)
        .filter(|fd| fd.active)
        .and_then(|fd| runtime.open_files.get(fd.open_file))
        .filter(|file| file.active && file.kind == FdKind::Regular)
    else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    let code = if request.code == POSIX_REQUEST_MAP_READ_PAGE {
        nanami_client::vfs::VFS_REQUEST_MAP_READ_PAGE
    } else {
        nanami_client::vfs::VFS_REQUEST_UNMAP_READ_PAGE
    };
    // The peer is authenticated by the IPC badge, never chosen by the client.
    match nanami_runtime::call_service_port(
        runtime.vfs_port,
        code,
        file.vfs_handle,
        request.arg1,
        request.identifier,
        0,
        4,
    ) {
        Ok(reply) => reply,
        Err(error) => (map_request_error_to_status(error), 0, 0),
    }
}
