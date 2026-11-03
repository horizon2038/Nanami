//! Real PT_INTERP installation, with synchronous Alpha operations modeled.
#![allow(dead_code)]
extern crate self as nanami_runtime;
use std::{cell::RefCell, collections::BTreeMap};
pub type Word = usize;
const PAGE: usize = 4096;

pub mod arch {
    pub const ELF_MACHINE: u16 = 62;
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/elf.rs"]
mod elf;
mod loader {
    use super::*;
    #[derive(Debug)]
    pub enum LoadError {
        InvalidArgument,
        InvalidElf,
        UnsupportedElf,
        Io,
    }
    pub struct LoadedElfImage {
        pub address: Word,
        pub size: Word,
        pub metadata: elf::ElfMetadata,
    }
    pub fn load_interpreter_elf_image(
        _: &mut state::Runtime,
        _: &[u8],
        _: Word,
    ) -> Result<LoadedElfImage, LoadError> {
        LOADED.with(|image| image.borrow_mut().take().ok_or(LoadError::Io))
    }
}
mod state {
    use super::*;
    pub struct Runtime;
    impl Runtime {
        pub fn add_mapping(&mut self, _: Word, _: Word, _: Word, _: Word) -> bool {
            true
        }
    }
}

#[derive(Default)]
struct Backend {
    pages: BTreeMap<Word, Box<[u8; PAGE]>>,
    calls: usize,
    initialized: usize,
    anonymous: usize,
    writes: usize,
    zeroed: usize,
    fail_at: Option<usize>,
}
thread_local! {
    static BACKEND: RefCell<Backend> = RefCell::new(Backend::default());
    static LOADED: RefCell<Option<loader::LoadedElfImage>> = const { RefCell::new(None) };
}

fn allocate(base: Word, bytes: Word, source: Word, copied: Word) -> Result<(), ()> {
    assert_eq!((base | bytes) & (PAGE - 1), 0);
    assert_ne!(bytes, 0);
    assert!(copied <= bytes);
    BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        let call = backend.calls;
        backend.calls += 1;
        if backend.fail_at == Some(call) {
            return Err(());
        }
        backend.zeroed += bytes - copied;
        for offset in (0..bytes).step_by(PAGE) {
            let mut page = Box::new([0; PAGE]);
            let count = copied.saturating_sub(offset).min(PAGE);
            if count != 0 {
                let data =
                    unsafe { std::slice::from_raw_parts((source + offset) as *const u8, count) };
                page[..count].copy_from_slice(data);
            }
            assert!(backend.pages.insert(base + offset, page).is_none());
        }
        Ok(())
    })
}
pub fn request_process_map_anonymous_at(
    _: Word,
    base: Word,
    bytes: Word,
) -> Result<(Word, Word), ()> {
    BACKEND.with(|backend| backend.borrow_mut().anonymous += 1);
    allocate(base, bytes, 0, 0)?;
    Ok((base, bytes))
}
pub fn request_process_map_initialized_at(
    _: Word,
    base: Word,
    bytes: Word,
    source: Word,
    copied: Word,
) -> Result<(), ()> {
    BACKEND.with(|backend| backend.borrow_mut().initialized += 1);
    allocate(base, bytes, source, copied)
}
pub fn request_process_memory_write(
    _: Word,
    base: Word,
    source: Word,
    bytes: Word,
) -> Result<(), ()> {
    BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        let call = backend.calls;
        backend.calls += 1;
        if backend.fail_at == Some(call) {
            return Err(());
        }
        backend.writes += 1;
        let input = unsafe { std::slice::from_raw_parts(source as *const u8, bytes) };
        for (offset, byte) in input.iter().enumerate() {
            let address = base + offset;
            backend.pages.get_mut(&(address & !(PAGE - 1))).unwrap()[address % PAGE] = *byte;
        }
        Ok(())
    })
}
pub fn request_process_mapping_protect(_: Word, _: Word, _: Word, _: Word) -> Result<(), ()> {
    Ok(())
}

