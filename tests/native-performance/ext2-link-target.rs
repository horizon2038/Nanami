//! Production symlink reader/cache; inode and block transport are modeled.
#![allow(dead_code)]
extern crate alloc;
extern crate self as nanami_runtime;
extern crate self as nanami_client;
use std::{alloc::{GlobalAlloc, Layout, System}, cell::Cell};
pub type Word = usize;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError { InvalidArgument, Protocol, Io }
pub mod block {
    use super::*;
    pub fn block_device_flush(_:Word)->Result<(),RequestError> {panic!("readlink must not flush");}
    pub fn block_device_write(_:Word,_:Word,_:Word,_:Word)->Result<Word,RequestError> {panic!("readlink must not write");}
}
thread_local! {
    static COUNTS:Cell<Option<(usize,usize)>> = const {Cell::new(None)};
    static FAIL:Cell<bool> = const {Cell::new(false)};
}
struct Counted;
unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout:Layout)->*mut u8 {
        if FAIL.try_with(Cell::get).unwrap_or(false) {return std::ptr::null_mut();}
        let result=System.alloc(layout);
        if !result.is_null() {let _=COUNTS.try_with(|c|if let Some((a,r))=c.get(){c.set(Some((a+1,r)));});}
        result
    }
    unsafe fn realloc(&self,p:*mut u8,layout:Layout,bytes:usize)->*mut u8 {
        if FAIL.try_with(Cell::get).unwrap_or(false) {return std::ptr::null_mut();}
        let result=System.realloc(p,layout,bytes);
        if !result.is_null() {let _=COUNTS.try_with(|c|if let Some((a,r))=c.get(){c.set(Some((a,r+1)));});}
        result
    }
    unsafe fn dealloc(&self,p:*mut u8,layout:Layout) {System.dealloc(p,layout);}
}
#[global_allocator]
static ALLOCATOR:Counted=Counted;
const EXT2_S_IFMT:u16=0xf000;
const EXT2_S_IFLNK:u16=0xa000;
const EXT2_INODE_BLOCK_POINTERS:usize=15;
const BLOCK_SIZE:usize=1024;
#[path = "../../nanami/servers/core-services/ext2-server/src/path_walk.rs"]
mod path_walk;
#[path = "../../nanami/servers/core-services/ext2-server/src/block_cache.rs"]
mod block_cache;
#[path = "../../nanami/servers/core-services/ext2-server/src/link_target.rs"]
mod production;
#[derive(Clone,Copy)]
struct Ext2Inode {mode:u16,size:u32,sectors:u32,block:[u32;15]}
struct Ext2Runtime {
    inode:Ext2Inode, block_size:usize, block_count:usize, block_shm_size:usize,
    block_shm:usize, block_cache:block_cache::BlockCache, scratch:Box<[u8]>,
    disk:Vec<u8>, mapping:Vec<u32>, reads:usize, fail_inode:bool,
    fail_map:Option<usize>, fail_read:Option<usize>,
}
fn read_inode(r:&mut Ext2Runtime,_:u32)->Result<Ext2Inode,RequestError> {
    if r.fail_inode {Err(RequestError::Io)} else {Ok(r.inode)}
}
fn get_data_block(r:&mut Ext2Runtime,_:&Ext2Inode,logical:usize)->Result<u32,RequestError> {
    if r.fail_map==Some(logical) {return Err(RequestError::Io);}
    r.mapping.get(logical).copied().ok_or(RequestError::Io)
}
fn validate_run(r:&Ext2Runtime,block:usize,count:usize)->Result<(),RequestError> {
    if count==0 || block.checked_add(count).is_none_or(|end|end>r.block_count)
        || count.checked_mul(r.block_size).is_none_or(|bytes|bytes>r.block_shm_size) {
        return Err(RequestError::InvalidArgument);
    } Ok(())
}
fn read_block(r:&mut Ext2Runtime,block:usize)->Result<(),RequestError> {
    validate_run(r,block,1)?;
    if r.block_cache.load(block,&mut r.scratch) {return Ok(());}
    r.reads+=1;
    if r.fail_read==Some(block) {return Err(RequestError::Io);}
    r.scratch.copy_from_slice(&r.disk[block*BLOCK_SIZE..(block+1)*BLOCK_SIZE]);
    r.block_cache.store(block,&r.scratch,None);Ok(())
}
fn fixture(bytes:usize,inline:bool)->Ext2Runtime {
    let mut scratch=vec![0xcc;BLOCK_SIZE].into_boxed_slice();
    Ext2Runtime {inode:Ext2Inode{mode:EXT2_S_IFLNK,size:bytes as u32,sectors:usize::from(!inline) as u32,
        block:std::array::from_fn(|i|u32::from_le_bytes(std::array::from_fn(|j|(i*4+j) as u8)))},
        block_size:BLOCK_SIZE,block_count:8,block_shm_size:BLOCK_SIZE,
        block_shm:scratch.as_mut_ptr() as usize,block_cache:block_cache::BlockCache::new(BLOCK_SIZE,8*BLOCK_SIZE,BLOCK_SIZE),
        scratch,disk:(0..8*BLOCK_SIZE).map(|i|((i*7+3)%251) as u8).collect(),mapping:vec![2,3,4,5],reads:0,
        fail_inode:false,fail_map:None,fail_read:None}
}

