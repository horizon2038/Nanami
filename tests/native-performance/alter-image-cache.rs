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
    read_ranges: Vec<(Word, Word, Word)>,
    short_read: Option<usize>,
    opens: usize,
    closes: usize,
    allocations: Vec<(Box<[u8]>, bool)>,
    change_during_read: bool,
    fail_read: bool,
    fail_stat: bool,
    fail_release: bool,
    fail_create: bool,
    fail_protect: bool,
    fail_self_pid: bool,
    self_pid_calls: usize,
    reclaimable_creates: usize,
    protection_attempts: Vec<(Word, Word, Word, Word)>,
    release_attempts: Vec<(Word, Word)>,
    peak_live_bytes: usize,
}
thread_local! { static BACKEND: RefCell<Backend> = RefCell::new(Backend::default()); }
pub fn get_self_pid() -> Result<Word, RequestError> {
    BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        backend.self_pid_calls += 1;
        if backend.fail_self_pid {
            return Err(RequestError::Transport);
        }
        Ok(18)
    })
}
pub fn request_process_mapping_protect(
    pid: Word,
    address: Word,
    bytes: Word,
    permissions: Word,
) -> Result<(), RequestError> {
    BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        backend
            .protection_attempts
            .push((pid, address, bytes, permissions));
        assert_eq!(pid, 18);
        assert_eq!(permissions, 1);
        assert!(backend.allocations.iter().any(|(buffer, released)| {
            !released && buffer.as_ptr() as Word == address && buffer.len() == bytes
        }));
        if backend.fail_protect {
            return Err(RequestError::Transport);
        }
        Ok(())
    })
}
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
pub fn request_memory_create(bytes: Word) -> Result<Word, RequestError> {
    BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        if backend.fail_create {
            return Err(RequestError::Transport);
        }
        assert_eq!(bytes % 4096, 0);
        let mut allocation = vec![0; bytes].into_boxed_slice();
        let address = allocation.as_mut_ptr() as Word;
        backend.allocations.push((allocation, false));
        backend.reclaimable_creates += 1;
        let live = backend
            .allocations
            .iter()
            .filter(|(_, released)| !released)
            .map(|(bytes, _)| bytes.len())
            .sum();
        backend.peak_live_bytes = backend.peak_live_bytes.max(live);
        Ok(address)
    })
}
pub fn request_mapping_release(address: Word, size: Word) -> Result<(), RequestError> {
    BACKEND.with(|b| {
        let mut b = b.borrow_mut();
        b.release_attempts.push((address, size));
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
                b.read_ranges.push((out, len, offset));
                if b.fail_read {
                    return Err(RequestError::Transport);
                }
                let count = len.min(b.bytes.len().saturating_sub(offset)).min(b.short_read.unwrap_or(usize::MAX));
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
#[test]
fn program_header_fields_and_truncation_with_padded_strides() {
    fn signature(m: &elf::ElfMetadata) -> Vec<usize> {
        let mut out = vec![m.elf_type, m.entry_point, m.program_header_offset,
            m.program_header_entry_size, m.program_header_count, m.load_segment_count,
            m.has_interpreter as usize, m.interpreter_offset, m.interpreter_size,
            m.program_header_vaddr, m.tls_vaddr, m.tls_file_size, m.tls_memory_size, m.tls_align];
        for s in m.segments.iter().chain(core::iter::once(&m.first_load)) {
            out.extend([s.offset, s.virtual_address, s.file_size, s.memory_size, s.flags]);
        }
        out
    }
    for phoff in [64usize, 80, 4096] {
        for stride in [56usize, 57, 64, 128] {
            let end = phoff + 4 * stride;
            let mut bytes = vec![0xa5; end + 16];
            bytes[..64].fill(0);
            bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
            bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
            bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
            bytes[24..32].copy_from_slice(&0x400080u64.to_le_bytes());
            bytes[32..40].copy_from_slice(&(phoff as u64).to_le_bytes());
            bytes[54..56].copy_from_slice(&(stride as u16).to_le_bytes());
            bytes[56..58].copy_from_slice(&4u16.to_le_bytes());
            let size = bytes.len();
            for (i, kind) in [1u32, 6, 7, 3].into_iter().enumerate() {
                let h = &mut bytes[phoff + i * stride..phoff + i * stride + 56];
                h.fill(0);
                h[..4].copy_from_slice(&kind.to_le_bytes());
                let (flags, offset, address, file, memory, align) = match kind {
                    1 => (5u32, 0u64, 0x400000u64, size as u64, (size + 128) as u64, 4096u64),
                    6 => (4, 0, 0x401234, 0, 0, 0),
                    7 => (6, 0, 0x402000, 17, 64, 32),
                    3 => (4, end as u64, 0, 7, 0, 0),
                    _ => unreachable!(),
                };
                h[4..8].copy_from_slice(&flags.to_le_bytes());
                for (at, value) in [(8, offset), (16, address), (32, file), (40, memory), (48, align)] {
                    h[at..at + 8].copy_from_slice(&value.to_le_bytes());
                }
            }
            bytes[end..end + 7].copy_from_slice(b"/ld.so\0");
            let mut expected = vec![2, 0x400080, phoff, stride, 4, 1, 1, end, 7,
                0x401234, 0x402000, 17, 64, 32];
            expected.extend([0, 0x400000, size, size + 128, 5]);
            expected.extend([0; 15 * 5]);
            expected.extend([0, 0x400000, size, size + 128, 5]);
            assert_eq!(signature(&elf::parse_elf64_header(&bytes).unwrap()), expected);
            assert_eq!(signature(&elf::parse_elf64_image(&bytes).unwrap()), expected);
            for cut in 0..end {
                assert!(elf::parse_elf64_header(&bytes[..cut]).is_err(), "phoff={phoff} stride={stride} cut={cut}");
            }
            assert_eq!(signature(&elf::parse_elf64_header(&bytes[..end]).unwrap()), expected);
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
fn cold_snapshot_fills_use_the_whole_transfer_window_and_resume_short_reads() {
    for capacity in [256, 4096, 65536] {
        for size in [capacity - 1, capacity, capacity + 1, 2 * capacity, 16 * capacity] {
            for limit in [None, Some(73), Some(capacity - 17)] {
                let mut shm = vec![0xa5; capacity];
                let mut r = fixture(&mut shm);
                let expected = elf_bytes(size);
                BACKEND.with(|b| {
                    let mut b = b.borrow_mut();
                    b.bytes = expected.clone();
                    b.short_read = limit;
                });
                let loaded = load(&mut r);
                assert_eq!(loaded.size, size);
                assert_eq!(unsafe { core::slice::from_raw_parts(loaded.address as *const u8, size) }, expected);
                BACKEND.with(|b| {
                    let b = b.borrow();
                    assert_eq!((b.opens, b.closes), (1, 1));
                    let effective = capacity.min(limit.unwrap_or(usize::MAX));
                    assert_eq!(b.reads, size.div_ceil(effective));
                    let mut copied = 0;
                    for &(out, requested, offset) in &b.read_ranges {
                        assert_eq!((out, requested, offset), (0, capacity.min(size - copied), copied));
                        copied += requested.min(effective);
                    }
                    assert_eq!(copied, size);
                    if limit.is_none() && size == capacity {
                        assert_eq!(b.reads, 1);
                        assert_eq!(size.div_ceil(capacity - abi::ALTER_IO_OFFSET), 2);
                    }
                });
            }
        }
    }
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
        loader::load_interpreter_elf_image(&mut r, b"/alias", first.address)
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
fn generation_pressure_retires_an_old_same_inode_before_an_unrelated_library() {
    let mut shm = [0; 4096];
    let mut r = fixture(&mut shm);
    let library_identity = 1 << 32 | 99;
    BACKEND.with(|b| b.borrow_mut().identity = library_identity);
    let library = load(&mut r);
    for version in 1..image_cache::ENTRIES {
        BACKEND.with(|b| b.borrow_mut().identity = version << 32 | 7);
        load(&mut r);
    }
    BACKEND.with(|b| b.borrow_mut().identity = image_cache::ENTRIES << 32 | 7);
    load(&mut r);
    assert_eq!(r.image_cache.entry(0).unwrap().identity, library_identity);
    assert_eq!(r.image_cache.entry(0).unwrap().address, library.address);
    assert_eq!(r.image_cache.entry(1).unwrap().identity, image_cache::ENTRIES << 32 | 7);
    let reads = BACKEND.with(|b| b.borrow().reads);
    BACKEND.with(|b| b.borrow_mut().identity = library_identity);
    assert_eq!(load(&mut r).address, library.address);
    assert_eq!(BACKEND.with(|b| b.borrow().reads), reads);
    assert!(r.image_cache.retained_bytes() <= image_cache::MAX_RETAINED_BYTES);
}

#[test]
fn generation_victims_keep_empty_capacity_protected_main_and_lru_within_the_group() {
    let mut cache = image_cache::ImageCache::new();
    cache.insert(0, 1 << 32 | 7, 0x10000, 4096, 4096, None);
    assert_eq!(cache.replacement(4096, None, 2 << 32 | 7), Some(1));
    for index in 1..image_cache::ENTRIES {
        let identity = if index == 1 { 2 << 32 | 7 } else { 1 << 32 | (index + 100) };
        cache.insert(index, identity, 0x10000 + index * 4096, 4096, 4096, None);
    }
    cache.get(1 << 32 | 7, 4096).unwrap(); // The other same-inode entry is older.
    assert_eq!(cache.replacement(4096, None, 3 << 32 | 7), Some(1));
    assert_eq!(cache.replacement(4096, Some(0x11000), 3 << 32 | 7), Some(0));
    assert_eq!(cache.replacement(4096, None, 0), Some(1));
    assert_eq!(cache.replacement(4096, None, 4 << 32 | 999), Some(1));
    cache.invalidate(0); // A recent failed owner must not precede older victims.
    assert_eq!(cache.replacement(4096, None, 3 << 32 | 7), Some(1));
    assert_eq!(cache.replacement(4096, None, 4 << 32 | 999), Some(1));
}

#[test]
fn generation_preference_never_overrides_rounded_byte_budget() {
    let mut cache = image_cache::ImageCache::new();
    for index in 0..8 {
        cache.insert(index, 1 << 32 | (index + 7), 0x1000000 + index * 0x200000,
            image_cache::MAX_IMAGE_BYTES, image_cache::MAX_IMAGE_BYTES, None);
    }
    assert_eq!(cache.replacement_mapping(3514168, true, 2 << 32 | 7), None);
    assert_eq!(cache.retained_bytes(), image_cache::MAX_RETAINED_BYTES);
}
#[test]
fn lru_eviction_keeps_the_current_main_image_alive() {
    let mut shm = [0; 4096];
    let mut r = fixture(&mut shm);
    let first = load(&mut r);
    for version in 2..=image_cache::ENTRIES {
        BACKEND.with(|b| b.borrow_mut().identity = version << 32 | 7);
        load(&mut r);
    }
    BACKEND.with(|b| b.borrow_mut().identity = 1 << 32 | 7);
    assert_eq!(load(&mut r).address, first.address);
    BACKEND.with(|b| b.borrow_mut().identity = (image_cache::ENTRIES + 1) << 32 | 7);
    loader::load_interpreter_elf_image(&mut r, b"/ld", first.address)
        .ok()
        .unwrap();
    BACKEND.with(|b| {
        let b = b.borrow();
        assert!(!b.allocations[0].1);
        assert_eq!(b.allocations[1].1, image_cache::IMMUTABLE_STORAGE);
        let expected_allocations = if image_cache::IMMUTABLE_STORAGE {
            image_cache::ENTRIES + 1
        } else {
            image_cache::ENTRIES
        };
        assert_eq!(b.allocations.len(), expected_allocations);
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
fn small_library_working_set_above_eight_entries_remains_resident() {
    let mut shm = [0; 4096];
    let mut r = fixture(&mut shm);
    let mut addresses = Vec::new();
    for version in 1..=32 {
        BACKEND.with(|b| {
            let mut b = b.borrow_mut();
            b.identity = version << 32 | 7;
            b.bytes[8000] = version as u8;
        });
        addresses.push(load(&mut r).address);
    }
    let reads = BACKEND.with(|b| b.borrow().reads);
    for (index, address) in addresses.into_iter().enumerate() {
        BACKEND.with(|b| b.borrow_mut().identity = (index + 1) << 32 | 7);
        assert_eq!(load(&mut r).address, address);
        assert_eq!(
            unsafe { *((address + 8000) as *const u8) },
            (index + 1) as u8
        );
    }
    assert_eq!(BACKEND.with(|b| b.borrow().reads), reads);
    assert!(r.image_cache.retained_bytes() <= image_cache::MAX_RETAINED_BYTES);
    assert!(std::mem::size_of::<image_cache::ImageCache>() <= 64 * 1024);
}

#[test]
fn large_library_churn_cannot_expand_the_original_snapshot_byte_budget() {
    let mut shm = [0; 65536];
    let mut r = fixture(&mut shm);
    BACKEND.with(|b| b.borrow_mut().bytes = elf_bytes(image_cache::MAX_IMAGE_BYTES));
    for version in 1..=image_cache::ENTRIES + 5 {
        BACKEND.with(|b| b.borrow_mut().identity = version << 32 | 7);
        load(&mut r);
        assert!(r.image_cache.retained_bytes() <= image_cache::MAX_RETAINED_BYTES);
    }
    BACKEND.with(|b| {
        let b = b.borrow();
        let expected_allocations = if image_cache::IMMUTABLE_STORAGE {
            image_cache::ENTRIES + 5
        } else {
            image_cache::MAX_RETAINED_BYTES / image_cache::MAX_IMAGE_BYTES
        };
        assert_eq!(b.allocations.len(), expected_allocations);
        assert_eq!(
            b.allocations
                .iter()
                .filter(|(_, released)| !released)
                .count(),
            image_cache::MAX_RETAINED_BYTES / image_cache::MAX_IMAGE_BYTES
        );
        assert!(b.peak_live_bytes <= image_cache::MAX_RETAINED_BYTES);
    });
}

#[test]
fn interpreter_falls_back_instead_of_overwriting_the_only_fitting_main_buffer() {
    let mut shm = [0; 65536];
    let mut r = fixture(&mut shm);
    BACKEND.with(|b| {
        let mut b = b.borrow_mut();
        b.bytes = elf_bytes(image_cache::MAX_IMAGE_BYTES);
        b.bytes[8000] = 0x5a;
    });
    let main = load(&mut r);
    let remaining_pages = (image_cache::MAX_RETAINED_BYTES - image_cache::MAX_IMAGE_BYTES) / 4096;
    for index in 1..image_cache::ENTRIES {
        let pages = if index == image_cache::ENTRIES - 1 {
            remaining_pages
                - (image_cache::ENTRIES - 2) * (remaining_pages / (image_cache::ENTRIES - 1))
        } else {
            remaining_pages / (image_cache::ENTRIES - 1)
        };
        // Metadata-only cold entries. None can admit the largest cacheable
        // interpreter at a full budget; the live main ELF is the only fit.
        r.image_cache
            .insert(index, (index + 1) << 32 | 9, index, 1, pages * 4096, None);
    }
    assert_eq!(
        r.image_cache.retained_bytes(),
        image_cache::MAX_RETAINED_BYTES
    );
    assert_eq!(
        r.image_cache
            .replacement(image_cache::MAX_IMAGE_BYTES, None, 0),
        Some(0)
    );
    assert_eq!(
        r.image_cache
            .replacement(image_cache::MAX_IMAGE_BYTES, Some(main.address), 0),
        None
    );
    BACKEND.with(|b| {
        let mut b = b.borrow_mut();
        b.identity = 100 << 32 | 7;
        b.bytes[8000] = 0x42;
    });
    let interpreter = loader::load_interpreter_elf_image(&mut r, b"/ld", main.address)
        .unwrap_or_else(|_| panic!("uncached interpreter load failed"));
    assert_ne!(interpreter.address, main.address);
    assert_eq!(unsafe { *((main.address + 8000) as *const u8) }, 0x5a);
    assert_eq!(
        unsafe { *((interpreter.address + 8000) as *const u8) },
        0x42
    );
    assert_eq!(
        r.image_cache.retained_bytes(),
        image_cache::MAX_RETAINED_BYTES
    );
    BACKEND.with(|b| assert_eq!(b.borrow().allocations.len(), 2));
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
        let expected_allocations = if image_cache::IMMUTABLE_STORAGE {
            100
        } else {
            image_cache::ENTRIES
        };
        assert_eq!(b.allocations.len(), expected_allocations);
        assert_eq!(
            b.allocations
                .iter()
                .filter(|(_, released)| !released)
                .count(),
            image_cache::ENTRIES
        );
        assert!(b.peak_live_bytes <= image_cache::MAX_RETAINED_BYTES);
    });
}
#[test]
fn fstat_failure_closes_before_allocating() {
    let mut shm = [0; 4096];
    let mut r = fixture(&mut shm);
    BACKEND.with(|b| b.borrow_mut().fail_stat = true);
    assert!(loader::load_interpreter_elf_image(&mut r, b"/elf", 0).is_err());
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
fn cacheable_dso_snapshot_reuses_segment_reads_and_still_observes_file_versions() {
    let mut shm = [0; 65536];
    let mut runtime = fixture(&mut shm);
    let bytes = image_cache::MAX_IMAGE_BYTES - 37;
    BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        backend.bytes = elf_bytes(bytes);
        backend.bytes[65000] = 0x5a;
    });
    let first_identity = 1 << 32 | 7;
    assert_eq!(
        loader::file_mapping_source(&mut runtime, 7, first_identity, bytes, 4096, 65536)
            .unwrap_or_else(|_| panic!("partial mapping source failed")),
        None
    );
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        assert_eq!(backend.reads, 0);
        assert!(backend.allocations.is_empty());
    });
    let first = loader::file_mapping_source(&mut runtime, 7, first_identity, bytes, 0, bytes)
        .unwrap_or_else(|_| panic!("whole DSO snapshot failed"))
        .expect("whole-file snapshot within the ceiling is cacheable");
    let reads = BACKEND.with(|backend| backend.borrow().reads);
    assert_eq!(
        loader::file_mapping_source(&mut runtime, 7, first_identity, bytes, 4096, 65536)
            .unwrap_or_else(|_| panic!("warm segment failed")),
        Some(first + 4096)
    );
    assert_eq!(BACKEND.with(|backend| backend.borrow().reads), reads);
    BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        backend.identity = 2 << 32 | 7;
        backend.bytes[65000] = 0x42;
    });
    let next = loader::file_mapping_source(&mut runtime, 7, 2 << 32 | 7, bytes, 0, bytes)
        .unwrap_or_else(|_| panic!("changed DSO snapshot failed"))
        .expect("changed cacheable DSO remains cacheable");
    assert_ne!(next, first);
    assert!(BACKEND.with(|backend| backend.borrow().reads) > reads);
    assert_eq!(unsafe { *((first + 65000) as *const u8) }, 0x5a);
    assert_eq!(unsafe { *((next + 65000) as *const u8) }, 0x42);
    assert!(runtime.image_cache.retained_bytes() <= image_cache::MAX_RETAINED_BYTES);
}

#[test]
fn over_budget_file_mapping_bypasses_cache_without_prefetch_or_allocation() {
    let mut shm = [0; 65536];
    let mut runtime = fixture(&mut shm);
    let bytes = image_cache::MAX_IMAGE_BYTES + 1;
    let source = loader::file_mapping_source(&mut runtime, 7, 1 << 32 | 7, bytes, 0, bytes)
        .unwrap_or_else(|_| panic!("uncached mapping source failed"));
    assert_eq!(source, None);
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        assert_eq!(backend.reads, 0);
        assert!(backend.allocations.is_empty());
    });
}

#[test]
fn whole_mapping_uses_one_lookup_and_one_selection_and_warm_ranges_only_one_lookup() {
    let mut shm = [0; 65536]; let mut runtime = fixture(&mut shm);
    let bytes = 8193; let identity = 1 << 32 | 7;
    BACKEND.with(|b| b.borrow_mut().bytes = elf_bytes(bytes));
    image_cache::take_query_counts();
    assert!(loader::file_mapping_source(&mut runtime, 7, identity, bytes, 4096, 4096).ok().unwrap().is_none());
    assert_eq!(image_cache::take_query_counts(), (1, 0));
    BACKEND.with(|b| { let b = b.borrow(); assert_eq!(b.reads, 0); assert!(b.allocations.is_empty()); });
    let first = loader::file_mapping_source(&mut runtime, 7, identity, bytes, 0, bytes).ok().unwrap().unwrap();
    assert_eq!(image_cache::take_query_counts(), (1, 1));
    let reads = BACKEND.with(|b| b.borrow().reads);
    assert_eq!(loader::file_mapping_source(&mut runtime, 7, identity, bytes, 4096, 4096).ok().unwrap(), Some(first + 4096));
    assert_eq!(image_cache::take_query_counts(), (1, 0));
    assert_eq!(BACKEND.with(|b| b.borrow().reads), reads);
    assert!(loader::file_mapping_source(&mut runtime, 7, identity, bytes, bytes, 1).is_err());
    assert_eq!(image_cache::take_query_counts(), (0, 0));
    let next = 2 << 32 | 7;
    BACKEND.with(|b| b.borrow_mut().identity = next);
    assert!(loader::readonly_file_mapping_source(&mut runtime, 7, next, bytes, 0, bytes).ok().unwrap().is_some());
    assert_eq!(image_cache::take_query_counts(), (1, 1));
    BACKEND.with(|b| { let b = b.borrow(); assert_eq!(b.opens, 0); assert_eq!(b.closes, 0); });
}

#[test]
fn a_preselected_mapping_fill_failure_retains_ownership_without_closing_the_callers_fd() {
    let mut shm = [0; 65536]; let mut runtime = fixture(&mut shm);
    let bytes = 8193; let identity = 1 << 32 | 7;
    BACKEND.with(|b| { let mut b = b.borrow_mut(); b.bytes = elf_bytes(bytes); b.fail_read = true; });
    image_cache::take_query_counts();
    assert!(loader::readonly_file_mapping_source(&mut runtime, 7, identity, bytes, 0, bytes).is_err());
    assert_eq!(image_cache::take_query_counts(), (1, 1));
    let owner = runtime.image_cache.entry(0).expect("failed fill remains owned");
    assert_eq!((owner.identity, owner.size), (0, 0));
    assert_eq!(owner.capacity, bytes.next_multiple_of(4096));
    let address = owner.address;
    assert!(runtime.image_cache.get(identity, bytes).is_none());
    BACKEND.with(|b| { let b = b.borrow(); assert_eq!(b.opens, 0); assert_eq!(b.closes, 0);
        assert!(b.allocations.iter().any(|(buffer, released)| !released && buffer.as_ptr() as Word == address)); });
}

#[test]
fn readonly_almost_whole_mapping_fills_one_tail_page_and_reuses_segments() {
    if !image_cache::IMMUTABLE_STORAGE { return; }
    for tail in [1, 80, 216, 752, 1440, 4095] {
        let mut shm = [0; 65536];
        let mut runtime = fixture(&mut shm);
        let prefix = 98304;
        let size = prefix + tail;
        let identity = 1 << 32 | 7;
        let bytes = elf_bytes(size);
        BACKEND.with(|b| b.borrow_mut().bytes = bytes.clone());
        let source = loader::readonly_file_mapping_source(&mut runtime, 7, identity, size, 0, prefix)
            .unwrap_or_else(|_| panic!("tail {tail} fill failed")).expect("bounded readonly tail");
        assert_eq!(unsafe { core::slice::from_raw_parts(source as *const u8, size) }, &bytes);
        let capacity = size.next_multiple_of(4096);
        assert!(unsafe { core::slice::from_raw_parts((source + size) as *const u8, capacity - size) }
            .iter().all(|byte| *byte == 0));
        assert!(runtime.image_cache.immutable_range(source, capacity));
        let reads = BACKEND.with(|b| {
            let b = b.borrow();
            assert_eq!(b.read_ranges.iter().map(|&(_, n, _)| n).sum::<usize>(), size);
            assert_eq!(b.reads, size.div_ceil(shm.len()));
            b.reads
        });
        for (offset, count) in [(4096, 65536), (prefix, tail)] {
            assert_eq!(loader::readonly_file_mapping_source(&mut runtime, 7, identity, size, offset, count)
                .unwrap_or_else(|_| panic!("warm read failed")), Some(source + offset));
        }
        BACKEND.with(|b| assert_eq!(b.borrow().reads, reads));
        assert!(runtime.image_cache.retained_bytes() <= image_cache::MAX_RETAINED_BYTES);
    }
}

#[test]
fn readonly_tail_admission_rejects_extra_pages_probes_offsets_and_unversioned_files() {
    for (size, offset, bytes, identity, readonly) in [
        (12288, 0, 8192, 1 << 32 | 7, true),
        (8193, 0, 8191, 1 << 32 | 7, true),
        (8193, 4096, 4096, 1 << 32 | 7, true),
        (4096, 0, 1, 1 << 32 | 7, true),
        (8193, 0, 8192, 7, true),
        (8193, 0, 8192, 1 << 32 | 7, false),
        (image_cache::MAX_RETAINED_BYTES + 1, 0, image_cache::MAX_RETAINED_BYTES, 1 << 32 | 7, true),
    ] {
        let mut shm = [0; 4096];
        let mut runtime = fixture(&mut shm);
        let f = if readonly { loader::readonly_file_mapping_source } else { loader::file_mapping_source };
        assert_eq!(f(&mut runtime, 7, identity, size, offset, bytes)
            .unwrap_or_else(|_| panic!("rejection failed")), None);
        BACKEND.with(|b| { let b = b.borrow(); assert_eq!(b.reads, 0); assert!(b.allocations.is_empty()); });
    }
}

#[test]
fn readonly_tail_fill_errors_and_changed_versions_never_publish() {
    if !image_cache::IMMUTABLE_STORAGE { return; }
    for change in [false, true] {
        let mut shm = [0; 4096];
        let mut runtime = fixture(&mut shm);
        let identity = 1 << 32 | 7;
        BACKEND.with(|b| {
            let mut b = b.borrow_mut(); b.bytes = elf_bytes(8193);
            b.change_during_read = change; b.fail_read = !change;
        });
        assert!(loader::readonly_file_mapping_source(&mut runtime, 7, identity, 8193, 0, 8192).is_err());
        assert!(runtime.image_cache.get(identity, 8193).is_none());
        assert_eq!(runtime.image_cache.entry(0).unwrap().identity, 0);
        BACKEND.with(|b| assert!(b.borrow().protection_attempts.is_empty()));
    }
}

#[test]
fn large_readonly_whole_mapping_uses_immutable_budget_without_prefetching_partial_or_writable_maps() {
    let mut shm = [0; 65536];
    let mut runtime = fixture(&mut shm);
    let bytes = 3514168;
    let identity = 1 << 32 | 7;
    BACKEND.with(|b| b.borrow_mut().bytes = elf_bytes(bytes));
    assert_eq!(loader::readonly_file_mapping_source(&mut runtime, 7, identity, bytes, 4096, 65536)
        .unwrap_or_else(|_| panic!("partial source failed")), None);
    assert_eq!(loader::file_mapping_source(&mut runtime, 7, identity, bytes, 0, bytes)
        .unwrap_or_else(|_| panic!("writable source failed")), None);
    BACKEND.with(|b| { let b = b.borrow(); assert_eq!(b.reads, 0); assert!(b.allocations.is_empty()); });
    let source = loader::readonly_file_mapping_source(&mut runtime, 7, identity, bytes, 0, bytes)
        .unwrap_or_else(|_| panic!("readonly source failed"));
    if !image_cache::IMMUTABLE_STORAGE {
        assert_eq!(source, None);
        BACKEND.with(|b| assert_eq!(b.borrow().reads, 0));
        return;
    }
    let source = source.expect("immutable full-file mapping fits the existing total budget");
    let reads = BACKEND.with(|b| b.borrow().reads);
    assert_eq!(loader::readonly_file_mapping_source(&mut runtime, 7, identity, bytes, 4096, 65536)
        .unwrap_or_else(|_| panic!("warm segment failed")), Some(source + 4096));
    assert_eq!(loader::file_mapping_source(&mut runtime, 7, identity, bytes, 0, bytes)
        .unwrap_or_else(|_| panic!("warm writable copy source failed")), Some(source));
    BACKEND.with(|b| assert_eq!(b.borrow().reads, reads));
    assert!(runtime.image_cache.retained_bytes() <= image_cache::MAX_RETAINED_BYTES);
    let capacity = bytes.next_multiple_of(4096);
    assert!(runtime.image_cache.immutable_range(source, capacity));
    assert!(!runtime.image_cache.immutable_range(source, capacity + 4096));
    assert!(unsafe { core::slice::from_raw_parts((source + bytes) as *const u8, capacity - bytes) }
        .iter().all(|byte| *byte == 0));
}

#[test]
fn readonly_mapping_admission_never_exceeds_the_original_total_budget() {
    let mut shm = [0; 65536];
    let mut runtime = fixture(&mut shm);
    let bytes = image_cache::MAX_RETAINED_BYTES + 1;
    assert_eq!(loader::readonly_file_mapping_source(&mut runtime, 7, 1 << 32 | 7, bytes, 0, bytes)
        .unwrap_or_else(|_| panic!("budget rejection failed")), None);
    BACKEND.with(|b| { let b = b.borrow(); assert_eq!(b.reads, 0); assert!(b.allocations.is_empty()); });
    let mut cache = image_cache::ImageCache::new();
    for index in 0..8 {
        cache.insert(index, ((index + 1) << 32) | 7, 0x10000 + index * 0x200000,
            image_cache::MAX_IMAGE_BYTES, image_cache::MAX_IMAGE_BYTES, None);
    }
    assert_eq!(cache.retained_bytes(), image_cache::MAX_RETAINED_BYTES);
    assert_eq!(cache.replacement_mapping(3514168, true, 0), None);
    assert_eq!(cache.retained_bytes(), image_cache::MAX_RETAINED_BYTES);
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
    assert!(loader::load_interpreter_elf_image(&mut r, b"/elf", 0).is_err());
    BACKEND.with(|b| assert_eq!(b.borrow().reads, reads));
}

fn fill_small_cache(runtime: &mut state::Runtime) -> Word {
    let mut first = 0;
    for version in 1..=image_cache::ENTRIES {
        BACKEND.with(|backend| {
            let mut backend = backend.borrow_mut();
            backend.identity = version << 32 | 7;
            backend.bytes[8000] = 0x5a;
        });
        let address = load(runtime).address;
        if version == 1 {
            first = address;
        }
    }
    BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        backend.identity = (image_cache::ENTRIES + 1) << 32 | 7;
        backend.bytes[8000] = 0x42;
    });
    first
}

