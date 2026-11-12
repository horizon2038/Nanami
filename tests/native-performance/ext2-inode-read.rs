//! Production inode reader and block cache; disk transport/inode cache modeled.
#![allow(dead_code)]
extern crate alloc;
extern crate self as nanami_runtime;
extern crate self as nanami_client;
pub type Word = usize;
pub use std::println;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError { InvalidArgument, Protocol, Io }
pub mod block {
    use super::*;
    pub fn block_device_flush(_:Word)->Result<(),RequestError> {panic!("inode read must not flush");}
    pub fn block_device_write(_:Word,_:Word,_:Word,_:Word)->Result<Word,RequestError> {panic!("inode read must not write");}
}
const EXT2_INODE_BLOCK_POINTERS: usize = 15;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ext2Inode { mode:u16, size:u32, links_count:u16, sectors:u32, block:[u32;15] }
struct Superblock { inodes_count:u32, inodes_per_group:u32, inode_size:usize }
struct Group { inode_table_block:u32 }
struct Ext2Runtime {
    superblock:Superblock, groups:Vec<Group>, group_count:usize,
    block_size:usize, block_count:usize, block_shm:usize, block_shm_size:usize,
    block_cache:block_cache::BlockCache, scratch:Box<[u8]>, disk:Vec<u8>,
    inodes:Vec<(u32,Ext2Inode)>, reads:Vec<usize>, commands:Vec<(usize,usize)>, fail:bool, fail_bulk:bool,
    fail_after_block:Option<usize>,
}
#[path = "../../nanami/servers/core-services/ext2-server/src/block_cache.rs"]
mod block_cache;
#[path = "../../nanami/servers/core-services/ext2-server/src/inode_read.rs"]
mod production;
fn load_cached_inode(r:&Ext2Runtime,n:u32)->Option<Ext2Inode> {r.inodes.iter().find(|(id,_)|*id==n).map(|(_,i)|*i)}
fn store_cached_inode(r:&mut Ext2Runtime,n:u32,i:Ext2Inode) {r.inodes.push((n,i));}
fn validate_run(r:&Ext2Runtime,b:usize,n:usize)->Result<(),RequestError> {
    if n==0 || b.checked_add(n).is_none_or(|end|end>r.block_count)
        || n.checked_mul(r.block_size).is_none_or(|bytes|bytes>r.block_shm_size) {return Err(RequestError::InvalidArgument);}
    Ok(())
}
fn read_block(r:&mut Ext2Runtime,b:usize)->Result<(),RequestError> {
    read_blocks(r,b,1)
}
fn read_blocks(r:&mut Ext2Runtime,b:usize,n:usize)->Result<(),RequestError> {
    validate_run(r,b,n)?;
    r.commands.push((b,n));
    if r.fail || (r.fail_bulk && n>1) {return Err(RequestError::Io);}
    for i in 0..n {
        if r.fail_after_block==Some(b+i) {return Err(RequestError::Io);}
        let out=&mut r.scratch[i*r.block_size..(i+1)*r.block_size];
        if r.block_cache.load(b+i,out) {continue;}
        r.reads.push(b+i);
        out.copy_from_slice(&r.disk[(b+i)*r.block_size..(b+i+1)*r.block_size]);
        r.block_cache.store(b+i,out,None);
    }
    Ok(())
}
fn expected(n:u32)->Ext2Inode {
    Ext2Inode {mode:[0x81a4,0x41ed,0xa1ff,0xc1a4][n as usize%4],size:n*12345,
        links_count:n as u16+2,sectors:n*987,block:std::array::from_fn(|i|0xf000_0000+n*32+i as u32)}
}
fn encode(out:&mut[u8],inode:Ext2Inode) {
    out[..2].copy_from_slice(&inode.mode.to_le_bytes());out[4..8].copy_from_slice(&inode.size.to_le_bytes());
    out[26..28].copy_from_slice(&inode.links_count.to_le_bytes());out[28..32].copy_from_slice(&inode.sectors.to_le_bytes());
    for (i,b) in inode.block.iter().enumerate() {out[40+i*4..44+i*4].copy_from_slice(&b.to_le_bytes());}
}
fn fixture(block_size:usize,inode_size:usize)->Ext2Runtime {
    let mut scratch=vec![0xcc;block_size].into_boxed_slice();
    let mut r=Ext2Runtime {superblock:Superblock{inodes_count:32,inodes_per_group:16,inode_size},groups:vec![Group{inode_table_block:4},Group{inode_table_block:32}],group_count:2,
        block_size,block_count:64,block_shm:scratch.as_mut_ptr() as usize,block_shm_size:block_size,
        block_cache:block_cache::BlockCache::new(block_size,8*block_size,block_size),scratch,disk:vec![0x5a;64*block_size],inodes:vec![],reads:vec![],commands:vec![],fail:false,fail_bulk:false,fail_after_block:None};
    for n in 1..=32_u32 {
        let index=n as usize-1;let base=r.groups[index/16].inode_table_block as usize*block_size+(index%16)*inode_size;
        encode(&mut r.disk[base..base+100],expected(n));
    }
    r
}

