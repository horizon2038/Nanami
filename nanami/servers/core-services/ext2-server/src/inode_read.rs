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
    let block = (runtime.groups[group_index].inode_table_block as usize)
        .checked_add(inode_offset / runtime.block_size).ok_or(RequestError::InvalidArgument)?;
    let in_block = inode_offset % runtime.block_size;
    if in_block.checked_add(RECORD_BYTES).is_none_or(|end| end > runtime.block_size) {
        return Err(RequestError::InvalidArgument);
    }
    // Keep the original block/transfer bounds even on a cache hit.
    validate_run(runtime, block, 1)?;
    let inode = if let Some(record) = runtime.block_cache.load_record::<RECORD_BYTES>(block, in_block) {
        decode(record)
    } else {
        read_block(runtime, block)?;
        // The checked record fits entirely in the successfully read block.
        // Decode cold scratch in place; do not add a second small copy there.
        decode(unsafe { &*((runtime.block_shm as usize + in_block) as *const [u8; RECORD_BYTES]) })
    };
    store_cached_inode(runtime, inode_no, inode);
    Ok(inode)
}

fn decode(record: &[u8; RECORD_BYTES]) -> Ext2Inode {
    let word = |offset| u32::from_le_bytes(record[offset..offset + 4].try_into().unwrap());
    let half = |offset| u16::from_le_bytes(record[offset..offset + 2].try_into().unwrap());
    let mut blocks = [0; EXT2_INODE_BLOCK_POINTERS];
    for (i, block) in blocks.iter_mut().enumerate() { *block = word(40 + i * 4); }
    Ext2Inode { mode: half(0), size: word(4), links_count: half(26), sectors: word(28), block: blocks }
}
