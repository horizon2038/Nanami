#![allow(dead_code)]
extern crate self as nanami_runtime;
use std::cell::RefCell;
type Word = usize;
const LINUX_PAGE_SIZE: usize = 4096;
const EIO: i32 = 5;
mod private_memory {
    pub(super) fn with_mapping_mutation<T>(_: &super::Runtime, _: usize, _: usize, _: usize, _: bool,
        operation: impl FnOnce() -> Result<T, i32>) -> Result<T, i32> { operation() }
}
mod file_snapshot {
    pub(super) fn map_uncached_readonly(_: &mut super::Runtime, _: usize, _: usize,
        _: usize, _: usize, _: usize, _: usize, _: Option<usize>) -> Result<bool, i32> { Ok(false) }
}
#[derive(Clone, Copy)]
struct LinuxFile {
    posix_fd: usize,
}
#[derive(Debug)]
pub struct RequestError;
fn map_request_error(_: RequestError) -> i32 {
    EIO
}
#[derive(Default)]
struct Destination {
    pages: Vec<(usize, Vec<u8>)>,
    permissions: Vec<Option<Word>>,
    fail_after: Option<usize>,
    requests: Vec<(Word, Word, Word)>,
}
thread_local! { static DESTINATION: RefCell<Destination> = RefCell::default(); }
pub fn request_process_map_initialized_at(
    _: Word,
    base: Word,
    bytes: Word,
    source: Word,
    source_bytes: Word,
) -> Result<(), RequestError> {
    publish(base, bytes, source, source_bytes, None)
}
pub fn request_process_map_initialized_protected_at(
    _: Word, base: Word, bytes: Word, source: Word, source_bytes: Word, permissions: Word,
) -> Result<(), RequestError> {
    publish(base, bytes, source, source_bytes, Some(permissions))
}
fn publish(
    base: Word, bytes: Word, source: Word, source_bytes: Word, permissions: Option<Word>,
) -> Result<(), RequestError> {
    assert_eq!(base % 4096, 0);
    assert_eq!(bytes % 4096, 0);
    assert!(source_bytes <= bytes);
    DESTINATION.with(|state| {
        let mut state = state.borrow_mut();
        state.requests.push((bytes, source, source_bytes));
        if state.fail_after == Some(state.pages.len()) {
            return Err(RequestError);
        }
        for offset in (0..bytes).step_by(4096) {
            if state.fail_after == Some(state.pages.len()) {
                return Err(RequestError);
            }
            assert!(!state.pages.iter().any(|(va, _)| *va == base + offset));
            let count = 4096.min(source_bytes.saturating_sub(offset));
            let mut page = vec![0; 4096];
            if count != 0 {
                page[..count].copy_from_slice(unsafe {
                    std::slice::from_raw_parts((source + offset) as *const u8, count)
                });
            }
            state.pages.push((base + offset, page));
            state.permissions.push(permissions);
        }
        Ok(())
    })
}
struct Runtime {
    image_read_capacity: usize,
    data: Vec<u8>,
    short_reads: Vec<usize>,
    reads: usize,
    fail_after: Option<usize>,
}
impl Runtime {
    fn posix_image_read_buffer_size(&self) -> usize { self.image_read_capacity }
    fn posix_read_buffer_size(&self) -> usize {
        65536
    }
    fn pread_posix(
        &mut self,
        _: usize,
        _: usize,
        bytes: usize,
        offset: usize,
    ) -> Result<(usize, usize), RequestError> {
        if self.fail_after == Some(self.reads) {
            return Err(RequestError);
        }
        let limit = self.short_reads.get(self.reads).copied().unwrap_or(self.image_read_capacity);
        self.reads += 1;
        let count = bytes.min(limit).min(self.data.len().saturating_sub(offset));
        Ok((count, self.data.as_ptr() as usize + offset))
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/file_mapping.rs"]
mod file_mapping;
fn run(data_len: usize, bytes: usize, mapped: usize, short_reads: Vec<usize>, permissions: Option<Word>) {
    DESTINATION.with(|state| *state.borrow_mut() = Destination::default());
    let mut r = Runtime {
        image_read_capacity: 65536,
        data: (0..data_len).map(|i| (i % 239) as u8).collect(),
        short_reads,
        reads: 0,
        fail_after: None,
    };
    file_mapping::initialize(
        &mut r,
        1,
        0x10000,
        mapped,
        LinuxFile { posix_fd: 7 },
        0,
        bytes,
        permissions,
    )
    .unwrap();
    DESTINATION.with(|state| {
        let state = state.borrow();
        assert_eq!(state.pages.len(), mapped / 4096);
        assert_eq!(state.permissions, vec![permissions; mapped / 4096]);
        let actual: Vec<u8> = state
            .pages
            .iter()
            .flat_map(|(_, data)| data.iter().copied())
            .collect();
        let initialized = data_len.min(bytes);
        assert_eq!(&actual[..initialized], &r.data[..initialized]);
        assert!(actual[initialized..].iter().all(|byte| *byte == 0));
    });
}
#[test]
fn large_uncached_file_initializes_each_destination_page_once() {
    for permissions in [None, Some(1), Some(3), Some(5)] {
        run(
            3 * 1024 * 1024 + 123,
            3 * 1024 * 1024 + 123,
            3 * 1024 * 1024 + 4096,
            vec![],
            permissions,
        );
    }
}
#[test]
fn arbitrary_short_reads_preserve_partial_pages_and_eof_zero_tail() {
    for permissions in [None, Some(1), Some(2), Some(3), Some(4), Some(5), Some(6), Some(7)] {
        run(20003, 24000, 24576, vec![3, 1, 4090, 4100, 2, 8000, 67], permissions);
        run(17, 17, 8192, vec![1; 17], permissions);
        run(0, 0, 8192, vec![], permissions);
    }
}

#[test]
fn every_partial_tail_length_after_a_reused_full_buffer_keeps_zero_suffix() {
    for tail in 1..4096 {
        // The first two reads fill/publish the local page. The third reuses
        // only a prefix of that storage; old suffix bytes must never escape.
        run(4096 + tail, 4096 + tail, 12288, vec![1, 4095, tail], Some(1));
        // Leave one byte buffered after the full-page publish so the final
        // read must still reuse the tail storage, even with a direct-final path.
        run(4096 + tail, 4096 + tail, 12288, vec![1, 4096, tail], Some(1));
    }
}
#[test]
fn nonzero_file_offset_is_not_lost_when_buffering_partial_pages() {
    DESTINATION.with(|state| *state.borrow_mut() = Destination::default());
    let mut r = Runtime {
        image_read_capacity: 65536,
        data: (0..20000).map(|i| (i % 239) as u8).collect(),
        short_reads: vec![7, 4096, 100],
        reads: 0,
        fail_after: None,
    };
    file_mapping::initialize(
        &mut r,
        1,
        0x10000,
        12288,
        LinuxFile { posix_fd: 7 },
        4096,
        9000,
        Some(5),
    )
    .unwrap();
    DESTINATION.with(|state| {
        let actual: Vec<u8> = state
            .borrow()
            .pages
            .iter()
            .flat_map(|(_, p)| p.iter().copied())
            .collect();
        assert_eq!(&actual[..9000], &r.data[4096..13096]);
        assert!(actual[9000..].iter().all(|byte| *byte == 0));
    });
}
#[test]
fn read_or_mapping_failure_stops_without_a_zero_fill_fallback() {
    for failure in [false, true] {
        DESTINATION.with(|state| {
            *state.borrow_mut() = Destination {
                fail_after: failure.then_some(16),
                ..Destination::default()
            }
        });
        let mut r = Runtime {
            image_read_capacity: 65536,
            data: vec![0x77; 131072],
            short_reads: vec![],
            reads: 0,
            fail_after: (!failure).then_some(1),
        };
        assert_eq!(
            file_mapping::initialize(
                &mut r,
                1,
                0x10000,
                131072,
                LinuxFile { posix_fd: 7 },
                0,
                131072,
                Some(1),
            ),
            Err(EIO)
        );
        DESTINATION.with(|state| assert_eq!(state.borrow().pages.len(), 16));
    }
}

#[test]
fn final_read_keeps_full_bytes_permissions_and_zero_suffix_across_read_shapes() {
    for permissions in [None, Some(1), Some(2), Some(3), Some(4), Some(5), Some(6), Some(7)] {
        for (length, mapped, short_reads, label) in [
            (4133, 12288, vec![], "single"),
            (65536 + 4133, 81920, vec![], "multi"),
            (4133, 12288, vec![1, 4095, 37], "buffer-drained"),
            (4133, 12288, vec![1, 4096, 36], "buffer-retained"),
        ] {
            DESTINATION.with(|s| *s.borrow_mut() = Destination::default());
            let mut r = Runtime { image_read_capacity:65536, data: (0..length).map(|i| (i % 239) as u8).collect(),
                short_reads, reads: 0, fail_after: None };
            file_mapping::initialize(&mut r, 1, 0x10000, mapped,
                LinuxFile { posix_fd: 7 }, 0, length, permissions).unwrap();
            DESTINATION.with(|s| {
                let s = s.borrow();
                let actual: Vec<_> = s.pages.iter().flat_map(|(_, p)| p.iter().copied()).collect();
                assert_eq!(&actual[..length], &r.data);
                assert_eq!(actual.len(), mapped);
                assert!(actual[length..].iter().all(|b| *b == 0));
                assert_eq!(s.permissions, vec![permissions; mapped / 4096]);
                if permissions == Some(1) {
                    let direct = s.requests.iter().filter(|(_, source, bytes)| *bytes != 0
                        && *source >= r.data.as_ptr() as Word
                        && source.checked_add(*bytes).is_some_and(|end|
                            end <= r.data.as_ptr() as Word + r.data.len())).count();
                    println!("MMAP-FINAL label={label} requests={} direct={direct} reads={}", s.requests.len(), r.reads);
                }
            });
        }
    }
}

#[test]
fn final_read_partial_constructor_fault_keeps_prefix_and_never_retries() {
    DESTINATION.with(|s| *s.borrow_mut() = Destination {
        fail_after: Some(2), ..Destination::default()
    });
    let mut r = Runtime { image_read_capacity:65536, data: (0..4133).map(|i| (i % 239) as u8).collect(),
        short_reads: vec![], reads: 0, fail_after: None };
    assert_eq!(file_mapping::initialize(&mut r, 1, 0x10000, 12288,
        LinuxFile { posix_fd: 7 }, 0, 4133, Some(5)), Err(EIO));
    assert_eq!(r.reads, 1);
    DESTINATION.with(|s| {
        let s = s.borrow();
        assert_eq!(s.pages.len(), 2);
        assert_eq!(s.permissions, vec![Some(5); 2]);
        let actual: Vec<_> = s.pages.iter().flat_map(|(_, p)| p.iter().copied()).collect();
        assert_eq!(&actual[..4133], &r.data);
        assert!(actual[4133..].iter().all(|b| *b == 0));
        println!("MMAP-FINAL-FAULT requests={} committed-pages={}", s.requests.len(), s.pages.len());
    });
}

#[test]
fn buffered_tail_and_eof_keep_data_zero_suffix_and_each_partial_failure_prefix() {
    for permissions in [None, Some(1), Some(2), Some(3), Some(4), Some(5), Some(6), Some(7)] {
        for eof in [false, true] {
            for failure in [None, Some(0), Some(1), Some(2), Some(3), Some(4)] {
                DESTINATION.with(|s| *s.borrow_mut() = Destination {
                    fail_after: failure, ..Destination::default()
                });
                let offset = 4096;
                let available = 8192 + 37;
                let mapped = 20480;
                let requested = if eof { 16384 } else { available };
                let mut r = Runtime {
                    image_read_capacity: 65536,
                    data: (0..offset + available).map(|i| (i % 239) as u8).collect(),
                    short_reads: vec![1, 4096, 4096, 37], reads: 0, fail_after: None,
                };
                let result = file_mapping::initialize(&mut r, 1, 0x10000, mapped,
                    LinuxFile { posix_fd: 7 }, offset, requested, permissions);
                assert_eq!(result, if failure.is_some() { Err(EIO) } else { Ok(false) });
                DESTINATION.with(|s| {
                    let s = s.borrow();
                    let pages = failure.unwrap_or(mapped / 4096);
                    assert_eq!(s.pages.len(), pages);
                    assert_eq!(s.permissions, vec![permissions; pages]);
                    let actual: Vec<_> = s.pages.iter().flat_map(|(_, p)| p.iter().copied()).collect();
                    let prefix = available.min(actual.len());
                    assert_eq!(&actual[..prefix], &r.data[offset..offset + prefix]);
                    assert!(actual[prefix..].iter().all(|b| *b == 0));
                    if permissions == Some(5) {
                        println!("MMAP-BUFFERED eof={eof} failure={failure:?} requests={} pages={pages} reads={}",
                            s.requests.len(), r.reads);
                    }
                });
            }
        }
    }
}

#[test]
fn large_file_window_combines_constructor_requests_without_changing_bytes_or_rights() {
    for permissions in [None,Some(1),Some(3),Some(5)] {
        for length in [1024usize*1024,1024*1024+17] {
            DESTINATION.with(|s| *s.borrow_mut()=Destination::default());
            let mut r=Runtime {image_read_capacity:1024*1024,data:(0..length).map(|i|(i%239) as u8).collect(),short_reads:vec![],reads:0,fail_after:None};
            let mapped=length.next_multiple_of(4096);
            file_mapping::initialize(&mut r,1,0x10000,mapped,LinuxFile{posix_fd:7},0,length,permissions).unwrap();
            assert_eq!(r.reads,length.div_ceil(1024*1024));
            DESTINATION.with(|s| {let s=s.borrow();let actual:Vec<_>=s.pages.iter().flat_map(|(_,p)|p.iter().copied()).collect();
                assert_eq!(&actual[..length],&r.data);assert!(actual[length..].iter().all(|v|*v==0));assert!(s.permissions.iter().all(|p|*p==permissions));
                assert_eq!(s.requests.len(),length.div_ceil(1024*1024));std::println!("MMAP-WINDOW bytes={length} reads={} constructors={}",r.reads,s.requests.len());});
        }
    }
}

#[test]
fn large_window_stops_after_read_or_partial_constructor_failure_without_replay() {
    for read_failure in [false,true] {
        DESTINATION.with(|s|*s.borrow_mut()=Destination{fail_after:(!read_failure).then_some(17),..Destination::default()});
        let mut r=Runtime{image_read_capacity:1024*1024,data:vec![0x73;1024*1024+17],short_reads:vec![],reads:0,fail_after:read_failure.then_some(1)};
        assert_eq!(file_mapping::initialize(&mut r,1,0x10000,1024*1024+4096,LinuxFile{posix_fd:7},0,1024*1024+17,Some(5)),Err(EIO));
        DESTINATION.with(|s|{let s=s.borrow();assert_eq!(s.requests.len(),1);assert_eq!(s.pages.len(),if read_failure {256} else {17});
            assert!(s.pages.iter().all(|(_,p)|p.iter().all(|v|*v==0x73)));});
        assert_eq!(r.reads,1);
    }
}
