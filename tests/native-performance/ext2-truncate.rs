//! Production block-tree pruning with a small in-memory block device.
#![allow(dead_code)]
extern crate alloc;
use std::{cmp::min, collections::BTreeSet, ptr};
const EXT2_MAX_DIRECT_BLOCKS: usize = 12;
const EXT2_SINGLE_INDIRECT_INDEX: usize = 12;
const EXT2_DOUBLE_INDIRECT_INDEX: usize = 13;
const BLOCK_SIZE: usize = 64;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestError {
    InvalidArgument,
    Transport,
}
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct Ext2Inode {
    size: u32,
    block: [u32; 15],
}
struct Ext2Runtime {
    block_size: usize,
    block_shm: usize,
    scratch: Box<[u8; BLOCK_SIZE]>,
    disk: Vec<[u8; BLOCK_SIZE]>,
    freed: BTreeSet<usize>,
    committed: Ext2Inode,
    inode_writes: usize,
    fail_inode: bool,
    fail_read: Option<usize>,
}
#[path = "../../nanami/servers/core-services/ext2-server/src/truncate.rs"]
mod production;
fn indirect_entries_per_block(_: &Ext2Runtime) -> usize {
    BLOCK_SIZE / 4
}
fn max_file_blocks(r: &Ext2Runtime) -> usize {
    let n = indirect_entries_per_block(r);
    12 + n + n * n
}
fn r32(p: usize) -> u32 {
    unsafe { ptr::read_unaligned(p as *const u32) }
}
fn w32_mem(p: usize, value: u32) {
    unsafe { ptr::write_unaligned(p as *mut u32, value) }
}
fn read_block(r: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    if r.fail_read == Some(block) {
        return Err(RequestError::Transport);
    }
    assert!(!r.freed.contains(&block));
    *r.scratch = r.disk[block];
    Ok(())
}
fn write_block(r: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    assert!(!r.freed.contains(&block));
    r.disk[block] = *r.scratch;
    Ok(())
}
fn write_blocks(r: &mut Ext2Runtime, block: usize, count: usize) -> Result<(), RequestError> {
    assert_eq!(count, 1);
    write_block(r, block)
}
fn write_inode(r: &mut Ext2Runtime, _: u32, inode: Ext2Inode) -> Result<(), RequestError> {
    if r.fail_inode {
        return Err(RequestError::Transport);
    }
    r.committed = inode;
    r.inode_writes += 1;
    Ok(())
}
fn table_get(r: &mut Ext2Runtime, block: u32, index: usize) -> Result<u32, RequestError> {
    if block == 0 {
        return Ok(0);
    }
    read_block(r, block as usize)?;
    Ok(r32(r.block_shm + index * 4))
}
fn get_data_block(
    r: &mut Ext2Runtime,
    inode: Ext2Inode,
    logical: usize,
) -> Result<u32, RequestError> {
    if logical < 12 {
        return Ok(inode.block[logical]);
    }
    let n = indirect_entries_per_block(r);
    if logical < 12 + n {
        return table_get(r, inode.block[12], logical - 12);
    }
    let i = logical - 12 - n;
    let table = table_get(r, inode.block[13], i / n)?;
    table_get(r, table, i % n)
}
fn references(r: &Ext2Runtime) -> BTreeSet<usize> {
    fn tree(r: &Ext2Runtime, found: &mut BTreeSet<usize>, block: u32, level: u32) {
        if block == 0 {
            return;
        }
        assert!(found.insert(block as usize));
        if level != 0 {
            for entry in r.disk[block as usize].chunks_exact(4) {
                tree(
                    r,
                    found,
                    u32::from_le_bytes(entry.try_into().unwrap()),
                    level - 1,
                );
            }
        }
    }
    let mut found = BTreeSet::new();
    for b in &r.committed.block[..12] {
        tree(r, &mut found, *b, 0);
    }
    tree(r, &mut found, r.committed.block[12], 1);
    tree(r, &mut found, r.committed.block[13], 2);
    found
}
fn free_block(r: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    assert!(
        !references(r).contains(&block),
        "free of a referenced block"
    );
    assert!(r.freed.insert(block), "double free");
    Ok(())
}
fn allocate(r: &mut Ext2Runtime, data: bool) -> u32 {
    let block = r.disk.len() as u32;
    r.disk.push([if data { 0x73 } else { 0 }; BLOCK_SIZE]);
    block
}
fn set(r: &mut Ext2Runtime, table: u32, index: usize, value: u32) {
    r.disk[table as usize][index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
}
fn fixture(blocks: usize) -> (Ext2Runtime, Ext2Inode) {
    let mut scratch = Box::new([0; BLOCK_SIZE]);
    let mut r = Ext2Runtime {
        block_size: BLOCK_SIZE,
        block_shm: scratch.as_mut_ptr() as usize,
        scratch,
        disk: vec![[0; BLOCK_SIZE]],
        freed: BTreeSet::new(),
        committed: Ext2Inode::default(),
        inode_writes: 0,
        fail_inode: false,
        fail_read: None,
    };
    let mut inode = Ext2Inode {
        size: (blocks * BLOCK_SIZE) as u32,
        ..Ext2Inode::default()
    };
    let n = indirect_entries_per_block(&r);
    for logical in 0..blocks {
        let block = allocate(&mut r, true);
        if logical < 12 {
            inode.block[logical] = block;
        } else if logical < 12 + n {
            if inode.block[12] == 0 {
                inode.block[12] = allocate(&mut r, false);
            }
            set(&mut r, inode.block[12], logical - 12, block);
        } else {
            if inode.block[13] == 0 {
                inode.block[13] = allocate(&mut r, false);
            }
            let index = logical - 12 - n;
            let mut child = table_get(&mut r, inode.block[13], index / n).unwrap();
            if child == 0 {
                child = allocate(&mut r, false);
                set(&mut r, inode.block[13], index / n, child);
            }
            set(&mut r, child, index % n, block);
        }
    }
    r.committed = inode;
    (r, inode)
}

#[test]
fn shrinking_across_every_block_tree_boundary_frees_only_unreachable_blocks() {
    for length in [
        0,
        1,
        12 * BLOCK_SIZE - 1,
        12 * BLOCK_SIZE,
        12 * BLOCK_SIZE + 1,
        28 * BLOCK_SIZE - 1,
        28 * BLOCK_SIZE,
        28 * BLOCK_SIZE + 1,
        44 * BLOCK_SIZE,
        45 * BLOCK_SIZE - 3,
    ] {
        let (mut r, mut inode) = fixture(50);
        let allocated = r.disk.len() - 1;
        production::resize_inode(&mut r, 7, &mut inode, length).unwrap();
        assert_eq!(inode.size as usize, length);
        assert_eq!(references(&r).len() + r.freed.len(), allocated);
        for logical in 0..50 {
            let b = get_data_block(&mut r, inode, logical).unwrap() as usize;
            if logical < length.div_ceil(BLOCK_SIZE) {
                assert_ne!(b, 0);
                for (i, byte) in r.disk[b].iter().copied().enumerate() {
                    assert_eq!(
                        byte,
                        if logical * BLOCK_SIZE + i < length {
                            0x73
                        } else {
                            0
                        }
                    );
                }
            } else {
                assert_eq!(b, 0);
            }
        }
    }
}
#[test]
fn shrink_then_grow_preserves_prefix_zeroes_tail_and_keeps_holes_sparse() {
    let (mut r, mut inode) = fixture(50);
    production::resize_inode(&mut r, 7, &mut inode, 5).unwrap();
    let allocation = r.disk.len();
    production::resize_inode(&mut r, 7, &mut inode, 50 * BLOCK_SIZE).unwrap();
    assert_eq!(r.disk.len(), allocation);
    assert_eq!(&r.disk[inode.block[0] as usize][..5], &[0x73; 5]);
    assert!(r.disk[inode.block[0] as usize][5..].iter().all(|b| *b == 0));
    for logical in 1..50 {
        assert_eq!(get_data_block(&mut r, inode, logical), Ok(0));
    }
}
#[test]
fn grow_clears_existing_partial_eof_and_same_size_does_no_io() {
    let (mut r, mut inode) = fixture(1);
    inode.size = 5;
    production::resize_inode(&mut r, 7, &mut inode, 100).unwrap();
    assert!(r.disk[inode.block[0] as usize][5..].iter().all(|b| *b == 0));
    r.fail_inode = true;
    production::resize_inode(&mut r, 7, &mut inode, 100).unwrap();
    assert_eq!(r.inode_writes, 1);
}
#[test]
fn read_or_inode_write_failure_never_releases_referenced_blocks() {
    for fail_read in [false, true] {
        let (mut r, mut inode) = fixture(50);
        if fail_read {
            r.fail_read = Some(inode.block[13] as usize);
        } else {
            r.fail_inode = true;
        }
        assert_eq!(
            production::resize_inode(&mut r, 7, &mut inode, 5),
            Err(RequestError::Transport)
        );
        assert!(r.freed.is_empty());
        assert_eq!(inode.size as usize, 50 * BLOCK_SIZE);
    }
}
#[test]
fn invalid_size_is_rejected_before_mutation() {
    let (mut r, mut inode) = fixture(1);
    let original = inode;
    assert_eq!(
        production::resize_inode(&mut r, 7, &mut inode, usize::MAX),
        Err(RequestError::InvalidArgument)
    );
    assert_eq!(inode, original);
    assert_eq!(r.inode_writes, 0);
    assert!(r.freed.is_empty());
}
