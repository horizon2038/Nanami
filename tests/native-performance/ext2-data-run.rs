#![allow(dead_code)]
extern crate alloc;
extern crate self as nanami_runtime;
extern crate self as nanami_client;
use std::collections::BTreeMap;
pub type Word = usize;
pub mod block {
    use super::*;
    pub fn block_device_flush(_:Word)->Result<(),RequestError> {panic!("pointer lookup must not flush");}
    pub fn block_device_write(_:Word,_:Word,_:Word,_:Word)->Result<Word,RequestError> {panic!("pointer lookup must not write");}
}
#[path = "../../nanami/servers/core-services/ext2-server/src/block_cache.rs"]
mod block_cache;

const EXT2_MAX_DIRECT_BLOCKS: usize = 12;
const EXT2_SINGLE_INDIRECT_INDEX: usize = 12;
const EXT2_DOUBLE_INDIRECT_INDEX: usize = 13;
#[derive(Clone, Copy)]
struct Ext2Inode {
    block: [u32; 15],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    InvalidArgument,
    Protocol,
    Unsupported,
    Io,
}
struct Ext2Runtime {
    block_size: usize,
    block_count: usize,
    block_cache: block_cache::BlockCache,
    block_shm: usize,
    scratch: Box<[u32]>,
    tables: BTreeMap<usize, Vec<u32>>,
    reads: Vec<usize>,
    fail: Option<usize>,
    cache_aware: bool,
}
fn validate_run(runtime:&Ext2Runtime, block:usize, count:usize)->Result<(),RequestError> {
    if count == 0 || block.checked_add(count).is_none_or(|end|end > runtime.block_count) {
        return Err(RequestError::InvalidArgument);
    }
    Ok(())
}
fn indirect_entries_per_block(runtime: &Ext2Runtime) -> usize {
    runtime.scratch.len()
}
fn r32(address: usize) -> u32 {
    unsafe { (address as *const u32).read() }
}
fn read_block(runtime: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    validate_run(runtime, block, 1)?;
    runtime.reads.push(block);
    if runtime.cache_aware {
        let bytes = unsafe {core::slice::from_raw_parts_mut(runtime.block_shm as *mut u8, runtime.block_size)};
        if runtime.block_cache.load(block, bytes) { return Ok(()); }
    }
    if runtime.fail == Some(block) {
        return Err(RequestError::Io);
    }
    runtime.scratch.copy_from_slice(runtime.tables.get(&block).ok_or(RequestError::Io)?);
    Ok(())
}
#[path = "../../nanami/servers/core-services/ext2-server/src/data_run.rs"]
mod data_run;
fn read_block_u32(runtime: &mut Ext2Runtime, block: usize, offset: usize) -> Result<u32, RequestError> {
    validate_run(runtime, block, 1)?;
    // Model a warm word lookup: count the table probe, never populate scratch.
    runtime.reads.push(block);
    if runtime.cache_aware {
        if let Some(bytes) = runtime.block_cache.load_record::<4>(block, offset) {
            return Ok(u32::from_le_bytes(*bytes));
        }
    }
    if runtime.fail == Some(block) { return Err(RequestError::Io); }
    runtime.tables.get(&block).and_then(|table|table.get(offset/4)).copied().ok_or(RequestError::Io)
}
use data_run::{get_data_block, get_data_run};

fn read_run(runtime: &mut Ext2Runtime, inode: &Ext2Inode, logical: usize, maximum: usize)
    -> Result<(u32, usize), RequestError>
{
    data_run::get_read_data_run(runtime, inode, logical, maximum)
}

