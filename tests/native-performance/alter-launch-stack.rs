//! Actual initial-launch stack path against an independently zeroed serializer.
#![allow(dead_code)]
extern crate self as nanami_runtime;
pub use std::println;
type Word=usize;
pub const OS_RESPONSE_INVALID_ARGUMENT: Word=1;
pub const OS_RESPONSE_ILLEGAL_OPERATION: Word=5;
pub const OS_RESPONSE_FATAL: Word=8;
mod state { pub const LINUX_FD_MAX: usize=256; }
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/constants.rs"]
mod constants;
use constants::*;
const LINUX_INITIAL_STACK_BYTES: usize=0x4000;
const ALTER_LAUNCH_MAX_ARGS: usize=64;
const ALTER_LAUNCH_MAX_ENVS: usize=64;
const PLATFORM_NUL: &[u8]=b"x86_64\0";
const REG_PC: Word=0;const REG_RSP: Word=1;
fn align_down_usize(n:usize,a:usize)->usize {n&!(a-1)}
#[derive(Clone,Copy,PartialEq)]
enum OsPersonality {Linux,FreeBsd}
mod personality {
    pub fn name(p:super::OsPersonality)-> &'static str {
        if p==super::OsPersonality::Linux {"linux"} else {"freebsd"}
    }
}
mod common {pub mod dynamic {pub struct Interpreter {pub load_bias:usize,pub entry:usize}}}
#[derive(Clone,Copy)]
struct TargetElfMetadata {
    entry_point:Word,program_header_vaddr:Word,program_header_entry_size:Word,
    program_header_count:Word,has_interpreter:bool,
}
type ElfMetadata=TargetElfMetadata;
fn elf()->ElfMetadata {ElfMetadata {entry_point:0x401000,program_header_vaddr:0x400040,
    program_header_entry_size:56,program_header_count:9,has_interpreter:false}}
