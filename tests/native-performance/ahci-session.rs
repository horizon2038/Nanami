//! Real block client and AHCI session code with owned RAM and simulated DMA.
#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_runtime;
use core::{ptr, sync::atomic::{fence, Ordering}};
use std::{cell::RefCell, collections::BTreeMap};
pub use std::println;
pub type Word = usize;
pub const OS_RESPONSE_OK: Word = 0;
pub const OS_RESPONSE_INVALID_ARGUMENT: Word = 1;
pub const OS_RESPONSE_ILLEGAL_OPERATION: Word = 4;
pub const OS_RESPONSE_FATAL: Word = 5;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError { InvalidArgument, Protocol, Transport, Unsupported, Status(Word) }
pub mod constants { pub use nanami_uapi::block::*; }
#[path = "../../nanami/servers/libs/nanami-client/src/block/service.rs"]
mod block_service;
pub mod block {
    pub use super::constants::*;
    pub use super::block_service::*;
}
const BLOCK_BYTES: usize = block::BLOCK_DEVICE_BLOCK_SIZE;
const SECTOR_BYTES: usize = 512;
const ATA_IDENTIFY_DEVICE: u8 = 0xec;
#[derive(Clone, Copy)]
struct AhciRuntime {
    dma_physical: Word,
    dma_virtual: Word,
    dma_data_bytes: usize,
    partition_sectors: u64,
    flush_command: u8,
}
#[derive(Clone, Copy)]
struct ServiceRequest {
    identifier: Word, code: Word, arg0: Word, arg1: Word, arg2: Word, arg3: Word,
}
#[path = "../../nanami/servers/core-services/ahci-server/src/app/dma.rs"]
mod dma;
use dma::*;
#[path = "../../nanami/servers/core-services/ahci-server/src/app/session.rs"]
mod session;

struct Memory { data: Box<[u64]>, physical: Word, bytes: usize, references: usize }
#[derive(Default)]
struct Machine {
    memory: BTreeMap<Word, Memory>,
    next_physical: Word,
    unsupported: bool,
    fail_share: bool,
    fail_dma: bool,
    fail_read: bool,
    fail_write: bool,
    fail_flush: bool,
    fail_recovery: bool,
    allocations: usize,
    releases: usize,
    recoveries: usize,
    calls: Vec<(Word, Word)>,
    transfers: Vec<(Word, usize, bool)>,
    disk: Vec<u8>,
}
thread_local! {
    static MACHINE: RefCell<Machine> = RefCell::new(Machine::default());
    static STATE: RefCell<Option<(AhciRuntime, session::ClientSession)>> = const { RefCell::new(None) };
}

fn allocate(bytes: usize) -> (Word, Word) {
    MACHINE.with_borrow_mut(|machine| {
        // Production DMA mappings are page aligned.
        // Reserve extra RAM and select an aligned span inside it.
        let mut data = vec![u64::MAX; bytes.div_ceil(8) + 512].into_boxed_slice();
        let local = (data.as_mut_ptr() as Word + 4095) & !4095;
        let physical = machine.next_physical;
        // Include the private header page when advancing physical RAM.
        machine.next_physical += (bytes + 4095) & !4095;
        machine.memory.insert(local, Memory { data, physical, bytes, references: 1 });
        machine.allocations += 1;
        (physical, local)
    })
}

