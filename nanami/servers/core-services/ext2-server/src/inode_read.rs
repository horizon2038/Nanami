//! Decode only the inode fields in use, without staging a cached whole block.
use super::*;

const RECORD_BYTES: usize = 40 + EXT2_INODE_BLOCK_POINTERS * 4;

pub(super) fn read_inode(runtime: &mut Ext2Runtime, inode_no: u32) -> Result<Ext2Inode, RequestError> {
    if inode_no == 0 || runtime.superblock.inodes_per_group == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let index = inode_no as usize - 1;
    if index >= runtime.superblock.inodes_count as usize {
        return Err(RequestError::InvalidArgument);
    }
    if let Some(inode) = load_cached_inode(runtime, inode_no) {
        return Ok(inode);
    }
    let group_index = index / runtime.superblock.inodes_per_group as usize;
    if group_index >= runtime.group_count || runtime.block_size == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let index_in_group = index % runtime.superblock.inodes_per_group as usize;
    let inode_offset = index_in_group.checked_mul(runtime.superblock.inode_size)
        .ok_or(RequestError::InvalidArgument)?;
    let table_index = inode_offset / runtime.block_size;
    let block = (runtime.groups[group_index].inode_table_block as usize)
        .checked_add(table_index).ok_or(RequestError::InvalidArgument)?;
    let in_block = inode_offset % runtime.block_size;
    if in_block.checked_add(RECORD_BYTES).is_none_or(|end| end > runtime.block_size) {
        return Err(RequestError::InvalidArgument);
    }
    // Keep the original block/transfer bounds even on a cache hit.
    validate_run(runtime, block, 1)?;
    let inode = if let Some(record) = runtime.block_cache.load_record::<RECORD_BYTES>(block, in_block) {
        decode(record)
    } else {
        read_table_block(runtime, group_index, table_index, block)?;
        // The checked record fits entirely in the successfully read block.
        // Decode cold scratch in place; do not add a second small copy there.
        decode(unsafe { &*((runtime.block_shm as usize + in_block) as *const [u8; RECORD_BYTES]) })
    };
    store_cached_inode(runtime, inode_no, inode);
    Ok(inode)
}

fn read_table_block(
    runtime: &mut Ext2Runtime, group: usize, within_table: usize, block: usize,
) -> Result<(), RequestError> {
    const ENABLED: bool = match option_env!("NANAMI_INODE_READ_AHEAD") {
        Some(value) => !(value.len() == 1 && value.as_bytes()[0] == b'0'),
        None => true,
    };
    // A cold isolated lookup keeps its one-block demand. Only a resident
    // preceding block in this table supplies locality evidence for read-ahead.
    let local = within_table != 0 && block.checked_sub(1)
        .is_some_and(|previous| runtime.block_cache.read_gaps(previous..block).next().is_none());
    let count = if ENABLED && local {
        table_read_ahead(runtime, group, within_table, block).unwrap_or(1)
    } else { 1 };
    if count <= 1 { return read_block(runtime, block); }
    // These are adjacent records in this inode table, never another table or
    // arbitrary file data. Existing read_blocks preserves resident dirty bytes
    // and reads only gaps, with its original device validation and retry policy.
    let result = read_blocks(runtime, block, count);
    if option_env!("NANAMI_INODE_READ_DIAG") == Some("1") {
        nanami_runtime::println!("[ext2.inode-ahead] block={} count={} ok={}", block, count, usize::from(result.is_ok()));
    }
    // Only the requested first block determines this lookup's success. An
    // unreadable speculative suffix must not reject a valid inode. A completed
    // prefix may already be cached; the ordinary path rechecks that live cache.
    result.or_else(|_| read_block(runtime, block))
}

fn table_read_ahead(
    runtime: &Ext2Runtime, group: usize, within_table: usize, block: usize,
) -> Option<usize> {
    let per_group = runtime.superblock.inodes_per_group as usize;
    let first_inode = group.checked_mul(per_group)?;
    let inodes = (runtime.superblock.inodes_count as usize).checked_sub(first_inode)?.min(per_group);
    let bytes = inodes.checked_mul(runtime.superblock.inode_size)?;
    let remaining = bytes.div_ceil(runtime.block_size).checked_sub(within_table)?;
    // At most 16 blocks, one quarter of the existing cache, and the existing
    // transfer window. No new buffer or persistent read-ahead state is needed.
    Some(remaining.min(16)
        .min(runtime.block_count.checked_sub(block)?)
        .min(runtime.block_shm_size / runtime.block_size)
        .min(runtime.block_cache.capacity_bytes() / runtime.block_size / 4)
        .max(1))
}

fn decode(record: &[u8; RECORD_BYTES]) -> Ext2Inode {
    let word = |offset| u32::from_le_bytes(record[offset..offset + 4].try_into().unwrap());
    let half = |offset| u16::from_le_bytes(record[offset..offset + 2].try_into().unwrap());
    let mut blocks = [0; EXT2_INODE_BLOCK_POINTERS];
    for (i, block) in blocks.iter_mut().enumerate() { *block = word(40 + i * 4); }
    Ext2Inode { mode: half(0), size: word(4), links_count: half(26), sectors: word(28), block: blocks }
}
