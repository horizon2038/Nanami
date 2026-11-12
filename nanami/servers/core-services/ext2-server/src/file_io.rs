use super::*;

pub(crate) fn handle_ftruncate(
    request: ServiceRequest,
    runtime: &mut Ext2Runtime,
) -> (Word, Word, Word) {
    let Some(file) = runtime.handles.get(request.arg0).copied().filter(|file| {
        file.active && file.owner_pid == request.identifier
    }) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    if file.mode & 0xf000 != EXT2_S_IFREG {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let result = read_inode(runtime, file.inode).and_then(|mut inode| {
        truncate::resize_inode(runtime, file.inode, &mut inode, request.arg1)
    });
    match result {
        Ok(()) => (nanami_runtime::OS_RESPONSE_OK, 0, 0),
        Err(error) => (map_request_error_to_status(error), 0, 0),
    }
}

pub(crate) fn handle_read(
    request: ServiceRequest,
    runtime: &mut Ext2Runtime,
) -> (Word, Word, Word) {
    let Some(session) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    handle_read_into(request, runtime, session, request.arg3 as usize)
}

pub(crate) fn handle_read_delegated(
    request: ServiceRequest,
    runtime: &mut Ext2Runtime,
) -> (Word, Word, Word) {
    let Some((session, out_offset)) = delegated_buffer(request, runtime) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    handle_read_into(request, runtime, session, out_offset)
}

pub(super) fn versioned_file_identity(runtime: &mut Ext2Runtime, file: FileHandle) -> Word {
    let version = if file.mode & EXT2_S_IFMT == EXT2_S_IFREG && !runtime.mapped_pages.pins(file.inode) {
        runtime.file_versions.get(file.inode)
    } else {
        0
    };
    file.inode as Word | ((version as Word) << 32)
}

pub(crate) fn handle_read_versioned(
    request: ServiceRequest, runtime: &mut Ext2Runtime,
) -> (Word, Word, Word) {
    let Some((session, output)) = delegated_buffer(request, runtime) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some(file) = runtime.handles.get(request.arg0).copied()
        .filter(|file| file.active && file.owner_pid == request.identifier)
    else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    read_versioned_into(request, runtime, session, output, file)
}

fn read_versioned_into(
    request: ServiceRequest, runtime: &mut Ext2Runtime,
    session: ClientSession, output: Word, file: FileHandle,
) -> (Word, Word, Word) {
    let identity = request.arg1;
    let size = request.arg2;
    if identity >> 32 == 0 || size == 0
        || output.checked_add(size).is_none_or(|end| end > session.shm_size)
    {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    if file.size as Word != size || versioned_file_identity(runtime, file) != identity {
        return (nanami_runtime::OS_RESPONSE_OK, 0, 0);
    }
    let Ok(inode) = read_inode(runtime, file.inode) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    if inode.size as Word != size {
        return (nanami_runtime::OS_RESPONSE_OK, 0, 0);
    }
    // The server dispatches no filesystem mutation during this positioned
    // read. Live file mappings were excluded by the version check above.
    match read_file(runtime, session, inode, 0, size, output) {
        Ok(bytes) if bytes == size => (nanami_runtime::OS_RESPONSE_OK, bytes, identity),
        Ok(_) => (nanami_runtime::OS_RESPONSE_OK, 0, 0),
        Err(status) => (status, 0, 0),
    }
}

pub(super) fn delegated_buffer(
    request: ServiceRequest,
    runtime: &Ext2Runtime,
) -> Option<(ClientSession, usize)> {
    let delegate_id = (request.arg3 >> nanami_client::vfs::VFS_DELEGATE_ID_SHIFT) as usize;
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
    let offset = (request.arg3 & nanami_client::vfs::VFS_DELEGATE_VALUE_MASK) as usize;
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
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let file = runtime.handles[handle];
    read_file_handle(request, runtime, session, out_offset, file)
}

pub(super) fn read_file_handle(
    request: ServiceRequest, runtime: &mut Ext2Runtime,
    session: ClientSession, out_offset: Word, file: FileHandle,
) -> (Word, Word, Word) {
    if (file.mode & EXT2_S_IFMT) != EXT2_S_IFREG {
        return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    }
    if request.arg1 as usize >= file.size as usize || request.arg2 == 0 {
        return (nanami_runtime::OS_RESPONSE_OK, 0, 0);
    }
    let Ok(inode) = read_inode(runtime, file.inode) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    match read_file(
        runtime,
        session,
        inode,
        request.arg1 as usize,
        request.arg2 as usize,
        out_offset,
    ) {
        Ok(bytes) => (nanami_runtime::OS_RESPONSE_OK, bytes as Word, 0),
        Err(status) => (status, 0, 0),
    }
}

pub(crate) fn handle_fstat_granted(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let Some((handle, _)) = super::read_grants::resolve(runtime, request.identifier, request.arg0) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    let file = runtime.handles[handle];
    super::file_stat(runtime, file, request.arg1)
}

pub(crate) fn handle_read_granted(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let Some((handle, session)) = super::read_grants::resolve(runtime, request.identifier, request.arg0) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    let file = runtime.handles[handle];
    read_file_handle(request, runtime, session, request.arg3, file)
}

pub(crate) fn handle_read_versioned_granted(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let Some((handle, session)) = super::read_grants::resolve(runtime, request.identifier, request.arg0) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    let file = runtime.handles[handle];
    read_versioned_into(request, runtime, session, request.arg3, file)
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
        let file = runtime.handles[handle];
        if matches!(
            file.mode & EXT2_S_IFMT,
            EXT2_S_IFREG | EXT2_S_IFDIR | EXT2_S_IFSOCK
        ) && runtime.frozen_by.is_some()
        {
            runtime.handles[handle].active = false;
            runtime.handles[handle].close_pending = true;
            return (nanami_runtime::OS_RESPONSE_OK, 0, 0);
        }
        runtime.handles[handle] = FileHandle::EMPTY;
        if matches!(
            file.mode & EXT2_S_IFMT,
            EXT2_S_IFREG | EXT2_S_IFDIR | EXT2_S_IFSOCK
        ) {
            if let Err(error) = release_unlinked_inode(runtime, file.inode) {
                runtime.handles[handle] = file;
                return (map_request_error_to_status(error), 0, 0);
            }
        }
        return (nanami_runtime::OS_RESPONSE_OK, 0, 0);
    }
    (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0)
}

pub(super) fn reclaim_closed_inodes(runtime: &mut Ext2Runtime) -> Result<(), RequestError> {
    for index in 0..runtime.handles.len() {
        let file = runtime.handles[index];
        if file.close_pending {
            runtime.handles[index] = FileHandle::EMPTY;
            if let Err(error) = release_unlinked_inode(runtime, file.inode) {
                runtime.handles[index] = file;
                return Err(error);
            }
        }
    }
    Ok(())
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
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let Some(session) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let file = runtime.handles[handle];
    if (file.mode & EXT2_S_IFMT) != EXT2_S_IFDIR {
        return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    }
    let Ok(inode) = read_inode(runtime, file.inode) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    match read_directory(
        runtime,
        session,
        inode,
        request.arg1 as usize,
        request.arg2 as usize,
        request.arg3 as usize,
        handle,
    ) {
        Ok((entries, next_index)) => (
            nanami_runtime::OS_RESPONSE_OK,
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
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
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
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let Some(session) = session.or_else(|| find_session(runtime, request.identifier)) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let file = runtime.handles[handle];
    if (file.mode & EXT2_S_IFMT) != EXT2_S_IFREG {
        return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    }
    let len = request.arg2 as usize;
    if input_offset
        .checked_add(len)
        .filter(|end| *end <= session.shm_size as usize)
        .is_none()
    {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let Ok(mut inode) = read_inode(runtime, file.inode) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
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
            (nanami_runtime::OS_RESPONSE_OK, bytes as Word, 0)
        }
        Err(status) => (status, 0, 0),
    }
}
