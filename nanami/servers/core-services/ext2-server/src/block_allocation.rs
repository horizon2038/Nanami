use super::*;

pub(super) fn ensure_data_block(
    runtime: &mut Ext2Runtime,
    inode: &mut Ext2Inode,
    logical_block: usize,
) -> Result<u32, RequestError> {
    ensure_data_block_with(runtime, inode, logical_block, zero_block)
}

pub(super) fn ensure_data_block_with(
    runtime: &mut Ext2Runtime,
    inode: &mut Ext2Inode,
    logical_block: usize,
    initialize: impl FnOnce(&mut Ext2Runtime, usize) -> Result<(), RequestError>,
) -> Result<u32, RequestError> {
    // Initialize the cached block before publishing its pointer. Writeback
    // persists file data before the metadata that references it.
    if logical_block < EXT2_MAX_DIRECT_BLOCKS {
        if inode.block[logical_block] == 0 {
            let block = alloc_block(runtime)?;
            initialize(runtime, block)?;
            inode.block[logical_block] = block as u32;
        }
        return Ok(inode.block[logical_block]);
    }
    let indirect_index = logical_block - EXT2_MAX_DIRECT_BLOCKS;
    let entries = indirect_entries_per_block(runtime);
    if indirect_index < entries {
        if inode.block[EXT2_SINGLE_INDIRECT_INDEX] == 0 {
            let block = alloc_block(runtime)?;
            zero_block(runtime, block)?;
            inode.block[EXT2_SINGLE_INDIRECT_INDEX] = block as u32;
        }
        let indirect_block = inode.block[EXT2_SINGLE_INDIRECT_INDEX] as usize;
        read_block(runtime, indirect_block)?;
        let entry_addr = runtime.block_shm as usize + indirect_index * 4;
        let mut block = r32(entry_addr);
        if block == 0 {
            block = alloc_block(runtime)? as u32;
            initialize(runtime, block as usize)?;
            // Allocation and initialization both overwrite block_shm.
            read_block(runtime, indirect_block)?;
            w32_mem(entry_addr, block);
            write_block(runtime, indirect_block)?;
        }
        return Ok(block);
    }

    let double_index = indirect_index - entries;
    if double_index >= entries * entries {
        return Err(RequestError::Unsupported);
    }
    if inode.block[EXT2_DOUBLE_INDIRECT_INDEX] == 0 {
        let block = alloc_block(runtime)?;
        zero_block(runtime, block)?;
        inode.block[EXT2_DOUBLE_INDIRECT_INDEX] = block as u32;
    }
    let double_block = inode.block[EXT2_DOUBLE_INDIRECT_INDEX] as usize;
    let first_index = double_index / entries;
    let second_index = double_index % entries;

    read_block(runtime, double_block)?;
    let first_entry_addr = runtime.block_shm as usize + first_index * 4;
    let mut indirect_block = r32(first_entry_addr);
    if indirect_block == 0 {
        indirect_block = alloc_block(runtime)? as u32;
        zero_block(runtime, indirect_block as usize)?;
        read_block(runtime, double_block)?;
        w32_mem(first_entry_addr, indirect_block);
        write_block(runtime, double_block)?;
    }

    let indirect_block = indirect_block as usize;
    read_block(runtime, indirect_block)?;
    let entry_addr = runtime.block_shm as usize + second_index * 4;
    let mut block = r32(entry_addr);
    if block == 0 {
        block = alloc_block(runtime)? as u32;
        initialize(runtime, block as usize)?;
        read_block(runtime, indirect_block)?;
        w32_mem(entry_addr, block);
        write_block(runtime, indirect_block)?;
    }
    Ok(block)
}
