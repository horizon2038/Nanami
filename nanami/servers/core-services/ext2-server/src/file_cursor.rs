//! One open-description position, shared by owner and authorized readonly peer.
//! The ordinary positioned operations never inspect or change this position.
use super::*;
use nanami_client::vfs::{VFS_CURSOR_SEEK_SET, VFS_CURSOR_SEEK_CUR, VFS_CURSOR_SEEK_END, VFS_DELEGATE_ID_SHIFT};

fn owned(runtime: &Ext2Runtime, request: &ServiceRequest) -> Option<usize> {
    runtime.handles.get(request.arg0)
        .filter(|file| file.active && file.owner_pid == request.identifier)
        .map(|_| request.arg0)
}

pub(super) fn read_owned(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let Some(handle) = owned(runtime, &request) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    let buffer = if request.arg3 >> VFS_DELEGATE_ID_SHIFT == 0 {
        find_session(runtime, request.identifier).map(|session| (session, request.arg3))
    } else { super::file_io::delegated_buffer(request, runtime) };
    let Some((session, output)) = buffer else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    read_into(request, runtime, handle, session, output)
}

pub(super) fn read_granted(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let Some((handle, session)) = super::read_grants::resolve(runtime, request.identifier, request.arg0) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    read_into(request, runtime, handle, session, request.arg3)
}

fn read_into(mut request: ServiceRequest, runtime: &mut Ext2Runtime, handle: usize,
    session: ClientSession, output: Word,
) -> (Word, Word, Word) {
    let file = runtime.handles[handle];
    let Some(position) = file.cursor.filter(|_| file.mode & EXT2_S_IFMT == EXT2_S_IFREG) else {
        return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    };
    if output.checked_add(request.arg2).is_none_or(|end| end > session.shm_size) {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    if request.arg1 != 0 { return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); }
    request.arg1 = position;
    let (status, bytes, detail) = super::file_io::read_file_handle(request, runtime, session, output, file);
    if status != nanami_runtime::OS_RESPONSE_OK { return (status, bytes, detail); }
    if bytes > request.arg2 { return (nanami_runtime::OS_RESPONSE_FATAL, 0, 0); }
    let next = position.saturating_add(bytes);
    // Ext2 dispatches no other request during this read. Commit after success,
    // before replying; a lost reply must never be mistaken for no mutation.
    runtime.handles[handle].cursor = Some(next);
    (status, bytes, next)
}

pub(super) fn seek_owned(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let Some(handle) = owned(runtime, &request) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    seek(request, runtime, handle)
}

pub(super) fn seek_granted(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let Some((handle, _)) = super::read_grants::resolve(runtime, request.identifier, request.arg0) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    seek(request, runtime, handle)
}

fn seek(request: ServiceRequest, runtime: &mut Ext2Runtime, handle: usize) -> (Word, Word, Word) {
    let file = runtime.handles[handle];
    let Some(position) = file.cursor.filter(|_| file.mode & EXT2_S_IFMT == EXT2_S_IFREG) else {
        return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    };
    let base = match request.arg2 {
        VFS_CURSOR_SEEK_SET => 0,
        VFS_CURSOR_SEEK_CUR => position,
        // This is the same open-handle size returned by FSTAT, which the
        // existing POSIX SEEK_END queried. No local POSIX offset is consulted.
        VFS_CURSOR_SEEK_END => file.size as Word,
        _ => return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    };
    let next = base.saturating_add(request.arg1);
    runtime.handles[handle].cursor = Some(next);
    (nanami_runtime::OS_RESPONSE_OK, next, 0)
}