#[test]
fn published_backing_is_retired_not_overwritten_by_immutable_eviction() {
    let mut shm = [0; 4096];
    let mut runtime = fixture(&mut shm);
    let first = fill_small_cache(&mut runtime);
    let replacement = load(&mut runtime);
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        let (old, retired) = &backend.allocations[0];
        assert_eq!(old.as_ptr() as Word, first);
        if image_cache::IMMUTABLE_STORAGE {
            assert!(*retired);
            // The mock retains retired bytes to model another physical owner.
            // The source VA is unmapped in production; do not read it there.
            // A later RAM object may reuse that VA with different backing.
            assert_eq!(old[8000], 0x5a);
            assert_eq!(backend.release_attempts, [(first, old.len())]);
        } else {
            // Baseline control: the legacy unshared heap buffer is refilled.
            assert!(!*retired);
            assert_eq!(replacement.address, first);
            assert_eq!(old[8000], 0x42);
            assert!(backend.release_attempts.is_empty());
        }
    });
    assert_eq!(
        unsafe { *((replacement.address + 8000) as *const u8) },
        0x42
    );
}

#[test]
fn immutable_file_padding_is_zero_and_cache_capacity_is_page_rounded() {
    let mut shm = [0; 4096];
    let mut runtime = fixture(&mut shm);
    let image = load(&mut runtime);
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        let (bytes, released) = &backend.allocations[0];
        assert!(!released);
        assert_eq!(bytes.len(), image.size.next_multiple_of(4096));
        assert_eq!(bytes.len(), runtime.image_cache.entry(0).unwrap().capacity);
        let padding = &bytes[image.size..];
        if image_cache::IMMUTABLE_STORAGE {
            assert!(padding.iter().all(|byte| *byte == 0));
            assert_eq!(backend.reclaimable_creates, 1);
            assert_eq!(
                backend.protection_attempts,
                [(18, image.address, bytes.len(), 1)]
            );
        } else {
            // Old mmap explicitly supplies its zero suffix to Alpha; it never
            // exposes this heap-buffer padding as guest file contents.
            assert!(padding.iter().all(|byte| *byte == 0xa5));
            assert_eq!(backend.reclaimable_creates, 0);
            assert!(backend.protection_attempts.is_empty());
        }
    });
}

