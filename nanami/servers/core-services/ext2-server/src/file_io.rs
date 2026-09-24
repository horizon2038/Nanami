use super::*;

pub(crate) fn handle_ftruncate(
    request: ServiceRequest,
    runtime: &mut Ext2Runtime,
) -> (Word, Word, Word) {
    let Some(file) = runtime.handles.get(request.arg0).copied().filter(|file| {
        file.active && file.owner_pid == request.identifier
    }) else {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    if file.mode & 0xf000 != EXT2_S_IFREG {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let result = read_inode(runtime, file.inode).and_then(|mut inode| {
        truncate::resize_inode(runtime, file.inode, &mut inode, request.arg1)
    });
    match result {
        Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
        Err(error) => (map_request_error_to_status(error), 0, 0),
    }
}

pub(crate) fn handle_read(
    request: ServiceRequest,
    runtime: &mut Ext2Runtime,
) -> (Word, Word, Word) {
    let Some(session) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    handle_read_into(request, runtime, session, request.arg3 as usize)
}

pub(crate) fn handle_read_delegated(
    request: ServiceRequest,
    runtime: &mut Ext2Runtime,
) -> (Word, Word, Word) {
    let Some((session, out_offset)) = delegated_buffer(request, runtime) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    handle_read_into(request, runtime, session, out_offset)
}

fn delegated_buffer(
    request: ServiceRequest,
    runtime: &Ext2Runtime,
) -> Option<(ClientSession, usize)> {
    let delegate_id = (request.arg3 >> nanami_services::vfs::VFS_DELEGATE_ID_SHIFT) as usize;
    let index = delegate_id.checked_sub(1)?;
    let delegated = runtime.delegated_sessions.get(index)?;
    if !delegated.active || delegated.owner_pid != request.identifier {
        return None;
    }
    let session = ClientSession {
        active: true,
        pid: delegated.peer_pid,
        shm_local: delegated.shm_local,
        shm_size: delegated.shm_size,
    };
    let offset = (request.arg3 & nanami_services::vfs::VFS_DELEGATE_VALUE_MASK) as usize;
    Some((session, offset))
}

pub(crate) fn handle_read_into(
    request: ServiceRequest,
    runtime: &mut Ext2Runtime,
    session: ClientSession,
    out_offset: usize,
) -> (Word, Word, Word) {
    let handle = request.arg0 as usize;
    if handle >= runtime.handles.len()
        || !runtime.handles[handle].active
        || runtime.handles[handle].owner_pid != request.identifier
    {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let file = runtime.handles[handle];
    if (file.mode & EXT2_S_IFREG) != EXT2_S_IFREG {
        return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    }
    if request.arg1 as usize >= file.size as usize || request.arg2 == 0 {
        return (libnanami::OS_RESPONSE_OK, 0, 0);
    }
    let Ok(inode) = read_inode(runtime, file.inode) else {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    match read_file(
        runtime,
        session,
        inode,
        request.arg1 as usize,
        request.arg2 as usize,
        out_offset,
    ) {
        Ok(bytes) => (libnanami::OS_RESPONSE_OK, bytes as Word, 0),
        Err(status) => (status, 0, 0),
    }
}

pub(crate) fn handle_close(
    request: ServiceRequest,
    runtime: &mut Ext2Runtime,
) -> (Word, Word, Word) {
    let handle = request.arg0 as usize;
    if handle < runtime.handles.len()
        && runtime.handles[handle].active
        && runtime.handles[handle].owner_pid == request.identifier
    {
        runtime.handles[handle] = FileHandle::EMPTY;
        return (libnanami::OS_RESPONSE_OK, 0, 0);
    }
    (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0)
}

pub(crate) fn handle_read_dir(
    request: ServiceRequest,
    runtime: &mut Ext2Runtime,
) -> (Word, Word, Word) {
    let handle = request.arg0 as usize;
    if handle >= runtime.handles.len()
        || !runtime.handles[handle].active
        || runtime.handles[handle].owner_pid != request.identifier
    {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let Some(session) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let file = runtime.handles[handle];
    if (file.mode & EXT2_S_IFDIR) != EXT2_S_IFDIR {
        return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    }
    let Ok(inode) = read_inode(runtime, file.inode) else {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    match read_directory(
        runtime,
        session,
        inode,
        request.arg1 as usize,
        request.arg2 as usize,
        request.arg3 as usize,
    ) {
        Ok((entries, next_index)) => (
            libnanami::OS_RESPONSE_OK,
            entries as Word,
            next_index as Word,
        ),
        Err(status) => (status, 0, 0),
    }
}

pub(crate) fn handle_write_file(
    request: ServiceRequest,
    runtime: &mut Ext2Runtime,
) -> (Word, Word, Word) {
    handle_write_from(request, runtime, None, request.arg3 as usize)
}

pub(crate) fn handle_write_delegated(
    request: ServiceRequest,
    runtime: &mut Ext2Runtime,
) -> (Word, Word, Word) {
    let Some((session, input_offset)) = delegated_buffer(request, runtime) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    handle_write_from(request, runtime, Some(session), input_offset)
}

fn handle_write_from(
    request: ServiceRequest,
    runtime: &mut Ext2Runtime,
    session: Option<ClientSession>,
    input_offset: usize,
) -> (Word, Word, Word) {
    let handle = request.arg0 as usize;
    if handle >= runtime.handles.len()
        || !runtime.handles[handle].active
        || runtime.handles[handle].owner_pid != request.identifier
    {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let Some(session) = session.or_else(|| find_session(runtime, request.identifier)) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let file = runtime.handles[handle];
    if (file.mode & EXT2_S_IFREG) != EXT2_S_IFREG {
        return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    }
    let len = request.arg2 as usize;
    if input_offset
        .checked_add(len)
        .filter(|end| *end <= session.shm_size as usize)
        .is_none()
    {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let Ok(mut inode) = read_inode(runtime, file.inode) else {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    match write_file(
        runtime,
        session,
        &mut inode,
        file.inode,
        request.arg1 as usize,
        len,
        input_offset,
    ) {
        Ok(bytes) => {
            runtime.handles[handle].size = inode.size;
            (libnanami::OS_RESPONSE_OK, bytes as Word, 0)
        }
        Err(status) => (status, 0, 0),
    }
}
