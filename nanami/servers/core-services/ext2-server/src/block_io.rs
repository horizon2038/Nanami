use super::*;

pub(super) fn zero_block(runtime: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    if block >= runtime.block_count || runtime.block_size as Word > runtime.block_shm_size {
        return Err(RequestError::InvalidArgument);
    }
    unsafe {
        ptr::write_bytes(runtime.block_shm as *mut u8, 0, runtime.block_size);
    }
    write_block(runtime, block)
}

pub(super) fn read_block(runtime: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    if load_cached_block(runtime, block) {
        return Ok(());
    }
    read_blocks_uncached(runtime, block, 1)?;
    store_cached_block(runtime, block, 0);
    Ok(())
}

pub(super) fn read_blocks(
    runtime: &mut Ext2Runtime,
    block: usize,
    count: usize,
) -> Result<(), RequestError> {
    if count == 1 {
        return read_block(runtime, block);
    }
    read_blocks_uncached(runtime, block, count)
}

pub(super) fn read_blocks_uncached(
    runtime: &mut Ext2Runtime,
    block: usize,
    count: usize,
) -> Result<(), RequestError> {
    let bytes = count
        .checked_mul(runtime.block_size)
        .ok_or(RequestError::InvalidArgument)?;
    let end = block
        .checked_add(count)
        .ok_or(RequestError::InvalidArgument)?;
    if count == 0
        || block >= runtime.block_count
        || end > runtime.block_count
        || bytes as Word > runtime.block_shm_size
    {
        return Err(RequestError::InvalidArgument);
    }
    let mut attempt = 0usize;
    let read = loop {
        match nanami_services::block::block_device_read(
            runtime.block_port,
            block as Word,
            count as Word,
            BLOCK_BUFFER_OFFSET,
        ) {
            Ok(read) => break read,
            Err(error)
                if attempt + 1 < BLOCK_READ_RETRY_LIMIT && is_retryable_block_read_error(error) =>
            {
                attempt += 1;
                libnanami::print!("[ext2-server] retry block read attempt=");
                libnanami::print!("{}", attempt + 1);
                libnanami::print!(" block=");
                libnanami::print!("{}", block);
                libnanami::print!(" count=");
                libnanami::print!("{}", count);
                libnanami::print!("\n");
                libnanami::yield_now();
            }
            Err(error) => return Err(error),
        }
    };
    if read as usize != bytes {
        return Err(RequestError::Protocol);
    }
    Ok(())
}

fn is_retryable_block_read_error(error: RequestError) -> bool {
    matches!(
        error,
        RequestError::Transport
            | RequestError::Protocol
            | RequestError::Status(libnanami::OS_RESPONSE_FATAL)
    )
}

pub(super) fn write_block(runtime: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    write_blocks(runtime, block, 1)
}

pub(super) fn write_blocks(
    runtime: &mut Ext2Runtime,
    block: usize,
    count: usize,
) -> Result<(), RequestError> {
    let bytes = count
        .checked_mul(runtime.block_size)
        .ok_or(RequestError::InvalidArgument)?;
    let end = block
        .checked_add(count)
        .ok_or(RequestError::InvalidArgument)?;
    if count == 0 || end > runtime.block_count || bytes > runtime.block_shm_size {
        return Err(RequestError::InvalidArgument);
    }
    let written = nanami_services::block::block_device_write(
        runtime.block_port,
        block as Word,
        count as Word,
        BLOCK_BUFFER_OFFSET,
    );
    match written {
        Ok(n) if n == bytes => {
            for offset in 0..count {
                store_cached_block(runtime, block + offset, offset * runtime.block_size);
            }
            Ok(())
        }
        result => {
            // A failed/short write may have changed a prefix on disk. Do not retain
            // stale cached blocks, and never replay the request through another path.
            for entry in &mut runtime.block_cache {
                if entry.valid && (block..end).contains(&entry.block) {
                    entry.valid = false;
                }
            }
            result.and(Err(RequestError::Protocol))
        }
    }
}

fn load_cached_block(runtime: &mut Ext2Runtime, block: usize) -> bool {
    if runtime.block_size > EXT2_BLOCK_CACHE_BYTES {
        return false;
    }
    let mut i = 0usize;
    while i < runtime.block_cache.len() {
        let entry = &runtime.block_cache[i];
        if entry.valid && entry.block == block {
            unsafe {
                ptr::copy_nonoverlapping(
                    entry.data.as_ptr(),
                    runtime.block_shm as *mut u8,
                    runtime.block_size,
                );
            }
            return true;
        }
        i += 1;
    }
    false
}

fn store_cached_block(runtime: &mut Ext2Runtime, block: usize, buffer_offset: usize) {
    if runtime.block_size > EXT2_BLOCK_CACHE_BYTES {
        return;
    }
    let mut index = runtime.block_cache_next;
    let mut i = 0usize;
    while i < runtime.block_cache.len() {
        if runtime.block_cache[i].valid && runtime.block_cache[i].block == block {
            index = i;
            break;
        }
        i += 1;
    }
    if i == runtime.block_cache.len() {
        runtime.block_cache_next = (runtime.block_cache_next + 1) % runtime.block_cache.len();
    }
    let entry = &mut runtime.block_cache[index];
    entry.valid = true;
    entry.block = block;
    unsafe {
        ptr::copy_nonoverlapping(
            (runtime.block_shm + buffer_offset) as *const u8,
            entry.data.as_mut_ptr(),
            runtime.block_size,
        );
    }
}