#[test]
fn sparse_read_runs_match_current_zero_pointers_and_stop_before_data_or_boundaries() {
    let (mut runtime, _) = fixture(256, false);
    let inode = Ext2Inode { block: [0; 15] };
    let mut counts = Vec::new();
    for logical in [0, 12, 268] {
        let (first, count) = read_run(&mut runtime, &inode, logical, 16).unwrap();
        assert_eq!(first, 0);
        assert!((1..=16).contains(&count));
        for index in 0..count {
            assert_eq!(get_data_block(&mut runtime, &inode, logical + index), Ok(0));
        }
        counts.push(count);
    }
    assert!(runtime.reads.is_empty());
    println!("SPARSE-READ missing-direct-single-double counts={counts:?}");
    assert_eq!(counts, [12, 16, 16]);
    let mut query_counts = Vec::new();
    for reads in [false, true] {
        let mut logical = 0;
        let mut queries = 0;
        while logical < 64 {
            let maximum = (64 - logical).min(16);
            let (first, count) = if reads {
                read_run(&mut runtime, &inode, logical, maximum)
            } else {
                get_data_run(&mut runtime, &inode, logical, maximum)
            }.unwrap();
            assert_eq!(first, 0);
            logical += count;
            queries += 1;
        }
        assert_eq!(logical, 64);
        query_counts.push(queries);
    }
    assert_eq!(query_counts, [64, 5]);
    println!("SPARSE-READ real-pointer-64-block-queries={query_counts:?}");
    for entries in [4, 16, 256] {
        for maximum in [0usize, 1, 2, 16, 64] {
            let (mut runtime, inode) = fixture(entries, false);
            runtime.cache_aware = true;
            let mut table = vec![0u32; entries];
            table[entries - 1] = 9000;
            let bytes: Vec<u8> = table.iter().flat_map(|word| word.to_le_bytes()).collect();
            runtime.block_cache.store(100, &bytes, Some(block_cache::BlockKind::Metadata));
            for offset in 0..entries {
                runtime.reads.clear();
                runtime.scratch.fill(0xdeadbeef);
                let (first, count) = read_run(&mut runtime, &inode, 12 + offset, maximum).unwrap();
                assert!(count >= 1 && count <= maximum.max(1));
                if offset == entries - 1 {
                    assert_eq!((first, count), (9000, 1));
                } else {
                    assert_eq!(first, 0);
                    assert!(offset + count <= entries - 1);
                    if maximum > 1 {
                        assert!(runtime.reads.is_empty());
                        assert!(runtime.scratch.iter().all(|word| *word == 0xdeadbeef));
                    }
                }
            }
        }
    }
}

fn fixture(entries: usize, fragmented: bool) -> (Ext2Runtime, Ext2Inode) {
    let mut values: Vec<u32> = (0..12 + entries + entries * entries)
        .map(|index| 1000 + index as u32)
        .collect();
    if fragmented {
        for (index, value) in values.iter_mut().enumerate() {
            if index % 7 == 3 {
                *value = 0;
            } else if index % 11 == 5 {
                *value += 1234;
            }
        }
    }
    let mut inode = Ext2Inode { block: [0; 15] };
    inode.block[..12].copy_from_slice(&values[..12]);
    inode.block[12] = 100;
    inode.block[13] = 101;
    let mut tables = BTreeMap::new();
    tables.insert(100, values[12..12 + entries].to_vec());
    tables.insert(101, (0..entries).map(|index| 200 + index as u32).collect());
    for index in 0..entries {
        let start = 12 + entries + index * entries;
        tables.insert(200 + index, values[start..start + entries].to_vec());
    }
    let mut scratch = vec![0xdeadbeef; entries].into_boxed_slice();
    (Ext2Runtime {
        block_size:entries*4, block_count:1024,
        block_cache:block_cache::BlockCache::new(entries*4, 32*entries*4, entries*4),
        block_shm: scratch.as_mut_ptr() as usize,
        scratch,
        tables,
        reads: Vec::new(),
        fail: None,
        cache_aware: false,
    }, inode)
}

fn old_run(r: &mut Ext2Runtime, inode: Ext2Inode, logical: usize, max: usize)
    -> Result<(u32, usize), RequestError>
{
    let first = get_data_block(r, &inode, logical)?;
    let mut count = 1;
    if first != 0 {
        while count < max {
            match get_data_block(r, &inode, logical + count) {
                Ok(next) if next != 0 && next as usize == first as usize + count => {
                    count += 1;
                }
                _ => break,
            }
        }
    }
    Ok((first, count))
}

