//! Pre-optimization directory reader, retained only as a test oracle.
use super::*;

pub(super) fn parent_name(runtime:&mut Ext2Runtime,parent:u32,directory:u32)
    ->Result<Vec<u8>,path_walk::Error> {
    use path_walk::Error;
    let inode=read_inode(runtime,parent).map_err(|_|Error::Io)?;
    for logical in 0..(inode.size as usize).div_ceil(runtime.block_size) {
        let block=get_data_block(runtime,inode,logical).map_err(|_|Error::Io)?;
        if block==0 {return Err(Error::Io);}
        read_block(runtime,block as usize).map_err(|_|Error::Io)?;
        let mut offset=0;
        while offset<runtime.block_size {
            let entry=parse_directory_entry(runtime.block_shm+offset,runtime.block_size-offset).ok_or(Error::Io)?;
            let name=unsafe {core::slice::from_raw_parts(entry.name_ptr,entry.name_len)};
            if entry.inode==directory && name!=b"." && name!=b".." {return Ok(name.to_vec());}
            offset+=entry.record_len;
        }
    }
    Err(Error::Missing)
}

pub(super) unsafe fn lookup_in_directory(
    runtime:&mut Ext2Runtime, parent:u32, name:*const u8, len:usize,
)->Option<u32> {
    if let Some(cached)=load_cached_dentry(runtime,parent,name,len) {return cached;}
    let inode=read_inode(runtime,parent).ok()?;
    if inode.mode & EXT2_S_IFMT != EXT2_S_IFDIR {return None;}
    let mut logical=0;
    while logical<max_file_blocks(runtime) {
        let block=get_data_block(runtime,inode,logical).ok()?;
        if block==0 {break;}
        read_block(runtime,block as usize).ok()?;
        let limit=min(runtime.block_size,inode.size.saturating_sub((logical*runtime.block_size) as u32) as usize);
        let mut offset=0;
        while offset<limit {
            let entry=parse_directory_entry(runtime.block_shm+offset,limit-offset)?;
            if entry.inode!=0 && entry.name_len==len && bytes_eq(entry.name_ptr,name,len) {
                store_cached_dentry(runtime,parent,name,len,Some(entry.inode));
                return Some(entry.inode);
            }
            offset+=entry.record_len;
        }
        logical+=1;
    }
    store_cached_dentry(runtime,parent,name,len,None);
    None
}

pub(super) fn read_directory(
    runtime: &mut Ext2Runtime,
    session: ClientSession,
    inode: Ext2Inode,
    start_index: usize,
    max_entries: usize,
    out_offset: usize,
) -> Result<(usize, usize), Word> {
    let record_bytes = nanami_client::vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES;
    let max_by_buffer = if out_offset <= session.shm_size as usize {
        (session.shm_size as usize - out_offset) / record_bytes
    } else {
        0
    };
    let limit_entries = min(max_entries, max_by_buffer);
    if limit_entries == 0 {
        return Ok((0, 0));
    }
    let mut seen = 0usize;
    let mut written = 0usize;
    let mut logical_block = 0usize;
    while logical_block < max_file_blocks(runtime) && written < limit_entries {
        let block = get_data_block(runtime, inode, logical_block)
            .map_err(|_| nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)?;
        if block == 0 {
            break;
        }
        if read_block(runtime, block as usize).is_err() {
            return Err(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR);
        }
        let block_file_offset = logical_block * runtime.block_size;
        if block_file_offset >= inode.size as usize {
            break;
        }
        let block_limit = min(runtime.block_size, inode.size as usize - block_file_offset);
        let mut offset = 0usize;
        while offset < block_limit && written < limit_entries {
            let Some(entry) =
                parse_directory_entry(runtime.block_shm as usize + offset, block_limit - offset)
            else {
                return Err(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR);
            };
            if entry.inode != 0 {
                if seen >= start_index {
                    unsafe {
                        write_vfs_dirent(
                            (session.shm_local as usize + out_offset + written * record_bytes)
                                as *mut u8,
                            entry,
                        );
                    }
                    written += 1;
                }
                seen += 1;
            }
            offset += entry.record_len;
        }
        logical_block += 1;
    }
    Ok((written, seen))
}
