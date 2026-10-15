//! Production VFS image loading with only service/Alpha requests mocked.
#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_runtime;

pub use std::println;
use std::{cell::RefCell, collections::VecDeque};
pub type Word = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    InvalidArgument,
    Protocol,
    Transport,
}

const PATH_OFFSET: usize = 0;
const FILE_CHUNK_OFFSET: usize = 0;
#[path = "../../nanami/servers/core-services/system-manager/src/image_loader.rs"]
mod image_loader;

struct VfsClient {
    port: Word,
    shm: Word,
    shm_size: Word,
    image: image_loader::ImageBuffer,
}

#[derive(Default)]
struct Backend {
    file: Vec<u8>,
    kind: Word,
    shm: Word,
    heap_requests: Vec<Word>,
    mappings: Vec<Box<[u8]>>,
    heap_result: Option<Result<(Word, Word), RequestError>>,
    open_error: Option<RequestError>,
    reads: Vec<(Word, Word)>,
    read_results: VecDeque<Result<Word, RequestError>>,
    opens: usize,
    closes: usize,
}

thread_local! {
    static BACKEND: RefCell<Backend> = RefCell::new(Backend::default());
}

fn client(shm: &mut [u8]) -> VfsClient {
    BACKEND.with(|backend| {
        *backend.borrow_mut() = Backend {
            kind: vfs::VFS_FILE_TYPE_REGULAR,
            shm: shm.as_mut_ptr() as Word,
            ..Backend::default()
        };
    });
    VfsClient {
        port: 1,
        shm: shm.as_mut_ptr() as Word,
        shm_size: shm.len(),
        image: image_loader::ImageBuffer::new(),
    }
}

fn set_file(size: usize, seed: u8) -> Vec<u8> {
    let file: Vec<u8> = (0..size).map(|i| (i as u8).wrapping_add(seed)).collect();
    BACKEND.with(|backend| backend.borrow_mut().file = file.clone());
    file
}

pub fn request_heap(size: Word) -> Result<(Word, Word), RequestError> {
    BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        backend.heap_requests.push(size);
        if let Some(result) = backend.heap_result.take() {
            return result;
        }
        let capacity = size.next_multiple_of(4096);
        let mut mapping = vec![0xa5; capacity + 2].into_boxed_slice();
        let base = unsafe { mapping.as_mut_ptr().add(1) } as Word;
        backend.mappings.push(mapping);
        Ok((base, capacity))
    })
}

pub mod vfs {
    use super::*;
    pub const VFS_FILE_TYPE_REGULAR: Word = 1;
    pub fn vfs_stat(_: Word, _: Word, _: Word) -> Result<(Word, Word, Word), RequestError> {
        BACKEND.with(|backend| {
            let backend = backend.borrow();
            Ok((0, backend.file.len(), backend.kind))
        })
    }
    pub fn vfs_open(_: Word, _: Word, _: Word) -> Result<Word, RequestError> {
        BACKEND.with(|backend| {
            let mut backend = backend.borrow_mut();
            if let Some(error) = backend.open_error {
                return Err(error);
            }
            backend.opens += 1;
            Ok(7)
        })
    }
    pub fn vfs_read(
        _: Word,
        handle: Word,
        offset: Word,
        len: Word,
        destination: Word,
    ) -> Result<Word, RequestError> {
        assert_eq!(handle, 7);
        BACKEND.with(|backend| {
            let mut backend = backend.borrow_mut();
            backend.reads.push((offset, len));
            let result = backend.read_results.pop_front().unwrap_or(Ok(len));
            if let Ok(bytes) = result {
                if bytes <= len {
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            backend.file.as_ptr().add(offset),
                            (backend.shm + destination) as *mut u8,
                            bytes,
                        );
                    }
                }
            }
            result
        })
    }
    pub fn vfs_close(_: Word, handle: Word) -> Result<(), RequestError> {
        assert_eq!(handle, 7);
        BACKEND.with(|backend| backend.borrow_mut().closes += 1);
        Ok(())
    }
}

pub mod diagnostics {
    #[derive(Clone, Copy)]
    pub enum RpcStage {
        LoadStat,
        LoadHeap,
        LoadOpen,
        LoadRead,
        LoadCopy,
        LoadClose,
    }
    pub fn measure<T>(_: RpcStage, _: usize, operation: impl FnOnce() -> T) -> T {
        operation()
    }
    pub fn stamp() -> usize {
        0
    }
    pub fn rpc(_: RpcStage, _: usize, _: usize, _: usize, _: bool) {}
}
fn log_spawn_stage(_: &str, error: RequestError) -> RequestError {
    error
}

fn load<const TRACE: bool>(vfs: &mut VfsClient) -> (Word, Vec<u8>) {
    let (base, size) = image_loader::load::<TRACE>(vfs, 4).unwrap();
    let bytes = unsafe { std::slice::from_raw_parts(base as *const u8, size) }.to_vec();
    BACKEND.with(|backend| {
        for mapping in &backend.borrow().mappings {
            assert_eq!(mapping[0], 0xa5);
            assert_eq!(mapping[mapping.len() - 1], 0xa5);
        }
    });
    (base, bytes)
}