pub fn request_dma(bytes: Word) -> Result<(Word, Word), RequestError> {
    if MACHINE.with_borrow(|machine| machine.fail_dma) { return Err(RequestError::Transport); }
    Ok(allocate(bytes))
}
pub fn request_mapping_share(_: Word, local: Word, _: Word) -> Result<Word, RequestError> {
    MACHINE.with_borrow_mut(|machine| {
        if machine.fail_share { return Err(RequestError::Transport); }
        machine.memory.get_mut(&local).unwrap().references += 1;
        Ok(local)
    })
}
pub fn request_shared_memory(_: Word, bytes: Word) -> Result<(Word, Word), RequestError> {
    let (_, local) = allocate((bytes + 4095) & !4095);
    unsafe { ptr::write_bytes(local as *mut u8, 0, (bytes + 4095) & !4095); }
    Ok((local, local))
}
pub fn request_mapping_release(local: Word, _: Word) -> Result<(), RequestError> {
    MACHINE.with_borrow_mut(|machine| {
        machine.releases += 1;
        let memory = machine.memory.get_mut(&local).unwrap();
        memory.references -= 1;
        if memory.references == 0 { machine.memory.remove(&local); }
    });
    Ok(())
}
fn configure_port(_: &AhciRuntime) -> Result<(), RequestError> {
    MACHINE.with_borrow_mut(|machine| {
        machine.recoveries += 1;
        if machine.fail_recovery { Err(RequestError::Transport) } else { Ok(()) }
    })
}
fn submit(_: &AhciRuntime, bytes: usize) -> Result<(), RequestError> {
    assert_eq!(bytes, 0); // This session path submits only a zero-data flush.
    if MACHINE.with_borrow(|m| m.fail_flush) { Err(RequestError::Transport) } else { Ok(()) }
}
fn status(error: RequestError) -> Word {
    match error {
        RequestError::InvalidArgument => OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Status(code) => code,
        _ => OS_RESPONSE_FATAL,
    }
}
fn transfer(_: &AhciRuntime, block: Word, bytes: usize, write: bool, physical: Word) -> Result<(), RequestError> {
    MACHINE.with_borrow_mut(|machine| {
        if block.checked_mul(BLOCK_BYTES).and_then(|start| start.checked_add(bytes))
            .is_none_or(|end| end > machine.disk.len()) { return Err(RequestError::InvalidArgument); }
        let (&local, memory) = machine.memory.iter().find(|(_, memory)|
            physical >= memory.physical && physical - memory.physical < memory.bytes
                && bytes <= memory.bytes - (physical - memory.physical)).unwrap();
        let pointer = local + physical - memory.physical;
        machine.transfers.push((physical, bytes, write));
        let start = block * BLOCK_BYTES;
        let failed = (machine.fail_read && !write) || (machine.fail_write && write);
        let count = if failed { bytes / 2 } else { bytes };
        unsafe {
            if write {
                machine.disk[start..start + count]
                    .copy_from_slice(core::slice::from_raw_parts(pointer as *const u8, count));
            } else {
                ptr::copy_nonoverlapping(machine.disk[start..].as_ptr(), pointer as *mut u8, count);
            }
        }
        if failed { Err(RequestError::Transport) } else { Ok(()) }
    })
}
fn call_port(_: Word, code: Word, a: Word, b: Word, c: Word, d: Word, words: u8) -> Result<(Word, Word, Word), RequestError> {
    assert_eq!(words, match code {
        block::BLOCK_DEVICE_REQUEST_CONTROL if a == block::BLOCK_DEVICE_CONTROL_GET_INFO => 2,
        block::BLOCK_DEVICE_REQUEST_CONTROL => 3,
        block::BLOCK_DEVICE_REQUEST_READ | block::BLOCK_DEVICE_REQUEST_WRITE => 4,
        block::BLOCK_DEVICE_REQUEST_FLUSH => 1,
        _ => panic!("unexpected block command"),
    });
    MACHINE.with_borrow_mut(|machine| machine.calls.push((code, a)));
    if a == block::BLOCK_DEVICE_CONTROL_ATTACH_DMA_READ_MEMORY
        && code == block::BLOCK_DEVICE_REQUEST_CONTROL
        && MACHINE.with_borrow(|machine| machine.unsupported)
    { return Ok((OS_RESPONSE_INVALID_ARGUMENT, 0, 0)); }
    STATE.with_borrow_mut(|state| {
        let (runtime, session) = state.as_mut().unwrap();
        session::handle_request(ServiceRequest { identifier: 42, code, arg0: a, arg1: b, arg2: c, arg3: d },
            session, runtime)
    })
}
fn reset() {
    reset_with_capacity(DMA_DATA_BYTES);
}
fn reset_with_capacity(capacity: usize) {
    STATE.with_borrow_mut(|state| *state = None);
    MACHINE.with_borrow_mut(|machine| *machine = Machine {
        disk: (0..4 * DMA_DATA_BYTES).map(|index| (index % 251) as u8).collect(),
        next_physical: 0x100000000,
        ..Machine::default()
    });
    let (physical, local) = allocate(DMA_DATA_OFFSET + capacity);
    unsafe { ptr::write_bytes(local as *mut u8, 0xa5, DMA_DATA_OFFSET + capacity); }
    STATE.with_borrow_mut(|state| *state = Some((AhciRuntime {
        dma_physical: physical, dma_virtual: local,
        dma_data_bytes: capacity,
        partition_sectors: (4 * DMA_DATA_BYTES / SECTOR_BYTES) as u64,
        flush_command: 0xea,
    }, session::ClientSession::EMPTY)));
}

