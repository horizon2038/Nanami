//! Actual native adapter: unsupported personalities never issue view requests.
#![allow(dead_code)]
extern crate self as nanami_runtime;
use std::cell::Cell;
pub type Word = usize;
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OsPersonality {
    Linux,
    FreeBsd,
}
#[derive(Clone, Copy)]
pub enum RequestError {
    Status(Word),
    Transport,
    Protocol,
}
pub use nanami_uapi::os::{OsPageViewEntry, OS_PAGE_VIEW_VECTOR_MAX,
    OS_RESPONSE_INVALID_ARGUMENT, OS_RESPONSE_PERMISSION_DENIED,
    OS_RESPONSE_INVALID_DESCRIPTOR, OS_RESPONSE_ILLEGAL_OPERATION, OS_RESPONSE_FATAL};
fn request_process_read_views(pid: Word, entries: &[OsPageViewEntry]) -> Result<(Word, Word), RequestError> {
    BATCH_REQUESTS.with(|count| count.set(count.get() + 1));
    for (index, entry) in entries.iter().enumerate() {
        match request_process_read_view(pid, entry.target, entry.local) {
            Ok(()) => {},
            Err(RequestError::Status(status)) => return Ok((index, status)),
            Err(error) => return Err(error),
        }
    }
    VECTOR_REPLY.with(Cell::get).unwrap_or(Ok((entries.len(), 0)))
}
fn request_process_write_views(pid: Word, entries: &[OsPageViewEntry]) -> Result<(Word, Word), RequestError> {
    BATCH_REQUESTS.with(|count| count.set(count.get() + 1));
    for (index, entry) in entries.iter().enumerate() {
        match request_process_write_view(pid, entry.target, entry.local) {
            Ok(()) => {},
            Err(RequestError::Status(status)) => return Ok((index, status)),
            Err(error) => return Err(error),
        }
    }
    VECTOR_REPLY.with(Cell::get).unwrap_or(Ok((entries.len(), 0)))
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/page_window.rs"]
pub mod page_window;
mod state {
    pub use super::{page_window, OsPersonality};
}
struct Mappings { generation: Cell<Word> }
impl Mappings {
    fn generation(&self) -> Word {
        self.generation.get()
    }
    fn invalidate_views(&self) { self.generation.set(self.generation.get() + 1); }
}
struct Process {
    personality: OsPersonality,
    exited: bool,
    vm_group: Word,
    mappings: Mappings,
}
struct Runtime {
    fail_metadata: bool,
    posix_shm: Word,
    posix_shm_size: Word,
    posix_direct_shm: Word,
    posix_direct_shm_size: Word,
    read_window: page_window::ReadWindow,
    write_window: page_window::WriteWindow,
    process: Process,
}
impl Runtime {
    fn managed_process(&self, pid: Word) -> Option<&Process> {
        (pid == 7).then_some(&self.process)
    }
    fn mapping_protection_needed(&self, pid: Word, _: Word, _: Word, _: Word) -> Option<bool> {
        self.managed_process(pid).map(|_| true)
    }
    fn protect_mapping(&mut self, _: Word, _: Word, _: Word, _: Word) -> bool {
        if self.fail_metadata { return false; }
        self.process.mappings.invalidate_views();
        true
    }
}
const LINUX_PAGE_SIZE: Word = 4096;
const LINUX_PROT_ALL: Word = 7;
const LINUX_PROT_WRITE: Word = 2;
const EINVAL: i32 = 22;
const ENOMEM: i32 = 12;
mod guest_views {
    pub(super) use super::production::{BeforeMmap, invalidate_range};
}
mod file_remap {
    use super::*;
    pub(super) fn snapshot_write(_: &Runtime, _: Word, _: Word, _: Word, _: Word) -> bool { false }
    pub(super) fn before_protect(_: &Runtime, _: Word, _: Word) {}
    pub(super) fn after_protect(_: &Runtime, _: Word, _: ()) {}
}
thread_local! {
    static PROTECT_FAIL: Cell<bool> = const { Cell::new(false) };
    static PROTECT_CALLS: Cell<usize> = const { Cell::new(0) };
}
mod memfd {
    use super::*;
    pub(super) fn protect(_: &Runtime, _: Word, _: Word, _: Word, _: Word) -> Result<(), i32> {
        PROTECT_CALLS.with(|n| n.set(n.get() + 1));
        if PROTECT_FAIL.with(Cell::get) { Err(5) } else { Ok(()) }
    }
}
mod private_memory {
    use super::*;
    pub(super) fn with_mapping_mutation<T>(r: &Runtime, pid: Word, base: Word, bytes: Word, _: bool,
        operation: impl FnOnce() -> Result<T, i32>) -> Result<T, i32> {
        // Model the mutation boundary; the separate quiescence harness uses
        // the real guard and checks stop ACKs before this range invalidation.
        production::invalidate_range(r, pid, base, bytes);
        operation()
    }
    pub(super) fn with_private_protect<T>(r: &Runtime, pid: Word, _: Word, _: Word,
        operation: impl FnOnce() -> Result<T, i32>) -> Result<T, i32> {
        production::invalidate(r, pid);
        operation()
    }
    pub(super) fn with_snapshot_write<T>(r: &Runtime, pid: Word, base: Word, bytes: Word,
        operation: impl FnOnce() -> Result<T, i32>) -> Result<T, i32> {
        with_private_protect(r, pid, base, bytes, operation)
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/memory_protection.rs"]
mod protection;
thread_local! {
    static REQUESTS: Cell<usize> = const { Cell::new(0) };
    static BATCH_REQUESTS: Cell<usize> = const { Cell::new(0) };
    static VECTOR_REPLY: Cell<Option<Result<(Word, Word), RequestError>>> = const { Cell::new(None) };
    static RELEASE_BATCHES: Cell<usize> = const { Cell::new(0) };
    static RELEASE_REPLY: Cell<Option<Result<Word, RequestError>>> = const { Cell::new(None) };
}
#[cfg(target_os = "linux")]
thread_local! {
    static INSTALL_VIEWS: Cell<bool> = const { Cell::new(false) };
    static INSTALL_READ_VIEWS: Cell<bool> = const { Cell::new(false) };
    static VIEWS: std::cell::RefCell<Vec<(Word, Word)>> = const { std::cell::RefCell::new(Vec::new()) };
}
#[cfg(target_os = "linux")]
extern "C" {
    fn mmap(address: *mut core::ffi::c_void, bytes: usize, prot: i32, flags: i32, fd: i32, offset: isize)
        -> *mut core::ffi::c_void;
    fn munmap(address: *mut core::ffi::c_void, bytes: usize) -> i32;
    fn mprotect(address: *mut core::ffi::c_void, bytes: usize, prot: i32) -> i32;
}
fn request_process_write_view(_: Word, page: Word, local: Word) -> Result<(), RequestError> {
    REQUESTS.with(|count| count.set(count.get() + 1));
    #[cfg(target_os = "linux")]
    if INSTALL_VIEWS.with(Cell::get) {
        let mapped = unsafe { mmap(local as *mut _, 4096, 3, 0x100022, -1, 0) };
        assert_eq!(mapped as Word, local, "{}", std::io::Error::last_os_error());
        VIEWS.with(|views| views.borrow_mut().push((page, local)));
        return Ok(());
    }
    // Refuse installation, so no synthetic fixed-VA mapping is dereferenced.
    Err(RequestError::Status(OS_RESPONSE_PERMISSION_DENIED))
}
fn request_process_read_view(_: Word, page: Word, local: Word) -> Result<(), RequestError> {
    REQUESTS.with(|count| count.set(count.get() + 1));
    #[cfg(target_os = "linux")]
    if INSTALL_READ_VIEWS.with(Cell::get) {
        let mapped = unsafe { mmap(local as *mut _,4096,3,0x100022,-1,0) };
        assert_eq!(mapped as Word,local,"{}",std::io::Error::last_os_error());
        unsafe {
            for offset in 0..4096 { (local as *mut u8).add(offset).write(read_sample(page+offset,0)); }
            assert_eq!(mprotect(local as *mut _,4096,1),0);
        }
        VIEWS.with(|views|views.borrow_mut().push((page,local)));
        return Ok(());
    }
    Err(RequestError::Status(OS_RESPONSE_PERMISSION_DENIED))
}
#[cfg(target_os = "linux")]
fn read_sample(address: Word, epoch: usize) -> u8 {
    (address.wrapping_mul(37) ^ (address >> 12) ^ epoch) as u8
}
fn request_mapping_release(local: Word, bytes: Word) -> Result<(), RequestError> {
    #[cfg(target_os = "linux")]
    if INSTALL_VIEWS.with(Cell::get) || INSTALL_READ_VIEWS.with(Cell::get) {
        assert_eq!(bytes, 4096);
        VIEWS.with(|views| {
            let mut views = views.borrow_mut();
            let index = views.iter().position(|(_, address)| *address == local).unwrap();
            views.remove(index);
        });
        assert_eq!(unsafe { munmap(local as *mut _, bytes) }, 0);
        return Ok(());
    }
    panic!("a denied installation owns no mapping to release");
}
fn request_mapping_release_views(locals: &[Word]) -> Result<Word, RequestError> {
    RELEASE_BATCHES.with(|count| count.set(count.get() + 1));
    let mut acknowledged = 0;
    for (index, &local) in locals.iter().enumerate() {
        #[cfg(target_os = "linux")]
        let present = VIEWS.with(|views| views.borrow().iter().any(|&(_, address)| address == local));
        #[cfg(not(target_os = "linux"))]
        let present = true;
        if !present || request_mapping_release(local, 4096).is_ok() { acknowledged |= 1 << index; }
    }
    RELEASE_REPLY.with(Cell::get).unwrap_or(Ok(acknowledged))
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/guest_views.rs"]
mod production;

#[test]
#[cfg(target_os = "linux")]
fn large_direct_write_inputs_use_readonly_native_aliases_and_fresh_samples() {
    let control = [0u8; 32];
    let mut direct = vec![0xabu8; 16391];
    let mut runtime = runtime(&control, OsPersonality::Linux);
    runtime.posix_direct_shm = direct.as_mut_ptr() as Word;
    runtime.posix_direct_shm_size = direct.len();
    let enabled = option_env!("NANAMI_GUEST_READ_VIEWS").map_or(cfg!(target_arch="x86_64"), |v| v == "1");
    if !enabled { return; }
    INSTALL_READ_VIEWS.with(|mode| mode.set(true));
    for epoch in 0..20 {
        if epoch != 0 {
            VIEWS.with(|views| for &(page, local) in views.borrow().iter() {
                unsafe {
                    assert_eq!(mprotect(local as *mut _, 4096, 3), 0);
                    for offset in 0..4096 { (local as *mut u8).add(offset).write(read_sample(page + offset, epoch)); }
                    assert_eq!(mprotect(local as *mut _, 4096, 1), 0);
                }
            });
        }
        let len = [16384, 4097, 8192, 12289, 16383][epoch % 5];
        direct.fill(0xab);
        assert!(production::copy_into_scratch(&runtime, 7, 0x5013, runtime.posix_direct_shm + 7, len));
        for offset in 0..len { assert_eq!(direct[7 + offset], read_sample(0x5013 + offset, epoch)); }
        assert!(direct[..7].iter().chain(&direct[7 + len..]).all(|b| *b == 0xab));
        REQUESTS.with(|count| assert_eq!(count.get(), 5));
    }
    production::invalidate(&runtime, 7);
    VIEWS.with(|views| assert!(views.borrow().is_empty()));
    INSTALL_READ_VIEWS.with(|mode| mode.set(false));
}

#[test]
#[cfg(target_os = "linux")]
fn repeated_direct_write_inputs_reuse_readonly_cross_page_views_and_current_samples() {
    let control = [0u8;32];
    let mut direct = vec![0xabu8;4119];
    let mut runtime = runtime(&control,OsPersonality::Linux);
    runtime.posix_direct_shm=direct.as_mut_ptr() as Word;
    runtime.posix_direct_shm_size=direct.len();
    let enabled=option_env!("NANAMI_GUEST_READ_VIEWS").map_or(cfg!(target_arch="x86_64"),|v|v=="1");
    if !enabled {
        assert!(!production::copy_into_scratch(&runtime,7,0x5013,runtime.posix_direct_shm+7,4096));
        REQUESTS.with(|count|assert_eq!(count.get(),0));
        return;
    }
    INSTALL_READ_VIEWS.with(|mode|mode.set(true));
    for epoch in 0..100 {
        if epoch != 0 {
            VIEWS.with(|views|for &(page,local) in views.borrow().iter() {
                // Simulate producer changes between samples. NativeEffects
                // sees a read-only alias again for the entire actual copy.
                unsafe {
                    assert_eq!(mprotect(local as *mut _,4096,3),0);
                    for offset in 0..4096 { (local as *mut u8).add(offset).write(read_sample(page+offset,epoch)); }
                    assert_eq!(mprotect(local as *mut _,4096,1),0);
                }
            });
        }
        let len=if epoch==0 {4096} else {[1,8,17,127,1024,4095,4096][epoch%7]};
        direct.fill(0xab);
        assert!(production::copy_into_scratch(&runtime,7,0x5013,runtime.posix_direct_shm+7,len));
        for offset in 0..len { assert_eq!(direct[7+offset],read_sample(0x5013+offset,epoch)); }
        assert!(direct[..7].iter().chain(&direct[7+len..]).all(|byte|*byte==0xab));
        REQUESTS.with(|count|assert_eq!(count.get(),2));
    }
    production::invalidate(&runtime,7);
    VIEWS.with(|views|assert!(views.borrow().is_empty()));
    INSTALL_READ_VIEWS.with(|mode|mode.set(false));
    println!("POSIX-WRITE-NATIVE read-grants=2 copies=100 warm-extra-grants=0 guards=ok readonly-during-copy=true");
}

#[test]
fn delegated_write_destination_bounds_and_failed_read_lease_preserve_bytes() {
    let control = [0u8; 32];
    let mut direct = vec![0xa5u8;128];
    let mut runtime = runtime(&control,OsPersonality::Linux);
    runtime.posix_direct_shm = direct.as_mut_ptr() as Word;
    runtime.posix_direct_shm_size = direct.len();
    let base = runtime.posix_direct_shm;
    let enabled = option_env!("NANAMI_GUEST_READ_VIEWS").map_or(cfg!(target_arch="x86_64"), |v|v=="1");
    assert!(!production::copy_into_scratch(&runtime,7,4096,base+120,8));
    REQUESTS.with(|count|assert_eq!(count.replace(0),usize::from(enabled)));
    for (destination,bytes) in [(base-1,1),(base+127,2),(base+128,1),(Word::MAX-3,8),(base,4097)] {
        assert!(!production::copy_into_scratch(&runtime,7,4096,destination,bytes));
    }
    runtime.posix_direct_shm_size=0;
    assert!(!production::copy_into_scratch(&runtime,7,4096,base,8));
    runtime.posix_direct_shm_size=128; runtime.posix_direct_shm=0;
    assert!(!production::copy_into_scratch(&runtime,7,4096,base,8));
    runtime.posix_direct_shm=base; runtime.process.exited=true;
    assert!(!production::copy_into_scratch(&runtime,7,4096,base,8));
    REQUESTS.with(|count|assert_eq!(count.get(),0));
    assert_eq!(direct,vec![0xa5;128]);
}

#[test]
fn actual_native_copy_preserves_every_alignment_tail_and_output_guard() {
    use std::mem::MaybeUninit;
    let lengths: Vec<_> = (0..80).chain([127,128,129,255,256,257,263,264,511,512,513,1023,1024,2047,2048,2049,2055,2056,4095,4096]).collect();
    for input_alignment in 0..8 {
        for output_alignment in 0..8 {
            for &len in &lengths {
                let mut input = vec![0x39; len + 32];
                let start = 8 + input_alignment;
                for (i,b) in input[start..start + len].iter_mut().enumerate() { *b = i.wrapping_mul(71) as u8; }
                let before = input.clone();
                let offset = 8 + output_alignment;
                let mut captured = vec![MaybeUninit::new(0xa5u8); len + 32];
                unsafe {
                    production::read_native_for_test(input.as_ptr().add(start) as Word,
                        &mut captured[offset..offset + len]);
                }
                let captured: Vec<_> = captured.iter().map(|b| unsafe { b.assume_init() }).collect();
                assert_eq!(&captured[offset..offset + len], &input[start..start + len]);
                assert!(captured[..offset].iter().chain(&captured[offset + len..]).all(|b| *b == 0xa5));
                assert_eq!(input,before);

                let mut destination = vec![0xa5u8; len + 32];
                unsafe {
                    production::write_native_for_test(destination.as_mut_ptr().add(offset) as Word,
                        &input[start..start + len]);
                }
                assert_eq!(&destination[offset..offset + len], &input[start..start + len]);
                assert!(destination[..offset].iter().chain(&destination[offset + len..]).all(|b| *b == 0xa5));
                assert_eq!(input,before);
            }
        }
    }
}

#[test]
#[cfg(target_os = "linux")]
fn long_native_copies_stop_exactly_at_an_inaccessible_page() {
    use std::mem::MaybeUninit;
    let allocation = unsafe { mmap(std::ptr::null_mut(), 3 * 4096, 0, 0x22, -1, 0) };
    assert_ne!(allocation as Word, Word::MAX);
    let page = allocation as Word + 4096;
    assert_eq!(unsafe { mprotect(page as *mut _, 4096, 3) }, 0);
    for len in [2047, 2048, 2049, 2055, 2056, 4095, 4096] {
        unsafe { std::ptr::write_bytes(page as *mut u8, 0x39, 4096); }
        assert_eq!(unsafe { mprotect(page as *mut _, 4096, 1) }, 0);
        let start = page + 4096 - len;
        let mut output = vec![MaybeUninit::new(0xa5u8); len + 16];
        unsafe { production::read_native_for_test(start, &mut output[7..7 + len]); }
        for (i, byte) in output.iter().enumerate() {
            assert_eq!(unsafe { byte.assume_init() }, if (7..7 + len).contains(&i) { 0x39 } else { 0xa5 });
        }
        assert_eq!(unsafe { mprotect(page as *mut _, 4096, 3) }, 0);
        let input = vec![0x71; len];
        unsafe { production::write_native_for_test(start, &input); }
        let bytes = unsafe { std::slice::from_raw_parts(page as *const u8, 4096) };
        assert!(bytes[..4096 - len].iter().all(|byte| *byte == 0x39));
        assert!(bytes[4096 - len..].iter().all(|byte| *byte == 0x71));
    }
    assert_eq!(unsafe { munmap(allocation, 3 * 4096) }, 0);
}

fn runtime(scratch: &[u8], personality: OsPersonality) -> Runtime {
    REQUESTS.with(|count| count.set(0));
    BATCH_REQUESTS.with(|count| count.set(0));
    VECTOR_REPLY.with(|reply| reply.set(None));
    RELEASE_BATCHES.with(|count| count.set(0));
    RELEASE_REPLY.with(|reply| reply.set(None));
    PROTECT_FAIL.with(|fail| fail.set(false));
    PROTECT_CALLS.with(|n| n.set(0));
    Runtime {
        fail_metadata: false,
        posix_shm: scratch.as_ptr() as Word,
        posix_shm_size: scratch.len(),
        posix_direct_shm: 0,
        posix_direct_shm_size: 0,
        read_window: page_window::ReadWindow::new(),
        write_window: page_window::WriteWindow::new(),
        process: Process {
            personality,
            exited: false,
            vm_group: 7,
            mappings: Mappings { generation: Cell::new(1) },
        },
    }
}

#[test]
#[cfg(target_os = "linux")]
fn protection_preflight_errors_leave_current_native_views_and_generation_untouched() {
    let mut r = runtime(&[0u8; 32], OsPersonality::Linux);
    INSTALL_VIEWS.with(|n| n.set(true));
    assert!(production::copy_from_owned(&r, 7, 8192, &[19; 8]));
    let generation = r.process.mappings.generation();
    for (pid, base, bytes, prot, error) in [(7, 0, 1, 1, EINVAL), (7, 1, 1, 1, EINVAL),
        (7, 4096, 0, 1, EINVAL), (7, 4096, 4096, 8, EINVAL),
        (7, 4096, Word::MAX, 1, ENOMEM), (99, 4096, 4096, 1, ENOMEM)] {
        assert_eq!(protection::protect(&mut r, pid, base, bytes, prot), Err(error));
        assert_eq!(r.process.mappings.generation(), generation);
        assert!(production::copy_from_owned_cached(&r, 7, 8192, &[21; 8]));
    }
    assert_eq!(PROTECT_CALLS.with(Cell::get), 0);
    assert_eq!(REQUESTS.with(Cell::get), 1);
    production::invalidate(&r, 7);
    INSTALL_VIEWS.with(|n| n.set(false));
}

#[test]
#[cfg(target_os = "linux")]
fn readonly_protection_commits_keep_disjoint_native_aliases_and_withdraw_touched_pages() {
    let mut r = runtime(&[0u8; 32], OsPersonality::Linux);
    INSTALL_READ_VIEWS.with(|n| n.set(true));
    INSTALL_VIEWS.with(|n| n.set(true));
    let mut output = [core::mem::MaybeUninit::uninit(); 8];
    assert!(production::copy_into_owned(&r, 7, 4096, &mut output));
    assert!(production::copy_from_owned(&r, 7, 8192, &[19; 8]));
    assert!(production::copy_into_owned(&r, 7, 0x401000, &mut output));
    assert!(production::copy_from_owned(&r, 7, 0x401000, &[21; 8]));
    let grants = REQUESTS.with(Cell::get);
    for round in 0..128 {
        assert_eq!(protection::protect(&mut r, 7, 0x401000, 4096, round & 1), Ok(0));
        assert!(production::copy_into_owned(&r, 7, 4096, &mut output));
        assert!(production::copy_from_owned_cached(&r, 7, 8192, &[round as u8; 8]));
        assert!(!production::copy_from_owned_cached(&r, 7, 0x401000, &[99; 8]));
    }
    assert_eq!(REQUESTS.with(Cell::get), grants);
    VIEWS.with(|v| assert_eq!(v.borrow().len(), 2));
    production::invalidate(&r, 7);
    INSTALL_READ_VIEWS.with(|n| n.set(false));
    INSTALL_VIEWS.with(|n| n.set(false));
}

#[test]
#[cfg(target_os = "linux")]
fn failed_protection_and_write_upgrades_never_reactivate_retained_aliases() {
    for failure in 0..3 {
        let mut r = runtime(&[0u8; 32], OsPersonality::Linux);
        INSTALL_VIEWS.with(|n| n.set(true));
        assert!(production::copy_from_owned(&r, 7, 8192, &[19; 8]));
        PROTECT_FAIL.with(|fail| fail.set(failure == 0));
        r.fail_metadata = failure == 1;
        let (prot, expected) = match failure { 0 => (1, Err(5)), 1 => (1, Err(ENOMEM)), _ => (3, Ok(0)) };
        assert_eq!(protection::protect(&mut r, 7, 0x401000, 4096, prot), expected);
        assert!(!production::copy_from_owned_cached(&r, 7, 8192, &[99; 8]));
        let before = production::BeforeMmap::capture(&r, 7).unwrap();
        r.process.mappings.invalidate_views();
        before.completed(&r, 7, 0x800000, 4096);
        assert!(!production::copy_from_owned_cached(&r, 7, 8192, &[99; 8]));
        assert_eq!(REQUESTS.with(Cell::get), 1);
        production::invalidate(&r, 7);
        VIEWS.with(|v| assert!(v.borrow().is_empty()));
        INSTALL_VIEWS.with(|n| n.set(false));
    }
}

#[test]
#[cfg(target_os = "linux")]
fn completed_private_mmap_preserves_disjoint_native_views_and_replacement_revalidates() {
    let mut runtime=runtime(&[0u8;32],OsPersonality::Linux);
    INSTALL_READ_VIEWS.with(|mode|mode.set(true));
    INSTALL_VIEWS.with(|mode|mode.set(true));
    let mut output=[core::mem::MaybeUninit::uninit();8];
    assert!(production::copy_into_owned(&runtime,7,4096,&mut output));
    assert!(production::copy_from_owned(&runtime,7,8192,&[19;8]));
    let first=REQUESTS.with(Cell::get);
    for next in 2..130 {
        let before=production::BeforeMmap::capture(&runtime,7).unwrap();
        runtime.process.mappings.generation.set(next);
        before.completed(&runtime,7,0x100000,8192);
        assert!(production::copy_into_owned(&runtime,7,4096,&mut output));
        assert!(production::copy_from_owned_cached(&runtime,7,8192,&[next as u8;8]));
    }
    assert_eq!(REQUESTS.with(Cell::get),first);
    let before=production::BeforeMmap::capture(&runtime,7).unwrap();
    runtime.process.mappings.generation.set(130);
    before.completed(&runtime,7,8192,4096);
    assert!(!production::copy_from_owned_cached(&runtime,7,8192,&[88;8]));
    assert_eq!(REQUESTS.with(Cell::get),first);
    assert!(production::copy_from_owned(&runtime,7,8192,&[88;8]));
    assert_eq!(REQUESTS.with(Cell::get),first+1);
    // A full COW/fork/exec invalidation releases aliases and cannot be undone
    // by the older saved generation, even for a disjoint completion range.
    let before=production::BeforeMmap::capture(&runtime,7).unwrap();
    production::invalidate(&runtime,7);
    before.completed(&runtime,7,0x100000,8192);
    assert!(!production::copy_from_owned_cached(&runtime,7,8192,&[99;8]));
    assert!(production::copy_into_owned(&runtime,7,4096,&mut output));
    assert_eq!(REQUESTS.with(Cell::get),first+2);
    production::invalidate(&runtime,7);
    INSTALL_READ_VIEWS.with(|mode|mode.set(false));
    INSTALL_VIEWS.with(|mode|mode.set(false));
}

#[test]
#[cfg(target_os = "linux")]
fn mmap_view_snapshot_cannot_rebase_exited_changed_owner_or_personality() {
    for case in 0..3 {
        let mut runtime=runtime(&[0u8;32],OsPersonality::Linux);
        INSTALL_VIEWS.with(|mode|mode.set(true));
        assert!(production::copy_from_owned(&runtime,7,4096,&[19;8]));
        let before=production::BeforeMmap::capture(&runtime,7).unwrap();
        runtime.process.mappings.generation.set(2);
        match case {
            0=>runtime.process.exited=true,
            1=>runtime.process.vm_group=8,
            _=>runtime.process.personality=OsPersonality::FreeBsd,
        }
        before.completed(&runtime,7,0x100000,8192);
        runtime.process.exited=false;runtime.process.vm_group=7;runtime.process.personality=OsPersonality::Linux;
        assert!(!production::copy_from_owned_cached(&runtime,7,4096,&[99;8]));
        production::invalidate(&runtime,7);
        INSTALL_VIEWS.with(|mode|mode.set(false));
    }
}

#[test]
fn freebsd_translation_never_attempts_a_writable_linux_view() {
    let scratch = [17u8; 32];
    let runtime = runtime(&scratch, OsPersonality::FreeBsd);
    assert!(!production::copy_from_owned(&runtime, 7, 4096, &[1; 8]));
    assert!(!production::copy_from_owned_many(
        &runtime,
        7,
        &[(4096, &[1; 8])]
    ));
    assert!(!production::copy_from_scratch(
        &runtime,
        7,
        4096,
        runtime.posix_shm,
        8
    ));
    REQUESTS.with(|count| assert_eq!(count.get(), 0));
}

#[test]
fn live_linux_can_attempt_the_optional_request_and_decline_without_storing() {
    let scratch = [17u8; 32];
    let runtime = runtime(&scratch, OsPersonality::Linux);
    assert!(!production::copy_from_owned(&runtime, 7, 4096, &[1; 8]));
    let enabled = match option_env!("NANAMI_GUEST_WRITE_VIEWS") {
        Some(value) => value == "1",
        None => cfg!(target_arch = "x86_64"),
    };
    REQUESTS.with(|count| assert_eq!(count.get(), usize::from(enabled)));
    assert!(!production::copy_from_owned_many(
        &runtime,
        7,
        &[(4096, &[1; 8])]
    ));
    REQUESTS.with(|count| assert_eq!(count.get(), 2 * usize::from(enabled)));
    assert_eq!(scratch, [17; 32]);
}

#[test]
fn bounds_exited_and_unknown_tasks_are_rejected_before_any_native_request() {
    let scratch = [17u8; 32];
    let mut runtime = runtime(&scratch, OsPersonality::Linux);
    assert!(!production::copy_from_scratch(&runtime, 7, 4096, 0, 8));
    assert!(!production::copy_from_scratch(
        &runtime,
        7,
        4096,
        runtime.posix_shm + 25,
        8
    ));
    assert!(!production::copy_from_scratch(
        &runtime,
        7,
        4096,
        Word::MAX - 3,
        8
    ));
    assert!(!production::copy_from_owned(&runtime, 99, 4096, &[1; 8]));
    assert!(!production::copy_from_owned(&runtime, 7, 4096, &[1; page_window::MAX_CONTIGUOUS_WRITE_BYTES + 1]));
    runtime.process.exited = true;
    assert!(!production::copy_from_owned(&runtime, 7, 4096, &[1; 8]));
    REQUESTS.with(|count| assert_eq!(count.get(), 0));
}

#[test]
fn direct_read_source_bounds_and_disabled_buffers_reject_before_grant() {
    let control = [0u8; 32];
    let direct = vec![0x39u8; 128];
    let mut runtime = runtime(&control, OsPersonality::Linux);
    runtime.posix_direct_shm = direct.as_ptr() as Word;
    runtime.posix_direct_shm_size = direct.len();
    let enabled = option_env!("NANAMI_GUEST_WRITE_VIEWS").map_or(cfg!(target_arch = "x86_64"), |v| v == "1");
    // A refused lease must leave the original checked-copy fallback possible.
    assert!(!production::copy_from_scratch(&runtime, 7, 4096, runtime.posix_direct_shm + 120, 8));
    REQUESTS.with(|count| assert_eq!(count.replace(0), usize::from(enabled)));
    for (source, bytes) in [(runtime.posix_direct_shm - 1, 1),
        (runtime.posix_direct_shm + 127, 2), (runtime.posix_direct_shm + 128, 1),
        (Word::MAX - 3, 8), (runtime.posix_direct_shm, 4097)] {
        assert!(!production::copy_from_scratch(&runtime, 7, 4096, source, bytes));
    }
    let source = runtime.posix_direct_shm;
    runtime.posix_direct_shm_size = 0;
    assert!(!production::copy_from_scratch(&runtime, 7, 4096, source, 8));
    runtime.posix_direct_shm_size = direct.len();
    runtime.posix_direct_shm = 0;
    assert!(!production::copy_from_scratch(&runtime, 7, 4096, source, 8));
    runtime.posix_direct_shm = Word::MAX - 4095;
    runtime.posix_direct_shm_size = Word::MAX;
    assert!(!production::copy_from_scratch(&runtime, 7, 4096, Word::MAX - 3, 8));
    runtime.posix_direct_shm = source;
    runtime.posix_direct_shm_size = direct.len();
    runtime.process.personality = OsPersonality::FreeBsd;
    assert!(!production::copy_from_scratch(&runtime, 7, 4096, source, 8));
    REQUESTS.with(|count| assert_eq!(count.get(), 0));
    assert_eq!(direct, vec![0x39; 128]);
}

#[test]
#[cfg(target_os = "linux")]
fn cached_copyout_adapter_never_installs_and_rejects_unowned_scratch() {
    let control = [37u8; 32];
    let unowned = [99u8; 32];
    let runtime = runtime(&control, OsPersonality::Linux);
    assert!(!production::copy_from_owned_cached(&runtime, 7, 0x5013, &control));
    REQUESTS.with(|n| assert_eq!(n.get(), 0));
    let enabled = option_env!("NANAMI_GUEST_WRITE_VIEWS").map_or(cfg!(target_arch="x86_64"), |v| v == "1");
    if !enabled { return; }
    INSTALL_VIEWS.with(|mode| mode.set(true));
    assert!(production::copy_from_owned(&runtime, 7, 0x5013, &control));
    assert!(!production::copy_from_owned_many_cached(&runtime, 7,
        &[(0x5013, &unowned), (0x6000, &unowned)]));
    for _ in 0..2048 {
        assert!(production::copy_from_owned_many_cached(&runtime, 7,
            &[(0x5013, &control), (0x5017, &control[..8])]));
    }
    assert!(!production::copy_from_scratch_cached(&runtime, 7, 0x5013, unowned.as_ptr() as Word, 32));
    for _ in 0..2048 {
        assert!(production::copy_from_scratch_cached(&runtime, 7, 0x5013, runtime.posix_shm, 32));
    }
    REQUESTS.with(|n| assert_eq!(n.get(), 1));
    VIEWS.with(|views| {
        let views = views.borrow();
        assert_eq!(unsafe { core::slice::from_raw_parts((views[0].1 + 19) as *const u8, 32) }, &control);
    });
    production::invalidate(&runtime, 7);
    assert!(!production::copy_from_owned_cached(&runtime, 7, 0x5013, &control));
    REQUESTS.with(|n| assert_eq!(n.get(), 1));
    INSTALL_VIEWS.with(|mode| mode.set(false));
}

#[test]
#[cfg(target_os = "linux")]
fn large_direct_copyouts_prepare_five_native_pages_then_need_no_more_grants() {
    let control = [0u8; 32];
    let mut direct = vec![0u8; 16391];
    let mut runtime = runtime(&control, OsPersonality::Linux);
    runtime.posix_direct_shm = direct.as_ptr() as Word;
    runtime.posix_direct_shm_size = direct.len();
    let enabled = option_env!("NANAMI_GUEST_WRITE_VIEWS").map_or(cfg!(target_arch = "x86_64"), |v| v == "1");
    if !enabled { return; }
    INSTALL_VIEWS.with(|mode| mode.set(true));
    let mut expected = vec![0u8; 5 * 4096];
    for epoch in 0..20 {
        let len = [16384, 4097, 8192, 12289, 16383][epoch % 5];
        for (i, byte) in direct.iter_mut().enumerate() { *byte = (i * 37 + epoch * 11) as u8; }
        let before = direct.clone();
        assert!(production::copy_from_scratch(&runtime, 7, 0x5013, runtime.posix_direct_shm + 7, len));
        expected[19..19 + len].copy_from_slice(&direct[7..7 + len]);
        VIEWS.with(|views| {
            let views = views.borrow(); assert_eq!(views.len(), 5);
            for &(page, local) in views.iter() {
                let offset = page - 0x5000;
                assert_eq!(unsafe { core::slice::from_raw_parts(local as *const u8, 4096) }, &expected[offset..offset + 4096]);
            }
        });
        assert_eq!(direct, before);
        REQUESTS.with(|count| assert_eq!(count.get(), 5));
    }
    production::invalidate(&runtime, 7);
    VIEWS.with(|views| assert!(views.borrow().is_empty()));
    INSTALL_VIEWS.with(|mode| mode.set(false));
}

#[test]
#[cfg(target_os = "linux")]
fn repeated_direct_read_copyouts_use_two_cross_page_leases_without_more_requests() {
    let control = [0u8; 32];
    let mut direct = vec![0u8; 4103];
    let mut runtime = runtime(&control, OsPersonality::Linux);
    runtime.posix_direct_shm = direct.as_ptr() as Word;
    runtime.posix_direct_shm_size = direct.len();
    let enabled = option_env!("NANAMI_GUEST_WRITE_VIEWS").map_or(cfg!(target_arch = "x86_64"), |v| v == "1");
    if !enabled {
        assert!(!production::copy_from_scratch(&runtime, 7, 0x5013, runtime.posix_direct_shm + 7, 4096));
        REQUESTS.with(|count| assert_eq!(count.get(), 0));
        return;
    }
    INSTALL_VIEWS.with(|mode| mode.set(true));
    let mut expected = vec![0u8; 8192];
    for index in 0usize..100 {
        let len = if index == 0 { 4096 } else { [1, 8, 17, 127, 1024, 4095, 4096][index % 7] };
        for (i, byte) in direct.iter_mut().enumerate() { *byte = (i * 37 + index * 11) as u8; }
        let before = direct.clone();
        assert!(production::copy_from_scratch(&runtime, 7, 0x5013, runtime.posix_direct_shm + 7, len));
        expected[19..19 + len].copy_from_slice(&direct[7..7 + len]);
        VIEWS.with(|views| {
            let views = views.borrow();
            assert_eq!(views.len(), 2);
            for &(page, local) in views.iter() {
                let offset = page - 0x5000;
                let actual = unsafe { core::slice::from_raw_parts(local as *const u8, 4096) };
                assert_eq!(actual, &expected[offset..offset + 4096]);
            }
        });
        assert_eq!(direct, before);
        REQUESTS.with(|count| assert_eq!(count.get(), 2));
    }
    production::invalidate(&runtime, 7);
    VIEWS.with(|views| assert!(views.borrow().is_empty()));
    INSTALL_VIEWS.with(|mode| mode.set(false));
}

#[test]
#[cfg(target_os = "linux")]
fn native_vector_fatal_transport_and_malformed_replies_keep_every_uncertain_pin() {
    let enabled = option_env!("NANAMI_GUEST_WRITE_VIEWS").map_or(cfg!(target_arch="x86_64"), |v| v=="1");
    if !enabled || option_env!("NANAMI_BATCH_PAGE_VIEWS") == Some("0") { return; }
    for reply in [Err(RequestError::Status(OS_RESPONSE_FATAL)), Err(RequestError::Transport),
        Err(RequestError::Protocol), Ok((99, 0))] {
        let scratch = vec![0x57; 4096];
        let runtime = runtime(&scratch, OsPersonality::Linux);
        INSTALL_VIEWS.with(|mode| mode.set(true));
        // The fake server installs both pages, then loses/corrupts its reply.
        // In particular actual FATAL=5 must never become NotInstalled.
        VECTOR_REPLY.with(|value| value.set(Some(reply)));
        assert!(!production::copy_from_scratch(&runtime, 7, 0x5013, runtime.posix_shm, 4096));
        VIEWS.with(|views| {
            assert_eq!(views.borrow().len(), 2);
            for &(_, local) in views.borrow().iter() {
                assert!(unsafe { core::slice::from_raw_parts(local as *const u8, 4096) }.iter().all(|&byte| byte==0));
            }
        });
        assert!(!production::copy_from_scratch_cached(&runtime, 7, 0x5013, runtime.posix_shm, 4096));
        REQUESTS.with(|count| assert_eq!(count.get(), 2));
        production::invalidate(&runtime, 7);
        VIEWS.with(|views| assert!(views.borrow().is_empty()));
        INSTALL_VIEWS.with(|mode| mode.set(false));
    }
}

#[test]
#[cfg(target_os = "linux")]
fn native_release_vectors_keep_unknown_acks_unready_and_retry_already_released_pages() {
    let enabled = option_env!("NANAMI_GUEST_WRITE_VIEWS").map_or(cfg!(target_arch="x86_64"), |v| v=="1");
    if !enabled || option_env!("NANAMI_BATCH_PAGE_RELEASE") == Some("0") { return; }
    for reply in [Err(RequestError::Transport), Err(RequestError::Protocol),
        Err(RequestError::Status(OS_RESPONSE_FATAL)), Ok(1 << 8)] {
        let scratch = vec![0x57; 4096];
        let runtime = runtime(&scratch, OsPersonality::Linux);
        INSTALL_VIEWS.with(|mode| mode.set(true));
        assert!(production::copy_from_scratch(&runtime, 7, 0x5013, runtime.posix_shm, 4096));
        RELEASE_REPLY.with(|value| value.set(Some(reply)));
        production::invalidate(&runtime, 7);
        VIEWS.with(|views| assert!(views.borrow().is_empty()));
        assert!(runtime.write_window.has_owner(7));
        assert!(!production::copy_from_scratch_cached(&runtime, 7, 0x5013, runtime.posix_shm, 4096));
        RELEASE_REPLY.with(|value| value.set(None));
        production::invalidate(&runtime, 7);
        assert!(!runtime.write_window.has_owner(7));
        RELEASE_BATCHES.with(|count| assert_eq!(count.get(), 2));
        INSTALL_VIEWS.with(|mode| mode.set(false));
    }
}

#[test]
#[cfg(target_os = "linux")]
fn full_io_quantum_copyouts_use_seventeen_native_aliases_and_cached_only_reuse() {
    let control = [0u8; 32];
    let mut direct = vec![0u8; 64 * 1024 + 7];
    let mut runtime = runtime(&control, OsPersonality::Linux);
    runtime.posix_direct_shm = direct.as_ptr() as Word;
    runtime.posix_direct_shm_size = direct.len();
    let enabled = option_env!("NANAMI_GUEST_WRITE_VIEWS").map_or(cfg!(target_arch = "x86_64"), |v| v == "1");
    if !enabled { return; }
    INSTALL_VIEWS.with(|mode| mode.set(true));
    let mut expected = vec![0u8; 17 * 4096];
    let batched = option_env!("NANAMI_BATCH_PAGE_VIEWS") != Some("0");
    assert_eq!(production::copy_from_scratch(&runtime, 7, 0x5013, runtime.posix_direct_shm + 7, 65536), batched);
    REQUESTS.with(|count| assert_eq!(count.get(), if batched { 17 } else { 0 }));
    BATCH_REQUESTS.with(|count| assert_eq!(count.get(), usize::from(batched)));
    for epoch in 0..32 {
        let len = [65536, 32769, 16385, 65535][epoch % 4];
        for (i, byte) in direct.iter_mut().enumerate() { *byte = (i * 37 + epoch * 11) as u8; }
        let before = direct.clone();
        let copied = if epoch == 0 {
            production::copy_from_scratch(&runtime, 7, 0x5013, runtime.posix_direct_shm + 7, len)
        } else {
            production::copy_from_scratch_cached(&runtime, 7, 0x5013, runtime.posix_direct_shm + 7, len)
        };
        assert!(copied);
        expected[19..19 + len].copy_from_slice(&direct[7..7 + len]);
        VIEWS.with(|views| {
            let views = views.borrow();
            assert_eq!(views.len(), 17);
            for &(page, local) in views.iter() {
                let offset = page - 0x5000;
                assert_eq!(unsafe { core::slice::from_raw_parts(local as *const u8, 4096) }, &expected[offset..offset + 4096]);
            }
        });
        assert_eq!(direct, before);
        REQUESTS.with(|count| assert_eq!(count.get(), 17));
        BATCH_REQUESTS.with(|count| assert_eq!(count.get(), usize::from(batched)));
    }
    production::invalidate(&runtime, 7);
    VIEWS.with(|views| assert!(views.borrow().is_empty()));
    INSTALL_VIEWS.with(|mode| mode.set(false));
}