fn target_elf_metadata_from_loaded(e:&ElfMetadata)->TargetElfMetadata {*e}
fn read_target_elf_metadata(_: &mut Runtime,_:Word)->TargetElfMetadata {elf()}
struct Runtime {exec_stack_buffer:Word,exec_stack_buffer_size:Word,client_shm:Word}
impl Runtime {fn set_fs_base(&mut self,_:Word,_:Word)->bool {true}}
struct LaunchInfo {argc:usize,envc:usize,argv_offsets:[usize;64],argv_lens:[usize;64],env_offsets:[usize;64],env_lens:[usize;64]}
fn linux_argv0(_: &Runtime,i:&LaunchInfo)->Result<(usize,usize),Word> {Ok((i.argv_offsets[0],i.argv_lens[0]))}
fn install_freebsd_initial_tls(_: &mut Runtime,_:Word,_:&TargetElfMetadata)->Result<Word,i32> {Ok(123)}
fn map_request_error_to_status(_:i32)->Word {OS_RESPONSE_INVALID_ARGUMENT}
pub fn request_heap(_:Word)->Result<(Word,Word),i32> {panic!("preallocated fixture")}
pub fn request_mapping_release(_:Word,_:Word)->Result<(),i32> {Ok(())}
#[derive(Default)]
struct Backend {writes:Vec<(Word,Vec<u8>)>,registers:usize,copy_fail:bool,register_fail:bool}
thread_local! {static BACKEND:std::cell::RefCell<Backend>=Default::default();}
pub fn request_process_memory_write(_:Word,address:Word,source:Word,size:Word)->Result<(),i32> {
    BACKEND.with(|b| {let mut b=b.borrow_mut();b.writes.push((address,unsafe {core::slice::from_raw_parts(source as *const u8,size)}.to_vec()));
        if b.copy_fail {Err(1)} else {Ok(())}})
}
pub fn request_process_memory_read(_:Word,_:Word,_:Word,_:Word)->Result<(),i32> {panic!("diagnostics disabled")}
fn read_register_value(_:Word,_:Word)->Result<Word,i32> {panic!("diagnostics disabled")}
fn write_exec_registers(_:Word,_:Word,_:Word,_:Word,_:Word,_:Word,_:Word,_:Word)->Result<(),i32> {
    BACKEND.with(|b| {let mut b=b.borrow_mut();b.registers+=1;if b.register_fail {Err(1)} else {Ok(())}})
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/launch/stack.rs"]
mod production;

fn fixture(args:&[Vec<u8>],env:&[Vec<u8>],strings:&mut Vec<u8>)->LaunchInfo {
    let mut i=LaunchInfo {argc:args.len(),envc:env.len(),argv_offsets:[0;64],argv_lens:[0;64],env_offsets:[0;64],env_lens:[0;64]};
    for (data,offsets,lens) in [(args,&mut i.argv_offsets,&mut i.argv_lens),(env,&mut i.env_offsets,&mut i.env_lens)] {
        for (n,s) in data.iter().enumerate() {offsets[n]=strings.len();lens[n]=s.len();strings.extend_from_slice(s);}
    }
    i
}
fn reference(args:&[Vec<u8>],env:&[Vec<u8>],freebsd:bool,interp:bool)->Result<(Word,Vec<u8>),Word> {
    let mut bytes=vec![0;LINUX_INITIAL_STACK_BYTES];let base=LINUX_STACK_TOP-bytes.len();let mut cursor=bytes.len();
    let mut push=|s:&[u8],terminate:bool|->Result<Word,Word> {
        let start=cursor.checked_sub(s.len()+usize::from(terminate)).ok_or(OS_RESPONSE_INVALID_ARGUMENT)?;
        bytes[start..start+s.len()].copy_from_slice(s);cursor=start/8*8;Ok(base+start)
    };
    let envp:Vec<_>=env.iter().map(|s|push(s,true)).collect::<Result<_,_>>()?;
    let argv:Vec<_>=args.iter().map(|s|push(s,true)).collect::<Result<_,_>>()?;
    let execfn=argv.first().copied().unwrap_or(0);
    let random=push(&[0x91,0x2a,0x5d,0x7c,0x10,0x42,0xb3,0xc4,0x09,0x88,0x31,0x65,0xaa,0xfe,0x28,0x53],false)?;
    let platform=push(PLATFORM_NUL,false)?;
    let aux=[(AT_PHDR,0x400040),(AT_PHENT,56),(AT_PHNUM,9),(AT_PAGESZ,4096),
        (AT_BASE,if interp {0x700000} else {0}),(AT_FLAGS,0),(AT_ENTRY,0x401000),(AT_HWCAP,0),
        (AT_CLKTCK,100),(AT_UID,0),(AT_EUID,0),(AT_GID,0),(AT_EGID,0),(AT_SECURE,0),
        (AT_RANDOM,random),(AT_EXECFN,execfn),(AT_PLATFORM,platform),(AT_NULL,0)];
    let mut words=vec![args.len()];words.extend(argv);words.push(0);words.extend(envp);words.push(0);
    for (k,v) in aux {words.extend([k,v]);}
    let table=words.len()*8;let aligned=cursor/16*16;
    if aligned<table+16 {return Err(OS_RESPONSE_INVALID_ARGUMENT);}
    let sp=(aligned-table)/16*16-usize::from(freebsd)*8;
    for (n,w) in words.iter().enumerate() {bytes[sp+n*8..sp+n*8+8].copy_from_slice(&w.to_ne_bytes());}
    Ok((base+sp,bytes[sp..].to_vec()))
}

#[test]
fn initial_stack_matches_zeroed_model_with_dirty_reuse_alignment_limits_and_personalities() {
    let mut work=vec![0xa5;LINUX_INITIAL_STACK_BYTES+32];
    let mut runtime=Runtime {exec_stack_buffer:work[16..].as_mut_ptr() as Word,exec_stack_buffer_size:LINUX_INITIAL_STACK_BYTES,client_shm:0};
    let mut seed=91u64;
    for step in 0..512 {
        seed=seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let counts=[0,1,2,7,32,64];let argc=counts[(seed>>32) as usize%6];let envc=counts[(seed>>40) as usize%6];
        let size=[0,1,7,8,15,16,31,127,255,511][(seed>>48) as usize%10];
        let args:Vec<_>=(0..argc).map(|i|vec![b'a'+(i%26) as u8;size+i%8]).collect();
        let env:Vec<_>=(0..envc).map(|i|vec![b'A'+(i%26) as u8;size+i%16]).collect();
        let mut strings=vec![];let info=fixture(&args,&env,&mut strings);runtime.client_shm=strings.as_ptr() as Word;
        let freebsd=step&1!=0;let interp=step&2!=0;let personality=if freebsd {OsPersonality::FreeBsd} else {OsPersonality::Linux};
        let interpreter=common::dynamic::Interpreter {load_bias:0x700000,entry:0x701000};
        work.fill((step as u8).wrapping_add(1));BACKEND.with(|b|*b.borrow_mut()=Backend::default());
        let expected=reference(&args,&env,freebsd,interp);
        let actual=production::prepare_initial_stack(&mut runtime,7,19,&info,personality,false,Some(&elf()),if interp {Some(&interpreter)} else {None});
        match expected {
            Ok((sp,data))=>{
                assert_eq!(actual,Ok(sp));BACKEND.with(|b|{let b=b.borrow();assert_eq!(b.writes,[(sp,data)]);assert_eq!(b.registers,1);});
                let prefix=sp-(LINUX_STACK_TOP-LINUX_INITIAL_STACK_BYTES);
                assert!(work[16..16+prefix].iter().all(|b|*b==(step as u8).wrapping_add(1)));
            }
            Err(e)=>{assert_eq!(actual,Err(e));BACKEND.with(|b|assert!(b.borrow().writes.is_empty()));}
        }
        assert!(work[..16].iter().chain(&work[16+LINUX_INITIAL_STACK_BYTES..]).all(|b|*b==(step as u8).wrapping_add(1)));
    }
}

#[test]
fn initial_stack_preserves_copy_failure_register_failure_and_missing_interpreter_order() {
    let mut work=vec![0xa5;LINUX_INITIAL_STACK_BYTES];let mut strings=vec![];
    let info=fixture(&[b"hello".to_vec()],&[],&mut strings);
    let mut runtime=Runtime {exec_stack_buffer:work.as_mut_ptr() as Word,exec_stack_buffer_size:work.len(),client_shm:strings.as_ptr() as Word};
    for copy_fail in [true,false] {
        BACKEND.with(|b|*b.borrow_mut()=Backend {copy_fail,register_fail:!copy_fail,..Backend::default()});
        assert_eq!(production::prepare_initial_stack(&mut runtime,7,19,&info,OsPersonality::Linux,false,Some(&elf()),None),
            Err(if copy_fail {OS_RESPONSE_INVALID_ARGUMENT} else {OS_RESPONSE_FATAL}));
        BACKEND.with(|b|{let b=b.borrow();assert_eq!(b.writes.len(),1);assert_eq!(b.registers,usize::from(!copy_fail));});
    }
    let mut metadata=elf();metadata.has_interpreter=true;BACKEND.with(|b|*b.borrow_mut()=Backend::default());work.fill(0x77);
    assert_eq!(production::prepare_initial_stack(&mut runtime,7,19,&info,OsPersonality::Linux,false,Some(&metadata),None),Err(OS_RESPONSE_ILLEGAL_OPERATION));
    assert!(work.iter().all(|b|*b==0x77));BACKEND.with(|b|assert!(b.borrow().writes.is_empty()));
}
