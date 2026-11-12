//! Proxy namespace operations, translating directory FDs into owned VFS handles.
use crate::*;
use nanami_client::vfs::*;

const ROOT_OFFSET: Word = 128;
const PATH_OFFSET: Word = ROOT_OFFSET + 4096;
const TARGET_OFFSET: Word = PATH_OFFSET + 4096;
const OUTPUT_OFFSET: Word = TARGET_OFFSET + 4096;

fn copy_input(
    runtime: &Runtime,
    session: &Session,
    offset: Word,
    bytes: Word,
    destination: Word,
) -> bool {
    if bytes > 4096
        || offset
            .checked_add(bytes)
            .is_none_or(|end| end > session.shm_size)
        || destination
            .checked_add(bytes)
            .is_none_or(|end| end > runtime.vfs_shm_size)
    {
        return false;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            (session.shm_local + offset) as *const u8,
            (runtime.vfs_shm + destination) as *mut u8,
            bytes,
        );
    }
    true
}

pub(crate) fn handle(runtime: &mut Runtime, message: ServiceRequest) -> (Word, Word, Word) {
    let invalid = (VFS_PATH_RESPONSE_ERROR, VFS_PATH_ERROR_INVALID, 0);
    let Some(index) = find_session(runtime, message.identifier) else {
        return invalid;
    };
    let session = &runtime.sessions[index];
    if message
        .arg0
        .checked_add(core::mem::size_of::<PathRequest>())
        .is_none_or(|end| end > session.shm_size)
    {
        return invalid;
    }
    let original = unsafe {
        core::ptr::read_unaligned((session.shm_local + message.arg0) as *const PathRequest)
    };
    let cursor_open = message.code == POSIX_REQUEST_OPEN_ROOTED_CURSOR_GRANTED;
    let granted = cursor_open || message.code == POSIX_REQUEST_OPEN_ROOTED_GRANTED;
    if (message.code == POSIX_REQUEST_OPEN_ROOTED || granted) != (original.operation == VFS_PATH_OPEN_READONLY) {
        // The legacy PATH entry must not silently acquire/return raw VFS handles.
        return (VFS_PATH_RESPONSE_ERROR, VFS_PATH_ERROR_UNSUPPORTED, 0);
    }
    if !copy_input(
        runtime,
        session,
        original.root_offset,
        original.root_bytes,
        ROOT_OFFSET,
    ) || !copy_input(
        runtime,
        session,
        original.path_offset,
        original.path_bytes,
        PATH_OFFSET,
    ) || !copy_input(
        runtime,
        session,
        original.target_offset,
        original.target_bytes,
        TARGET_OFFSET,
    ) || original
        .output_offset
        .checked_add(original.output_bytes)
        .is_none_or(|end| end > session.shm_size)
    {
        return invalid;
    }
    let absolute = original.path_bytes != 0
        && unsafe { *((runtime.vfs_shm + PATH_OFFSET) as *const u8) == b'/' };
    let directory = if absolute || original.directory == VFS_PATH_DIRECTORY_ROOT {
        VFS_PATH_DIRECTORY_ROOT
    } else {
        let Some(fd) = session.fds.get(original.directory).filter(|fd| fd.active) else {
            return invalid;
        };
        let Some(file) = runtime
            .open_files
            .get(fd.open_file)
            .filter(|file| file.active)
        else {
            return invalid;
        };
        if file.kind != FdKind::Directory {
            return (VFS_PATH_RESPONSE_ERROR, VFS_PATH_ERROR_NOT_DIRECTORY, 0);
        }
        file.vfs_handle
    };
    let mut request = PathRequest {
        directory,
        root_offset: ROOT_OFFSET,
        path_offset: PATH_OFFSET,
        target_offset: TARGET_OFFSET,
        output_offset: OUTPUT_OFFSET,
        output_bytes: original
            .output_bytes
            .min(runtime.vfs_shm_size.saturating_sub(OUTPUT_OFFSET)),
        ..original
    };
    let client_output = session.shm_local + original.output_offset;
    let open_prefix = core::mem::size_of::<PathOpenResult>();
    let requested_open = request.operation == VFS_PATH_OPEN_READONLY;
    let output_limit = request.output_bytes;
    let resolve_only = requested_open && !can_allocate_open_file_and_fd(runtime, index);
    if requested_open {
        if request.output_bytes < open_prefix || request.target_bytes != core::mem::size_of::<Word>()
            || request.flags & !(VFS_PATH_FOLLOW_FINAL | VFS_PATH_ALLOW_MISSING) != 0
        {
            return invalid;
        }
        let options = unsafe {
            core::ptr::read_unaligned((runtime.vfs_shm + TARGET_OFFSET) as *const Word)
        };
        if Word::from_le(options) & !VFS_OPEN_DIRECTORY != 0 {
            return invalid;
        }
        // A malformed reply must not expose an old descriptor left in SHM.
        unsafe {
            core::ptr::write_unaligned(
                (runtime.vfs_shm + OUTPUT_OFFSET) as *mut PathOpenResult,
                PathOpenResult { descriptor: VFS_PATH_OPEN_NONE },
            );
        }
        if resolve_only {
            // Preserve virtual/fallback opens even when this proxy cannot own
            // another FD. No VFS handle may be acquired on this route.
            request.operation = VFS_PATH_RESOLVE;
            request.output_offset += open_prefix;
            request.output_bytes -= open_prefix;
        }
    }
    unsafe {
        core::ptr::write_unaligned(runtime.vfs_shm as *mut PathRequest, request);
    }
    let result = if granted && !resolve_only {
        let mut delegated = request;
        delegated.flags |= runtime.sessions[index].direct_vfs_delegate << VFS_DELEGATE_ID_SHIFT;
        unsafe { core::ptr::write_unaligned(runtime.vfs_shm as *mut PathRequest, delegated); }
        let result = if cursor_open {
            match vfs_rooted_open_cursor_granted_request(runtime.vfs_port, 0) {
                Err(PathError::Filesystem(VFS_PATH_ERROR_UNSUPPORTED)) =>
                    vfs_rooted_open_granted_request(runtime.vfs_port, 0),
                result => result,
            }
        } else { vfs_rooted_open_granted_request(runtime.vfs_port, 0) };
        match result {
            Err(PathError::Filesystem(VFS_PATH_ERROR_UNSUPPORTED)) => {
                unsafe { core::ptr::write_unaligned(runtime.vfs_shm as *mut PathRequest, request); }
                vfs_path_request(runtime.vfs_port, 0)
            }
            other => other,
        }
    } else { vfs_path_request(runtime.vfs_port, 0) };
    match result {
        Ok((mut detail0, wire_type)) => {
            let cursor = cursor_open && wire_type & VFS_OPEN_CURSOR_TYPE_FLAG != 0;
            let detail1 = if granted { wire_type & VFS_OPEN_GRANT_TYPE_MASK } else { wire_type };
            let detail1 = if cursor_open { detail1 & !VFS_OPEN_CURSOR_TYPE_FLAG } else { detail1 };
            if resolve_only {
                let Some(total) = detail0.checked_add(open_prefix) else { return invalid; };
                detail0 = total;
            }
            if requested_open {
                let opened = unsafe {
                    core::ptr::read_unaligned(
                        (runtime.vfs_shm + OUTPUT_OFFSET) as *const PathOpenResult,
                    )
                };
                if detail0 <= open_prefix || detail0 > output_limit
                    || unsafe { *((runtime.vfs_shm + OUTPUT_OFFSET + open_prefix) as *const u8) } != b'/'
                    || (opened.descriptor != VFS_PATH_OPEN_NONE
                        && !matches!(detail1, VFS_FILE_TYPE_REGULAR | VFS_FILE_TYPE_DIRECTORY))
                    || (cursor && (opened.descriptor == VFS_PATH_OPEN_NONE
                        || detail1 != VFS_FILE_TYPE_REGULAR || wire_type >> VFS_OPEN_GRANT_TYPE_BITS == 0))
                {
                    if opened.descriptor != VFS_PATH_OPEN_NONE {
                        let _ = vfs_close(runtime.vfs_port, opened.descriptor);
                    }
                    return invalid;
                }
                let descriptor = if opened.descriptor == VFS_PATH_OPEN_NONE {
                    VFS_PATH_OPEN_NONE
                } else {
                    let kind = if detail1 == VFS_FILE_TYPE_DIRECTORY {
                        FdKind::Directory
                    } else {
                        FdKind::Regular
                    };
                    let (status, fd, _) = alloc_open_file_and_fd(
                        runtime, index, kind, opened.descriptor, 0,
                    );
                    if status != nanami_runtime::OS_RESPONSE_OK {
                        // The allocator closes the VFS handle on either failure.
                        return (status, 0, 0);
                    }
                    let open_index = runtime.sessions[index].fds[fd].open_file;
                    runtime.open_files[open_index].vfs_cursor = cursor;
                    fd
                };
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        (runtime.vfs_shm + OUTPUT_OFFSET) as *const u8,
                        client_output as *mut u8,
                        detail0,
                    );
                    core::ptr::write_unaligned(
                        client_output as *mut PathOpenResult,
                        PathOpenResult { descriptor },
                    );
                }
            } else if matches!(request.operation, VFS_PATH_RESOLVE | VFS_PATH_READLINK) {
                if detail0 > request.output_bytes {
                    return invalid;
                }
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        (runtime.vfs_shm + OUTPUT_OFFSET) as *const u8,
                        client_output as *mut u8,
                        detail0,
                    );
                }
            }
            (nanami_runtime::OS_RESPONSE_OK, detail0, if granted { wire_type } else { detail1 })
        }
        Err(PathError::Filesystem(error)) => (VFS_PATH_RESPONSE_ERROR, error, 0),
        Err(PathError::Transport(error)) => (map_request_error_to_status(error), 0, 0),
    }
}

pub(crate) fn handle_fchmod(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
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
    if !matches!(file.kind, FdKind::Regular | FdKind::Directory) {
        return (VFS_PATH_RESPONSE_ERROR, VFS_PATH_ERROR_UNSUPPORTED, 0);
    }
    match vfs_fchmod(runtime.vfs_port, file.vfs_handle, request.arg1) {
        Ok(()) => (nanami_runtime::OS_RESPONSE_OK, 0, 0),
        Err(PathError::Filesystem(error)) => (VFS_PATH_RESPONSE_ERROR, error, 0),
        Err(PathError::Transport(error)) => (map_request_error_to_status(error), 0, 0),
    }
}
