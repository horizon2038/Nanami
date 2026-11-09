//! Bounded owned-byte search and actual pathname fetch/error ordering.
#![allow(dead_code)]
type Word=usize;
const EFAULT:i32=14;const ENAMETOOLONG:i32=36;const LINUX_PAGE_SIZE:Word=4096;
#[path = "../../nanami/servers/apps/alter/shared/src/common/nul_scan.rs"]
pub mod nul_scan;
mod common {pub use crate::nul_scan;}
#[cfg(feature="wait-shape")]
mod wait_shape {pub fn path(_: &mut super::Runtime,_:usize,_:usize) {}}
struct Runtime {posix_shm:Word,posix_shm_size:Word}
#[derive(Clone)]
struct Memory {data:Vec<u8>,fail:Option<usize>,calls:Vec<(Word,Word,Word)>}
impl Memory {
    fn copy(&mut self,pid:Word,source:Word,out:&mut [u8])->Result<(),i32> {
        let index=self.calls.len();self.calls.push((pid,source,out.len()));
        let offset=source.checked_sub(0x10000).ok_or(EFAULT)?;
        let end=offset.checked_add(out.len()).filter(|end|*end<=self.data.len()).ok_or(EFAULT)?;
        if self.fail==Some(index) {
            let n=out.len().min(3);out[..n].copy_from_slice(&self.data[offset..offset+n]);return Err(EFAULT);
        }
        out.copy_from_slice(&self.data[offset..end]);Ok(())
    }
}
thread_local! {static MEMORY:std::cell::RefCell<Memory>=std::cell::RefCell::new(Memory {data:vec![],fail:None,calls:vec![]});}
mod guest_memory {
    pub fn copyin_into(_: &mut super::Runtime,pid:usize,source:usize,destination:usize,bytes:usize)->Result<(),i32> {
        super::MEMORY.with(|m|m.borrow_mut().copy(pid,source,unsafe {core::slice::from_raw_parts_mut(destination as *mut u8,bytes)}))
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/path_string.rs"]
mod production;

fn original(memory:&mut Memory,array:Word,scratch:&mut [u8])->Result<Word,i32> {
    if array==0 {return Err(EFAULT);}
    let max=scratch.len().min(4096);let mut copied=0;
    while copied<max {
        let source=array.checked_add(copied).ok_or(EFAULT)?;
        let remaining=4096-(source&4095);let chunk=(max-copied).min(remaining).min(128.max(copied));
        memory.copy(7,source,&mut scratch[copied..copied+chunk])?;
        let end=copied+chunk;
        while copied<end {if scratch[copied]==0 {return Ok(copied);}copied+=1;}
    }
    Err(ENAMETOOLONG)
}

#[test]
fn word_search_matches_byte_search_for_every_byte_pair_and_alignment_tail() {
    for pair in 0u32..=65535 {
        for offset in 0..core::mem::size_of::<usize>() {
            let mut bytes=[0xff;24];bytes[offset]=pair as u8;bytes[offset+1]=(pair>>8) as u8;
            assert_eq!(nul_scan::find_nul(&bytes),bytes.iter().position(|b|*b==0));
        }
    }
    for alignment in 0..8 {
        for len in 0..130 {
            let mut storage=vec![0;len+32];let start=8+alignment;
            storage[start..start+len].fill(0x81);
            assert_eq!(nul_scan::find_nul(&storage[start..start+len]),None);
            for zero in 0..len {
                storage[start+zero]=0;
                assert_eq!(nul_scan::find_nul(&storage[start..start+len]),Some(zero));
                storage[start+zero]=0x81;
            }
        }
        let mut storage=vec![0;4096+32];let start=8+alignment;storage[start..start+4096].fill(1);
        for zero in [0,7,8,31,127,128,4095] {
            storage[start+zero]=0;assert_eq!(nul_scan::find_nul(&storage[start..start+4096]),Some(zero));storage[start+zero]=1;
        }
    }
}

#[test]
fn actual_path_reader_keeps_copy_boundaries_fault_prefix_and_errno() {
    for capacity in [0,1,7,128,255,4096,8192] {
        for offset in [0,1,7,4080,4095] {
            for zero in [Some(0),Some(1),Some(7),Some(8),Some(127),Some(128),Some(129),Some(4095),None] {
                for fail in [None,Some(0),Some(1),Some(2)] {
                    let mut data=vec![0x81;4*4096];if let Some(zero)=zero {data[offset+zero]=0;}
                    let memory=Memory {data,fail,calls:vec![]};let mut old=memory.clone();let mut expected=vec![0xa5;capacity];
                    let mut scratch=vec![0xa5;capacity+32];let mut runtime=Runtime {posix_shm:scratch[16..].as_mut_ptr() as Word,posix_shm_size:capacity};
                    MEMORY.with(|m|*m.borrow_mut()=memory);
                    let a=original(&mut old,0x10000+offset,&mut expected);
                    let b=production::read_c_string(&mut runtime,7,0x10000+offset);
                    assert_eq!(a,b,"capacity={capacity} offset={offset} zero={zero:?} fail={fail:?}");
                    assert_eq!(&scratch[16..16+capacity],expected.as_slice());
                    assert!(scratch[..16].iter().chain(&scratch[16+capacity..]).all(|b|*b==0xa5));
                    MEMORY.with(|m|assert_eq!(m.borrow().calls,old.calls));
                }
            }
        }
    }
    let mut scratch=[0xa5;32];let mut runtime=Runtime {posix_shm:scratch.as_mut_ptr() as Word,posix_shm_size:scratch.len()};
    assert_eq!(production::read_c_string(&mut runtime,7,0),Err(EFAULT));
}