#[test]
fn all_inline_lengths_copy_exact_bytes_without_any_reallocation_or_padding_growth() {
    for bytes in 0..=60 {
        let mut r=fixture(bytes,true);
        COUNTS.with(|c|c.set(Some((0,0))));
        let result=production::read_link(&mut r,7);
        let counts=COUNTS.with(Cell::take).unwrap();
        assert_eq!(result,Ok((0..bytes).map(|i|i as u8).collect()));
        assert_eq!(counts,(usize::from(bytes!=0),0),"length={bytes}");
        assert_eq!(r.reads,0);
        assert!(r.scratch.iter().all(|b|*b==0xcc));
    }
}

#[test]
fn cold_then_warm_block_targets_match_exact_partial_bytes_without_warm_scratch_staging() {
    for bytes in [1,60,61,1023,1024,1025,4096] {
        let mut r=fixture(bytes,false);
        let expected=r.disk[2*BLOCK_SIZE..2*BLOCK_SIZE+bytes].to_vec();
        assert_eq!(production::read_link(&mut r,7),Ok(expected.clone()));
        let reads=r.reads;assert_eq!(reads,bytes.div_ceil(BLOCK_SIZE));
        r.scratch.fill(0xcc);
        assert_eq!(production::read_link(&mut r,7),Ok(expected));
        assert_eq!(r.reads,reads);
        assert!(r.scratch.iter().all(|b|*b==0xcc));
    }
}

#[test]
fn warm_targets_use_current_dirty_payload_and_keep_bounds_and_holes_checked() {
    let mut r=fixture(1025,false);
    r.block_cache.store(2,&vec![0x31;BLOCK_SIZE],Some(block_cache::BlockKind::Data));
    r.block_cache.store(3,&vec![0x32;BLOCK_SIZE],Some(block_cache::BlockKind::Data));
    let mut expected=vec![0x31;1024];expected.push(0x32);
    assert_eq!(production::read_link(&mut r,7),Ok(expected));
    assert_eq!(r.reads,0);assert!(r.scratch.iter().all(|b|*b==0xcc));
    r.block_count=2;
    assert_eq!(production::read_link(&mut r,7),Err(path_walk::Error::Io));
    r.block_count=8;r.block_shm_size=0;
    assert_eq!(production::read_link(&mut r,7),Err(path_walk::Error::Io));
    r.block_shm_size=BLOCK_SIZE;r.mapping[1]=0;
    assert_eq!(production::read_link(&mut r,7),Err(path_walk::Error::Io));
    r.mapping[1]=3;r.fail_map=Some(1);
    assert_eq!(production::read_link(&mut r,7),Err(path_walk::Error::Io));
}

#[test]
fn inode_type_size_allocation_and_payload_errors_keep_the_existing_precedence() {
    let mut r=fixture(1,true);r.fail_inode=true;
    assert_eq!(production::read_link(&mut r,7),Err(path_walk::Error::Io));
    r.fail_inode=false;r.inode.mode=0x8000;
    assert_eq!(production::read_link(&mut r,7),Err(path_walk::Error::Invalid));
    r.inode.mode=EXT2_S_IFLNK;r.inode.size=4097;
    assert_eq!(production::read_link(&mut r,7),Err(path_walk::Error::TooLong));
    r.inode.size=61;
    assert_eq!(production::read_link(&mut r,7),Err(path_walk::Error::Io));
    FAIL.with(|f|f.set(true));
    let result=production::read_link(&mut r,7);
    FAIL.with(|f|f.set(false));
    assert_eq!(result,Err(path_walk::Error::NoSpace));
    let mut r=fixture(1025,false);r.fail_read=Some(3);
    assert_eq!(production::read_link(&mut r,7),Err(path_walk::Error::Io));
    assert_eq!(r.reads,2);
}
