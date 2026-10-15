//! Real ext2 dirent parsing/writing and continuation logic; block I/O mocked.
#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_runtime;
use std::{cmp::min, ptr};
type Word = usize;
const MAX_HANDLES: usize = 2;
const EXT2_NAME_LEN_MAX: usize = 255;
const BLOCK_SIZE: usize = 128;
pub const OS_RESPONSE_INVALID_DESCRIPTOR: Word = 3;
pub mod vfs {
    pub use nanami_uapi::vfs::*;
}
#[derive(Clone, Copy)]
struct ClientSession {
    shm_local: Word,
    shm_size: Word,
}
#[derive(Clone, Copy)]
struct Ext2Inode {
    size: u32,
}
#[derive(Clone, Copy)]
struct Ext2DirectoryEntry {
    inode: u32,
    record_len: usize,
    name_len: usize,
    file_type: u8,
    name_ptr: *const u8,
}
struct Handle {
    inode: u32,
}
struct Ext2Runtime {
    handles: [Handle; MAX_HANDLES],
    directory_cursors: directory::Cursors,
    block_size: usize,
    block_shm: Word,
    scratch: Box<[u8; BLOCK_SIZE]>,
    disk: Vec<[u8; BLOCK_SIZE]>,
    reads: usize,
    fail_read: Option<usize>,
}
#[path = "baseline-directory.rs"]
mod baseline;
#[path = "../../nanami/servers/core-services/ext2-server/src/directory.rs"]
mod directory;
use directory::{parse_directory_entry, write_vfs_dirent};
fn max_file_blocks(_: &Ext2Runtime) -> usize {
    65536
}
fn get_data_block(r: &mut Ext2Runtime, _: Ext2Inode, logical: usize) -> Result<u32, ()> {
    Ok(if logical + 1 < r.disk.len() {
        (logical + 1) as u32
    } else {
        0
    })
}
fn read_block(r: &mut Ext2Runtime, block: usize) -> Result<(), ()> {
    r.reads += 1;
    if r.fail_read == Some(block) {
        return Err(());
    }
    *r.scratch = r.disk[block];
    Ok(())
}
fn r16(p: usize) -> u16 {
    unsafe { ptr::read_unaligned(p as *const u16) }
}
fn r32(p: usize) -> u32 {
    unsafe { ptr::read_unaligned(p as *const u32) }
}
unsafe fn write_word(p: *mut u8, value: Word) {
    ptr::write_unaligned(p as *mut Word, value);
}
fn ext2_dirent_file_type_to_vfs(kind: u8) -> Word {
    match kind {
        1 => vfs::VFS_FILE_TYPE_REGULAR,
        2 => vfs::VFS_FILE_TYPE_DIRECTORY,
        _ => vfs::VFS_FILE_TYPE_UNKNOWN,
    }
}
fn fixture(entries: usize) -> (Ext2Runtime, Ext2Inode) {
    let mut disk = vec![[0; BLOCK_SIZE]];
    for block in 0..entries.div_ceil(8) {
        let mut data = [0; BLOCK_SIZE];
        for index in 0..8 {
            let number = block * 8 + index;
            let base = index * 16;
            let live = number < entries && number % 7 != 3;
            data[base..base + 4]
                .copy_from_slice(&if live { number as u32 + 1 } else { 0 }.to_le_bytes());
            data[base + 4..base + 6].copy_from_slice(&16u16.to_le_bytes());
            data[base + 6] = 8;
            data[base + 7] = if number % 5 == 0 { 2 } else { 1 };
            data[base + 8..base + 16].copy_from_slice(format!("f{number:07}").as_bytes());
        }
        disk.push(data);
    }
    let inode = Ext2Inode {
        size: ((disk.len() - 1) * BLOCK_SIZE) as u32,
    };
    let mut scratch = Box::new([0; BLOCK_SIZE]);
    (
        Ext2Runtime {
            handles: [Handle { inode: 7 }, Handle { inode: 7 }],
            directory_cursors: directory::Cursors::new(),
            block_size: BLOCK_SIZE,
            block_shm: scratch.as_mut_ptr() as Word,
            scratch,
            disk,
            reads: 0,
            fail_read: None,
        },
        inode,
    )
}
fn compare(
    actual: &mut Ext2Runtime,
    expected: &mut Ext2Runtime,
    inode: Ext2Inode,
    handle: usize,
    start: usize,
    limit: usize,
) -> (usize, usize) {
    let mut result = vec![0xa5u8; 8 + 16 * vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES + 8];
    let mut reference = result.clone();
    let session = ClientSession {
        shm_local: result.as_mut_ptr() as Word + 8,
        shm_size: result.len() - 16,
    };
    let baseline_session = ClientSession {
        shm_local: reference.as_mut_ptr() as Word + 8,
        ..session
    };
    let a = directory::read_directory(actual, session, inode, start, limit, 0, handle);
    let b = baseline::read_directory(expected, baseline_session, inode, start, limit, 0);
    assert_eq!(a, b, "start={start} limit={limit}");
    assert_eq!(result, reference);
    assert!(result[..8]
        .iter()
        .chain(&result[result.len() - 8..])
        .all(|&byte| byte == 0xa5));
    a.unwrap()
}

