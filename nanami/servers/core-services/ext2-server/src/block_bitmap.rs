//! Bitmap bit zero is the group's first block, not absolute block zero.
use super::*;

pub(super) fn alloc_block(runtime: &mut Ext2Runtime) -> Result<usize, RequestError> {
    let first = runtime.superblock.first_data_block as usize;
    let blocks_per_group = runtime.superblock.blocks_per_group as usize;
    let mut group = 0usize;
    while group < runtime.group_count {
        let limit = runtime.groups[group].block_count as usize;
        if let Some(bit) = alloc_bitmap_bit(
            runtime,
            runtime.groups[group].block_bitmap_block as usize,
            0,
            limit,
        )? {
            adjust_free_count(runtime, group, false, -1)?;
            return Ok(first + group * blocks_per_group + bit);
        }
        group += 1;
    }
    Err(RequestError::Unsupported)
}

pub(super) fn free_block(runtime: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    let first = runtime.superblock.first_data_block as usize;
    if block < first || block >= runtime.block_count {
        return Err(RequestError::InvalidArgument);
    }
    let blocks_per_group = runtime.superblock.blocks_per_group as usize;
    let relative = block - first;
    let group = relative / blocks_per_group;
    if group >= runtime.group_count {
        return Err(RequestError::InvalidArgument);
    }
    let bit = relative % blocks_per_group;
    clear_bitmap_bit(
        runtime,
        runtime.groups[group].block_bitmap_block as usize,
        bit,
    )?;
    adjust_free_count(runtime, group, false, 1)
}