#[test]
fn legacy_command_capacity_is_advertised_and_limits_large_shared_windows_before_copying() {
    for direct in [true, false] {
        reset_with_capacity(DMA_FALLBACK_DATA_BYTES);
        let (window, size) = if direct {
            block::block_device_attach_dma_read_memory(20, 0).unwrap()
        } else { block::block_device_attach_shared_memory(20, DMA_DATA_BYTES).unwrap() };
        assert_eq!(size, if direct { DMA_FALLBACK_DATA_BYTES } else { DMA_DATA_BYTES });
        unsafe { ptr::write_bytes(window as *mut u8, 0x61, size); }
        assert!(block::block_device_write(20, 0, DMA_FALLBACK_DATA_BYTES / BLOCK_BYTES + 1, 0).is_err());
        assert!(block::block_device_read(20, 0, DMA_FALLBACK_DATA_BYTES / BLOCK_BYTES + 1, 0).is_err());
        assert_eq!(MACHINE.with_borrow(|m| m.transfers.len()), 0);
        let runtime = STATE.with_borrow(|s| s.unwrap().0);
        assert!(unsafe { core::slice::from_raw_parts(runtime.dma_virtual as *const u8,
            DMA_DATA_OFFSET + DMA_FALLBACK_DATA_BYTES) }.iter().all(|b| *b == 0xa5));
        assert_eq!(block::block_device_read(20, 0, DMA_FALLBACK_DATA_BYTES / BLOCK_BYTES, 0).unwrap(), DMA_FALLBACK_DATA_BYTES);
        if !direct {
            // The peer may explicitly own a larger SHM span. A small transfer
            // at a high source offset still fits the private bounce buffer.
            assert_eq!(block::block_device_write(20, 0, 1, DMA_DATA_BYTES - BLOCK_BYTES).unwrap(), BLOCK_BYTES);
        }
    }
}

#[test]
fn direct_reads_target_the_client_window_and_keep_writes_and_unaligned_reads_compatible() {
    reset();
    let (window, size) = block::block_device_attach_dma_read_memory(20, 0).unwrap();
    assert_eq!(size, DMA_DATA_BYTES);
    assert!(unsafe { core::slice::from_raw_parts(window as *const u8, size) }.iter().all(|&b| b == 0));
    let allocations = MACHINE.with_borrow(|machine| machine.allocations);
    assert_eq!(block::block_device_attach_dma_read_memory(20, 0).unwrap(), (window, size));
    assert_eq!(MACHINE.with_borrow(|machine| machine.allocations), allocations);
    assert_eq!(block::block_device_read(20, 3, 64, 2048).unwrap(), 64 * BLOCK_BYTES);
    MACHINE.with_borrow(|machine| {
        let (runtime, _) = STATE.with_borrow(|state| state.unwrap());
        assert!(unsafe { core::slice::from_raw_parts((runtime.dma_virtual + DMA_DATA_OFFSET) as *const u8, DMA_DATA_BYTES) }
            .iter().all(|&byte| byte == 0xa5));
        assert_eq!(unsafe { core::slice::from_raw_parts((window + 2048) as *const u8, 64 * BLOCK_BYTES) },
            &machine.disk[3 * BLOCK_BYTES..67 * BLOCK_BYTES]);
        assert_eq!(machine.transfers.last().unwrap().0, machine.memory[&window].physical + 2048);
    });
    unsafe { ptr::write_bytes((window + 4096) as *mut u8, 0x19, BLOCK_BYTES); }
    block::block_device_write(20, 8, 1, 4096).unwrap();
    MACHINE.with_borrow(|machine| assert!(machine.disk[8 * BLOCK_BYTES..9 * BLOCK_BYTES]
        .iter().all(|&byte| byte == 0x19)));
    block::block_device_read(20, 7, 1, 1).unwrap();
    MACHINE.with_borrow(|machine| assert_eq!(
        unsafe { core::slice::from_raw_parts((window + 1) as *const u8, BLOCK_BYTES) },
        &machine.disk[7 * BLOCK_BYTES..8 * BLOCK_BYTES]));
    block::block_device_flush(20).unwrap();
}

