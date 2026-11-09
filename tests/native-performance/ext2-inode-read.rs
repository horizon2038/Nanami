//! Production inode reader and block cache; disk transport/inode cache modeled.
#![allow(dead_code)]
extern crate alloc;
extern crate self as nanami_runtime;
extern crate self as nanami_client;
pub type Word = usize;
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
    inodes:Vec<(u32,Ext2Inode)>, reads:Vec<usize>, fail:bool,
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
    validate_run(r,b,1)?;r.reads.push(b);
    if r.fail {return Err(RequestError::Io);}
    r.scratch.copy_from_slice(&r.disk[b*r.block_size..(b+1)*r.block_size]);
    r.block_cache.store(b,&r.scratch,None);Ok(())
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
        block_cache:block_cache::BlockCache::new(block_size,8*block_size,block_size),scratch,disk:vec![0x5a;64*block_size],inodes:vec![],reads:vec![],fail:false};
    for n in 1..=32_u32 {
        let index=n as usize-1;let base=r.groups[index/16].inode_table_block as usize*block_size+(index%16)*inode_size;
        encode(&mut r.disk[base..base+100],expected(n));
    }
    r
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