fn large_table(block_size:usize,inode_size:usize,inodes:u32,window:usize,cache_blocks:usize)->Ext2Runtime {
    let mut r=fixture(block_size,inode_size);
    r.superblock.inodes_count=inodes;r.superblock.inodes_per_group=inodes;
    r.group_count=1;r.groups.truncate(1);
    r.disk.resize((4+(inodes as usize*inode_size).div_ceil(block_size)+16)*block_size,0x5a);
    r.block_count=r.disk.len()/block_size;
    for n in 1..=inodes {
        let base=4*block_size+(n as usize-1)*inode_size;
        encode(&mut r.disk[base..base+100],expected(n));
    }
    r.scratch=vec![0xcc;window*block_size].into_boxed_slice();
    r.block_shm=r.scratch.as_mut_ptr() as usize;r.block_shm_size=r.scratch.len();
    r.block_cache=block_cache::BlockCache::new(block_size,cache_blocks*block_size,window*block_size);
    r
}

fn prime_previous(r:&mut Ext2Runtime,block:usize) {
    r.block_cache.store(block,&r.disk[block*r.block_size..(block+1)*r.block_size],None);
}

#[test]
fn adjacent_inode_records_share_one_bounded_table_read() {
    let mut r=large_table(1024,128,128,16,64);
    for n in 1..=128 {assert_eq!(production::read_inode(&mut r,n),Ok(expected(n)));r.inodes.clear();}
    assert_eq!(r.commands,[(4,1),(5,15)]);
    assert_eq!(r.reads,(4..20).collect::<Vec<_>>());
    println!("INODE-TABLE-READ-AHEAD inodes=128 blocks=16 device-ranges-before=16 after=2");
}

#[test]
fn isolated_cold_inode_lookups_read_only_the_requested_blocks() {
    let mut r=large_table(1024,128,128,16,64);
    for n in [1,33,65,97] {assert_eq!(production::read_inode(&mut r,n),Ok(expected(n)));r.inodes.clear();}
    assert_eq!(r.commands,[(4,1),(8,1),(12,1),(16,1)]);
    assert_eq!(r.reads,[4,8,12,16]);
}

#[test]
fn read_ahead_stops_at_partial_last_table_and_never_enters_another_group() {
    let mut r=large_table(1024,128,19,16,64);
    for n in 1..=19 {assert_eq!(production::read_inode(&mut r,n),Ok(expected(n)));r.inodes.clear();}
    assert_eq!(r.commands,[(4,1),(5,2)]);
    let mut r=fixture(1024,128);
    r.scratch=vec![0xcc;16*1024].into_boxed_slice();r.block_shm=r.scratch.as_mut_ptr() as usize;r.block_shm_size=r.scratch.len();
    r.block_cache=block_cache::BlockCache::new(1024,64*1024,16*1024);
    for n in [1,9,17,25] {assert_eq!(production::read_inode(&mut r,n),Ok(expected(n)));}
    assert_eq!(r.commands,[(4,1),(5,1),(32,1),(33,1)]);
}