#[test]
fn unsupported_or_failed_dma_setup_falls_back_and_legacy_read_errors_keep_old_bytes() {
    for setup in 0..3 {
        reset();
        MACHINE.with_borrow_mut(|machine| match setup {
            0 => machine.unsupported = true,
            1 => machine.fail_dma = true,
            _ => machine.fail_share = true,
        });
        let (window, _) = block::block_device_attach_dma_read_memory(20, 0).unwrap();
        if setup == 2 { assert_eq!(MACHINE.with_borrow(|machine| machine.releases), 1); }
        unsafe { ptr::write_bytes(window as *mut u8, 0x61, BLOCK_BYTES); }
        MACHINE.with_borrow_mut(|machine| machine.fail_read = true);
        assert!(block::block_device_read(20, 0, 1, 0).is_err());
        assert!(unsafe { core::slice::from_raw_parts(window as *const u8, BLOCK_BYTES) }
            .iter().all(|&byte| byte == 0x61));
    }
}

#[test]
fn failed_direct_read_recovers_before_reply_and_failed_recovery_propagates_without_a_reply() {
    reset();
    let (window, _) = block::block_device_attach_dma_read_memory(20, 0).unwrap();
    MACHINE.with_borrow_mut(|machine| machine.fail_read = true);
    assert!(block::block_device_read(20, 0, 1, 0).is_err());
    assert_eq!(MACHINE.with_borrow(|machine| machine.recoveries), 1);
    assert!(unsafe { core::slice::from_raw_parts(window as *const u8, BLOCK_BYTES) } != &[0; BLOCK_BYTES]);
    MACHINE.with_borrow_mut(|machine| { machine.fail_read = false; });
    assert_eq!(block::block_device_read(20, 0, 1, 0).unwrap(), BLOCK_BYTES);
    MACHINE.with_borrow_mut(|machine| { machine.fail_read = true; machine.fail_recovery = true; });
    assert_eq!(block::block_device_read(20, 0, 1, 0), Err(RequestError::Transport));
}

#[test]
fn failed_legacy_read_recovers_before_private_dma_can_be_reused() {
    reset();
    let (window, _) = block::block_device_attach_shared_memory(20, 4096).unwrap();
    unsafe { ptr::write_bytes(window as *mut u8, 0x6b, 4096); }
    MACHINE.with_borrow_mut(|m| m.fail_read = true);
    assert!(block::block_device_read(20, 0, 1, 0).is_err());
    assert_eq!(MACHINE.with_borrow(|m| m.recoveries), 1);
    assert!(unsafe { core::slice::from_raw_parts(window as *const u8, 4096) }.iter().all(|&b| b == 0x6b));
    MACHINE.with_borrow_mut(|m| { m.fail_read = false; });
    assert_eq!(block::block_device_read(20, 0, 1, 0).unwrap(), BLOCK_BYTES);
    MACHINE.with_borrow_mut(|m| { m.fail_read = true; m.fail_recovery = true; });
    assert_eq!(block::block_device_read(20, 0, 1, 0), Err(RequestError::Transport));
}

