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
        return Err(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT);
    }
    let file_size = inode.size as usize;
    if file_offset >= file_size || len == 0 {
        return Ok(0);
    }
    let mut remaining = min(len, file_size - file_offset);
    let mut copied = 0usize;
    // Device geometry is fixed after mount; block/cache I/O cannot change it.
    let block_size = runtime.block_size;
    let block_capacity = runtime.block_shm_size as usize / block_size;
    while remaining > 0 {
        let logical_block = (file_offset + copied) / block_size;
        let block_offset = (file_offset + copied) % block_size;
        let needed_blocks =
            (block_offset + remaining + block_size - 1) / block_size;
        let max_blocks = min(needed_blocks, block_capacity.max(1));
        let Ok((physical_block, run_blocks)) =
            get_read_data_run(runtime, &inode, logical_block, max_blocks)
        else {
            return Err(nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION);
        };
        if physical_block == 0 {
            let n = min(remaining, run_blocks * block_size - block_offset);
            unsafe {
                ptr::write_bytes(
                    (session.shm_local as usize + out_offset + copied) as *mut u8,
                    0,
                    n,
                );
            }
            copied += n;
            remaining -= n;
            continue;
        }
        if block_capacity == 0 {
            return Err(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT);
        }
        validate_run(runtime, physical_block as usize, run_blocks)
            .map_err(map_request_error_to_status)?;
        let n = min(remaining, run_blocks * block_size - block_offset);
        let output = unsafe {
            core::slice::from_raw_parts_mut(
                (session.shm_local as usize + out_offset + copied) as *mut u8,
                n,
            )
        };
        if !runtime.block_cache.load_range(physical_block as usize, block_offset, output) {
            read_blocks(runtime, physical_block as usize, run_blocks)
                .map_err(map_request_error_to_status)?;
            unsafe {
                ptr::copy_nonoverlapping(
                    (runtime.block_shm as usize + block_offset) as *const u8,
                    output.as_mut_ptr(),
                    n,
                );
            }
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
    if len == 0 {
        return Ok(0);
    }
    if file_offset.checked_add(len).is_none() {
        return Err(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT);
    }
    let end = file_offset + len;
    if end > inode.size as usize && runtime.mapped_pages.pins(inode_no) {
        return Err(nanami_client::vfs::VFS_RESPONSE_MAPPING_BUSY);
    }
    if end > runtime.block_size * max_file_blocks(runtime) {
        return Err(nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION);
    }
    let mut copied = 0usize;
    let mut inode_dirty = false;
    let mut allocation_changed = false;
    // Same-size writes need not write the inode. Invalidate before the first
    // possible data mutation, not only after successful metadata writeback.
    runtime.file_versions.invalidate(inode_no);
    let block_size = runtime.block_size;
    let block_capacity = runtime.block_shm_size as usize / block_size;
    while copied < len {
        let logical_block = (file_offset + copied) / block_size;
        let block_offset = (file_offset + copied) % block_size;
        let mut n = min(len - copied, block_size - block_offset);
        let whole_blocks = block_offset == 0 && n == block_size;
        let max_blocks = if whole_blocks {
            min(
                (len - copied) / block_size,
                block_capacity,
            )
        } else {
            1
        };
        let (existing, run_blocks) = get_data_run(runtime, inode, logical_block, max_blocks)
            .map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
        if existing == 0 {
            ensure_data_block_with(runtime, inode, logical_block, |runtime, block| {
                // Initialize new blocks with their payload in one cached write.
                // Partial writes must still zero all previously unallocated bytes.
                unsafe {
                    if n != block_size {
                        ptr::write_bytes(runtime.block_shm as *mut u8, 0, block_size);
                    }
                    ptr::copy_nonoverlapping(
                        (session.shm_local as usize + input_offset + copied) as *const u8,
                        (runtime.block_shm as usize + block_offset) as *mut u8,
                        n,
                    );
                }
                write_blocks(runtime, block, 1)
            })
            .map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
            inode_dirty = true;
            allocation_changed = true;
            unsafe {
                runtime.mapped_pages.update(
                    inode_no,
                    file_offset + copied,
                    session.shm_local + input_offset + copied,
                    n,
                );
            }
            copied += n;
            continue;
        }
        let physical_block = existing as usize;
        if whole_blocks {
            // Look ahead only through already allocated, physically contiguous blocks.
            // Block-map reads may overwrite the scratch buffer: finish them before
            // staging any payload. Allocation and partial-block writes keep their path.
            n = run_blocks * block_size;
        } else {
            read_block(runtime, physical_block)
                .map_err(|_| nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)?;
        }
        unsafe {
            ptr::copy_nonoverlapping(
                (session.shm_local as usize + input_offset + copied) as *const u8,
                (runtime.block_shm as usize + block_offset) as *mut u8,
                n,
            );
        }
        write_blocks(runtime, physical_block, run_blocks)
            .map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
        unsafe {
            runtime.mapped_pages.update(
                inode_no,
                file_offset + copied,
                session.shm_local + input_offset + copied,
                n,
            );
        }
        copied += n;
    }
    if end > inode.size as usize {
        inode.size = end as u32;
        inode_dirty = true;
    }
    if inode_dirty {
        if allocation_changed {
            write_inode(runtime, inode_no, *inode)
        } else {
            // The inode came from read_inode and all payload blocks already
            // existed. Its stored sector count remains valid as size grows.
            write_inode_existing_blocks(runtime, inode_no, *inode)
        }.map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
    }
    Ok(copied)
}
