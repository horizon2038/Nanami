use super::*;

pub(super) fn write_inode(
    runtime: &mut Ext2Runtime, inode_no: u32, inode: Ext2Inode,
) -> Result<(), RequestError> {
    write(runtime, inode_no, inode, true)
}

/// Only for writes to existing blocks of an inode read from this runtime.
/// The request must not have allocated, removed or replaced any block pointer.
pub(super) fn write_inode_existing_blocks(
    runtime: &mut Ext2Runtime, inode_no: u32, inode: Ext2Inode,
) -> Result<(), RequestError> {
    write(runtime, inode_no, inode, false)
}

fn write(
    runtime: &mut Ext2Runtime, inode_no: u32, mut inode: Ext2Inode, recount: bool,
) -> Result<(), RequestError> {
    // Invalidation and ordered inode publication apply to both paths, including
    // errors. Only the pointer-tree recount is optional, never the inode write.
    runtime.file_versions.invalidate(inode_no);
    if inode_no == 0 || runtime.superblock.inodes_per_group == 0 {
        return Err(RequestError::InvalidArgument);
    }
    if recount {
        inode.sectors = data_sectors(runtime, inode)?;
    }
    let index = inode_no as usize - 1;
    if index >= runtime.superblock.inodes_count as usize {
        return Err(RequestError::InvalidArgument);
    }
    let group_index = index / runtime.superblock.inodes_per_group as usize;
    if group_index >= runtime.group_count {
        return Err(RequestError::InvalidArgument);
    }
    let index_in_group = index % runtime.superblock.inodes_per_group as usize;
    let inode_offset = index_in_group * runtime.superblock.inode_size;
    let block =
        runtime.groups[group_index].inode_table_block as usize + inode_offset / runtime.block_size;
    let in_block = inode_offset % runtime.block_size;
    read_block(runtime, block)?;
    let p = runtime.block_shm as usize + in_block;
    w16_mem(p, inode.mode);
    w32_mem(p + 4, inode.size);
    w32_mem(p + 28, inode.sectors);
    w16_mem(p + 26, inode.links_count);
    for i in 0..EXT2_INODE_BLOCK_POINTERS {
        w32_mem(p + 40 + i * 4, inode.block[i]);
    }
    if inode.mode & EXT2_S_IFMT == EXT2_S_IFREG {
        // A regular inode record cannot change directory bytes or their live
        // entry indexes, even when sharing an inode-table block with a directory.
        write_inode_table_block(runtime, block)?;
    } else {
        write_block(runtime, block)?;
    }
    invalidate_cached_inode(runtime, inode_no);
    store_cached_inode(runtime, inode_no, inode);
    update_cached_handle_metadata(runtime, inode_no, inode);
    Ok(())
}