#[test]
fn failed_write_recovers_before_error_reply_and_failed_recovery_stops_dispatch() {
    reset();
    let (window, _) = block::block_device_attach_dma_read_memory(20, 4096).unwrap();
    unsafe { ptr::write_bytes(window as *mut u8, 0x3c, 4096); }
    MACHINE.with_borrow_mut(|m| m.fail_write = true);
    assert!(block::block_device_write(20, 0, 1, 0).is_err());
    assert_eq!(MACHINE.with_borrow(|m| m.recoveries), 1);
    MACHINE.with_borrow_mut(|m| m.fail_write = false);
    assert_eq!(block::block_device_write(20, 0, 1, 0).unwrap(), BLOCK_BYTES);
    MACHINE.with_borrow_mut(|m| { m.fail_write = true; m.fail_recovery = true; });
    assert_eq!(block::block_device_write(20, 0, 1, 0), Err(RequestError::Transport));
}

#[test]
fn failed_flush_recovers_the_command_slot_and_unsupported_flush_has_no_dma_to_stop() {
    reset();
    block::block_device_attach_shared_memory(20, 4096).unwrap();
    MACHINE.with_borrow_mut(|m| m.fail_flush = true);
    assert!(block::block_device_flush(20).is_err());
    assert_eq!(MACHINE.with_borrow(|m| m.recoveries), 1);
    MACHINE.with_borrow_mut(|m| { m.fail_flush = false; });
    block::block_device_flush(20).unwrap();
    MACHINE.with_borrow_mut(|m| { m.fail_flush = true; m.fail_recovery = true; });
    assert_eq!(block::block_device_flush(20), Err(RequestError::Transport));
    STATE.with_borrow_mut(|s| s.as_mut().unwrap().0.flush_command = 0);
    let before = MACHINE.with_borrow(|m| m.recoveries);
    assert!(block::block_device_flush(20).is_err());
    assert_eq!(MACHINE.with_borrow(|m| m.recoveries), before);
}

#[test]
fn dma_window_zeroes_page_padding_and_rejects_foreign_or_out_of_bounds_transfers_before_dma() {
    reset();
    let (window, size) = block::block_device_attach_dma_read_memory(20, 4097).unwrap();
    assert_eq!(size, 4097);
    assert!(unsafe { core::slice::from_raw_parts(window as *const u8, 8192) }
        .iter().all(|&byte| byte == 0));
    for (count, offset) in [(0, 0), (5, 0), (1, 4096), (1, usize::MAX), (usize::MAX, 0)] {
        assert!(block::block_device_read(20, 0, count, offset).is_err());
    }
    let foreign = ServiceRequest {
        identifier: 43, code: block::BLOCK_DEVICE_REQUEST_READ, arg0: 0, arg1: 1, arg2: 0, arg3: 0,
    };
    STATE.with_borrow_mut(|state| {
        let (runtime, session) = state.as_mut().unwrap();
        assert_eq!(session::handle_request(foreign, session, runtime),
            Ok((OS_RESPONSE_INVALID_ARGUMENT, 0, 0)));
    });
    assert!(MACHINE.with_borrow(|machine| machine.transfers.is_empty()));
}

#[test]
fn full_window_read_matches_legacy_bytes_with_no_post_dma_staging_copy() {
    let mut expected = Vec::new();
    for direct in [false, true] {
        reset();
        let (window, bytes) = if direct {
            block::block_device_attach_dma_read_memory(20, 0).unwrap()
        } else {
            block::block_device_attach_shared_memory(20, 0).unwrap()
        };
        assert_eq!(bytes, DMA_DATA_BYTES);
        assert_eq!(block::block_device_read(20, 2, bytes / BLOCK_BYTES, 0).unwrap(), bytes);
        let actual = unsafe { core::slice::from_raw_parts(window as *const u8, bytes) }.to_vec();
        if direct { assert!(actual == expected); } else { expected = actual; }
        MACHINE.with_borrow(|machine| {
            let runtime = STATE.with_borrow(|state| state.unwrap().0);
            let bounce = unsafe {
                core::slice::from_raw_parts((runtime.dma_virtual + DMA_DATA_OFFSET) as *const u8, bytes)
            };
            if direct {
                assert!(bounce.iter().all(|&byte| byte == 0xa5));
            } else {
                assert!(bounce == expected);
            }
            assert_eq!(machine.transfers.len(), 1);
        });
    }
    println!("AHCI-DIRECT-READ bytes={} staging_copy_before={} staging_copy_after=0", DMA_DATA_BYTES, DMA_DATA_BYTES);
}