#[test]
fn snapshot_owner_queries_across_fills_eviction_and_cache_hits() {
    if !image_cache::IMMUTABLE_STORAGE {
        return;
    }
    let mut shm = [0; 4096];
    let mut runtime = fixture(&mut shm);
    for index in 0..64 {
        BACKEND.with(|backend| backend.borrow_mut().identity = (1 << 32) | (1000 + index));
        let first = load(&mut runtime);
        let hit = load(&mut runtime);
        assert_eq!(first.address, hit.address);
    }
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        assert_eq!(backend.protection_attempts.len(), 64);
        assert_eq!(backend.opens, backend.closes);
        println!("SNAPSHOT-OWNER fills=64 hits=64 self-pid-queries={}", backend.self_pid_calls);
        assert_eq!(backend.self_pid_calls, 1);
    });
}

#[test]
fn snapshot_owner_failure_retries_and_protection_stays_mandatory() {
    if !image_cache::IMMUTABLE_STORAGE {
        return;
    }
    let mut shm = [0; 4096];
    let mut runtime = fixture(&mut shm);
    BACKEND.with(|backend| backend.borrow_mut().fail_self_pid = true);
    assert!(loader::load_interpreter_elf_image(&mut runtime, b"/elf", 0).is_err());
    assert!(runtime.image_cache.get(1 << 32 | 7, 8193).is_none());
    assert_eq!(runtime.image_cache.entry(0).unwrap().identity, 0);
    BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        assert!(backend.protection_attempts.is_empty());
        backend.fail_self_pid = false;
    });
    load(&mut runtime);
    BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        backend.identity = (2 << 32) | 7;
        backend.fail_protect = true;
    });
    assert!(loader::load_interpreter_elf_image(&mut runtime, b"/elf", 0).is_err());
    assert!(runtime.image_cache.get(2 << 32 | 7, 8193).is_none());
    BACKEND.with(|backend| backend.borrow_mut().fail_protect = false);
    load(&mut runtime);
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        assert_eq!(backend.protection_attempts.len(), 3);
        assert_eq!(backend.opens, backend.closes);
        println!("SNAPSHOT-OWNER failure/retry/protect-failure/retry self-pid-queries={}",
            backend.self_pid_calls);
        assert_eq!(backend.self_pid_calls, 2);
    });
}

