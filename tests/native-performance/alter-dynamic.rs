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
        pub fn add_mapping(&mut self, _: Word, base: Word, bytes: Word, rights: Word) -> bool {
            BACKEND.with_borrow_mut(|b| {
                if b.fail_track { return false; }
                b.tracked.push((base, bytes, rights));
                true
            })
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
    tracked: Vec<(Word, Word, Word)>,
    protections: Vec<(Word, Word, Word)>,
    fail_track: bool,
    fail_protect: bool,
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
pub fn request_process_mapping_protect(_: Word, base: Word, bytes: Word, rights: Word) -> Result<(), ()> {
    BACKEND.with_borrow_mut(|b| {
        b.protections.push((base, bytes, rights));
        if b.fail_protect { Err(()) } else { Ok(()) }
    })
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

    #[test]
    fn acknowledged_main_permissions_remove_four_rpcs_with_identical_vma_publication() {
        let mut segments = [segment(0, PAGE, PAGE), segment(2 * PAGE, PAGE, PAGE),
            segment(4 * PAGE, PAGE, PAGE), segment(6 * PAGE, PAGE, 2 * PAGE)];
        for (seg, flags) in segments.iter_mut().zip([4, 5, 4, 6]) { seg.flags = flags; }
        let (_storage, interpreter, main) = fixture_with_metadata(&segments);
        let mut snapshots = Vec::new();
        for installed in [false, true] {
            BACKEND.with_borrow_mut(|b| *b = Backend::default());
            assert!(track(&mut state::Runtime, 1, &main, Some(&interpreter), installed));
            snapshots.push(BACKEND.with_borrow(|b| (b.tracked.clone(), b.protections.clone())));
        }
        assert_eq!(snapshots[0].0, snapshots[1].0);
        assert_eq!(snapshots[0].1.len(), 4);
        assert!(snapshots[1].1.is_empty());
        assert_eq!(snapshots[1].0.last(), Some(&(BASE, 8 * PAGE, 7)));
        println!("INITIAL-ELF-RIGHTS main_loads=4 protect_rpcs_before=4 protect_rpcs_after=0 vmas_equal=true interpreter_unchanged=true");
    }

    #[test]
    fn initial_permission_ack_keeps_metadata_failures_and_legacy_protect_errors_authoritative() {
        let (_storage, _interpreter, main) = fixture_with_metadata(&[segment(0, PAGE, PAGE)]);
        for installed in [false, true] {
            BACKEND.with_borrow_mut(|b| { *b = Backend::default(); b.fail_track = true; });
            assert!(!track(&mut state::Runtime, 1, &main, None, installed));
            BACKEND.with_borrow(|b| assert!(b.protections.is_empty()));
        }
        BACKEND.with_borrow_mut(|b| { *b = Backend::default(); b.fail_protect = true; });
        assert!(!track(&mut state::Runtime, 1, &main, None, false));
        BACKEND.with_borrow(|b| assert_eq!(b.protections.len(), 1));
    }

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
        let (data, interpreter, _) = fixture_with_metadata(segments);
        (data, interpreter)
    }

    fn fixture_with_metadata(segments: &[LoadSegment]) -> (Vec<u8>, Interpreter, ElfMetadata) {
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
        (data, interpreter, metadata)
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

    fn check_source(source: &[u8], image: &Interpreter, segments: &[LoadSegment]) {
        let low = segments.iter().filter(|s| s.memory_size != 0)
            .map(|s| s.virtual_address & !(PAGE - 1)).min().unwrap();
        let high = segments.iter().filter(|s| s.memory_size != 0)
            .map(|s| (s.virtual_address + s.memory_size).next_multiple_of(PAGE)).max().unwrap();
        let mut expected = vec![0; high - low];
        for s in segments {
            if s.file_size != 0 {
                let start = s.virtual_address - low;
                expected[start..start+s.file_size].copy_from_slice(&source[s.offset..s.offset+s.file_size]);
            }
        }
        BACKEND.with(|backend| *backend.borrow_mut() = Backend::default());
        install(7, image).unwrap();
        BACKEND.with(|backend| {
            let backend=backend.borrow();
            assert_eq!(backend.pages.len()*PAGE,expected.len());
            for (index,expected) in expected.chunks(PAGE).enumerate() {
                assert_eq!(&backend.pages[&(BASE+index*PAGE)][..],expected);
            }
        });
    }

    fn clear_file_gaps(source: &mut [u8], segments: &[LoadSegment]) {
        let low=segments.iter().filter(|s| s.memory_size!=0)
            .map(|s|s.virtual_address & !(PAGE-1)).min().unwrap();
        let first=segments.iter().find(|s|s.file_size!=0).unwrap();
        let offset=first.offset-(first.virtual_address-low);
        let mut covered=0;
        for s in segments.iter().filter(|s|s.file_size!=0) {
            let start=s.virtual_address-low;
            source[offset+covered..offset+start].fill(0);
            covered=start+s.file_size;
        }
    }

    #[test]
    fn linear_zero_padded_interpreter_uses_one_constructor_and_no_write_requests() {
        for (low,offset) in [(0,0),(0x8000,0x1000)] {
            let mut segments=[segment(low+17, PAGE+31, PAGE+500),
                segment(low+3*PAGE+731,2*PAGE,4*PAGE),segment(low+9*PAGE,0,3*PAGE)];
            for s in &mut segments { s.offset=offset+s.virtual_address-low; }
            let (mut source,image)=fixture(&segments);
            clear_file_gaps(&mut source,&segments);
            check_source(&source,&image,&segments);
            BACKEND.with(|b| {
                let b=b.borrow(); assert_eq!((b.calls,b.initialized,b.anonymous,b.writes),(1,1,0,0));
                let copied=3*PAGE+731+2*PAGE;
                assert_eq!(b.zeroed,12*PAGE-copied);
            });
            // A failed initialized map is terminal even if it installed a prefix.
            BACKEND.with(|b| *b.borrow_mut()=Backend {fail_at:Some(0),..Backend::default()});
            assert!(matches!(install(7,&image),Err(LoadError::Io)));
            BACKEND.with(|b|assert_eq!(b.borrow().calls,1));
        }
    }

    #[test]
    fn nonzero_file_padding_and_non_linear_offsets_keep_legacy_zeroes_and_write_order() {
        let segments=[segment(0,155,3000),segment(2*PAGE+731,128,256)];
        let (mut source,image)=fixture(&segments);
        clear_file_gaps(&mut source,&segments);
        source[PAGE+7]=0x7f;
        check_source(&source,&image,&segments);
        BACKEND.with(|b| {
            let b=b.borrow();assert_eq!((b.calls,b.initialized,b.anonymous,b.writes),(3,0,1,2));
        });
        let mut second=segment(2*PAGE,128,256);second.offset=4*PAGE;
        let segments=[segment(0,155,3000),second];
        let (mut source,image)=fixture(&segments);source[155..2*PAGE].fill(0);
        check_source(&source,&image,&segments);
        BACKEND.with(|b|assert_eq!(b.borrow().anonymous,1));
    }

    #[test]
    fn generated_zero_gap_layouts_match_legacy_bytes_with_one_request() {
        let mut seed=129_u64;
        for _ in 0..256 {
            let mut segments=Vec::new();let mut end=0;
            for _ in 0..4 {
                seed=seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let start=end+(seed as usize%PAGE);
                let file=1+(seed>>19) as usize%(3*PAGE);
                segments.push(segment(start,file,file+(seed>>41) as usize%PAGE));
                end=(start+file+PAGE).next_multiple_of(PAGE);
            }
            let (mut source,image)=fixture(&segments);clear_file_gaps(&mut source,&segments);
            check_source(&source,&image,&segments);
            BACKEND.with(|b|assert_eq!(b.borrow().calls,1));
        }
    }

    #[test]
    #[ignore = "requires NANAMI_INTERPRETER_ELF artifact; work model, not wall time"]
    fn real_interpreter_initialization_work() {
        let source=std::fs::read(std::env::var("NANAMI_INTERPRETER_ELF").unwrap()).unwrap();
        let metadata=crate::elf::parse_elf64_image(&source).unwrap();
        LOADED.with(|image|*image.borrow_mut()=Some(LoadedElfImage {
            address:source.as_ptr() as Word,size:source.len(),metadata,
        }));
        let path=b"/ld.so\0";let mut main=metadata;
        main.has_interpreter=true;main.interpreter_offset=0;main.interpreter_size=path.len();main.load_segment_count=0;
        let executable=LoadedElfImage {address:path.as_ptr() as Word,size:path.len(),metadata:main};
        let image=prepare_in_root(&mut state::Runtime,&executable,b"").unwrap().unwrap();
        check_source(&source,&image,&metadata.segments[..metadata.load_segment_count]);
        let low=metadata.segments[..metadata.load_segment_count].iter().filter(|s|s.memory_size!=0)
            .map(|s|s.virtual_address & !(PAGE-1)).min().unwrap();
        let high=metadata.segments[..metadata.load_segment_count].iter().filter(|s|s.memory_size!=0)
            .map(|s|(s.virtual_address+s.memory_size).next_multiple_of(PAGE)).max().unwrap();
        let legacy_writes=metadata.segments[..metadata.load_segment_count].iter().filter(|s|s.file_size!=0).count();
        let legacy_copied:usize=metadata.segments[..metadata.load_segment_count].iter().map(|s|s.file_size).sum();
        BACKEND.with(|b| {
            let b=b.borrow();assert_eq!((b.calls,b.initialized,b.anonymous,b.writes),(1,1,0,0));
            std::println!("INTERPRETER-LINEAR-MODEL legacy-requests={} requests={} legacy-zeroed={} zeroed={} legacy-copied={} copied={} bytes-match=true",legacy_writes+1,b.calls,high-low,b.zeroed,legacy_copied,high-low-b.zeroed);
        });
    }
}
