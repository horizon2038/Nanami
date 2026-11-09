//! Request-local sector accounting without rereading each indirect entry.
use super::*;

pub(super) fn data_sectors(runtime: &mut Ext2Runtime, inode: Ext2Inode) -> Result<u32, RequestError> {
    if inode.mode & EXT2_S_IFMT == EXT2_S_IFLNK
        && inode.sectors == 0
        && inode.size as usize <= EXT2_INODE_BLOCK_POINTERS * 4
    {
        return Ok(0);
    }
    let mut blocks = inode.block[..EXT2_MAX_DIRECT_BLOCKS]
        .iter().filter(|&&block| block != 0).count() as u32;
    let single = inode.block[EXT2_SINGLE_INDIRECT_INDEX];
    if single != 0 {
        blocks += 1 + count_leaf(runtime, single as usize)?;
    }
    let double = inode.block[EXT2_DOUBLE_INDIRECT_INDEX];
    if double != 0 {
        blocks += 1;
        let entries = indirect_entries_per_block(runtime);
        // Reading a child destroys block_shm. Capture a bounded batch of root
        // pointers first; no allocation, fixed block-size limit or persistent
        // cache/provenance is introduced. Sparse tables still count every
        // nonzero pointer, including pointers beyond the current file size.
        let mut children = [0u32; 64];
        let mut first = 0;
        while first < entries {
            read_block(runtime, double as usize)?;
            let count = children.len().min(entries - first);
            for (offset, child) in children[..count].iter_mut().enumerate() {
                *child = r32(runtime.block_shm as usize + (first + offset) * 4);
            }
            for &child in &children[..count] {
                if child != 0 {
                    blocks += 1 + count_leaf(runtime, child as usize)?;
                }
            }
            first += count;
        }
    }
    Ok(blocks * ((runtime.block_size / 512) as u32))
}

fn count_leaf(runtime: &mut Ext2Runtime, block: usize) -> Result<u32, RequestError> {
    read_block(runtime, block)?;
    let mut count = 0;
    for entry in 0..indirect_entries_per_block(runtime) {
        if r32(runtime.block_shm as usize + entry * 4) != 0 {
            count += 1;
        }
    }
    Ok(count)
}