#[test]
fn requested_block_succeeds_when_only_bulk_read_fails() {
    let mut r=large_table(1024,128,128,16,64);prime_previous(&mut r,4);r.fail_bulk=true;
    assert_eq!(production::read_inode(&mut r,9),Ok(expected(9)));
    assert_eq!(r.commands,[(5,15),(5,1)]);assert_eq!(r.reads,[5]);
    r.inodes.clear();r.scratch.fill(0xcc);
    assert_eq!(production::read_inode(&mut r,10),Ok(expected(10)));
    assert_eq!(r.commands.len(),2);assert!(r.scratch.iter().all(|b|*b==0xcc));
}

#[test]
fn requested_read_failure_still_publishes_no_inode_or_partial_scratch() {
    let mut r=large_table(1024,128,128,16,64);prime_previous(&mut r,4);r.fail=true;
    assert_eq!(production::read_inode(&mut r,9),Err(RequestError::Io));
    assert_eq!(r.commands,[(5,15),(5,1)]);assert!(r.inodes.is_empty());
    assert!(r.scratch.iter().all(|b|*b==0xcc));
}

#[test]
fn failed_suffix_reuses_completed_prefix_and_keeps_dirty_neighbor_current() {
    let mut r=large_table(1024,128,128,16,64);prime_previous(&mut r,4);
    let mut bytes=r.disk[6*1024..7*1024].to_vec();let mut dirty=expected(17);dirty.links_count=47;
    encode(&mut bytes[..100],dirty);r.block_cache.store(6,&bytes,Some(block_cache::BlockKind::Metadata));
    r.fail_after_block=Some(7);
    assert_eq!(production::read_inode(&mut r,9),Ok(expected(9)));
    assert_eq!(r.commands,[(5,15),(5,1)]);assert_eq!(r.reads,[5]);
    assert_eq!(production::read_inode(&mut r,17),Ok(dirty));assert!(r.block_cache.is_dirty());
    assert_eq!(&r.scratch[..1024],&r.disk[5*1024..6*1024]);
}

#[test]
fn prefetched_neighbors_keep_current_dirty_records_and_future_updates() {
    let mut r=large_table(1024,128,128,16,64);prime_previous(&mut r,4);
    let mut bytes=r.disk[6*1024..7*1024].to_vec();let mut changed=expected(17);changed.size=0xabcd1234;
    encode(&mut bytes[..100],changed);r.block_cache.store(6,&bytes,Some(block_cache::BlockKind::Metadata));
    assert_eq!(production::read_inode(&mut r,9),Ok(expected(9)));
    assert_eq!(production::read_inode(&mut r,17),Ok(changed));assert!(r.block_cache.is_dirty());
    assert!(!r.reads.contains(&6));
    changed.links_count=113;encode(&mut bytes[..100],changed);r.block_cache.store(6,&bytes,Some(block_cache::BlockKind::Metadata));
    r.inodes.clear();r.fail=true;
    assert_eq!(production::read_inode(&mut r,17),Ok(changed));assert_eq!(r.commands,[(5,15)]);
}

#[test]
fn look_ahead_is_bounded_by_window_cache_filesystem_and_current_table_position() {
    for (window,cache,expected_count) in [(1,64,1),(2,64,2),(16,16,4),(16,64,15)] {
        let mut r=large_table(1024,128,128,window,cache);prime_previous(&mut r,4);
        assert_eq!(production::read_inode(&mut r,9),Ok(expected(9)));
        assert_eq!(r.commands,[(5,expected_count)]);
    }
    let mut r=large_table(1024,128,128,16,64);prime_previous(&mut r,4);r.block_count=7;
    assert_eq!(production::read_inode(&mut r,9),Ok(expected(9)));assert_eq!(r.commands,[(5,2)]);
    let mut r=large_table(1024,128,128,16,64);
    assert_eq!(production::read_inode(&mut r,121),Ok(expected(121)));assert_eq!(r.commands,[(19,1)]);
}