#[test]
fn immutable_protection_failure_keeps_an_unpublished_owned_snapshot() {
    let mut shm = [0; 4096];
    let mut runtime = fixture(&mut shm);
    BACKEND.with(|backend| backend.borrow_mut().fail_protect = true);
    let result = loader::load_interpreter_elf_image(&mut runtime, b"/elf", 0);
    if image_cache::IMMUTABLE_STORAGE {
        assert!(result.is_err());
        assert!(runtime.image_cache.get(1 << 32 | 7, 8193).is_none());
        let owner = runtime.image_cache.entry(0).unwrap();
        assert_eq!((owner.identity, owner.size, owner.capacity), (0, 0, 12288));
        BACKEND.with(|backend| {
            let backend = backend.borrow();
            assert!(!backend.allocations[0].1);
            assert_eq!(backend.protection_attempts.len(), 1);
            assert_eq!(backend.opens, backend.closes);
        });
        BACKEND.with(|backend| backend.borrow_mut().fail_protect = false);
        load(&mut runtime);
        assert!(runtime.image_cache.get(1 << 32 | 7, 8193).is_some());
    } else {
        assert!(result.is_ok());
        BACKEND.with(|backend| assert!(backend.borrow().protection_attempts.is_empty()));
    }
}

#[test]
fn failed_immutable_retirement_rejects_hits_retains_ownership_and_retries() {
    let mut shm = [0; 4096];
    let mut runtime = fixture(&mut shm);
    let first = fill_small_cache(&mut runtime);
    BACKEND.with(|backend| backend.borrow_mut().fail_release = true);
    if image_cache::IMMUTABLE_STORAGE {
        for attempt in 1..=2 {
            assert!(loader::load_interpreter_elf_image(&mut runtime, b"/elf", 0).is_err());
            assert!(runtime.image_cache.get(1 << 32 | 7, 8193).is_none());
            let owner = runtime.image_cache.entry(0).unwrap();
            assert_eq!(
                (owner.identity, owner.address, owner.capacity),
                (0, first, 12288)
            );
            BACKEND.with(|backend| {
                let backend = backend.borrow();
                assert_eq!(backend.reclaimable_creates, image_cache::ENTRIES);
                assert_eq!(backend.release_attempts.len(), attempt);
                assert!(!backend.allocations[0].1);
                assert_eq!(backend.opens, backend.closes);
            });
        }
        BACKEND.with(|backend| backend.borrow_mut().fail_release = false);
        load(&mut runtime);
        BACKEND.with(|backend| {
            let backend = backend.borrow();
            assert!(backend.allocations[0].1);
            assert_eq!(backend.release_attempts.len(), 3);
            assert_eq!(backend.reclaimable_creates, image_cache::ENTRIES + 1);
        });
    } else {
        // No retirement is attempted for a same-capacity legacy refill.
        assert_eq!(load(&mut runtime).address, first);
        BACKEND.with(|backend| assert!(backend.borrow().release_attempts.is_empty()));
    }
}