#[test]
fn sequential_batches_are_linear_and_match_full_rescans_with_deleted_entries() {
    let (mut actual, inode) = fixture(1024);
    let (mut expected, _) = fixture(1024);
    let mut start = 0;
    loop {
        let (entries, next) = compare(&mut actual, &mut expected, inode, 0, start, 1);
        if entries == 0 {
            break;
        }
        assert_eq!(next, start + 1);
        start = next;
    }
    assert!(actual.reads <= 1024);
    assert!(expected.reads > actual.reads * 32);
}

#[test]
fn rewind_forward_backward_seek_and_interleaved_handles_preserve_index_cookies() {
    let (mut actual, inode) = fixture(96);
    let (mut expected, _) = fixture(96);
    for (handle, start, limit) in [
        (0, 0, 3),
        (1, 0, 7),
        (0, 3, 4),
        (1, 7, 8),
        (0, 7, 4),
        (0, 2, 1),
        (0, 70, 3),
        (0, 0, 2),
        (0, 200, 5),
        (0, 1, 16),
    ] {
        compare(&mut actual, &mut expected, inode, handle, start, limit);
    }
    actual.handles[0].inode = 9;
    compare(&mut actual, &mut expected, inode, 0, 17, 2);
}

#[test]
fn metadata_mutation_invalidates_cached_byte_position_even_when_size_is_unchanged() {
    let (mut actual, inode) = fixture(48);
    let (mut expected, _) = fixture(48);
    let (_, next) = compare(&mut actual, &mut expected, inode, 0, 0, 8);
    actual.disk[1][..4].fill(0);
    expected.disk[1][..4].fill(0);
    directory::invalidate(&mut actual);
    compare(&mut actual, &mut expected, inode, 0, next, 8);
}

#[test]
fn failed_io_and_malformed_entries_do_not_commit_a_bad_cursor() {
    let (mut actual, inode) = fixture(48);
    let (mut expected, _) = fixture(48);
    let (_, next) = compare(&mut actual, &mut expected, inode, 0, 0, 6);
    actual.fail_read = Some(2);
    expected.fail_read = Some(2);
    let mut bytes = vec![0u8; 16 * vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES];
    let session = ClientSession {
        shm_local: bytes.as_mut_ptr() as Word,
        shm_size: bytes.len(),
    };
    assert!(directory::read_directory(&mut actual, session, inode, next, 16, 0, 0).is_err());
    actual.fail_read = None;
    expected.fail_read = None;
    compare(&mut actual, &mut expected, inode, 0, next, 16);
    actual.disk[1][4..6].fill(0);
    directory::invalidate(&mut actual);
    assert!(directory::read_directory(&mut actual, session, inode, 0, 16, 0, 0).is_err());
}

#[test]
fn zero_capacity_and_invalid_output_range_do_not_advance_the_cursor() {
    let (mut actual, inode) = fixture(16);
    let (mut expected, _) = fixture(16);
    let (_, next) = compare(&mut actual, &mut expected, inode, 0, 0, 3);
    let session = ClientSession {
        shm_local: 0,
        shm_size: 0,
    };
    assert_eq!(
        directory::read_directory(&mut actual, session, inode, next, 1, 1, 0),
        Ok((0, 0))
    );
    assert_eq!(
        directory::read_directory(&mut actual, session, inode, next, 0, 0, 0),
        Ok((0, 0))
    );
    compare(&mut actual, &mut expected, inode, 0, next, 3);
}
