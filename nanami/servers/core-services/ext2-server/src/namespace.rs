use super::*;
use super::link_target::read_link;
use super::directory_parent::read_parent_name;
use alloc::vec::Vec;
use nanami_client::vfs::*;
use path_walk::{Error, Filesystem, Kind, Node};

#[path = "namespace_disk.rs"]
mod disk;
use disk::kind;

fn input(session: ClientSession, offset: Word, bytes: Word) -> Result<Vec<u8>, Error> {
    if bytes > path_walk::PATH_MAX
        || offset
            .checked_add(bytes)
            .is_none_or(|end| end > session.shm_size)
    {
        return Err(Error::Invalid);
    }
    Ok(
        unsafe { core::slice::from_raw_parts((session.shm_local + offset) as *const u8, bytes) }
            .to_vec(),
    )
}

fn output(session: ClientSession, request: PathRequest, bytes: &[u8]) -> Result<Word, Error> {
    let count = bytes.len().min(request.output_bytes);
    if request
        .output_offset
        .checked_add(count)
        .is_none_or(|end| end > session.shm_size)
    {
        return Err(Error::Invalid);
    }
    unsafe {
        ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            (session.shm_local + request.output_offset) as *mut u8,
            count,
        );
    }
    Ok(count)
}

pub(super) fn handle(message: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let result = (|| {
        let session = find_session(runtime, message.identifier).ok_or(Error::Invalid)?;
        let bytes = core::mem::size_of::<PathRequest>();
        if message
            .arg0
            .checked_add(bytes)
            .is_none_or(|end| end > session.shm_size)
        {
            return Err(Error::Invalid);
        }
        let mut request = unsafe {
            ptr::read_unaligned((session.shm_local + message.arg0) as *const PathRequest)
        };
        let cursor = message.code == VFS_REQUEST_OPEN_ROOTED_CURSOR_GRANTED;
        let granted = cursor || message.code == VFS_REQUEST_OPEN_ROOTED_GRANTED;
        let delegate = if granted { request.flags >> VFS_DELEGATE_ID_SHIFT } else { 0 };
        if granted {
            if request.operation != VFS_PATH_OPEN_READONLY { return Err(Error::Invalid); }
            request.flags &= VFS_DELEGATE_VALUE_MASK;
        }
        if request.flags
            & !(VFS_PATH_FOLLOW_FINAL | VFS_PATH_ALLOW_MISSING | VFS_PATH_WITH_METADATA)
            != 0
            || (request.flags & VFS_PATH_WITH_METADATA != 0
                && request.operation != VFS_PATH_RESOLVE)
        {
            return Err(Error::Invalid);
        }
        let root = canonical_path::resolve_root(runtime, session, request.root_offset, request.root_bytes)?;
        let mut path_storage = [0u8; 256];
        let path = super::path_input::path(session.shm_local, session.shm_size,
            request.path_offset, request.path_bytes, &mut path_storage)?;
        let start = if path.first() == Some(&b'/') || request.directory == VFS_PATH_DIRECTORY_ROOT {
            root
        } else {
            let handle = runtime
                .handles
                .get(request.directory)
                .filter(|handle| handle.active && handle.owner_pid == message.identifier)
                .ok_or(Error::Invalid)?;
            if handle.mode & EXT2_S_IFMT != EXT2_S_IFDIR {
                return Err(Error::NotDirectory);
            }
            handle.inode
        };
        let follow = request.operation != VFS_PATH_READLINK
            && request.operation != VFS_PATH_SYMLINK
            && request.flags & VFS_PATH_FOLLOW_FINAL != 0;
        let allow_missing = request.operation == VFS_PATH_SYMLINK
            || (matches!(request.operation, VFS_PATH_RESOLVE | VFS_PATH_OPEN_READONLY)
                && request.flags & VFS_PATH_ALLOW_MISSING != 0);
        // Only SYMLINK consumes the owned final name (including rollback).
        // Keep the same complete walk and canonical path for other requests.
        let cacheable = matches!(request.operation,
            VFS_PATH_RESOLVE | VFS_PATH_OPEN_READONLY | VFS_PATH_READLINK | VFS_PATH_STAT);
        let (resolved, inode) = disk::resolve(runtime, root, start, &path, follow,
            allow_missing, request.operation == VFS_PATH_SYMLINK, cacheable)?;
        let load_inode = |runtime: &mut Ext2Runtime, number| {
            inode.map_or_else(|| read_inode(runtime, number).map_err(|_| Error::Io), Ok)
        };
        match request.operation {
            VFS_PATH_OPEN_READONLY => {
                let flags = super::path_input::word(session.shm_local, session.shm_size,
                    request.target_offset, request.target_bytes)?;
                let total = core::mem::size_of::<PathOpenResult>()
                    .checked_add(resolved.path.len()).ok_or(Error::TooLong)?;
                if request.output_offset.checked_add(total)
                    .is_none_or(|end| end > session.shm_size)
                {
                    return Err(Error::TooLong);
                }
                let output = unsafe {
                    core::slice::from_raw_parts_mut(
                        (session.shm_local + request.output_offset) as *mut u8,
                        request.output_bytes.min(total),
                    )
                };
                let mut grant = 0;
                let result = readonly_open::complete(&resolved.path, resolved.node.map(|node| node.kind),
                    flags, output, || {
                        let node = resolved.node.ok_or(Error::Missing)?;
                        let inode = load_inode(runtime, node.inode)?;
                        let handle = alloc_handle(runtime, message.identifier, node.inode, inode);
                        if granted {
                            if let Some(handle) = handle {
                                grant = super::read_grants::issue(runtime, handle, message.identifier, delegate);
                                if cursor && grant != 0 {
                                    runtime.handles[handle].cursor = Some(0);
                                }
                            }
                        }
                        Ok(handle)
                    });
                result.map(|(bytes, kind)| (bytes, kind | (grant << VFS_OPEN_GRANT_TYPE_BITS)
                    | if cursor && grant != 0 { VFS_OPEN_CURSOR_TYPE_FLAG } else { 0 }))
            }
            VFS_PATH_RESOLVE => {
                let metadata_bytes = if request.flags & VFS_PATH_WITH_METADATA != 0 {
                    core::mem::size_of::<PathMetadata>()
                } else {
                    0
                };
                if metadata_bytes + resolved.path.len() > request.output_bytes {
                    return Err(Error::TooLong);
                }
                let file_type = resolved.node.map_or(0, |node| match node.kind {
                    Kind::File => VFS_FILE_TYPE_REGULAR,
                    Kind::Directory => VFS_FILE_TYPE_DIRECTORY,
                    Kind::Symlink => VFS_FILE_TYPE_SYMLINK,
                    Kind::Socket => VFS_FILE_TYPE_SOCKET,
                });
                if metadata_bytes != 0 {
                    let metadata = match resolved.node {
                        Some(node) => {
                            let inode = load_inode(runtime, node.inode)?;
                            PathMetadata {
                                inode: node.inode as Word,
                                size: inode.size as Word,
                                mode: inode.mode as Word,
                                file_type,
                            }
                        }
                        None => PathMetadata::default(),
                    };
                    let bytes = unsafe {
                        core::slice::from_raw_parts(
                            &metadata as *const PathMetadata as *const u8,
                            metadata_bytes,
                        )
                    };
                    output(session, request, bytes)?;
                }
                let path_output = PathRequest {
                    output_offset: request.output_offset + metadata_bytes,
                    output_bytes: request.output_bytes - metadata_bytes,
                    ..request
                };
                Ok((
                    metadata_bytes + output(session, path_output, &resolved.path)?,
                    file_type,
                ))
            }
            VFS_PATH_READLINK => {
                let inode = resolved.node.ok_or(Error::Missing)?.inode;
                Ok((output(session, request, &read_link(runtime, inode)?)?, 0))
            }
            VFS_PATH_STAT => {
                let number = resolved.node.ok_or(Error::Missing)?.inode;
                let inode = load_inode(runtime, number)?;
                Ok((
                    number as Word,
                    (inode.size as Word) | (file_kind(inode.mode) << VFS_STAT_TYPE_SHIFT),
                ))
            }
            VFS_PATH_CHMOD => {
                let mode = super::path_input::word(session.shm_local, session.shm_size,
                    request.target_offset, request.target_bytes)?;
                let number = resolved.node.ok_or(Error::Missing)?.inode;
                chmod_inode(runtime, number, mode)?;
                Ok((0, 0))
            }
            VFS_PATH_SYMLINK => {
                if resolved.node.is_some() {
                    return Err(Error::Exists);
                }
                if resolved.name.is_empty() {
                    return Err(Error::Invalid);
                }
                writeback::check_writable(runtime).map_err(|_| Error::ReadOnly)?;
                let target = input(session, request.target_offset, request.target_bytes)?;
                if target.is_empty() || target.contains(&0) {
                    return Err(Error::Invalid);
                }
                let number = create_node(
                    runtime,
                    resolved.parent,
                    resolved.name.as_ptr(),
                    resolved.name.len(),
                    EXT2_S_IFLNK,
                    VFS_MODE_PRESENT | 0o777,
                )
                .map_err(|_| Error::Io)?;
                let mut inode = read_inode(runtime, number).map_err(|_| Error::Io)?;
                inode.mode = EXT2_S_IFLNK | 0o777;
                let result = if target.len() <= EXT2_INODE_BLOCK_POINTERS * 4 {
                    for (index, byte) in target.iter().enumerate() {
                        inode.block[index / 4] |= (*byte as u32) << ((index % 4) * 8);
                    }
                    inode.size = target.len() as u32;
                    write_inode(runtime, number, inode).map_err(|_| Error::Io)
                } else {
                    inode.sectors = 1; // Mark block-backed data while constructing the inode.
                    let source = ClientSession {
                        shm_local: target.as_ptr() as Word,
                        shm_size: target.len(),
                        ..session
                    };
                    write_file(runtime, source, &mut inode, number, 0, target.len(), 0)
                        .map(|_| ())
                        .map_err(|_| Error::Io)
                };
                if let Err(error) = result {
                    let _ = write_inode(runtime, number, inode);
                    let _ = remove_node(
                        runtime,
                        resolved.parent,
                        resolved.name.as_ptr(),
                        resolved.name.len(),
                    );
                    return Err(error);
                }
                Ok((number as Word, 0))
            }
            _ => Err(Error::Invalid),
        }
    })();
    match result {
        Ok((detail0, detail1)) => (nanami_runtime::OS_RESPONSE_OK, detail0, detail1),
        Err(error) => (VFS_PATH_RESPONSE_ERROR, error_code(error), 0),
    }
}

