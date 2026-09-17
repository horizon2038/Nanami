use super::*;

pub(super) fn zero_block(runtime: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    validate_run(runtime, block, 1)?;
    unsafe {
        ptr::write_bytes(runtime.block_shm as *mut u8, 0, runtime.block_size);
    }
    write_block(runtime, block)
}

pub(super) fn read_block(runtime: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    read_blocks(runtime, block, 1)
}

pub(super) fn read_blocks(
    runtime: &mut Ext2Runtime,
    block: usize,
    count: usize,
) -> Result<(), RequestError> {
    validate_run(runtime, block, count)?;
    let mut offset = 0;
    while offset < count {
        let target = unsafe {
            core::slice::from_raw_parts_mut(
                (runtime.block_shm + offset * runtime.block_size) as *mut u8,
                runtime.block_size,
            )
        };
        if runtime.block_cache.load(block + offset, target) {
            offset += 1;
            continue;
        }
        let first = offset;
        offset += 1;
        while offset < count && !runtime.block_cache.contains(block + offset) {
            offset += 1;
        }
        read_blocks_uncached(
            runtime,
            block + first,
            offset - first,
            first * runtime.block_size,
        )?;
        for index in first..offset {
            let source = unsafe {
                core::slice::from_raw_parts(
                    (runtime.block_shm + index * runtime.block_size) as *const u8,
                    runtime.block_size,
                )
            };
            runtime.block_cache.store(block + index, source, None);
        }
    }
    Ok(())
}

fn validate_run(runtime: &Ext2Runtime, block: usize, count: usize) -> Result<(), RequestError> {
    if count == 0
        || block
            .checked_add(count)
            .is_none_or(|end| end > runtime.block_count)
        || count
            .checked_mul(runtime.block_size)
            .is_none_or(|bytes| bytes > runtime.block_shm_size)
    {
        return Err(RequestError::InvalidArgument);
    }
    Ok(())
}

fn read_blocks_uncached(
    runtime: &Ext2Runtime,
    block: usize,
    count: usize,
    offset: usize,
) -> Result<(), RequestError> {
    let mut attempt = 0;
    let read = loop {
        match nanami_services::block::block_device_read(runtime.block_port, block, count, offset) {
            Ok(read) => break read,
            Err(error)
                if attempt + 1 < BLOCK_READ_RETRY_LIMIT
                    && matches!(
                        error,
                        RequestError::Transport
                            | RequestError::Protocol
                            | RequestError::Status(libnanami::OS_RESPONSE_FATAL)
                    ) =>
            {
                attempt += 1;
                libnanami::print!(
                    "[ext2-server] retry block read attempt={} block={} count={}\n",
                    attempt + 1,
                    block,
                    count
                );
                libnanami::yield_now();
            }
            Err(error) => return Err(error),
        }
    };
    if read == count * runtime.block_size {
        Ok(())
    } else {
        Err(RequestError::Protocol)
    }
}

pub(super) fn write_block(runtime: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    cache_write(runtime, block, 1, block_cache::BlockKind::Metadata)
}

pub(super) fn write_blocks(
    runtime: &mut Ext2Runtime,
    block: usize,
    count: usize,
) -> Result<(), RequestError> {
    cache_write(runtime, block, count, block_cache::BlockKind::Data)
}

fn cache_write(
    runtime: &mut Ext2Runtime,
    block: usize,
    count: usize,
    kind: block_cache::BlockKind,
) -> Result<(), RequestError> {
    validate_run(runtime, block, count)?;
    runtime.block_cache.check_writable()?;
    if !runtime.block_cache.has_room_for(block, count) {
        flush_blocks(runtime)?;
    }
    for offset in 0..count {
        let source = unsafe {
            core::slice::from_raw_parts(
                (runtime.block_shm + offset * runtime.block_size) as *const u8,
                runtime.block_size,
            )
        };
        runtime
            .block_cache
            .store(block + offset, source, Some(kind));
    }
    Ok(())
}

pub(super) fn flush_blocks(runtime: &mut Ext2Runtime) -> Result<(), RequestError> {
    let scratch = unsafe {
        core::slice::from_raw_parts_mut(runtime.block_shm as *mut u8, runtime.block_shm_size)
    };
    runtime.block_cache.flush(runtime.block_port, scratch)
}