#[path = "../../nanami/servers/apps/alter/shared/src/common/dynamic.rs"]
mod dynamic;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        dynamic::*,
        elf::{ElfMetadata, LoadSegment},
        loader::{LoadError, LoadedElfImage},
    };
    const BASE: Word = 0x6000_0000;

    fn segment(start: Word, file: Word, memory: Word) -> LoadSegment {
        LoadSegment {
            offset: start,
            virtual_address: start,
            file_size: file,
            memory_size: memory,
            flags: 7,
        }
    }

    fn fixture(segments: &[LoadSegment]) -> (Vec<u8>, Interpreter) {
        let empty = segment(0, 0, 0);
        let mut loads = [empty; crate::elf::MAX_LOAD_SEGMENTS];
        loads[..segments.len()].copy_from_slice(segments);
        let mut metadata = ElfMetadata {
            elf_type: 3,
            entry_point: 0,
            program_header_offset: 64,
            program_header_entry_size: 56,
            program_header_count: segments.len(),
            load_segment_count: segments.len(),
            has_interpreter: false,
            interpreter_offset: 0,
            interpreter_size: 0,
            segments: loads,
            first_load: loads[0],
            program_header_vaddr: 0,
            tls_vaddr: 0,
            tls_file_size: 0,
            tls_memory_size: 0,
            tls_align: 0,
        };
        let low = segments
            .iter()
            .filter(|s| s.memory_size != 0)
            .map(|s| s.virtual_address & !(PAGE - 1))
            .min()
            .unwrap();
        let bytes = segments
            .iter()
            .map(|s| s.offset + s.file_size)
            .max()
            .unwrap()
            .max(64);
        let data: Vec<u8> = (0..bytes)
            .map(|n| ((n * 31 + n / 17) % 255 + 1) as u8)
            .collect();
        metadata.entry_point = low;
        LOADED.with(|image| {
            *image.borrow_mut() = Some(LoadedElfImage {
                address: data.as_ptr() as Word,
                size: data.len(),
                metadata,
            });
        });
        let path = b"/ld.so\0";
        let mut main = metadata;
        main.has_interpreter = true;
        main.interpreter_offset = 0;
        main.interpreter_size = path.len();
        main.load_segment_count = 0;
        let executable = LoadedElfImage {
            address: path.as_ptr() as Word,
            size: path.len(),
            metadata: main,
        };
        let interpreter = prepare_in_root(&mut state::Runtime, &executable, b"")
            .unwrap()
            .unwrap();
        (data, interpreter)
    }

    fn compare_legacy_contents(segments: &[LoadSegment]) {
        let (source, image) = fixture(segments);
        let low = segments
            .iter()
            .filter(|s| s.memory_size != 0)
            .map(|s| s.virtual_address & !(PAGE - 1))
            .min()
            .unwrap();
        let high = segments
            .iter()
            .filter(|s| s.memory_size != 0)
            .map(|s| (s.virtual_address + s.memory_size).next_multiple_of(PAGE))
            .max()
            .unwrap();
        let size = high - low;
        let mut expected = vec![0; size];
        // Independent old rule: zero the complete span, then write each
        // file segment in program-header order. BSS never erases a write.
        for segment in segments {
            if segment.file_size == 0 {
                continue;
            }
            let start = segment.virtual_address - low;
            expected[start..start + segment.file_size]
                .copy_from_slice(&source[segment.offset..segment.offset + segment.file_size]);
        }
        BACKEND.with(|backend| *backend.borrow_mut() = Backend::default());
        install(7, &image).unwrap();
        BACKEND.with(|backend| {
            let backend = backend.borrow();
            assert_eq!(backend.pages.len() * PAGE, size);
            for (index, expected) in expected.chunks(PAGE).enumerate() {
                assert_eq!(&backend.pages[&(BASE + index * PAGE)][..], expected);
            }
        });
    }

    #[test]
    fn semantic_aligned_unaligned_gaps_bss_and_short_head() {
        compare_legacy_contents(&[
            segment(0, 155, 3000),
            segment(2 * PAGE + 731, 128, 256),
            segment(4 * PAGE + 137, 3 * PAGE + 200, 5 * PAGE),
            segment(11 * PAGE, 0, PAGE + 1),
            segment(15 * PAGE, 0, 0),
        ]);
    }

    #[test]
    fn semantic_shared_boundary_and_overlapping_file_writes_keep_header_order() {
        let mut second = segment(PAGE + 700, 200, PAGE + 500);
        second.offset = 7 * PAGE + 700;
        compare_legacy_contents(&[segment(0, 3 * PAGE, 4 * PAGE), second]);
        BACKEND.with(|backend| assert_eq!(backend.borrow().anonymous, 1));
    }

    #[test]
    fn semantic_distinct_file_bytes_sharing_one_page_and_fileless_bss() {
        let mut second = segment(2048, 300, 1000);
        second.offset = 5 * PAGE + 2048;
        compare_legacy_contents(&[segment(0, 200, 200), second]);
        compare_legacy_contents(&[segment(PAGE + 51, 0, 3 * PAGE)]);
    }

    #[test]
    fn semantic_unsorted_segments_use_the_existing_path() {
        compare_legacy_contents(&[segment(3 * PAGE, 100, PAGE), segment(0, 200, PAGE)]);
        BACKEND.with(|backend| assert_eq!(backend.borrow().anonymous, 1));
    }

    #[test]
    fn semantic_generated_disjoint_layouts_match_every_byte() {
        let mut seed = 31u64;
        for _ in 0..256 {
            let mut segments = Vec::new();
            let mut end = 0;
            for _ in 0..4 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let start = end + (seed as Word % 3) * PAGE + (seed >> 32) as Word % PAGE;
                let memory = 1 + (seed >> 17) as Word % (3 * PAGE);
                let file = (seed >> 11) as Word % (memory + 1);
                let mut load = segment(start, file, memory);
                load.offset = ((seed >> 8) as Word % 6) * PAGE + start % PAGE;
                segments.push(load);
                end = (start + memory).next_multiple_of(PAGE);
            }
            compare_legacy_contents(&segments);
        }
    }

    #[test]
    #[ignore = "work-count comparison, not a correctness or wall-time benchmark"]
    fn interpreter_initialization_work() {
        let segments = [
            segment(0, 3 * PAGE + 17, 4 * PAGE),
            segment(5 * PAGE + 731, 2 * PAGE, 3 * PAGE),
        ];
        compare_legacy_contents(&segments);
        BACKEND.with(|backend| {
            let backend = backend.borrow();
            std::println!(
                "INTERPRETER-WORK requests={} anonymous={} initialized={} writes={} zeroed-bytes={}",
                backend.calls, backend.anonymous, backend.initialized,
                backend.writes, backend.zeroed,
            );
        });
    }

    #[test]
    fn failure_stops_before_any_later_mapping_request() {
        let (_source, image) = fixture(&[
            segment(0, PAGE, PAGE),
            segment(3 * PAGE + 137, 2 * PAGE, 4 * PAGE),
        ]);
        BACKEND.with(|backend| *backend.borrow_mut() = Backend::default());
        install(7, &image).unwrap();
        let calls = BACKEND.with(|backend| backend.borrow().calls);
        for fail_at in 0..calls {
            BACKEND.with(|backend| {
                *backend.borrow_mut() = Backend {
                    fail_at: Some(fail_at),
                    ..Backend::default()
                }
            });
            assert!(matches!(install(7, &image), Err(LoadError::Io)));
            BACKEND.with(|backend| assert_eq!(backend.borrow().calls, fail_at + 1));
        }
    }
}
