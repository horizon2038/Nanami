//! Continuation positions are a cache, never a new externally visible cookie.
use super::*;

#[derive(Clone, Copy)]
struct Cursor {
    inode: u32,
    generation: u64,
    index: usize,
    block: usize,
    offset: usize,
}
impl Cursor {
    const EMPTY: Self = Self {
        inode: 0,
        generation: 0,
        index: 0,
        block: 0,
        offset: 0,
    };
}

pub(super) struct Cursors {
    generation: u64,
    entries: [Cursor; MAX_HANDLES],
}
impl Cursors {
    pub(super) const fn new() -> Self {
        Self {
            generation: 1,
            entries: [Cursor::EMPTY; MAX_HANDLES],
        }
    }
}

pub(super) fn invalidate(runtime: &mut Ext2Runtime) {
    let cursors = &mut runtime.directory_cursors;
    if let Some(next) = cursors.generation.checked_add(1) {
        cursors.generation = next;
    } else {
        cursors.entries.fill(Cursor::EMPTY);
        cursors.generation = 1;
    }
}

pub(super) fn read_directory(
    runtime: &mut Ext2Runtime,
    session: ClientSession,
    inode: Ext2Inode,
    start_index: usize,
    max_entries: usize,
    out_offset: usize,
    handle: usize,
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
    let inode_no = runtime.handles[handle].inode;
    let generation = runtime.directory_cursors.generation;
    let cached = runtime.directory_cursors.entries[handle];
    let mut cursor = if start_index != 0
        && cached.inode == inode_no
        && cached.generation == generation
        && cached.index == start_index
    {
        cached
    } else {
        Cursor {
            inode: inode_no,
            generation,
            ..Cursor::EMPTY
        }
    };
    let mut written = 0usize;
    while cursor.block < max_file_blocks(runtime) && written < limit_entries {
        let block_file_offset = cursor.block * runtime.block_size;
        if block_file_offset >= inode.size as usize {
            break;
        }
        let block = get_data_block(runtime, inode, cursor.block)
            .map_err(|_| nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)?;
        if block == 0 {
            break;
        }
        read_block(runtime, block as usize)
            .map_err(|_| nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)?;
        let block_limit = min(runtime.block_size, inode.size as usize - block_file_offset);
        while cursor.offset < block_limit && written < limit_entries {
            let Some(entry) = parse_directory_entry(
                runtime.block_shm as usize + cursor.offset,
                block_limit - cursor.offset,
            ) else {
                return Err(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR);
            };
            if entry.inode != 0 {
                if cursor.index >= start_index {
                    unsafe {
                        write_vfs_dirent(
                            (session.shm_local as usize + out_offset + written * record_bytes)
                                as *mut u8,
                            entry,
                        );
                    }
                    written += 1;
                }
                cursor.index += 1;
            }
            cursor.offset += entry.record_len;
        }
        if cursor.offset == block_limit {
            cursor.block += 1;
            cursor.offset = 0;
        }
    }
    runtime.directory_cursors.entries[handle] = cursor;
    Ok((written, cursor.index))
}

pub(super) fn parse_directory_entry(addr: usize, available: usize) -> Option<Ext2DirectoryEntry> {
    if available < 8 {
        return None;
    }
    let record_len = r16(addr + 4) as usize;
    let name_len = unsafe { *((addr + 6) as *const u8) as usize };
    let file_type = unsafe { *((addr + 7) as *const u8) };
    if record_len < 8
        || record_len > available
        || name_len > EXT2_NAME_LEN_MAX
        || 8 + name_len > record_len
    {
        return None;
    }
    Some(Ext2DirectoryEntry {
        inode: r32(addr + 0),
        record_len,
        name_len,
        file_type,
        name_ptr: (addr + 8) as *const u8,
    })
}

pub(super) unsafe fn write_vfs_dirent(dst: *mut u8, entry: Ext2DirectoryEntry) {
    write_word(
        dst.add(nanami_client::vfs::VFS_DIRECTORY_ENTRY_INODE_OFFSET),
        entry.inode as Word,
    );
    write_word(
        dst.add(nanami_client::vfs::VFS_DIRECTORY_ENTRY_TYPE_OFFSET),
        ext2_dirent_file_type_to_vfs(entry.file_type),
    );
    write_word(
        dst.add(nanami_client::vfs::VFS_DIRECTORY_ENTRY_NAME_LEN_OFFSET),
        entry.name_len as Word,
    );
    write_word(
        dst.add(nanami_client::vfs::VFS_DIRECTORY_ENTRY_RECORD_LEN_OFFSET),
        nanami_client::vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES as Word,
    );
    ptr::write_bytes(
        dst.add(nanami_client::vfs::VFS_DIRECTORY_ENTRY_NAME_OFFSET),
        0,
        nanami_client::vfs::VFS_DIRECTORY_ENTRY_NAME_BYTES,
    );
    ptr::copy_nonoverlapping(
        entry.name_ptr,
        dst.add(nanami_client::vfs::VFS_DIRECTORY_ENTRY_NAME_OFFSET),
        min(
            entry.name_len,
            nanami_client::vfs::VFS_DIRECTORY_ENTRY_NAME_BYTES,
        ),
    );
}
