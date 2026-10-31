//! The production ELF loader/cache with only POSIX and Alpha requests mocked.
#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_runtime;
use std::cell::RefCell;
pub type Word = usize;
pub const OS_RESPONSE_INVALID_ARGUMENT: Word = 1;
pub const OS_RESPONSE_INVALID_DESCRIPTOR: Word = 2;
pub const OS_RESPONSE_FATAL: Word = 3;
pub const OS_RESPONSE_ILLEGAL_OPERATION: Word = 4;
#[derive(Clone, Copy, Debug)]
pub enum RequestError {
    Status(Word),
    Transport,
    InvalidArgument,
    Unsupported,
    Protocol,
}
pub mod arch {
    pub const ELF_MACHINE: u16 = 62;
}
pub mod abi {
    pub const ALTER_PATH_MAX: usize = 512;
    pub const ALTER_IO_OFFSET: usize = 128;
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/elf.rs"]
mod elf;
#[path = "../../nanami/servers/apps/alter/shared/src/common/image_cache.rs"]
mod image_cache;
#[path = "../../nanami/servers/apps/alter/shared/src/common/loader.rs"]
mod loader;

#[derive(Default)]
struct Backend {
    bytes: Vec<u8>,
    identity: Word,
    reads: usize,
    opens: usize,
    closes: usize,
    allocations: Vec<(Box<[u8]>, bool)>,
    change_during_read: bool,
    fail_read: bool,
    fail_stat: bool,
    fail_release: bool,
}
thread_local! { static BACKEND: RefCell<Backend> = RefCell::new(Backend::default()); }
pub fn request_heap(bytes: Word) -> Result<(Word, Word), RequestError> {
    BACKEND.with(|b| {
        let mut b = b.borrow_mut();
        let capacity = bytes.next_multiple_of(4096);
        let mut allocation = vec![0xa5; capacity].into_boxed_slice();
        let address = allocation.as_mut_ptr() as Word;
        b.allocations.push((allocation, false));
        Ok((address, capacity))
    })
}
pub fn request_mapping_release(address: Word, size: Word) -> Result<(), RequestError> {
    BACKEND.with(|b| {
        let mut b = b.borrow_mut();
        if b.fail_release {
            return Err(RequestError::Transport);
        }
        let (buffer, released) = b
            .allocations
            .iter_mut()
            .find(|(buffer, _)| buffer.as_ptr() as Word == address)
            .unwrap();
        assert!(!*released);
        assert_eq!(buffer.len(), size);
        *released = true;
        Ok(())
    })
}
pub mod posix {
    use super::*;
    pub const POSIX_STAT_INCLUDE_VERSION: Word = 2;
    pub const POSIX_FILE_TYPE_REGULAR: Word = 1;
    pub fn posix_open(_: Word, _: Word, _: Word, _: Word) -> Result<Word, RequestError> {
        BACKEND.with(|b| b.borrow_mut().opens += 1);
        Ok(7)
    }
    pub fn posix_close(_: Word, fd: Word) -> Result<(), RequestError> {
        assert_eq!(fd, 7);
        BACKEND.with(|b| b.borrow_mut().closes += 1);
        Ok(())
    }
    pub fn posix_fstat_flags(
        _: Word,
        fd: Word,
        flags: Word,
    ) -> Result<(Word, Word, Word, Word, Word), RequestError> {
        assert_eq!(fd, 7);
        assert_eq!(flags, POSIX_STAT_INCLUDE_VERSION);
        BACKEND.with(|b| {
            let b = b.borrow();
            if b.fail_stat {
                return Err(RequestError::Transport);
            }
            Ok((b.identity, b.bytes.len(), POSIX_FILE_TYPE_REGULAR, 0, 0))
        })
    }
}
mod state {
    use super::*;
    pub struct Runtime {
        pub posix_port: Word,
        pub posix_shm: Word,
        pub posix_shm_size: Word,
        pub client_shm: Word,
        pub client_shm_size: Word,
        pub exec_image_buffer: Word,
        pub exec_image_buffer_size: Word,
        pub interpreter_image_buffer: Word,
        pub interpreter_image_buffer_size: Word,
        pub image_cache: image_cache::ImageCache,
    }
    impl Runtime {
        pub fn posix_read_buffer_size(&self) -> Word {
            self.posix_shm_size
        }
        pub fn pread_posix(
            &self,
            fd: Word,
            out: Word,
            len: Word,
            offset: Word,
        ) -> Result<(Word, Word), RequestError> {
            assert_eq!(fd, 7);
            BACKEND.with(|b| {
                let mut b = b.borrow_mut();
                b.reads += 1;
                if b.fail_read {
                    return Err(RequestError::Transport);
                }
                let count = len.min(b.bytes.len().saturating_sub(offset));
                assert!(out + count <= self.posix_shm_size);
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        b.bytes.as_ptr().add(offset),
                        (self.posix_shm + out) as *mut u8,
                        count,
                    );
                }
                if b.change_during_read {
                    b.identity += 1 << 32;
                }
                Ok((count, self.posix_shm + out))
            })
        }
        pub fn read_posix(
            &self,
            fd: Word,
            out: Word,
            len: Word,
        ) -> Result<(Word, Word), RequestError> {
            self.pread_posix(fd, out, len, 0)
        }
    }
}
fn elf_bytes(size: usize) -> Vec<u8> {
    let mut b = vec![0; size];
    b[..6].copy_from_slice(b"\x7fELF\x02\x01");
    b[16..18].copy_from_slice(&2u16.to_le_bytes());
    b[18..20].copy_from_slice(&62u16.to_le_bytes());
    b[24..32].copy_from_slice(&0x400080u64.to_le_bytes());
    b[32..40].copy_from_slice(&64u64.to_le_bytes());
    b[54..56].copy_from_slice(&56u16.to_le_bytes());
    b[56..58].copy_from_slice(&1u16.to_le_bytes());
    b[64..68].copy_from_slice(&1u32.to_le_bytes());
    b[68..72].copy_from_slice(&5u32.to_le_bytes());
    b[80..88].copy_from_slice(&0x400000u64.to_le_bytes());
    b[96..104].copy_from_slice(&(size as u64).to_le_bytes());
    b[104..112].copy_from_slice(&(size as u64).to_le_bytes());
    b
}
fn fixture(shm: &mut [u8]) -> state::Runtime {
    BACKEND.with(|b| {
        *b.borrow_mut() = Backend {
            bytes: elf_bytes(8193),
            identity: 1 << 32 | 7,
            ..Backend::default()
        }
    });
    state::Runtime {
        posix_port: 1,
        posix_shm: shm.as_mut_ptr() as Word,
        posix_shm_size: shm.len(),
        client_shm: shm.as_mut_ptr() as Word,
        client_shm_size: shm.len(),
        exec_image_buffer: 0,
        exec_image_buffer_size: 0,
        interpreter_image_buffer: 0,
        interpreter_image_buffer_size: 0,
        image_cache: image_cache::ImageCache::new(),
    }
}
fn load(runtime: &mut state::Runtime) -> loader::LoadedElfImage {
    unsafe {
        std::ptr::copy_nonoverlapping(b"/elf".as_ptr(), runtime.client_shm as *mut u8, 4);
    }
    loader::load_linux_elf_image(runtime, 0, 4).unwrap_or_else(|_| panic!("ELF load failed"))
}
#[test]
fn repeated_exec_fork_and_interpreter_hits_skip_all_payload_io_and_allocation() {
    let mut shm = [0; 4096];
    let mut r = fixture(&mut shm);
    let first = load(&mut r);
    let reads = BACKEND.with(|b| b.borrow().reads);
    assert_eq!(load(&mut r).address, first.address);
    assert_eq!(
        loader::load_linux_elf_image(&mut r, 0, 4)
            .ok()
            .unwrap()
            .address,
        first.address
    );
    assert_eq!(
        loader::load_interpreter_elf_image(&mut r, b"/alias")
            .ok()
            .unwrap()
            .address,
        first.address
    );
    BACKEND.with(|b| {
        let b = b.borrow();
        assert_eq!(b.reads, reads);
        assert_eq!(b.allocations.len(), 1);
        assert_eq!(b.opens, 4);
        assert_eq!(b.closes, 4);
    });
}
#[test]
fn same_size_content_update_and_path_replacement_reload() {
    let mut shm = [0; 4096];
    let mut r = fixture(&mut shm);
    let first = load(&mut r);
    BACKEND.with(|b| {
        let mut b = b.borrow_mut();
        b.identity += 1 << 32;
        b.bytes[8000] = 0x42;
    });
    let changed = load(&mut r);
    assert_ne!(first.address, changed.address);
    assert_eq!(unsafe { *((changed.address + 8000) as *const u8) }, 0x42);
    BACKEND.with(|b| b.borrow_mut().identity = 3 << 32 | 9);
    assert_ne!(load(&mut r).address, changed.address);
}
#[test]
fn lru_eviction_reuses_capacity_and_keeps_the_current_main_image_alive() {
    let mut shm = [0; 4096];
    let mut r = fixture(&mut shm);
    let first = load(&mut r);
    for version in 2..=8 {
        BACKEND.with(|b| b.borrow_mut().identity = version << 32 | 7);
        load(&mut r);
    }
    BACKEND.with(|b| b.borrow_mut().identity = 1 << 32 | 7);
    assert_eq!(load(&mut r).address, first.address);
    BACKEND.with(|b| b.borrow_mut().identity = 9 << 32 | 7);
    loader::load_interpreter_elf_image(&mut r, b"/ld")
        .ok()
        .unwrap();
    BACKEND.with(|b| {
        let b = b.borrow();
        assert!(!b.allocations[0].1);
        assert!(!b.allocations[1].1);
        assert_eq!(b.allocations.len(), image_cache::ENTRIES);
        assert_eq!(
            b.allocations
                .iter()
                .filter(|(_, released)| !released)
                .count(),
            image_cache::ENTRIES
        );
    });
}
#[test]
fn uncacheable_files_use_reusable_buffers_but_never_skip_reads() {
    let mut shm = [0; 4096];
    let mut r = fixture(&mut shm);
    BACKEND.with(|b| b.borrow_mut().identity = 7);
    let first = load(&mut r);
    let reads = BACKEND.with(|b| b.borrow().reads);
    assert_eq!(load(&mut r).address, first.address);
    BACKEND.with(|b| {
        let b = b.borrow();
        assert_eq!(b.reads, 2 * reads);
        assert_eq!(b.allocations.len(), 1);
    });
}
#[test]
fn changing_file_or_read_error_never_enters_cache_and_always_closes() {
    for change in [false, true] {
        let mut shm = [0; 4096];
        let mut r = fixture(&mut shm);
        BACKEND.with(|b| {
            let mut b = b.borrow_mut();
            b.change_during_read = change;
            b.fail_read = !change;
        });
        unsafe {
            std::ptr::copy_nonoverlapping(b"/elf".as_ptr(), r.client_shm as *mut u8, 4);
        }
        assert!(loader::load_linux_elf_image(&mut r, 0, 4).is_err());
        assert_eq!(r.image_cache.entry(0).unwrap().identity, 0);
        BACKEND.with(|b| {
            let b = b.borrow();
            assert_eq!(b.opens, b.closes);
            assert!(!b.allocations[0].1);
        });
    }
}

