use super::*;
use alloc::vec::Vec;

pub(super) fn resize_inode(
    runtime: &mut Ext2Runtime,
    inode_no: u32,
    inode: &mut Ext2Inode,
    length: usize,
) -> Result<(), RequestError> {
    if length > u32::MAX as usize || length > runtime.block_size * max_file_blocks(runtime) {
        return Err(RequestError::InvalidArgument);
    }
    if length == inode.size as usize {
        return Ok(());
    }
    // Like the read/write paths, support direct, single and double indirect blocks.
    if inode.block[EXT2_DOUBLE_INDIRECT_INDEX + 1] != 0 {
        return Err(RequestError::InvalidArgument);
    }
    let mut resized = *inode;
    let mut plan = PrunePlan::default();
    if length < inode.size as usize {
        let keep = length.div_ceil(runtime.block_size);
        for (index, block) in resized.block[..EXT2_MAX_DIRECT_BLOCKS]
            .iter_mut()
            .enumerate()
        {
            if index >= keep && *block != 0 {
                plan.released.push(*block);
                *block = 0;
            }
        }
        let entries = indirect_entries_per_block(runtime);
        resized.block[EXT2_SINGLE_INDIRECT_INDEX] = plan.prune(
            runtime,
            resized.block[EXT2_SINGLE_INDIRECT_INDEX],
            1,
            EXT2_MAX_DIRECT_BLOCKS,
            keep,
        )?;
        resized.block[EXT2_DOUBLE_INDIRECT_INDEX] = plan.prune(
            runtime,
            resized.block[EXT2_DOUBLE_INDIRECT_INDEX],
            2,
            EXT2_MAX_DIRECT_BLOCKS + entries,
            keep,
        )?;
    }
    // Neither growing a sparse file nor truncating one allocates data blocks.
    // Clear the retained partial block so shrink-then-grow cannot expose old data.
    zero_tail(runtime, *inode, min(length, inode.size as usize))?;
    for (block, entries) in plan.tables {
        for (index, entry) in entries.into_iter().enumerate() {
            w32_mem(runtime.block_shm as usize + index * 4, entry);
        }
        write_block(runtime, block as usize)?;
    }
    resized.size = length as u32;
    // Remove references before making blocks reusable. On an I/O error, leaking
    // blocks is preferable to assigning a still-referenced block to another file.
    // Persistence uses the existing dirty cache/fsync policy, not a journal.
    write_inode(runtime, inode_no, resized)?;
    *inode = resized;
    for block in plan.released {
        free_block(runtime, block as usize)?;
    }
    Ok(())
}

fn zero_tail(
    runtime: &mut Ext2Runtime,
    inode: Ext2Inode,
    offset: usize,
) -> Result<(), RequestError> {
    let in_block = offset % runtime.block_size;
    if in_block == 0 {
        return Ok(());
    }
    let block = get_data_block(runtime, inode, offset / runtime.block_size)?;
    if block != 0 {
        read_block(runtime, block as usize)?;
        unsafe {
            ptr::write_bytes(
                (runtime.block_shm as usize + in_block) as *mut u8,
                0,
                runtime.block_size - in_block,
            );
        }
        write_blocks(runtime, block as usize, 1)?;
    }
    Ok(())
}

#[derive(Default)]
struct PrunePlan {
    tables: Vec<(u32, Vec<u32>)>,
    released: Vec<u32>,
}

impl PrunePlan {
    fn prune(
        &mut self,
        runtime: &mut Ext2Runtime,
        block: u32,
        level: u32,
        first: usize,
        keep: usize,
    ) -> Result<u32, RequestError> {
        let entries = indirect_entries_per_block(runtime);
        let span = entries.pow(level - 1);
        if block == 0 || first + entries * span <= keep {
            return Ok(block);
        }
        read_block(runtime, block as usize)?;
        // Recursive reads reuse the same IPC scratch buffer; retain this table.
        let mut table: Vec<u32> = (0..entries)
            .map(|index| r32(runtime.block_shm as usize + index * 4))
            .collect();
        let mut changed = false;
        for (index, child) in table.iter_mut().enumerate() {
            let start = first + index * span;
            if *child == 0 || start + span <= keep {
                continue;
            }
            let next = if level == 1 {
                self.released.push(*child);
                0
            } else {
                self.prune(runtime, *child, level - 1, start, keep)?
            };
            changed |= next != *child;
            *child = next;
        }
        if table.iter().all(|entry| *entry == 0) {
            self.released.push(block);
            Ok(0)
        } else {
            if changed {
                self.tables.push((block, table));
            }
            Ok(block)
        }
    }
}
