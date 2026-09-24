use super::*;

pub(crate) fn handle_ftruncate(
    runtime: &mut Runtime,
    request: ServiceRequest,
) -> (Word, Word, Word) {
    let Some(session) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some(file) = runtime.sessions[session]
        .fds
        .get(request.arg0)
        .filter(|fd| fd.active)
        .and_then(|fd| runtime.open_files.get(fd.open_file))
        .filter(|file| file.active)
    else {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    if file.kind != FdKind::Regular {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    // Truncation does not change the open-file-description offset (including dup).
    let result =
        nanami_services::vfs::vfs_ftruncate(runtime.vfs_port, file.vfs_handle, request.arg1)
            .and_then(|()| {
                if file.status_flags & POSIX_O_SYNC != 0 {
                    nanami_services::vfs::vfs_fsync(runtime.vfs_port, file.vfs_handle)
                } else {
                    Ok(())
                }
            });
    match result {
        Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
        Err(error) => (map_request_error_to_status(error), 0, 0),
    }
}
