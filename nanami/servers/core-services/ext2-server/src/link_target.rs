//! Read current symlink bytes without constructing discarded inline padding.
use super::*;
use alloc::vec::Vec;
use path_walk::Error;

pub(super) fn read_link(runtime: &mut Ext2Runtime, number: u32) -> Result<Vec<u8>, Error> {
    let inode = read_inode(runtime, number).map_err(|_| Error::Io)?;
    if inode.mode & EXT2_S_IFMT != EXT2_S_IFLNK {
        return Err(Error::Invalid);
    }
    let bytes = inode.size as usize;
    if bytes > path_walk::PATH_MAX {
        return Err(Error::TooLong);
    }
    let mut output = Vec::new();
    output.try_reserve(bytes).map_err(|_| Error::NoSpace)?;
    if inode.sectors == 0 {
        if bytes > EXT2_INODE_BLOCK_POINTERS * 4 {
            return Err(Error::Io);
        }
        for block in inode.block {
            let count = (bytes - output.len()).min(4);
            if count == 0 { break; }
            output.extend_from_slice(&block.to_le_bytes()[..count]);
        }
    } else {
        let mut copied = 0;
        while copied < bytes {
            let block = get_data_block(runtime, &inode, copied / runtime.block_size)
                .map_err(|_| Error::Io)?;
            if block == 0 {
                return Err(Error::Io);
            }
            validate_run(runtime, block as usize, 1).map_err(|_| Error::Io)?;
            let count = (bytes - copied).min(runtime.block_size);
            if let Some(data) = runtime.block_cache.load_slice(block as usize, 0, count) {
                output.extend_from_slice(data);
            } else {
                read_block(runtime, block as usize).map_err(|_| Error::Io)?;
                output.extend_from_slice(unsafe {
                    core::slice::from_raw_parts(runtime.block_shm as *const u8, count)
                });
            }
            copied += count;
        }
    }
    Ok(output)
}