#[test]
fn unknown_total_table_geometry_keeps_valid_single_record_path() {
    let mut r=large_table(1024,128,128,16,64);r.superblock.inode_size=usize::MAX;
    assert_eq!(production::read_inode(&mut r,1),Ok(expected(1)));assert_eq!(r.commands,[(4,1)]);
}

#[test]
fn cold_and_warm_records_match_all_fields_across_blocks_groups_and_inode_sizes() {
    for block in [1024,2048,4096] {for stride in [128,256,512] {
        let mut r=fixture(block,stride);
        for n in 1..=32 {
            assert_eq!(production::read_inode(&mut r,n),Ok(expected(n)));
            r.inodes.clear();r.scratch.fill(0xcc);let reads=r.reads.len();
            assert_eq!(production::read_inode(&mut r,n),Ok(expected(n)));
            assert_eq!(r.reads.len(),reads);assert!(r.scratch.iter().all(|b|*b==0xcc));
            r.inodes.clear();
        }
    }}
}

#[test]
fn dirty_cached_inode_fields_are_current_after_decoded_cache_invalidation() {
    let mut r=fixture(1024,128);let mut bytes=r.disk[4*1024..5*1024].to_vec();
    let mut inode=expected(3);inode.size=0xfedcba98;inode.block[14]=0x12345678;
    encode(&mut bytes[256..356],inode);r.block_cache.store(4,&bytes,Some(block_cache::BlockKind::Metadata));r.fail=true;
    assert_eq!(production::read_inode(&mut r,3),Ok(inode));assert!(r.reads.is_empty());
    assert!(r.scratch.iter().all(|b|*b==0xcc));assert!(r.block_cache.is_dirty());
    inode.links_count=19;inode.sectors=0x98765432;encode(&mut bytes[256..356],inode);
    r.block_cache.store(4,&bytes,Some(block_cache::BlockKind::Metadata));r.inodes.clear();
    assert_eq!(production::read_inode(&mut r,3),Ok(inode));assert!(r.reads.is_empty());
}

#[test]
fn failed_reads_publish_no_decoded_entry_and_existing_decoded_hits_need_no_io() {
    let mut r=fixture(1024,128);r.fail=true;
    assert_eq!(production::read_inode(&mut r,1),Err(RequestError::Io));assert!(r.inodes.is_empty());
    assert!(r.scratch.iter().all(|b|*b==0xcc));r.fail=false;
    assert_eq!(production::read_inode(&mut r,1),Ok(expected(1)));let reads=r.reads.len();
    r.block_cache=block_cache::BlockCache::new(1024,8192,1024);r.fail=true;
    assert_eq!(production::read_inode(&mut r,1),Ok(expected(1)));assert_eq!(r.reads.len(),reads);
}

#[test]
fn malformed_table_ranges_and_invalid_inode_numbers_fail_before_copy_or_publication() {
    for case in 0..8 {
        let mut r=fixture(1024,128);let mut n=1;
        match case {0=>n=0,1=>n=33,2=>r.superblock.inodes_per_group=0,3=>r.group_count=0,
            4=>{r.superblock.inode_size=1000;n=2;},5=>{r.superblock.inode_size=usize::MAX;n=3;},
            6=>{r.groups[0].inode_table_block=64;r.block_cache.store(64,&vec![0;1024],None);},
            _=>{r.block_cache.store(4,&vec![0;1024],None);r.block_shm_size=1023;}}
        assert_eq!(production::read_inode(&mut r,n),Err(RequestError::InvalidArgument));
        assert!(r.reads.is_empty());assert!(r.inodes.is_empty());assert!(r.scratch.iter().all(|b|*b==0xcc));
    }
}