#[test]
fn failed_immutable_creation_does_not_publish_or_lose_an_owner_record() {
    let mut shm = [0; 4096];
    let mut runtime = fixture(&mut shm);
    BACKEND.with(|backend| backend.borrow_mut().fail_create = true);
    if image_cache::IMMUTABLE_STORAGE {
        assert!(loader::load_interpreter_elf_image(&mut runtime, b"/elf", 0).is_err());
        assert!(runtime.image_cache.entry(0).is_none());
        BACKEND.with(|backend| {
            let backend = backend.borrow();
            assert!(backend.allocations.is_empty());
            assert_eq!(backend.opens, backend.closes);
        });
        BACKEND.with(|backend| backend.borrow_mut().fail_create = false);
        load(&mut runtime);
        BACKEND.with(|backend| assert_eq!(backend.borrow().reclaimable_creates, 1));
    } else {
        load(&mut runtime);
        BACKEND.with(|backend| assert_eq!(backend.borrow().reclaimable_creates, 0));
    }
}

#[test]
fn failed_fill_stays_owned_until_its_entry_is_selected_for_replacement() {
    let mut shm = [0; 4096];
    let mut runtime = fixture(&mut shm);
    BACKEND.with(|backend| backend.borrow_mut().fail_read = true);
    assert!(loader::load_interpreter_elf_image(&mut runtime, b"/elf", 0).is_err());
    let failed = runtime.image_cache.entry(0).unwrap().address;
    assert_eq!(runtime.image_cache.entry(0).unwrap().identity, 0);
    BACKEND.with(|backend| backend.borrow_mut().fail_read = false);
    // Empty LRU entries can be filled first; this failed owner still counts
    // against admission. Once chosen as victim it must retire, not refill.
    for version in 2..=image_cache::ENTRIES {
        BACKEND.with(|backend| backend.borrow_mut().identity = version << 32 | 7);
        load(&mut runtime);
    }
    BACKEND.with(|backend| {
        backend.borrow_mut().identity = (image_cache::ENTRIES + 1) << 32 | 7;
    });
    let image = load(&mut runtime);
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        if image_cache::IMMUTABLE_STORAGE {
            assert!(backend.allocations[0].1);
            assert_eq!(backend.reclaimable_creates, image_cache::ENTRIES + 1);
        } else {
            assert_eq!(image.address, failed);
            assert!(!backend.allocations[0].1);
        }
        assert_eq!(backend.opens, backend.closes);
    });
}

#[test]
fn immutable_mapping_admission_requires_a_live_complete_owned_range() {
    let mut cache = image_cache::ImageCache::new();
    cache.insert(0, (7 << 32) | 3, 0x10000, 5003, 8192, None);
    assert_eq!(cache.immutable_range(0x10000, 8192), image_cache::IMMUTABLE_STORAGE);
    assert_eq!(cache.immutable_range(0x11000, 4096), image_cache::IMMUTABLE_STORAGE);
    for (address, bytes) in [(0x10000, 0), (0xffff, 1), (0x10000, 8193), (usize::MAX, 2)] {
        assert!(!cache.immutable_range(address, bytes));
    }
    cache.invalidate(0);
    assert!(!cache.immutable_range(0x10000, 4096));
    cache.remove(0);
    assert!(!cache.immutable_range(0x10000, 4096));
}
