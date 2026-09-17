//! Production ext2 transfer/cache loops; block IPC and inode mapping are mocked.
#![allow(dead_code)]
extern crate alloc;
extern crate self as libnanami;
extern crate self as nanami_services;
pub use std::print;
use std::{cell::RefCell, cmp::min, ptr};
pub type Word = usize;
pub const OS_RESPONSE_OK: Word = 0;
pub const OS_RESPONSE_INVALID_ARGUMENT: Word = 1;
pub const OS_RESPONSE_INVALID_DESCRIPTOR: Word = 3;
pub const OS_RESPONSE_ILLEGAL_OPERATION: Word = 4;
pub const OS_RESPONSE_FATAL: Word = 5;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    InvalidArgument,
    Protocol,
    Transport,
    Unsupported,
    Status(Word),
}

const BLOCK_READ_RETRY_LIMIT: usize = 4;
const BLOCK_SIZE: usize = 1024;
const BUFFER_SIZE: usize = 16 * BLOCK_SIZE;
const SLOT_TIMER_SERVICE: Word = 24;
pub const PROCESS_SLOT_NOTIFICATION: Word = 18;

#[derive(Clone, Copy)]
struct Ext2Inode {
    size: u32,
}
#[derive(Clone, Copy)]
struct ClientSession {
    shm_local: Word,
    shm_size: Word,
}
struct Ext2Runtime {
    block_port: Word,
    block_shm: Word,
    block_shm_size: Word,
    block_size: usize,
    block_count: usize,
    block_cache: block_cache::BlockCache,
    writeback: writeback::Writeback,
    handles: [FileHandle; 2],
    mapping: Vec<u32>,
    scratch: Box<[u8; BUFFER_SIZE]>,
}

struct FileHandle {
    active: bool,
    owner_pid: Word,
}
fn find_session(_runtime: &Ext2Runtime, pid: Word) -> Option<usize> {
    if pid == 16 {
        Some(0)
    } else {
        None
    }
}
fn log_request_error(_message: &str, _error: RequestError) {}
pub mod ipc {
    use super::*;
    #[derive(Clone, Copy)]
    pub struct ServiceRequest {
        pub identifier: Word,
        pub code: Word,
        pub arg0: Word,
    }
    pub fn process_slot_descriptor(slot: Word) -> Word {
        slot
    }
    pub fn bind_current_thread_notification(_slot: Word) -> Result<(), RequestError> {
        Ok(())
    }
}
use ipc::ServiceRequest;
pub mod vfs {
    pub const VFS_REQUEST_FSYNC: usize = 0x9111;
    pub const VFS_REQUEST_SYNC: usize = 0x9112;
}
pub mod registry {
    use super::*;
    pub fn connect_timer_service(_slot: Word) -> Result<(), RequestError> {
        Ok(())
    }
}
pub mod timer {
    use super::*;
    pub fn timer_service_sleep_async_on_notification_milliseconds(
        port: Word,
        ms: Word,
        slot: Word,
    ) -> Result<(), RequestError> {
        assert_eq!((port, ms, slot), (24, 1000, 18));
        FAKE.with(|fake| {
            let mut fake = fake.borrow_mut();
            fake.alarms += 1;
            fake.timer_error.map_or(Ok(()), Err)
        })
    }
}