#[test]
fn direct_single_double_boundaries_and_fragments_match_original_scan() {
    for fragmented in [false, true] {
        for logical in 0..35 {
            for maximum in [0, 1, 2, 4, 8, 40] {
                let (mut old, inode) = fixture(4, fragmented);
                let (mut new, _) = fixture(4, fragmented);
                let expected = old_run(&mut old, inode, logical, maximum);
                assert_eq!(get_data_run(&mut new, &inode, logical, maximum), expected,
                    "fragmented={fragmented} logical={logical} max={maximum}");
                let (mut reader, _) = fixture(4, fragmented);
                let result = read_run(&mut reader, &inode, logical, maximum);
                if let Ok((0, count)) = result {
                    assert_eq!(expected, Ok((0, 1)));
                    assert!(count >= 1 && count <= maximum.max(1));
                    for index in 0..count {
                        assert_eq!(get_data_block(&mut old, &inode, logical + index), Ok(0));
                    }
                } else {
                    assert_eq!(result, expected);
                }
            }
        }
    }
}

#[test]
fn single_indirect_table_is_loaded_once_for_256_blocks() {
    let (mut old, inode) = fixture(256, false);
    let (mut new, _) = fixture(256, false);
    assert_eq!(get_data_run(&mut new, &inode, 12, 256), old_run(&mut old, inode, 12, 256));
    assert_eq!(old.reads.len(), 256);
    assert_eq!(new.reads, [100]);
}

#[test]
fn double_indirect_table_is_loaded_twice_for_256_blocks() {
    let (mut old, inode) = fixture(256, false);
    let (mut new, _) = fixture(256, false);
    assert_eq!(get_data_run(&mut new, &inode, 268, 256), old_run(&mut old, inode, 268, 256));
    assert_eq!(old.reads.len(), 512);
    assert_eq!(new.reads, [101, 200]);
}

#[test]
fn sparse_parent_tables_never_reuse_previous_scratch() {
    for parent in [100, 101, 200] {
        for logical in 0..32 {
            let (mut old, mut inode) = fixture(4, false);
            let (mut new, _) = fixture(4, false);
            if parent == 100 {
                inode.block[12] = 0;
            } else if parent == 101 {
                inode.block[13] = 0;
            } else {
                old.tables.get_mut(&101).unwrap()[0] = 0;
                new.tables.get_mut(&101).unwrap()[0] = 0;
            }
            assert_eq!(get_data_run(&mut new, &inode, logical, 40),
                old_run(&mut old, inode, logical, 40));
        }
    }
}

#[test]
fn initial_errors_propagate_and_lookahead_errors_stop_at_valid_prefix() {
    for failing in [100, 101, 200, 201] {
        for logical in 0..32 {
            let (mut old, inode) = fixture(4, false);
            let (mut new, _) = fixture(4, false);
            old.fail = Some(failing);
            new.fail = Some(failing);
            assert_eq!(get_data_run(&mut new, &inode, logical, 40),
                old_run(&mut old, inode, logical, 40));
            let (mut reader, _) = fixture(4, false);
            reader.fail = Some(failing);
            let (mut scalar, _) = fixture(4, false);
            scalar.fail = Some(failing);
            assert_eq!(read_run(&mut reader, &inode, logical, 40),
                old_run(&mut scalar, inode, logical, 40));
        }
    }
}

#[test]
fn a_later_call_observes_modified_mapping_and_overwritten_scratch() {
    let (mut runtime, inode) = fixture(4, false);
    assert_eq!(get_data_run(&mut runtime, &inode, 12, 4), Ok((1012, 4)));
    runtime.scratch.fill(999);
    runtime.tables.get_mut(&100).unwrap()[1] = 9000;
    assert_eq!(get_data_run(&mut runtime, &inode, 12, 4), Ok((1012, 1)));
    assert_eq!(get_data_run(&mut runtime, &inode, 13, 4), Ok((9000, 1)));
}

