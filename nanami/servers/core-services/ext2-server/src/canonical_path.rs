//! Non-symlink-following path lookup and the current rooted namespace anchor.
use super::*;
use path_walk::Error;

const INLINE_ROOT_BYTES: usize = 128;

pub(super) struct RootCache {
    path: [u8; INLINE_ROOT_BYTES],
    len: usize,
    inode: u32,
}

impl RootCache {
    pub(super) const fn new() -> Self {
        Self { path: [0; INLINE_ROOT_BYTES], len: 0, inode: 0 }
    }

    pub(super) fn invalidate(&mut self) {
        self.len = 0;
    }
}

pub(super) fn resolve_root(
    runtime: &mut Ext2Runtime, session: ClientSession, offset: usize, bytes: usize,
) -> Result<u32, Error> {
    if bytes > INLINE_ROOT_BYTES {
        // Preserve bounded lookup for a large caller-supplied root without a
        // heap allocation or a new persistent cache key.
        path_input::root(session.shm_local, session.shm_size, offset, bytes)?;
        let path = (session.shm_local + offset) as *const u8;
        return directory_inode(runtime, path, bytes);
    }
    let mut storage = [0; INLINE_ROOT_BYTES];
    let path = path_input::path(session.shm_local, session.shm_size, offset, bytes, &mut storage)?;
    if path.first() != Some(&b'/') {
        return Err(Error::Invalid);
    }
    // Snapshot the exact key used for the cold lookup. Never bind a pathname
    // borrowed from client SHM to the result of a different walk.
    let cache = &runtime.root_path;
    if path.len() == cache.len && path.as_ref() == &cache.path[..cache.len] {
        return Ok(cache.inode);
    }
    let inode = directory_inode(runtime, path.as_ptr(), path.len())?;
    // The filesystem's own root needs no component lookups and should not
    // displace a previously resolved container root.
    if path.len() > 1 {
        let cache = &mut runtime.root_path;
        cache.path[..path.len()].copy_from_slice(&path);
        cache.len = path.len();
        cache.inode = inode;
    }
    Ok(inode)
}

fn directory_inode(
    runtime: &mut Ext2Runtime, path: *const u8, len: usize,
) -> Result<u32, Error> {
    let inode = unsafe { lookup(runtime, path, len) }.ok_or(Error::Missing)?;
    if read_inode(runtime, inode).map_err(|_| Error::Io)?.mode & EXT2_S_IFMT != EXT2_S_IFDIR {
        return Err(Error::NotDirectory);
    }
    Ok(inode)
}

/// `path` addresses `len` validated readable bytes in an owned snapshot or
/// the caller's synchronous shared-memory request.
pub(super) unsafe fn lookup(
    runtime: &mut Ext2Runtime, path: *const u8, len: usize,
) -> Option<u32> {
    if len == 0 || *path != b'/' {
        return None;
    }
    let mut current = EXT2_ROOT_INODE;
    let mut start = 1;
    while start < len {
        while start < len && *path.add(start) == b'/' {
            start += 1;
        }
        if start == len {
            break;
        }
        let mut end = start;
        while end < len && *path.add(end) != b'/' {
            end += 1;
        }
        let bytes = end - start;
        if bytes == 0 || bytes > EXT2_NAME_LEN_MAX {
            return None;
        }
        current = lookup_in_directory(runtime, current, path.add(start), bytes)?;
        // Only namespace-rooted component walks follow symlinks. Canonical
        // paths and namespace anchors cannot traverse a replaced link.
        if end < len && read_inode(runtime, current).ok()?.mode & EXT2_S_IFMT == EXT2_S_IFLNK {
            return None;
        }
        start = end;
    }
    Some(current)
}
