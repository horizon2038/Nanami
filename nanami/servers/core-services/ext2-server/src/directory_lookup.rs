//! Request-local directory search; name-cache policy remains with the caller.
use super::*;

unsafe fn name_query<'a>(parent: u32, name: *const u8, len: usize) -> Option<dentry_cache::Query<'a>> {
    if len == 0 || len > EXT2_NAME_LEN_MAX { return None; }
    // The request's name remains immutable across block/inode I/O, as it
    // already must for DirectoryQuery and the final cache store below.
    dentry_cache::Query::new(parent, core::slice::from_raw_parts(name, len))
}

pub(super) unsafe fn lookup_in_directory_with_inode(
    runtime: &mut Ext2Runtime, dir_inode: u32, inode: Ext2Inode,
    name: *const u8, name_len: usize,
) -> Option<u32> {
    let query = name_query(dir_inode, name, name_len);
    if let Some(cached) = query.as_ref().and_then(|query| runtime.dentry_cache.load_query(query)) {
        return cached;
    }
    scan_directory(runtime, inode, name, name_len, query.as_ref())
}

/// The caller has already confirmed this parent's kind in the same read-only
/// request. Delay its full inode until a name-cache miss, preserving loader
/// errors instead of translating them into a missing name.
pub(super) unsafe fn lookup_in_directory_with_loader<E>(
    runtime: &mut Ext2Runtime, dir_inode: u32, name: *const u8, name_len: usize,
    load: impl FnOnce(&mut Ext2Runtime) -> Result<Ext2Inode, E>,
) -> Result<Option<u32>, E> {
    let query = name_query(dir_inode, name, name_len);
    if let Some(cached) = query.as_ref().and_then(|query| runtime.dentry_cache.load_query(query)) {
        return Ok(cached);
    }
    let inode = load(runtime)?;
    Ok(scan_directory(runtime, inode, name, name_len, query.as_ref()))
}

pub(super) unsafe fn lookup_in_directory(
    runtime: &mut Ext2Runtime, dir_inode: u32, name: *const u8, name_len: usize,
) -> Option<u32> {
    let query = name_query(dir_inode, name, name_len);
    if let Some(cached) = query.as_ref().and_then(|query| runtime.dentry_cache.load_query(query)) {
        return cached;
    }
    let inode = read_inode(runtime, dir_inode).ok()?;
    scan_directory(runtime, inode, name, name_len, query.as_ref())
}

// The supplied inode is a request-local value for this exact directory. No
// mutation or service dispatch occurs between the caller's read and this scan.
unsafe fn scan_directory(
    runtime: &mut Ext2Runtime, inode: Ext2Inode,
    name: *const u8, name_len: usize,
    name_query: Option<&dentry_cache::Query<'_>>,
) -> Option<u32> {
    if (inode.mode & EXT2_S_IFMT) != EXT2_S_IFDIR { return None; }
    let mut logical_block = 0usize;
    let mut query = None;
    while logical_block < max_file_blocks(runtime) {
        // Resolve only blocks containing directory bytes. In particular, a
        // miss at the direct-block boundary must not read an unused indirect
        // table merely to discover that the directory has already ended.
        let file_offset = logical_block * runtime.block_size;
        if file_offset >= inode.size as usize {
            break;
        }
        let block = get_data_block(runtime, &inode, logical_block).ok()?;
        if block == 0 { break; }
        validate_run(runtime, block as usize, 1).ok()?;
        let limit = min(runtime.block_size, inode.size as usize - file_offset);
        let query = query.get_or_insert_with(|| block_cache::DirectoryQuery::new(
            core::slice::from_raw_parts(name, name_len),
        ));
        let found = if let Some(result) = runtime.block_cache.lookup_directory(
            block as usize, limit, query,
            |bytes| find(bytes, name, name_len),
            |bytes, names| summarize(bytes, names),
        ) {
            result.ok()?
        } else {
            read_block(runtime, block as usize).ok()?;
            let bytes = core::slice::from_raw_parts(runtime.block_shm as *const u8, limit);
            find(bytes, name, name_len).ok()?
        };
        if let Some(inode) = found {
            store_cached_dentry_query(runtime, name_query, Some(inode));
            return Some(inode);
        }
        logical_block += 1;
    }
    store_cached_dentry_query(runtime, name_query, None);
    None
}

unsafe fn summarize(bytes: &[u8], names: &mut [u64; 4]) -> bool {
    let mut offset = 0;
    while offset < bytes.len() {
        let Some(entry) = parse_directory_entry(bytes.as_ptr() as usize + offset, bytes.len() - offset)
        else { return false; };
        if entry.inode != 0 {
            block_cache::BlockCache::record_directory_name(names,
                core::slice::from_raw_parts(entry.name_ptr, entry.name_len));
        }
        offset += entry.record_len;
    }
    true
}

unsafe fn find(bytes: &[u8], name: *const u8, name_len: usize) -> Result<Option<u32>, ()> {
    let mut offset = 0;
    while offset < bytes.len() {
        let entry = parse_directory_entry(bytes.as_ptr() as usize + offset, bytes.len() - offset)
            .ok_or(())?;
        if entry.inode != 0 && entry.name_len == name_len && bytes_eq(entry.name_ptr, name, name_len) {
            return Ok(Some(entry.inode));
        }
        offset += entry.record_len;
    }
    Ok(None)
}
