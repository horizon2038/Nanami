use super::*;

pub(super) fn get_data_block(
    runtime: &mut Ext2Runtime,
    inode: &Ext2Inode,
    logical_block: usize,
) -> Result<u32, RequestError> {
    lookup_block(runtime, inode, logical_block, false)
}

fn lookup_block(
    runtime: &mut Ext2Runtime, inode: &Ext2Inode, logical_block: usize, leaf_in_scratch: bool,
) -> Result<u32, RequestError> {
    if logical_block < EXT2_MAX_DIRECT_BLOCKS {
        return Ok(inode.block[logical_block]);
    }
    let indirect_index = logical_block - EXT2_MAX_DIRECT_BLOCKS;
    let entries = indirect_entries_per_block(runtime);
    if indirect_index < entries {
        let indirect_block = inode.block[EXT2_SINGLE_INDIRECT_INDEX];
        if indirect_block == 0 {
            return Ok(0);
        }
        return leaf_pointer(runtime, indirect_block, indirect_index, leaf_in_scratch);
    }
    let double_index = indirect_index - entries;
    if double_index >= entries * entries {
        return Err(RequestError::Unsupported);
    }
    let double_block = inode.block[EXT2_DOUBLE_INDIRECT_INDEX];
    if double_block == 0 {
        return Ok(0);
    }
    let indirect_block = read_block_u32(runtime, double_block as usize, (double_index / entries) * 4)?;
    if indirect_block == 0 {
        return Ok(0);
    }
    leaf_pointer(runtime, indirect_block, double_index % entries, leaf_in_scratch)
}

fn leaf_pointer(
    runtime: &mut Ext2Runtime, block: u32, index: usize, in_scratch: bool,
) -> Result<u32, RequestError> {
    if in_scratch {
        read_block(runtime, block as usize)?;
        Ok(r32(runtime.block_shm as usize + index * 4))
    } else {
        read_block_u32(runtime, block as usize, index * 4)
    }
}

// Warm runs contained in one leaf borrow the cache directly. Other runs keep
// the existing scratch-table scan; nothing survives either request-local scan.
pub(super) fn get_data_run(
    runtime: &mut Ext2Runtime,
    inode: &Ext2Inode,
    logical: usize,
    maximum: usize,
) -> Result<(u32, usize), RequestError> {
    data_run(runtime, inode, logical, maximum, false)
}

// Read-only callers can zero adjacent holes together. Writers retain their
// one-hole allocation path, without speculative scans of later zero pointers.
pub(super) fn get_read_data_run(
    runtime: &mut Ext2Runtime, inode: &Ext2Inode, logical: usize, maximum: usize,
) -> Result<(u32, usize), RequestError> {
    data_run(runtime, inode, logical, maximum, true)
}

fn data_run(
    runtime: &mut Ext2Runtime, inode: &Ext2Inode, logical: usize, maximum: usize,
    coalesce_holes: bool,
) -> Result<(u32, usize), RequestError> {
    let (first, mut count) = match cached_leaf_run(runtime, inode, logical, maximum, coalesce_holes)? {
        Some(LeafRun::Complete(first, count)) => return Ok((first, count)),
        Some(LeafRun::Cold { leaf, offset }) => {
            // No I/O/cache mutation followed resolution. A missing child does
            // not require reading its already resolved parent a second time.
            (leaf_pointer(runtime, leaf, offset, true)?, 1)
        }
        Some(LeafRun::Prefix { first, count, leaf }) => {
            // Preserve the scratch image that the old fallback staged before
            // any fallible lookahead, without resolving/rescanning this leaf.
            read_block(runtime, leaf as usize)?;
            (first, count)
        }
        None => (lookup_block(runtime, inode, logical, maximum > 1)?, 1),
    };
    if first == 0 {
        if coalesce_holes && logical < EXT2_MAX_DIRECT_BLOCKS {
            while count < maximum && logical + count < EXT2_MAX_DIRECT_BLOCKS
                && inode.block[logical + count] == 0
            {
                count += 1;
            }
        }
        return Ok((first, count));
    }
    let entries = indirect_entries_per_block(runtime);
    while count < maximum {
        let next_logical = logical + count;
        let next = if next_logical < EXT2_MAX_DIRECT_BLOCKS {
            inode.block[next_logical]
        } else {
            let index = next_logical - EXT2_MAX_DIRECT_BLOCKS;
            let table_offset = if index < entries {
                index
            } else {
                (index - entries) % entries
            };
            // Offset zero starts a new leaf table (including single -> double).
            // A direct -> indirect transition must load it as well.
            if table_offset == 0 {
                match lookup_block(runtime, inode, next_logical, true) {
                    Ok(block) => block,
                    Err(_) => break,
                }
            } else {
                r32(runtime.block_shm as usize + table_offset * 4)
            }
        };
        if next == 0 || next as usize != first as usize + count {
            break;
        }
        count += 1;
    }
    Ok((first, count))
}