#[test]
fn repeated_boot_and_exec_images_reuse_page_rounded_capacity() {
    let mut shm = [0; 123];
    let mut vfs = client(&mut shm);
    let first = set_file(4097, 1);
    let (base, snapshot) = load::<true>(&mut vfs);
    assert_eq!(snapshot, first);
    for i in 0..100 {
        let expected = set_file(8192 - i, i as u8);
        let (next, actual) = load::<false>(&mut vfs);
        assert_eq!(next, base);
        assert_eq!(actual, expected);
    }
    assert_eq!(snapshot, first); // The completed spawn's private snapshot is unchanged.
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        assert_eq!(backend.heap_requests, [4097]);
        assert_eq!(backend.opens, 101);
        assert_eq!(backend.closes, 101);
    });
}

#[test]
fn grows_only_for_images_larger_than_current_capacity() {
    let mut shm = [0; 4096];
    let mut vfs = client(&mut shm);
    for size in [
        0xc768, 0xe180, 0xcf98, 0x692e38, 0xe3c0, 0x1d330, 0xe540, 0x15878, 0x64078, 0x644b8,
        0xaa3b0,
    ] {
        let expected = set_file(size, size as u8);
        assert_eq!(load::<false>(&mut vfs).1, expected);
    }
    BACKEND.with(|backend| {
        assert_eq!(backend.borrow().heap_requests, [0xc768, 0xe180, 0x692e38]);
    });
}

#[test]
fn partial_reads_advance_by_returned_bytes_and_close_once() {
    let mut shm = [0; 8];
    let mut vfs = client(&mut shm);
    let expected = set_file(19, 3);
    BACKEND.with(|backend| backend.borrow_mut().read_results.extend([Ok(3), Ok(2)]));
    assert_eq!(load::<false>(&mut vfs).1, expected);
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        assert_eq!(backend.reads, [(0, 8), (3, 8), (5, 8), (13, 6)]);
        assert_eq!(backend.closes, 1);
    });
}

#[test]
fn read_failures_zero_and_oversized_replies_close_without_overcopy() {
    for failure in [Err(RequestError::Transport), Ok(0), Ok(9)] {
        let mut shm = [0; 8];
        let mut vfs = client(&mut shm);
        set_file(16, 1);
        BACKEND.with(|backend| backend.borrow_mut().read_results.push_back(failure));
        let error = match failure {
            Err(error) => error,
            Ok(_) => RequestError::Protocol,
        };
        assert_eq!(image_loader::load::<false>(&mut vfs, 4), Err(error));
        BACKEND.with(|backend| {
            let backend = backend.borrow();
            assert_eq!(backend.closes, 1);
            assert!(backend.mappings[0].iter().all(|&byte| byte == 0xa5));
        });
        let expected = set_file(16, 9);
        assert_eq!(load::<false>(&mut vfs).1, expected);
        BACKEND.with(|backend| assert_eq!(backend.borrow().heap_requests, [16]));
    }
}

#[test]
fn failed_growth_preserves_old_buffer_and_can_retry() {
    for result in [
        Err(RequestError::Transport),
        Ok((0, 16384)),
        Ok((1, 100)),
        Ok((usize::MAX - 4095, 16384)),
    ] {
        let mut shm = [0; 1024];
        let mut vfs = client(&mut shm);
        let expected = set_file(16, 7);
        let (base, _) = load::<false>(&mut vfs);
        set_file(8192, 9);
        BACKEND.with(|backend| backend.borrow_mut().heap_result = Some(result));
        assert!(image_loader::load::<false>(&mut vfs, 4).is_err());
        set_file(16, 7);
        assert_eq!(load::<false>(&mut vfs), (base, expected));
        let expected = set_file(8192, 5);
        let (larger, actual) = load::<false>(&mut vfs);
        assert_ne!(larger, base);
        assert_eq!(actual, expected);
    }
}

#[test]
fn rejects_invalid_files_before_allocating_or_opening() {
    let mut shm = [0; 8];
    let mut vfs = client(&mut shm);
    for (size, kind) in [(0, vfs::VFS_FILE_TYPE_REGULAR), (100, 2)] {
        set_file(size, 1);
        BACKEND.with(|backend| backend.borrow_mut().kind = kind);
        assert_eq!(
            image_loader::load::<false>(&mut vfs, 4),
            Err(RequestError::InvalidArgument)
        );
    }
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        assert!(backend.heap_requests.is_empty());
        assert_eq!(backend.opens, 0);
    });
}

#[test]
fn open_failure_does_not_close_an_unopened_handle() {
    let mut shm = [0; 8];
    let mut vfs = client(&mut shm);
    set_file(8, 1);
    BACKEND.with(|backend| backend.borrow_mut().open_error = Some(RequestError::Transport));
    assert_eq!(
        image_loader::load::<false>(&mut vfs, 4),
        Err(RequestError::Transport)
    );
    BACKEND.with(|backend| assert_eq!(backend.borrow().closes, 0));
}
