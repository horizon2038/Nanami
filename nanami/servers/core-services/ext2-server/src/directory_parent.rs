//! Find a child's current name without retaining inode/name hints across calls.
use super::*;
use alloc::vec::Vec;
use path_walk::Error;

pub(super) fn read_parent_name(
    runtime: &mut Ext2Runtime, parent: u32, directory: u32,
) -> Result<Vec<u8>, Error> {
    let inode = read_inode(runtime, parent).map_err(|_| Error::Io)?;
    for logical in 0..(inode.size as usize).div_ceil(runtime.block_size) {
        let block = get_data_block(runtime, &inode, logical).map_err(|_| Error::Io)?;
        if block == 0 { return Err(Error::Io); }
        validate_run(runtime, block as usize, 1).map_err(|_| Error::Io)?;
        // Match the old parent scan's full-block extent, including its final
        // block. Borrow through name.to_vec(); return no pointer into the cache.
        let name = if let Some(bytes) = runtime.block_cache.load_slice(block as usize, 0, runtime.block_size) {
            find_name(bytes, directory)?
        } else {
            read_block(runtime, block as usize).map_err(|_| Error::Io)?;
            let bytes = unsafe { core::slice::from_raw_parts(runtime.block_shm as *const u8, runtime.block_size) };
            find_name(bytes, directory)?
        };
        if let Some(name) = name { return Ok(name); }
    }
    Err(Error::Missing)
}

fn find_name(bytes: &[u8], directory: u32) -> Result<Option<Vec<u8>>, Error> {
    let mut offset = 0;
    while offset < bytes.len() {
        let entry = parse_directory_entry(bytes.as_ptr() as usize + offset, bytes.len() - offset)
            .ok_or(Error::Io)?;
        let name = unsafe { core::slice::from_raw_parts(entry.name_ptr, entry.name_len) };
        if entry.inode == directory && name != b"." && name != b".." {
            return Ok(Some(name.to_vec()));
        }
        offset += entry.record_len;
    }
    Ok(None)
}
