use super::*;

pub(crate) fn handle_sync(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(session) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let result = if request.code == POSIX_REQUEST_SYNC {
        nanami_services::vfs::vfs_sync(runtime.vfs_port)
    } else {
        let Some(fd) = runtime.sessions[session]
            .fds
            .get(request.arg0)
            .filter(|fd| fd.active)
        else {
            return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
        };
        let Some(file) = runtime
            .open_files
            .get(fd.open_file)
            .filter(|file| file.active)
        else {
            return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
        };
        if !matches!(file.kind, FdKind::Regular | FdKind::Directory) {
            return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
        }
        nanami_services::vfs::vfs_fsync(runtime.vfs_port, file.vfs_handle)
    };
    match result {
        Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
        Err(error) => (map_request_error_to_status(error), 0, 0),
    }
}
