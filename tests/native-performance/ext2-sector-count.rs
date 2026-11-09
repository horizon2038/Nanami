//! Production inode accounting with destructively reused block scratch.
use std::collections::BTreeMap;
const EXT2_MAX_DIRECT_BLOCKS: usize = 12;
const EXT2_SINGLE_INDIRECT_INDEX: usize = 12;
const EXT2_DOUBLE_INDIRECT_INDEX: usize = 13;
const EXT2_INODE_BLOCK_POINTERS: usize = 15;
const EXT2_S_IFMT: u16 = 0xf000;
const EXT2_S_IFLNK: u16 = 0xa000;
#[derive(Clone, Copy)]
struct Ext2Inode {
    mode: u16,
    size: u32,
    sectors: u32,
    block: [u32; 15],
}
#[derive(Debug, PartialEq, Eq)]
enum RequestError { Io }
struct Ext2Runtime {
    block_size: usize,
    block_shm: usize,
    scratch: Box<[u32]>,
    tables: BTreeMap<usize, Vec<u32>>,
    reads: Vec<usize>,
    fail: Option<usize>,
}
fn indirect_entries_per_block(r: &Ext2Runtime) -> usize { r.scratch.len() }
fn r32(address: usize) -> u32 { unsafe { (address as *const u32).read() } }
fn read_block(r: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    r.reads.push(block);
    r.scratch.fill(0xdeadbeef);
    if r.fail == Some(block) { return Err(RequestError::Io); }
    r.scratch.copy_from_slice(r.tables.get(&block).ok_or(RequestError::Io)?);
    Ok(())
}
#[path = "../../nanami/servers/core-services/ext2-server/src/sector_count.rs"]
mod sector_count;

fn fixture(entries: usize, sparse: bool) -> (Ext2Runtime, Ext2Inode) {
    let mut inode = Ext2Inode { mode: 0x8000, size: 1, sectors: 0, block: [0; 15] };
    for i in 0..12 { inode.block[i] = (i + 1) as u32; }
    inode.block[12] = 20;
    inode.block[13] = 21;
    let mut tables = BTreeMap::new();
    tables.insert(20, (0..entries).map(|i| if sparse && i % 3 == 0 { 0 } else { 100 }).collect());
    let mut root = vec![0; entries];
    for (i, pointer) in root.iter_mut().enumerate() {
        if !sparse || i % 17 == 0 || i + 1 == entries {
            *pointer = 1000 + i as u32;
            tables.insert(*pointer as usize, (0..entries).map(|j| {
                if sparse && j % 5 == 0 { 0 } else { 200 }
            }).collect());
        }
    }
    tables.insert(21, root);
    let mut scratch = vec![0; entries].into_boxed_slice();
    (Ext2Runtime { block_size: entries * 4, block_shm: scratch.as_mut_ptr() as usize,
        scratch, tables, reads: Vec::new(), fail: None }, inode)
}

// Independent graph accounting counts nonzero entries, not file-size coverage.
fn expected(r: &Ext2Runtime, inode: Ext2Inode) -> u32 {
    let mut blocks = inode.block[..12].iter().filter(|&&b| b != 0).count();
    if inode.block[12] != 0 {
        blocks += 1 + r.tables[&(inode.block[12] as usize)].iter().filter(|&&b| b != 0).count();
    }
    if inode.block[13] != 0 {
        blocks += 1;
        for &child in &r.tables[&(inode.block[13] as usize)] {
            if child != 0 {
                blocks += 1 + r.tables[&(child as usize)].iter().filter(|&&b| b != 0).count();
            }
        }
    }
    blocks as u32 * (r.block_size / 512) as u32
}

#[test]
fn dense_sparse_and_partial_batches_preserve_all_block_accounting() {
    for entries in [128, 129, 256, 512, 1024] {
        for sparse in [false, true] {
            let (mut r, inode) = fixture(entries, sparse);
            let want = expected(&r, inode);
            let leaves = r.tables[&21].iter().filter(|&&b| b != 0).count();
            assert_eq!(sector_count::data_sectors(&mut r, inode), Ok(want));
            assert_eq!(r.reads.len(), 1 + entries.div_ceil(64) + leaves);
            assert_eq!(r.reads.iter().filter(|&&b| b == 21).count(), entries.div_ceil(64));
            for &child in r.tables[&21].iter().filter(|&&b| b != 0) {
                assert_eq!(r.reads.iter().filter(|&&b| b == child as usize).count(), 1);
            }
        }
    }
}

#[test]
fn direct_single_empty_and_inline_symlinks_keep_existing_rules() {
    let (mut r, mut inode) = fixture(256, true);
    inode.block[13] = 0;
    assert_eq!(sector_count::data_sectors(&mut r, inode), Ok(expected(&r, inode)));
    assert_eq!(r.reads, vec![20]);
    r.reads.clear(); inode.block[12] = 0;
    assert_eq!(sector_count::data_sectors(&mut r, inode), Ok(24));
    assert!(r.reads.is_empty());
    inode.block.fill(0);
    assert_eq!(sector_count::data_sectors(&mut r, inode), Ok(0));
    inode.mode = EXT2_S_IFLNK; inode.block.fill(0xdeadbeef); inode.size = 60;
    assert_eq!(sector_count::data_sectors(&mut r, inode), Ok(0));
    assert!(r.reads.is_empty());
    inode.size = 61;
    assert_eq!(sector_count::data_sectors(&mut r, inode), Err(RequestError::Io));
    inode.size = 60; inode.sectors = 1;
    assert_eq!(sector_count::data_sectors(&mut r, inode), Err(RequestError::Io));
}

#[test]
fn root_and_leaf_failures_propagate_without_counting_stale_scratch() {
    for block in [20, 21, 1000, 1068, 1255] {
        let (mut r, inode) = fixture(256, true); r.fail = Some(block);
        assert_eq!(sector_count::data_sectors(&mut r, inode), Err(RequestError::Io));
        assert_eq!(r.reads.last(), Some(&block));
    }
}

#[test]
fn reused_leaf_pointer_is_counted_for_each_root_entry_as_before() {
    let (mut r, inode) = fixture(256, true);
    r.tables.get_mut(&21).unwrap()[65] = 1000;
    let want = expected(&r, inode);
    assert_eq!(sector_count::data_sectors(&mut r, inode), Ok(want));
    assert_eq!(r.reads.iter().filter(|&&b| b == 1000).count(), 2);
}
