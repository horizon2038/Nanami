use super::*;

pub(super) fn read_file(
    runtime: &mut Ext2Runtime,
    session: ClientSession,
    inode: Ext2Inode,
    file_offset: usize,
    len: usize,
    out_offset: usize,
) -> Result<usize, Word> {
    if out_offset
        .checked_add(len)
        .filter(|end| *end <= session.shm_size as usize)
        .is_none()
    {
        return Err(libnanami::OS_RESPONSE_INVALID_ARGUMENT);
    }
    let file_size = inode.size as usize;
    if file_offset >= file_size || len == 0 {
        return Ok(0);
    }
    let mut remaining = min(len, file_size - file_offset);
    let mut copied = 0usize;
    while remaining > 0 {
        let logical_block = (file_offset + copied) / runtime.block_size;
        let Ok(physical_block) = get_data_block(runtime, inode, logical_block) else {
            return Err(libnanami::OS_RESPONSE_ILLEGAL_OPERATION);
        };
        if physical_block == 0 {
            break;
        }
        let block_offset = (file_offset + copied) % runtime.block_size;
        let block_capacity = runtime.block_shm_size as usize / runtime.block_size;
        if block_capacity == 0 {
            return Err(libnanami::OS_RESPONSE_INVALID_ARGUMENT);
        }
        let needed_blocks =
            (block_offset + remaining + runtime.block_size - 1) / runtime.block_size;
        let max_blocks = min(needed_blocks, block_capacity);
        let mut run_blocks = 1usize;
        while run_blocks < max_blocks {
            let Ok(next_block) = get_data_block(runtime, inode, logical_block + run_blocks) else {
                break;
            };
            if next_block == 0 || next_block as usize != physical_block as usize + run_blocks {
                break;
            }
            run_blocks += 1;
        }
        read_blocks(runtime, physical_block as usize, run_blocks)
            .map_err(map_request_error_to_status)?;
        let n = min(remaining, run_blocks * runtime.block_size - block_offset);
        unsafe {
            ptr::copy_nonoverlapping(
                (runtime.block_shm as usize + block_offset) as *const u8,
                (session.shm_local as usize + out_offset + copied) as *mut u8,
                n,
            );
        }
        copied += n;
        remaining -= n;
    }
    Ok(copied)
}

pub(super) fn write_file(
    runtime: &mut Ext2Runtime,
    session: ClientSession,
    inode: &mut Ext2Inode,
    inode_no: u32,
    file_offset: usize,
    len: usize,
    input_offset: usize,
) -> Result<usize, Word> {
    if file_offset.checked_add(len).is_none() {
        return Err(libnanami::OS_RESPONSE_INVALID_ARGUMENT);
    }
    let end = file_offset + len;
    if end > runtime.block_size * max_file_blocks(runtime) {
        return Err(libnanami::OS_RESPONSE_ILLEGAL_OPERATION);
    }
    let mut copied = 0usize;
    let mut inode_dirty = false;
    while copied < len {
        let logical_block = (file_offset + copied) / runtime.block_size;
        let block_offset = (file_offset + copied) % runtime.block_size;
        let mut n = min(len - copied, runtime.block_size - block_offset);
        let existing = get_data_block(runtime, *inode, logical_block)
            .map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
        let physical_block = if existing != 0 {
            existing as usize
        } else {
            inode_dirty = true;
            ensure_data_block(runtime, inode, logical_block)
                .map_err(|_| libnanami::OS_RESPONSE_FATAL)? as usize
        };
        let mut run_blocks = 1usize;
        if block_offset == 0 && n == runtime.block_size {
            // Look ahead only through already allocated, physically contiguous blocks.
            // Block-map reads may overwrite the scratch buffer: finish them before
            // staging any payload. Allocation and partial-block writes keep their path.
            let max_blocks = min(
                (len - copied) / runtime.block_size,
                runtime.block_shm_size as usize / runtime.block_size,
            );
            while run_blocks < max_blocks {
                match get_data_block(runtime, *inode, logical_block + run_blocks) {
                    Ok(next) if next != 0 && next as usize == physical_block + run_blocks => {
                        run_blocks += 1
                    }
                    _ => break,
                }
            }
            n = run_blocks * runtime.block_size;
        } else {
            read_block(runtime, physical_block)
                .map_err(|_| libnanami::OS_RESPONSE_INVALID_DESCRIPTOR)?;
        }
        unsafe {
            ptr::copy_nonoverlapping(
                (session.shm_local as usize + input_offset + copied) as *const u8,
                (runtime.block_shm as usize + block_offset) as *mut u8,
                n,
            );
        }
        write_blocks(runtime, physical_block, run_blocks)
            .map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
        copied += n;
    }
    if end > inode.size as usize {
        inode.size = end as u32;
        inode_dirty = true;
    }
    if inode_dirty {
        write_inode(runtime, inode_no, *inode).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
    }
    Ok(copied)
}