enum LeafRun {
    Complete(u32, usize),
    Cold { leaf: u32, offset: usize },
    Prefix { first: u32, count: usize, leaf: u32 },
}

fn cached_leaf_run(
    runtime: &mut Ext2Runtime, inode: &Ext2Inode, logical: usize, maximum: usize,
    coalesce_holes: bool,
) -> Result<Option<LeafRun>, RequestError> {
    if maximum <= 1 || logical < EXT2_MAX_DIRECT_BLOCKS { return Ok(None); }
    let entries = indirect_entries_per_block(runtime);
    let indirect = logical - EXT2_MAX_DIRECT_BLOCKS;
    let (leaf, offset) = if indirect < entries {
        (inode.block[EXT2_SINGLE_INDIRECT_INDEX], indirect)
    } else {
        let double = indirect - entries;
        if double >= entries * entries { return Err(RequestError::Unsupported); }
        let parent = inode.block[EXT2_DOUBLE_INDIRECT_INDEX];
        if parent == 0 {
            let count = if coalesce_holes { maximum.min(entries * entries - double) } else { 1 };
            return Ok(Some(LeafRun::Complete(0, count)));
        }
        validate_run(runtime, parent as usize, 1)?;
        let Some(word) = runtime.block_cache.load_record::<4>(parent as usize, (double / entries) * 4) else {
            return Ok(None);
        };
        (u32::from_le_bytes(*word), double % entries)
    };
    if leaf == 0 {
        let count = if coalesce_holes { maximum.min(entries - offset) } else { 1 };
        return Ok(Some(LeafRun::Complete(0, count)));
    }
    validate_run(runtime, leaf as usize, 1)?;
    let limit = maximum.min(entries - offset);
    let Some(bytes) = runtime.block_cache.load_slice(leaf as usize, offset * 4, limit * 4) else {
        return Ok(Some(LeafRun::Cold { leaf, offset }));
    };
    let first = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    if first == 0 {
        let mut count = 1;
        if coalesce_holes {
            for word in bytes[4..].chunks_exact(4) {
                if u32::from_le_bytes(word.try_into().unwrap()) != 0 {
                    break;
                }
                count += 1;
            }
        }
        return Ok(Some(LeafRun::Complete(0, count)));
    }
    let mut count = 1;
    for word in bytes[4..].chunks_exact(4) {
        let next = u32::from_le_bytes(word.try_into().unwrap());
        if next == 0 || next as usize != first as usize + count {
            return Ok(Some(LeafRun::Complete(first, count)));
        }
        count += 1;
    }
    if count == maximum { return Ok(Some(LeafRun::Complete(first, count))); }
    // A contiguous prefix reaches a new leaf. Carry its already checked run
    // forward; the caller stages this leaf before resuming the old scanner.
    Ok(Some(LeafRun::Prefix { first, count, leaf }))
}
