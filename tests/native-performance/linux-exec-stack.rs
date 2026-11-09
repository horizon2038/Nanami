//! Production exec stack serializer against an independently zeroed byte model.
#![allow(dead_code)]
extern crate self as nanami_runtime;
type Word = usize;
mod state { pub const LINUX_FD_MAX: usize = 256; }
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/constants.rs"]
mod constants;
use constants::*;
const ALTER_LAUNCH_MAX_ARGS: usize = 64;
const ALTER_LAUNCH_MAX_ENVS: usize = 64;
const PLATFORM: &[u8] = b"x86_64";
fn align_down_usize(n: usize, align: usize) -> usize { n & !(align - 1) }
struct ExecStringSnapshot {
    buffer: Word, used: usize, argc: usize, envc: usize,
    argv_offsets: [usize; 64], argv_lens: [usize; 64],
    env_offsets: [usize; 64], env_lens: [usize; 64],
}
#[derive(Clone, Copy, PartialEq)]
enum OsPersonality { Linux, FreeBsd }
#[derive(Clone, Copy)]
struct CurrentElfMetadata {
    entry_point: Word, program_header_vaddr: Word,
    program_header_entry_size: Word, program_header_count: Word,
}
mod common {
    pub mod dynamic { pub struct Interpreter { pub load_bias: usize, pub entry: usize } }
}
struct Process {
    pcb: Word, personality: OsPersonality, argument_vector: Word, argument_count: usize,
}
struct Runtime {
    exec_stack_buffer: Word, exec_stack_buffer_size: Word,
    process: Process, register_ok: bool,
}
impl Runtime {
    fn managed_process(&self, _: Word) -> Option<&Process> { Some(&self.process) }
    fn managed_process_mut(&mut self, _: Word) -> Option<&mut Process> { Some(&mut self.process) }
    fn set_fs_base(&mut self, _: Word, _: Word) -> bool { true }
}
#[derive(Default)]
struct Backend { writes: Vec<(Word, Vec<u8>)>, fail: bool, registers: usize }
thread_local! { static BACKEND: std::cell::RefCell<Backend> = Default::default(); }
pub fn request_heap(_: Word) -> Result<(Word, Word), i32> { panic!("preallocated fixture") }
pub fn request_process_memory_write(_: Word, address: Word, source: Word, size: Word) -> Result<(), i32> {
    BACKEND.with(|b| {
        let mut b = b.borrow_mut();
        b.writes.push((address, unsafe { core::slice::from_raw_parts(source as *const u8, size) }.to_vec()));
        if b.fail { Err(EFAULT) } else { Ok(()) }
    })
}
fn map_request_error(e: i32) -> i32 { e }
fn read_current_elf_metadata(_: &mut Runtime, _: Word, _: Word) -> Result<CurrentElfMetadata, i32> { Ok(elf()) }
fn register_linux_stack_mappings(r: &mut Runtime, _: Word) -> bool { r.register_ok }
fn process_diagnostics_enabled(_: &Runtime, _: Word) -> bool { false }
fn log_exec_image_entry(_: &Runtime, _: Word, _: Word) {}
fn log_exec_stack(_: &Runtime, _: Word, _: Word) {}
fn install_freebsd_exec_tls(_: &mut Runtime, _: Word, _: &CurrentElfMetadata) -> Result<Word, i32> { Ok(123) }
fn write_exec_registers(_: Word, _: Word, _: Word, _: Word, _: Word, _: Word, _: Word, _: Word) -> Result<(), i32> {
    BACKEND.with(|b| b.borrow_mut().registers += 1); Ok(())
}
fn elf() -> CurrentElfMetadata {
    CurrentElfMetadata { entry_point: 0x401000, program_header_vaddr: 0x400040,
        program_header_entry_size: 56, program_header_count: 9 }
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/exec_stack.rs"]
mod production;

fn snapshot(args: &[Vec<u8>], env: &[Vec<u8>], storage: &mut Vec<u8>) -> ExecStringSnapshot {
    let mut s = ExecStringSnapshot { buffer: 0, used: 0, argc: args.len(), envc: env.len(),
        argv_offsets: [0; 64], argv_lens: [0; 64], env_offsets: [0; 64], env_lens: [0; 64] };
    for (strings, offsets, lens) in [(args, &mut s.argv_offsets, &mut s.argv_lens),
        (env, &mut s.env_offsets, &mut s.env_lens)] {
        for (i, bytes) in strings.iter().enumerate() {
            offsets[i] = storage.len(); lens[i] = bytes.len(); storage.extend_from_slice(bytes);
        }
    }
    s.buffer = storage.as_ptr() as Word; s.used = storage.len(); s
}

fn reference(args: &[Vec<u8>], env: &[Vec<u8>], path: &[u8], freebsd: bool, interp: bool)
    -> Result<(Word, Vec<u8>, usize), i32> {
    let mut bytes = vec![0; LINUX_EXEC_STACK_BYTES];
    let base = LINUX_STACK_TOP - bytes.len();
    let mut cursor = bytes.len();
    let mut push = |s: &[u8]| -> Result<Word, i32> {
        let start = cursor.checked_sub(s.len() + 1).ok_or(ENOMEM)?;
        bytes[start..start + s.len()].copy_from_slice(s);
        cursor = start / 8 * 8; Ok(base + start)
    };
    let envp: Vec<_> = env.iter().map(|s| push(s)).collect::<Result<_, _>>()?;
    let argv: Vec<_> = if args.is_empty() { vec![push(b"")?] }
        else { args.iter().map(|s| push(s)).collect::<Result<_, _>>()? };
    let execfn = push(path)?;
    let random = push(&[0x5d,0x8f,0x2a,0x91,0x48,0x3c,0x11,0x67,0x02,0xee,0x70,0x31,0xaa,0x4b,0xd2,0x09])?;
    let platform = push(PLATFORM)?;
    let aux = [(AT_PHDR,0x400040),(AT_PHENT,56),(AT_PHNUM,9),(AT_PAGESZ,LINUX_PAGE_SIZE),
        (AT_BASE,if interp { 0x700000 } else { 0 }),(AT_FLAGS,0),(AT_ENTRY,0x401000),
        (AT_HWCAP,0),(AT_CLKTCK,100),(AT_UID,0),(AT_EUID,0),(AT_GID,0),(AT_EGID,0),
        (AT_SECURE,0),(AT_RANDOM,random),(AT_EXECFN,execfn),(AT_PLATFORM,platform),(AT_NULL,0)];
    let mut words = vec![argv.len()];words.extend(&argv);words.push(0);words.extend(&envp);words.push(0);
    for (key,value) in aux { words.extend([key,value]); }
    let table = words.len() * 8;
    let aligned = cursor / 16 * 16;
    if aligned < table + 16 { return Err(ENOMEM); }
    let sp = (aligned - table) / 16 * 16 - usize::from(freebsd) * 8;
    for (i, word) in words.iter().enumerate() { bytes[sp+i*8..sp+i*8+8].copy_from_slice(&word.to_ne_bytes()); }
    Ok((base+sp,bytes[sp..].to_vec(),argv.len()))
}

#[test]
fn transmitted_stack_matches_zeroed_model_across_dirty_reuse_alignment_and_limits() {
    let mut work = vec![0xa5; LINUX_EXEC_STACK_BYTES + 32];
    let mut r = Runtime { exec_stack_buffer: work[16..].as_mut_ptr() as Word,
        exec_stack_buffer_size: LINUX_EXEC_STACK_BYTES,
        process: Process { pcb: 19, personality: OsPersonality::Linux, argument_vector: 0, argument_count: 0 }, register_ok: true };
    let mut seed = 71u64;
    for step in 0..512 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let counts = [0,1,2,7,32,64];
        let argc = counts[(seed >> 32) as usize % counts.len()];
        let envc = counts[(seed >> 40) as usize % counts.len()];
        let size = [0,1,7,8,15,16,31,127,255,511][(seed >> 48) as usize % 10];
        let args: Vec<_> = (0..argc).map(|i| vec![b'a'+(i%26) as u8; size+i%8]).collect();
        let env: Vec<_> = (0..envc).map(|i| vec![b'A'+(i%26) as u8; size+i%16]).collect();
        let mut path = [0; LINUX_EXEC_PATH_MAX];let pathlen = step % path.len();path[..pathlen].fill(b'p');
        let mut strings = vec![];let s = snapshot(&args,&env,&mut strings);
        let freebsd = step & 1 != 0;let interp = step & 2 != 0;
        r.process.personality = if freebsd { OsPersonality::FreeBsd } else { OsPersonality::Linux };
        r.process.argument_vector = 0;r.process.argument_count = 0;
        BACKEND.with(|b| *b.borrow_mut() = Backend::default());
        work.fill((step as u8).wrapping_add(1));
        let interpreter = common::dynamic::Interpreter { load_bias: 0x700000, entry: 0x701000 };
        let expected = reference(&args,&env,&path[..pathlen],freebsd,interp);
        let actual = production::rewrite_linux_stack(&mut r, 1, &path, pathlen, &s, 0,
            Some(elf()), if interp { Some(&interpreter) } else { None });
        match expected {
            Ok((sp, data, argc)) => {
                assert_eq!(actual, Ok(()));
                BACKEND.with(|b| { let b=b.borrow();assert_eq!(b.writes,[(sp,data)]);assert_eq!(b.registers,1); });
                assert_eq!((r.process.argument_vector,r.process.argument_count),(sp+8,argc));
                let used = sp - (LINUX_STACK_TOP-LINUX_EXEC_STACK_BYTES);
                assert!(work[16..16+used].iter().all(|b| *b == (step as u8).wrapping_add(1)));
            }
            Err(e) => { assert_eq!(actual,Err(e));BACKEND.with(|b| assert!(b.borrow().writes.is_empty())); }
        }
        assert!(work[..16].iter().chain(&work[16+LINUX_EXEC_STACK_BYTES..]).all(|b| *b == (step as u8).wrapping_add(1)));
    }
}

#[test]
fn failed_copyout_or_registration_keeps_publication_order() {
    let mut work = vec![0xa5; LINUX_EXEC_STACK_BYTES];
    let mut r = Runtime { exec_stack_buffer: work.as_mut_ptr() as Word, exec_stack_buffer_size: work.len(),
        process: Process { pcb: 19, personality: OsPersonality::Linux, argument_vector: 999, argument_count: 999 }, register_ok: true };
    let mut storage = vec![];let s=snapshot(&[b"hello".to_vec()],&[],&mut storage);let path=[0;LINUX_EXEC_PATH_MAX];
    for fail in [true,false] {
        BACKEND.with(|b| *b.borrow_mut()=Backend { fail, ..Backend::default() });r.register_ok=fail;
        assert_eq!(production::rewrite_linux_stack(&mut r,1,&path,0,&s,0,Some(elf()),None),Err(if fail { EFAULT } else { ENOMEM }));
        assert_eq!((r.process.argument_vector,r.process.argument_count),(999,999));
        BACKEND.with(|b| {let b=b.borrow();assert_eq!(b.writes.len(),1);assert_eq!(b.registers,0);});
    }
}