#[test]
fn repeated_cache_eviction_does_not_accumulate_released_alpha_allocations() {
    let mut shm = [0; 4096];
    let mut r = fixture(&mut shm);
    for version in 1..=100 {
        BACKEND.with(|b| b.borrow_mut().identity = version << 32 | 7);
        load(&mut r);
    }
    BACKEND.with(|b| {
        let b = b.borrow();
        assert_eq!(b.allocations.len(), image_cache::ENTRIES);
        assert!(!b.allocations.iter().any(|(_, released)| *released));
    });
}
#[test]
fn fstat_failure_closes_before_allocating() {
    let mut shm = [0; 4096];
    let mut r = fixture(&mut shm);
    BACKEND.with(|b| b.borrow_mut().fail_stat = true);
    assert!(loader::load_interpreter_elf_image(&mut r, b"/elf").is_err());
    BACKEND.with(|b| {
        let b = b.borrow();
        assert_eq!(b.opens, b.closes);
        assert!(b.allocations.is_empty());
    });
}

#[test]
fn whole_file_mmap_populates_snapshot_and_fixed_segment_maps_do_not_reread() {
    let mut shm = [0; 4096];
    let mut r = fixture(&mut shm);
    let identity = 1 << 32 | 7;
    assert!(
        loader::file_mapping_source(&mut r, 7, identity, 8193, 4096, 4096)
            .ok()
            .unwrap()
            .is_none()
    );
    let first = loader::file_mapping_source(&mut r, 7, identity, 8193, 0, 8193)
        .ok()
        .unwrap()
        .unwrap();
    let reads = BACKEND.with(|b| b.borrow().reads);
    assert_eq!(
        loader::file_mapping_source(&mut r, 7, identity, 8193, 4096, 4096)
            .ok()
            .unwrap(),
        Some(first + 4096)
    );
    assert!(loader::file_mapping_source(&mut r, 7, identity, 8193, 8193, 1).is_err());
    BACKEND.with(|b| assert_eq!(b.borrow().reads, reads));
}

#[test]
fn non_elf_file_mappings_cache_bytes_but_exec_still_rejects_them() {
    let mut shm = [0; 4096];
    let mut r = fixture(&mut shm);
    BACKEND.with(|b| b.borrow_mut().bytes[0] = 0);
    let identity = 1 << 32 | 7;
    assert!(
        loader::file_mapping_source(&mut r, 7, identity, 8193, 0, 8193)
            .ok()
            .unwrap()
            .is_some()
    );
    let reads = BACKEND.with(|b| b.borrow().reads);
    assert!(loader::load_interpreter_elf_image(&mut r, b"/elf").is_err());
    BACKEND.with(|b| assert_eq!(b.borrow().reads, reads));
}
