//! Production brk with allocator/release effects visible to an independent model.
#![allow(dead_code)]
use std::collections::VecDeque;
type Word=usize;
const ESRCH:i32=3;
const LINUX_PROT_READ:Word=1;
const LINUX_PROT_WRITE:Word=2;
#[derive(Clone,Copy,Debug,PartialEq)]
struct Mapping {base:Word,size:Word,prot:Word}
struct Process {program_break:Word,mapped_break:Word,mappings:Vec<Mapping>}
struct Runtime {
    process:Process,exists:bool,syncs:usize,
    results:VecDeque<Result<(Word,Word),i32>>,
    allocations:Vec<(Word,Word)>,fixed:Vec<Word>,
}
impl Runtime {
    fn new()->Self {
        Self {process:Process {program_break:0x100025,mapped_break:0x101000,
            mappings:vec![Mapping {base:0x100000,size:4096,prot:3}]},exists:true,syncs:0,
            results:VecDeque::new(),allocations:Vec::new(),fixed:Vec::new()}
    }
    fn managed_process(&self,_:Word)->Option<&Process> {self.exists.then_some(&self.process)}
    fn managed_process_mut(&mut self,_:Word)->Option<&mut Process> {self.exists.then_some(&mut self.process)}
    fn sync_program_break(&mut self,_:Word) {self.syncs+=1;}
}
fn align_up_word(n:Word,a:Word)->Word {(n+a-1)&!(a-1)}
fn map_anonymous_tracked(r:&mut Runtime,_:Word,len:Word,prot:Word)->Result<(Word,Word),i32> {
    r.allocations.push((len,prot));
    let answer=r.results.pop_front().expect("unexpected allocation")?;
    r.process.mappings.push(Mapping {base:answer.0,size:answer.1,prot});Ok(answer)
}
fn map_anonymous_at_tracked(r:&mut Runtime,pid:Word,base:Word,len:Word,prot:Word)->Result<(Word,Word),i32> {
    r.fixed.push(base);let answer=map_anonymous_tracked(r,pid,len,prot)?;assert_eq!(answer.0,base);Ok(answer)
}
#[path="../../nanami/servers/apps/alter/shared/src/personality/linux/program_break.rs"]
mod production;

#[test]
fn occupied_growth_does_not_allocate_zero_protect_or_publish_a_region() {
    for (base,size) in [(0x101000,4096),(0x102000,1),(0x100fff,2),(0x103fff,1)] {
        for prot in 0..8 {
            let mut r=Runtime::new();
            r.process.mappings.insert(0,Mapping {base,size,prot});
            let before=r.process.mappings.clone();
            assert_eq!(production::sys_brk(&mut r,1,0x104000),Ok(0x100025));
            assert!(r.allocations.is_empty());assert!(r.fixed.is_empty());assert_eq!(r.syncs,0);
            assert_eq!(r.process.mappings,before);assert_eq!(r.process.mapped_break,0x101000);
        }
    }
}

#[test]
fn adjacent_and_empty_entries_allow_contiguous_growth() {
    let mut r=Runtime::new();
    r.process.mappings.insert(0,Mapping {base:0x104000,size:4096,prot:0});
    r.process.mappings.push(Mapping {base:0x102000,size:0,prot:0});
    r.process.mappings.push(Mapping {base:0,size:usize::MAX,prot:0});
    r.results.push_back(Ok((0x101000,3*4096)));
    assert_eq!(production::sys_brk(&mut r,1,0x103123),Ok(0x103123));
    assert_eq!(r.allocations,[(3*4096,3)]);assert_eq!(r.fixed,[0x101000]);
    assert_eq!(r.process.mapped_break,0x104000);assert_eq!(r.process.program_break,0x103123);
    assert_eq!(r.syncs,1);
}

#[test]
fn free_gap_grows_in_place_instead_of_speculating_at_a_later_cursor() {
    let mut r=Runtime::new();
    r.results.push_back(Ok((0x101000,3*4096)));
    assert_eq!(production::sys_brk(&mut r,1,0x104000),Ok(0x104000));
    assert_eq!(r.fixed,[0x101000]);
    assert_eq!(r.process.mapped_break,0x104000);assert_eq!(r.process.program_break,0x104000);
    assert_eq!(r.syncs,1);
}

#[test]
fn rejected_fixed_allocation_keeps_old_break_and_existing_mapping_metadata() {
    let mut r=Runtime::new();let before=r.process.mappings.clone();r.results.push_back(Err(12));
    assert_eq!(production::sys_brk(&mut r,1,0x104000),Err(12));
    assert_eq!(r.fixed,[0x101000]);assert_eq!(r.process.mappings,before);
    assert_eq!(r.process.mapped_break,0x101000);assert_eq!(r.syncs,0);
}

#[test]
fn initialization_query_shrink_and_allocation_failure_keep_existing_behavior() {
    let mut r=Runtime::new();r.process.program_break=0;r.process.mapped_break=0;r.process.mappings.clear();
    r.results.push_back(Ok((0x500000,4096)));
    assert_eq!(production::sys_brk(&mut r,1,0),Ok(0x500000));
    assert_eq!(r.process.mapped_break,0x501000);assert_eq!(r.syncs,1);
    assert_eq!(production::sys_brk(&mut r,1,0x500080),Ok(0x500080));
    assert_eq!(production::sys_brk(&mut r,1,0x500010),Ok(0x500080));
    assert_eq!(r.process.program_break,0x500080);assert_eq!(r.process.mapped_break,0x501000);
    r.exists=false;assert_eq!(production::sys_brk(&mut r,1,0),Err(ESRCH));
}
