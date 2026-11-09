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
}
pub const OS_RESPONSE_INVALID_ARGUMENT: Word = 1;
pub const OS_RESPONSE_PERMISSION_DENIED: Word = 2;
pub const OS_RESPONSE_INVALID_DESCRIPTOR: Word = 4;
pub const OS_RESPONSE_ILLEGAL_OPERATION: Word = 5;
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/page_window.rs"]
pub mod page_window;
mod state {
    pub use super::{page_window, OsPersonality};
}
struct Mappings;
impl Mappings {
    fn generation(&self) -> Word {
        1
    }
    fn invalidate_views(&self) {}
}
struct Process {
    personality: OsPersonality,
    exited: bool,
    vm_group: Word,
    mappings: Mappings,
}
struct Runtime {
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
}
thread_local! { static REQUESTS: Cell<usize> = const { Cell::new(0) }; }
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
    Runtime {
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
            mappings: Mappings,
        },
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
    assert!(!production::copy_from_owned(&runtime, 7, 4096, &[1; 16385]));
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