fn chmod_inode(runtime: &mut Ext2Runtime, number: u32, mode: Word) -> Result<(), Error> {
    if mode & !0o7777 != 0 {
        return Err(Error::Invalid);
    }
    writeback::check_writable(runtime).map_err(|_| Error::ReadOnly)?;
    let mut inode = read_inode(runtime, number).map_err(|_| Error::Io)?;
    if kind(inode) == Kind::Symlink {
        return Err(Error::Unsupported);
    }
    inode.mode = (inode.mode & EXT2_S_IFMT) | mode as u16;
    // write_inode updates inode and open-handle caches and dirties the existing
    // writeback cache. Existing fsync/sync/periodic flush persists the metadata.
    write_inode(runtime, number, inode).map_err(|_| Error::Io)
}

pub(super) fn handle_fchmod(
    request: ServiceRequest,
    runtime: &mut Ext2Runtime,
) -> (Word, Word, Word) {
    let Some(handle) = runtime
        .handles
        .get(request.arg0)
        .copied()
        .filter(|handle| handle.active && handle.owner_pid == request.identifier)
    else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    match chmod_inode(runtime, handle.inode, request.arg1) {
        Ok(()) => (nanami_runtime::OS_RESPONSE_OK, 0, 0),
        Err(error) => (VFS_PATH_RESPONSE_ERROR, error_code(error), 0),
    }
}

fn error_code(error: Error) -> Word {
    match error {
        Error::Invalid => VFS_PATH_ERROR_INVALID,
        Error::Missing => VFS_PATH_ERROR_MISSING,
        Error::NotDirectory => VFS_PATH_ERROR_NOT_DIRECTORY,
        Error::Loop => VFS_PATH_ERROR_LOOP,
        Error::TooLong => VFS_PATH_ERROR_TOO_LONG,
        Error::Exists => VFS_PATH_ERROR_EXISTS,
        Error::Io => VFS_PATH_ERROR_IO,
        Error::NoSpace => VFS_PATH_ERROR_NO_SPACE,
        Error::ReadOnly => VFS_PATH_ERROR_READ_ONLY,
        Error::Unsupported => VFS_PATH_ERROR_UNSUPPORTED,
    }
}
