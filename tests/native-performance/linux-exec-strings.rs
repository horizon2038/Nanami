//! Actual exec pointer decoder versus the former per-word checked-copy model.
#![allow(dead_code)]
extern crate self as nanami_runtime;
type Word = usize;
#[path = "../../nanami/servers/apps/alter/shared/src/common/nul_scan.rs"]
pub mod nul_scan;
mod common { pub use crate::nul_scan; }
mod state { pub const LINUX_FD_MAX: usize = 256; }
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/constants.rs"]
mod constants;
use constants::*;
const ALTER_LAUNCH_MAX_ARGS: usize = 64;
const ALTER_LAUNCH_MAX_ENVS: usize = 64;
struct Runtime { exec_snapshot_buffer: Word, exec_snapshot_buffer_size: Word }
fn map_request_error(e: i32) -> i32 { e }
pub fn request_heap(_: Word) -> Result<(Word, Word), i32> { panic!("preallocated fixture") }
#[derive(Clone,Default)]
struct Memory {
    pages: std::collections::BTreeMap<Word, Vec<u8>>, fail: Option<Word>,
    calls: Vec<(Word,Word)>,
}
impl Memory {
    fn fetch(&mut self, pid: Word, page: Word, buffer: &mut [u8]) -> Result<(), i32> {
        self.calls.push((pid,page));
        let Some(bytes) = self.pages.get(&page) else { return Err(EFAULT); };
        if self.fail == Some(page) {
            buffer[..19].copy_from_slice(&bytes[..19]);
            return Err(EFAULT);
        }
        buffer.copy_from_slice(bytes);Ok(())
    }
}
thread_local! { static MEMORY: std::cell::RefCell<Memory> = Default::default(); }
pub fn request_process_memory_read_checked(pid: Word, page: Word, destination: Word, bytes: Word) -> Result<(),i32> {
    assert_eq!(bytes,LINUX_PAGE_SIZE);
    MEMORY.with(|m| m.borrow_mut().fetch(pid,page,unsafe {core::slice::from_raw_parts_mut(destination as *mut u8,bytes)}))
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/exec_strings.rs"]
mod production;

struct Original { page: Word, valid: bool, pid: Word, buffer: Vec<u8>, memory: Memory }
impl Original {
    fn decode(&mut self, pid: Word, array: Word, out: &mut [Word]) -> Result<usize,i32> {
        if array == 0 { return Ok(0); }
        let mut count=0;
        while count<out.len() {
            let source=array.checked_add(count.checked_mul(8).ok_or(EFAULT)?).ok_or(EFAULT)?;
            let mut bytes=[0;8];let mut copied=0;
            while copied<8 {
                let address=source.checked_add(copied).ok_or(EFAULT)?;
                let page=address&!(LINUX_PAGE_SIZE-1);
                if !self.valid || self.page!=page || self.pid!=pid {
                    self.memory.fetch(pid,page,&mut self.buffer)?;
                    self.valid=true;self.page=page;self.pid=pid;
                }
                let offset=address-page;let n=(8-copied).min(LINUX_PAGE_SIZE-offset);
                bytes[copied..copied+n].copy_from_slice(&self.buffer[offset..offset+n]);copied+=n;
            }
            let value=Word::from_ne_bytes(bytes);
            if value==0 { return Ok(count); }
            out[count]=value;count+=1;
        }
        Ok(count)
    }
}

fn compare(array: Word, slots: usize, memory: Memory) {
    let mut original=Original {page:0,valid:false,pid:0,buffer:vec![0xa5;LINUX_PAGE_SIZE],memory:memory.clone()};
    let mut scratch=vec![0xa5;LINUX_PAGE_SIZE+32];
    let mut page=production::ExecGuestPage {buffer:scratch[16..].as_mut_ptr() as Word,pid:0,page:0,valid:false};
    let mut expected=vec![0xdead;slots];let mut actual=expected.clone();
    MEMORY.with(|m| *m.borrow_mut()=memory);
    let old=original.decode(7,array,&mut expected);
    let new=production::read_guest_pointer_array(&mut page,7,array,&mut actual);
    assert_eq!(new,old,"array={array:x} slots={slots}");assert_eq!(actual,expected);
    assert_eq!((page.pid,page.page,page.valid),(original.pid,original.page,original.valid));
    assert_eq!(&scratch[16..16+LINUX_PAGE_SIZE],original.buffer.as_slice());
    MEMORY.with(|m| assert_eq!(m.borrow().calls,original.memory.calls));
    assert!(scratch[..16].iter().chain(&scratch[16+LINUX_PAGE_SIZE..]).all(|b| *b==0xa5));
}

#[test]
fn page_batches_match_old_output_fault_prefix_cache_state_and_exact_rpc_order() {
    let base=0x10000;
    for offset in [0,1,7,8,4080,4087,4088,4089,4095] {
        for slots in [0,1,2,7,63,64,65,513] {
            for terminator in [Some(0),Some(1),Some(slots/2),Some(slots),None] {
                let mut memory=Memory::default();
                let mut data=vec![0x5a;4*LINUX_PAGE_SIZE];
                for i in 0..slots+1 {
                    let value=if terminator==Some(i) {0} else {0x12340000+i};
                    data[offset+i*8..offset+(i+1)*8].copy_from_slice(&value.to_ne_bytes());
                }
                for (i,bytes) in data.chunks(LINUX_PAGE_SIZE).enumerate() {memory.pages.insert(base+i*LINUX_PAGE_SIZE,bytes.to_vec());}
                for fail in [None,Some(base),Some(base+LINUX_PAGE_SIZE),Some(base+2*LINUX_PAGE_SIZE)] {
                    memory.fail=fail;compare(base+offset,slots,memory.clone());
                }
            }
        }
    }
}

#[test]
fn null_empty_missing_page_and_top_address_overflow_preserve_old_behavior() {
    compare(0,64,Memory::default());compare(0x10000,0,Memory::default());compare(0x10000,64,Memory::default());
    let top=usize::MAX&!(LINUX_PAGE_SIZE-1);
    let mut memory=Memory::default();memory.pages.insert(top,vec![0x5a;LINUX_PAGE_SIZE]);
    for offset in [4080,4087,4088,4089,4095] {compare(top+offset,64,memory.clone());}
}

#[test]
fn exec_string_search_keeps_nul_length_page_reads_and_initialized_output_prefix() {
    let base=0x10000;
    for offset in [0,1,7,4089,4095] {
        for len in [0,1,7,8,31,127,128,4095,4096] {
            let mut data=vec![0x81;3*LINUX_PAGE_SIZE];data[offset+len]=0;
            let mut memory=Memory::default();
            for (i,p) in data.chunks(LINUX_PAGE_SIZE).enumerate() {memory.pages.insert(base+i*LINUX_PAGE_SIZE,p.to_vec());}
            MEMORY.with(|m|*m.borrow_mut()=memory);
            let mut page_bytes=vec![0xa5u8;LINUX_PAGE_SIZE];
            let mut page=production::ExecGuestPage {buffer:page_bytes.as_mut_ptr() as Word,pid:0,page:0,valid:false};
            let mut output=vec![0xa5u8;LINUX_EXEC_SNAPSHOT_BYTES];
            let result=production::snapshot_guest_string(&mut page,7,output.as_mut_ptr() as Word,13,base+offset);
            assert_eq!(result,if len==4096 {Err(ENAMETOOLONG)} else {Ok((13,len))});
            assert!(output[..13].iter().chain(&output[13+len..]).all(|b|*b==0xa5));
            assert!(output[13..13+len].iter().all(|b|*b==0x81));
            let last=(offset+len.min(4095))/LINUX_PAGE_SIZE;
            MEMORY.with(|m|assert_eq!(m.borrow().calls,(0..=last).map(|i|(7,base+i*LINUX_PAGE_SIZE)).collect::<Vec<_>>()));
        }
    }
}