#[test]
fn scalar_and_single_block_runs_need_no_scratch_but_multi_block_runs_load_the_leaf() {
    for logical in [12,15,16,19,31] {
        let (mut runtime,inode)=fixture(4,false);
        runtime.scratch.fill(0xdeadbeef);
        assert_eq!(get_data_block(&mut runtime,&inode,logical),Ok(1000+logical as u32));
        assert!(runtime.scratch.iter().all(|v|*v==0xdeadbeef));
        assert_eq!(get_data_run(&mut runtime, &inode,logical,1),Ok((1000+logical as u32,1)));
        assert!(runtime.scratch.iter().all(|v|*v==0xdeadbeef));
        assert_eq!(get_data_run(&mut runtime, &inode,logical,2),Ok((1000+logical as u32,2.min(32-logical))));
        assert!(runtime.scratch.iter().any(|v|*v!=0xdeadbeef));
    }
}

fn warm_tables(runtime: &mut Ext2Runtime) {
    runtime.block_cache = block_cache::BlockCache::new(runtime.block_size,
        (runtime.tables.len() + 1) * runtime.block_size, runtime.block_size);
    for (&block, words) in &runtime.tables {
        let bytes:Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        runtime.block_cache.store(block, &bytes, None);
    }
}

#[test]
fn warm_leaf_runs_match_point_oracle_and_skip_scratch_staging_inside_the_table() {
    for entries in [4, 16, 256] {
        for fragmented in [false, true] {
            for logical in [12, 13, 11 + entries, 12 + entries, 13 + entries,
                11 + 2 * entries, 12 + 2 * entries] {
                for maximum in [0, 1, 2, 4, 8, 64] {
                    let (mut scalar, inode) = fixture(entries, fragmented);
                    let (mut cached, _) = fixture(entries, fragmented);
                    warm_tables(&mut cached);
                    let expected = old_run(&mut scalar, inode, logical, maximum);
                    assert_eq!(get_data_run(&mut cached, &inode, logical, maximum), expected);
                    if let Ok((first, count)) = expected {
                        let available = entries - (logical - 12) % entries;
                        if maximum > 1 && (first == 0 || count < available || maximum <= available) {
                            assert!(cached.reads.is_empty());
                            assert!(cached.scratch.iter().all(|word| *word == 0xdeadbeef));
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn warm_leaf_and_double_parent_follow_current_dirty_cache_not_old_scratch_or_disk() {
    let (mut runtime, inode) = fixture(16, false);
    warm_tables(&mut runtime);
    let mut leaf:Vec<u8> = runtime.tables[&100].iter().flat_map(|word| word.to_le_bytes()).collect();
    leaf[4..8].copy_from_slice(&9000_u32.to_le_bytes());
    runtime.block_cache.store(100, &leaf, Some(block_cache::BlockKind::Metadata));
    assert_eq!(get_data_run(&mut runtime, &inode, 12, 8), Ok((1012, 1)));
    assert_eq!(get_data_run(&mut runtime, &inode, 13, 8), Ok((9000, 1)));
    let mut parent:Vec<u8> = runtime.tables[&101].iter().flat_map(|word| word.to_le_bytes()).collect();
    parent[..4].copy_from_slice(&201_u32.to_le_bytes());
    runtime.block_cache.store(101, &parent, Some(block_cache::BlockKind::Metadata));
    assert_eq!(get_data_run(&mut runtime, &inode, 28, 8), Ok((1044, 8)));
    parent[..4].fill(0);
    runtime.block_cache.store(101, &parent, Some(block_cache::BlockKind::Metadata));
    assert_eq!(get_data_run(&mut runtime, &inode, 28, 8), Ok((0, 1)));
    assert!(runtime.reads.is_empty());
    assert!(runtime.scratch.iter().all(|word| *word == 0xdeadbeef));
}

#[test]
fn warm_prefix_continues_from_current_dirty_leaf_and_preserves_lookahead_scratch() {
    fn prepare(case: usize) -> (Ext2Runtime, Ext2Inode, Vec<u32>) {
        let (mut r, inode) = fixture(4, false);
        r.cache_aware = true;
        let leaf = vec![1016u32,1017,9018,9019];
        r.tables.get_mut(&201).unwrap().copy_from_slice(&[9020,9021,9022,9023]);
        let mut parent = r.tables[&101].clone();
        if case == 4 { parent[1] = 0; }
        for (block, words) in [(101, parent), (200, leaf.clone())] {
            let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            r.block_cache.store(block, &bytes, Some(block_cache::BlockKind::Metadata));
        }
        r.fail = match case { 1 => Some(201), 2 => Some(200), 3 => Some(101), _ => None };
        if case == 5 { r.block_count = 201; }
        (r, inode, leaf)
    }
    for case in 0..6 {
        let (mut actual, inode, leaf) = prepare(case);
        let (mut scalar, _, _) = prepare(case);
        let expected = old_run(&mut scalar, inode, 18, 6);
        assert_eq!(get_data_run(&mut actual, &inode, 18, 6), expected);
        let stopped = matches!(case, 1 | 4 | 5);
        assert_eq!(expected, Ok((9018, if stopped { 2 } else { 6 })));
        assert_eq!(actual.reads, if matches!(case, 4 | 5) { vec![200,101] } else { vec![200,101,201] });
        assert_eq!(&*actual.scratch, if stopped { leaf.as_slice() } else { actual.tables[&201].as_slice() });
    }
}

#[test]
fn cached_probe_misses_boundaries_and_invalid_tables_keep_original_fallback_and_errors() {
    for warm in [false, true] {
        let (mut runtime, inode) = fixture(4, false);
        if warm { warm_tables(&mut runtime); }
        runtime.fail = Some(201);
        assert_eq!(get_data_run(&mut runtime, &inode, 16, 8), Ok((1016, 4)));
        assert_eq!(runtime.reads, if warm { vec![200, 101, 201] } else { vec![101, 200, 101, 201] });
    }
    let (mut runtime, inode) = fixture(4, false);
    let parent:Vec<u8> = runtime.tables[&101].iter().flat_map(|word| word.to_le_bytes()).collect();
    runtime.block_cache.store(101, &parent, None);
    assert_eq!(get_data_run(&mut runtime, &inode, 16, 3), Ok((1016, 3)));
    assert_eq!(runtime.reads, [200]); // Reuse the resolved warm parent for the cold leaf.
    let (mut runtime, inode) = fixture(4, false);
    warm_tables(&mut runtime);
    runtime.block_count = 100;
    assert_eq!(get_data_run(&mut runtime, &inode, 12, 3), Err(RequestError::InvalidArgument));
    assert_eq!(get_data_run(&mut runtime, &inode, 16, 3), Err(RequestError::InvalidArgument));
    assert!(runtime.reads.is_empty());
}

#[test]
fn resolved_cold_leaf_keeps_dirty_parent_choice_initial_errors_and_staging() {
    for case in 0..4 {
        let (mut r, inode) = fixture(4, false);
        r.cache_aware = true;
        let mut parent = r.tables[&101].clone();
        parent[0] = if case == 2 { 0 } else { 201 };
        let bytes: Vec<_> = parent.iter().flat_map(|v| v.to_le_bytes()).collect();
        r.block_cache.store(101, &bytes, Some(block_cache::BlockKind::Metadata));
        if case == 1 { r.fail = Some(201); }
        if case == 3 { r.block_count = 201; }
        let expected = match case {
            1 => Err(RequestError::Io),
            2 => Ok((0,1)),
            3 => Err(RequestError::InvalidArgument),
            _ => Ok((1020,3)),
        };
        assert_eq!(get_data_run(&mut r, &inode,16,3),expected);
        assert_eq!(r.reads, if case < 2 {vec![201]} else {vec![]});
        if case == 0 {assert_eq!(&*r.scratch,r.tables[&201].as_slice());}
        else {assert!(r.scratch.iter().all(|v|*v==0xdeadbeef));}
    }
    let (mut r,inode)=fixture(4,false);
    assert_eq!(get_data_run(&mut r, &inode,16,3),Ok((1016,3)));
    assert_eq!(r.reads,[101,200]); // An uncached parent retains the original path.
}
