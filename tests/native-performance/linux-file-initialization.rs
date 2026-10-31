#![allow(dead_code)]
extern crate self as nanami_runtime;
use std::cell::RefCell;
type Word = usize;
const LINUX_PAGE_SIZE: usize = 4096;
const EIO: i32 = 5;
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
    fail_after: Option<usize>,
}
thread_local! { static DESTINATION: RefCell<Destination> = RefCell::default(); }
pub fn request_process_map_initialized_at(
    _: Word,
    base: Word,
    bytes: Word,
    source: Word,
    source_bytes: Word,
) -> Result<(), RequestError> {
    assert_eq!(base % 4096, 0);
    assert_eq!(bytes % 4096, 0);
    assert!(source_bytes <= bytes);
    DESTINATION.with(|state| {
        let mut state = state.borrow_mut();
        if state.fail_after == Some(state.pages.len()) {
            return Err(RequestError);
        }
        for offset in (0..bytes).step_by(4096) {
            assert!(!state.pages.iter().any(|(va, _)| *va == base + offset));
            let count = 4096.min(source_bytes.saturating_sub(offset));
            let mut page = vec![0; 4096];
            if count != 0 {
                page[..count].copy_from_slice(unsafe {
                    std::slice::from_raw_parts((source + offset) as *const u8, count)
                });
            }
            state.pages.push((base + offset, page));
        }
        Ok(())
    })
}
struct Runtime {
    data: Vec<u8>,
    short_reads: Vec<usize>,
    reads: usize,
    fail_after: Option<usize>,
}
impl Runtime {
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
        let limit = self.short_reads.get(self.reads).copied().unwrap_or(65536);
        self.reads += 1;
        let count = bytes.min(limit).min(self.data.len().saturating_sub(offset));
        Ok((count, self.data.as_ptr() as usize + offset))
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/file_mapping.rs"]
mod file_mapping;
fn run(data_len: usize, bytes: usize, mapped: usize, short_reads: Vec<usize>) {
    DESTINATION.with(|state| *state.borrow_mut() = Destination::default());
    let mut r = Runtime {
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
    )
    .unwrap();
    DESTINATION.with(|state| {
        let state = state.borrow();
        assert_eq!(state.pages.len(), mapped / 4096);
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
    run(
        3 * 1024 * 1024 + 123,
        3 * 1024 * 1024 + 123,
        3 * 1024 * 1024 + 4096,
        vec![],
    );
}
#[test]
fn arbitrary_short_reads_preserve_partial_pages_and_eof_zero_tail() {
    run(20003, 24000, 24576, vec![3, 1, 4090, 4100, 2, 8000, 67]);
    run(17, 17, 8192, vec![1; 17]);
    run(0, 0, 8192, vec![]);
}
#[test]
fn nonzero_file_offset_is_not_lost_when_buffering_partial_pages() {
    DESTINATION.with(|state| *state.borrow_mut() = Destination::default());
    let mut r = Runtime {
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
                131072
            ),
            Err(EIO)
        );
        DESTINATION.with(|state| assert_eq!(state.borrow().pages.len(), 16));
    }
}
