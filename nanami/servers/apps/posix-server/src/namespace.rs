//! Proxy namespace operations, translating directory FDs into owned VFS handles.
use crate::*;
use nanami_client::vfs::*;

const ROOT_OFFSET: Word = 128;
const PATH_OFFSET: Word = ROOT_OFFSET + 4096;
const TARGET_OFFSET: Word = PATH_OFFSET + 4096;
const OUTPUT_OFFSET: Word = TARGET_OFFSET + 4096;

fn copy_input(
    runtime: &Runtime,
    session: Session,
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
    let session = runtime.sessions[index];
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
    let request = PathRequest {
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
    unsafe {
        core::ptr::write_unaligned(runtime.vfs_shm as *mut PathRequest, request);
    }
    match vfs_path_request(runtime.vfs_port, 0) {
        Ok((detail0, detail1)) => {
            if matches!(request.operation, VFS_PATH_RESOLVE | VFS_PATH_READLINK) {
                if detail0 > request.output_bytes {
                    return invalid;
                }
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        (runtime.vfs_shm + OUTPUT_OFFSET) as *const u8,
                        (session.shm_local + original.output_offset) as *mut u8,
                        detail0,
                    );
                }
            }
            (nanami_runtime::OS_RESPONSE_OK, detail0, detail1)
        }
        Err(PathError::Filesystem(error)) => (VFS_PATH_RESPONSE_ERROR, error, 0),
        Err(PathError::Transport(error)) => (map_request_error_to_status(error), 0, 0),
    }
}