#[derive(Default)]
struct Fake {
    disk: Vec<u8>,
    durable: Vec<u8>,
    shm: usize,
    reads: Vec<(usize, usize)>,
    writes: Vec<(usize, usize)>,
    inode_writes: usize,
    write_result: Option<Result<usize, RequestError>>,
    flush_error: Option<RequestError>,
    events: Vec<(char, usize, usize)>,
    alarms: usize,
    timer_error: Option<RequestError>,
    next_block: usize,
}
thread_local! { static FAKE: RefCell<Fake> = RefCell::new(Fake::default()); }
pub fn yield_now() {}
pub mod block {
    use super::*;
    pub fn block_device_read(
        _port: Word,
        first: Word,
        count: Word,
        offset: Word,
    ) -> Result<Word, RequestError> {
        assert!(offset + count * BLOCK_SIZE <= BUFFER_SIZE);
        FAKE.with(|fake| {
            let mut fake = fake.borrow_mut();
            fake.reads.push((first, count));
            let bytes = count * BLOCK_SIZE;
            unsafe {
                ptr::copy_nonoverlapping(
                    fake.disk.as_ptr().add(first * BLOCK_SIZE),
                    (fake.shm + offset) as *mut u8,
                    bytes,
                );
            }
            Ok(bytes)
        })
    }
    pub fn block_device_write(
        _port: Word,
        first: Word,
        count: Word,
        offset: Word,
    ) -> Result<Word, RequestError> {
        assert_eq!(offset, 0);
        FAKE.with(|fake| {
            let mut fake = fake.borrow_mut();
            fake.writes.push((first, count));
            fake.events.push(('W', first, count));
            let bytes = count * BLOCK_SIZE;
            let result = fake.write_result.unwrap_or(Ok(bytes));
            // A transport failure may also have committed a prefix.
            let committed = result.unwrap_or(BLOCK_SIZE).min(bytes);
            unsafe {
                ptr::copy_nonoverlapping(
                    fake.shm as *const u8,
                    fake.disk.as_mut_ptr().add(first * BLOCK_SIZE),
                    committed,
                );
            }
            result
        })
    }
    pub fn block_device_flush(_port: Word) -> Result<(), RequestError> {
        FAKE.with(|fake| {
            let mut fake = fake.borrow_mut();
            fake.events.push(('F', 0, 0));
            if let Some(error) = fake.flush_error {
                return Err(error);
            }
            fake.durable = fake.disk.clone();
            Ok(())
        })
    }
}
#[path = "../../../nanami/servers/core-services/ext2-server/src/block_cache.rs"]
mod block_cache;
#[path = "../../../nanami/servers/core-services/ext2-server/src/block_io.rs"]
mod block_io;
#[path = "../../../nanami/servers/core-services/ext2-server/src/writeback.rs"]
mod writeback;
use block_io::*;
#[path = "../../../nanami/servers/core-services/ext2-server/src/data_io.rs"]
mod data_io;
use data_io::*;

fn max_file_blocks(_runtime: &Ext2Runtime) -> usize {
    512
}
fn get_data_block(
    runtime: &mut Ext2Runtime,
    _inode: Ext2Inode,
    logical: usize,
) -> Result<u32, RequestError> {
    // Simulate indirect-block traversal clobbering the shared scratch buffer.
    runtime.scratch.fill(0xbb);
    runtime
        .mapping
        .get(logical)
        .copied()
        .ok_or(RequestError::InvalidArgument)
}
fn ensure_data_block_with(
    runtime: &mut Ext2Runtime,
    _inode: &mut Ext2Inode,
    logical: usize,
    initialize: impl FnOnce(&mut Ext2Runtime, usize) -> Result<(), RequestError>,
) -> Result<u32, RequestError> {
    if runtime.mapping[logical] == 0 {
        let block = FAKE.with(|fake| {
            let mut fake = fake.borrow_mut();
            let block = fake.next_block;
            fake.next_block += 1;
            block as u32
        });
        initialize(runtime, block as usize)?;
        runtime.mapping[logical] = block;
    }
    Ok(runtime.mapping[logical])
}
fn write_inode(
    _runtime: &mut Ext2Runtime,
    _inode_no: u32,
    _inode: Ext2Inode,
) -> Result<(), RequestError> {
    FAKE.with(|fake| fake.borrow_mut().inode_writes += 1);
    Ok(())
}
fn map_request_error_to_status(_error: RequestError) -> Word {
    OS_RESPONSE_FATAL
}

fn runtime(mapping: Vec<u32>) -> Ext2Runtime {
    let mut scratch = Box::new([0xcc; BUFFER_SIZE]);
    FAKE.with(|fake| {
        *fake.borrow_mut() = Fake {
            disk: vec![0x77; 1024 * BLOCK_SIZE],
            durable: vec![0x77; 1024 * BLOCK_SIZE],
            shm: scratch.as_mut_ptr() as usize,
            next_block: 512,
            ..Fake::default()
        }
    });
    Ext2Runtime {
        block_port: 1,
        block_shm: scratch.as_mut_ptr() as usize,
        block_shm_size: BUFFER_SIZE,
        block_size: BLOCK_SIZE,
        block_count: 1024,
        block_cache: block_cache::BlockCache::new(BLOCK_SIZE, 128 * BLOCK_SIZE, BUFFER_SIZE),
        writeback: writeback::Writeback::new(Some(SLOT_TIMER_SERVICE)),
        handles: [
            FileHandle {
                active: true,
                owner_pid: 16,
            },
            FileHandle {
                active: false,
                owner_pid: 16,
            },
        ],
        mapping,
        scratch,
    }
}
fn write(
    runtime: &mut Ext2Runtime,
    inode: &mut Ext2Inode,
    offset: usize,
    input: &[u8],
) -> Result<usize, Word> {
    write_file(
        runtime,
        ClientSession {
            shm_local: input.as_ptr() as Word,
            shm_size: input.len(),
        },
        inode,
        7,
        offset,
        input.len(),
        0,
    )
}

#[path = "block-allocation.rs"]
mod block_allocation_tests;
mod cache;
mod transfers;
#[path = "writeback-tests.rs"]
mod writeback_tests;
