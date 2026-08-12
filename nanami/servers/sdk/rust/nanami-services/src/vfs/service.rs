use a9n_abi::CapabilityDescriptor;

use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

use super::constants::{
    VFS_CONTROL_ATTACH_DELEGATED_SHARED_MEMORY, VFS_CONTROL_ATTACH_SHARED_MEMORY,
    VFS_DELEGATE_ID_SHIFT, VFS_DELEGATE_VALUE_MASK, VFS_OPEN_HANDLE_MASK, VFS_OPEN_INODE_SHIFT,
    VFS_REQUEST_CLOSE, VFS_REQUEST_CONTROL, VFS_REQUEST_CREATE, VFS_REQUEST_FSTAT,
    VFS_REQUEST_MKDIR, VFS_REQUEST_OPEN, VFS_REQUEST_OPEN_COMPOUND, VFS_REQUEST_READ,
    VFS_REQUEST_READ_DELEGATED, VFS_REQUEST_READ_DIR, VFS_REQUEST_REMOVE, VFS_REQUEST_RENAME,
    VFS_REQUEST_STAT, VFS_REQUEST_WRITE, VFS_STAT_SIZE_MASK, VFS_STAT_TYPE_SHIFT,
};

pub fn vfs_attach_shared_memory(
    service_port: CapabilityDescriptor,
    size_bytes: Word,
) -> Result<(Word, Word), RequestError> {
    let (status, local_vaddr, mapped_size) = call_port(
        service_port,
        VFS_REQUEST_CONTROL,
        VFS_CONTROL_ATTACH_SHARED_MEMORY,
        size_bytes,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((local_vaddr, mapped_size))
}

pub fn vfs_attach_delegated_shared_memory(
    service_port: CapabilityDescriptor,
    peer_pid: Word,
    size_bytes: Word,
) -> Result<(Word, Word, Word), RequestError> {
    let (status, peer_vaddr, packed) = call_port(
        service_port,
        VFS_REQUEST_CONTROL,
        VFS_CONTROL_ATTACH_DELEGATED_SHARED_MEMORY,
        peer_pid,
        size_bytes,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    let mapped_size = packed & VFS_DELEGATE_VALUE_MASK;
    let delegate_id = packed >> VFS_DELEGATE_ID_SHIFT;
    Ok((peer_vaddr, mapped_size, delegate_id))
}

pub fn vfs_open(
    service_port: CapabilityDescriptor,
    path_offset: Word,
    path_len: Word,
) -> Result<Word, RequestError> {
    let (status, handle, _) = call_port(
        service_port,
        VFS_REQUEST_OPEN,
        path_offset,
        path_len,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(handle)
}

pub fn vfs_open_compound(
    service_port: CapabilityDescriptor,
    path_offset: Word,
    path_len: Word,
    flags: Word,
) -> Result<(Word, Word, Word, Word), RequestError> {
    let (status, opened, metadata) = call_port(
        service_port,
        VFS_REQUEST_OPEN_COMPOUND,
        path_offset,
        path_len,
        flags,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    let handle = opened & VFS_OPEN_HANDLE_MASK;
    let inode = opened >> VFS_OPEN_INODE_SHIFT;
    let (_, size, kind) = decode_stat(inode, metadata);
    Ok((handle, inode, size, kind))
}

pub fn vfs_read(
    service_port: CapabilityDescriptor,
    handle: Word,
    file_offset: Word,
    len: Word,
    out_offset: Word,
) -> Result<Word, RequestError> {
    let (status, bytes, _) = call_port(
        service_port,
        VFS_REQUEST_READ,
        handle,
        file_offset,
        len,
        out_offset,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(bytes)
}

pub fn vfs_read_delegated(
    service_port: CapabilityDescriptor,
    handle: Word,
    file_offset: Word,
    len: Word,
    delegate_id: Word,
    out_offset: Word,
) -> Result<Word, RequestError> {
    if delegate_id == 0
        || delegate_id > VFS_DELEGATE_VALUE_MASK
        || out_offset > VFS_DELEGATE_VALUE_MASK
    {
        return Err(RequestError::InvalidArgument);
    }
    let destination = (delegate_id << VFS_DELEGATE_ID_SHIFT) | out_offset;
    let (status, bytes, _) = call_port(
        service_port,
        VFS_REQUEST_READ_DELEGATED,
        handle,
        file_offset,
        len,
        destination,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(bytes)
}

pub fn vfs_write(
    service_port: CapabilityDescriptor,
    handle: Word,
    file_offset: Word,
    len: Word,
    input_offset: Word,
) -> Result<Word, RequestError> {
    let (status, bytes, _) = call_port(
        service_port,
        VFS_REQUEST_WRITE,
        handle,
        file_offset,
        len,
        input_offset,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(bytes)
}

pub fn vfs_create(
    service_port: CapabilityDescriptor,
    path_offset: Word,
    path_len: Word,
) -> Result<Word, RequestError> {
    let (status, inode, _) = call_port(
        service_port,
        VFS_REQUEST_CREATE,
        path_offset,
        path_len,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(inode)
}

pub fn vfs_mkdir(
    service_port: CapabilityDescriptor,
    path_offset: Word,
    path_len: Word,
) -> Result<Word, RequestError> {
    let (status, inode, _) = call_port(
        service_port,
        VFS_REQUEST_MKDIR,
        path_offset,
        path_len,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(inode)
}

pub fn vfs_remove(
    service_port: CapabilityDescriptor,
    path_offset: Word,
    path_len: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(
        service_port,
        VFS_REQUEST_REMOVE,
        path_offset,
        path_len,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn vfs_rename(
    service_port: CapabilityDescriptor,
    old_path_offset: Word,
    old_path_len: Word,
    new_path_offset: Word,
    new_path_len: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(
        service_port,
        VFS_REQUEST_RENAME,
        old_path_offset,
        old_path_len,
        new_path_offset,
        new_path_len,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn vfs_stat(
    service_port: CapabilityDescriptor,
    path_offset: Word,
    path_len: Word,
) -> Result<(Word, Word, Word), RequestError> {
    let (status, inode, metadata) = call_port(
        service_port,
        VFS_REQUEST_STAT,
        path_offset,
        path_len,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(decode_stat(inode, metadata))
}

pub fn vfs_fstat(
    service_port: CapabilityDescriptor,
    handle: Word,
) -> Result<(Word, Word, Word), RequestError> {
    let (status, inode, metadata) = call_port(service_port, VFS_REQUEST_FSTAT, handle, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(decode_stat(inode, metadata))
}

pub fn vfs_read_dir(
    service_port: CapabilityDescriptor,
    handle: Word,
    start_index: Word,
    max_entries: Word,
    out_offset: Word,
) -> Result<(Word, Word), RequestError> {
    let (status, entries, next_index) = call_port(
        service_port,
        VFS_REQUEST_READ_DIR,
        handle,
        start_index,
        max_entries,
        out_offset,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((entries, next_index))
}

fn decode_stat(inode: Word, metadata: Word) -> (Word, Word, Word) {
    let size = metadata & VFS_STAT_SIZE_MASK;
    let kind = metadata >> VFS_STAT_TYPE_SHIFT;
    (inode, size, kind)
}

pub fn vfs_close(service_port: CapabilityDescriptor, handle: Word) -> Result<(), RequestError> {
    let (status, _, _) = call_port(service_port, VFS_REQUEST_CLOSE, handle, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}
