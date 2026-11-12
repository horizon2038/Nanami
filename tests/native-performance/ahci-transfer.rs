//! Production AHCI command encoding and transfer-window bounds; no hardware I/O.
#![allow(dead_code)]
extern crate self as nanami_client;
pub mod block { pub use nanami_uapi::block::*; }
use core::{ptr, sync::atomic::{fence, Ordering}};
type Word = usize;
#[derive(Debug, PartialEq, Eq)]
enum RequestError { InvalidArgument, Transport }
const BLOCK_BYTES: usize = block::BLOCK_DEVICE_BLOCK_SIZE;
const ATA_IDENTIFY_DEVICE: u8 = 0xec;
const PORT_IS_TFES: u32 = 1 << 30;
struct AhciRuntime { dma_physical: Word, dma_virtual: Word, dma_data_bytes: usize }
#[path = "../../nanami/servers/core-services/ahci-server/src/app/dma.rs"]
mod dma;
use dma::*;
#[path = "../../nanami/servers/core-services/ahci-server/src/app/completion.rs"]
mod completion;
#[path = "../../nanami/servers/core-services/ahci-server/src/app/command_state.rs"]
mod command_state;

#[test]
fn completed_command_omits_repeated_ready_checks_until_invalidation() {
    use std::cell::Cell;
    let state = command_state::State::new();
    let ready = Cell::new(0);
    let issued = Cell::new(0);
    let completed = Cell::new(0);
    for index in 0..100 {
        if index == 34 || index == 68 { state.invalidate(); }
        state.submit(|| { ready.set(ready.get() + 1); Ok(()) },
            || issued.set(issued.get() + 1),
            || { completed.set(completed.get() + 1); Ok(()) }).unwrap();
    }
    assert_eq!((ready.get(), issued.get(), completed.get()), (3, 100, 100));
}

#[test]
fn failed_cold_readiness_never_issues_and_cannot_establish_warm_state() {
    use std::cell::Cell;
    let state = command_state::State::new();
    let ready = Cell::new(0);
    assert_eq!(state.submit(|| { ready.set(ready.get() + 1); Err(RequestError::Transport) },
        || panic!("busy device must not be issued"), || panic!("nothing was issued")),
        Err(RequestError::Transport));
    state.submit(|| { ready.set(ready.get() + 1); Ok(()) }, || {}, || Ok(())).unwrap();
    assert_eq!(ready.get(), 2);
}

#[test]
fn failed_warm_command_consumes_completion_proof_before_callbacks() {
    use std::cell::Cell;
    let state = command_state::State::new();
    let ready = Cell::new(0);
    state.submit(|| { ready.set(ready.get() + 1); Ok(()) }, || {}, || Ok(())).unwrap();
    assert_eq!(state.submit(|| panic!("preceding command completed"), || {},
        || Err(RequestError::Transport)), Err(RequestError::Transport));
    state.submit(|| { ready.set(ready.get() + 1); Ok(()) }, || {}, || Ok(())).unwrap();
    state.submit(|| panic!("only successful completion restores warm state"), || {}, || Ok(())).unwrap();
    assert_eq!(ready.get(), 2);
}

#[test]
fn completion_state_is_separate_for_each_controller_and_reset() {
    use std::cell::Cell;
    let states = [command_state::State::new(), command_state::State::new()];
    let ready = Cell::new(0);
    for index in [0, 1, 0, 1] {
        states[index].submit(|| { ready.set(ready.get() + 1); Ok(()) }, || {}, || Ok(())).unwrap();
    }
    assert_eq!(ready.get(), 2);
    states[0].invalidate();
    states[1].submit(|| panic!("other controller was not reset"), || {}, || Ok(())).unwrap();
    states[0].submit(|| { ready.set(ready.get() + 1); Ok(()) }, || {}, || Ok(())).unwrap();
    assert_eq!(ready.get(), 3);
}

#[test]
fn backoff_reduces_polls_for_asynchronous_completion_with_bounded_detection_delay() {
    use std::cell::Cell;
    for ready_at in [0, 1, 8, 32, 256, 4096, 65536] {
        let elapsed = Cell::new(0usize);
        let reads = Cell::new(0usize);
        let pauses = Cell::new(0usize);
        completion::wait_with_backoff(|register| match register {
            completion::Register::Issued => {
                let busy = elapsed.get() < ready_at;
                reads.set(reads.get() + 1);
                elapsed.set(elapsed.get() + 1);
                u32::from(busy)
            }
            completion::Register::Status => 0,
        }, || true, || {}, |count| {
            assert!((1..=64).contains(&count));
            pauses.set(pauses.get() + count);
            elapsed.set(elapsed.get() + count);
        }, 100_000).unwrap();
        assert!(elapsed.get() >= ready_at);
        assert!(elapsed.get() <= ready_at + 65);
        if ready_at == 0 {
            assert_eq!((reads.get(), pauses.get()), (1, 0));
        }
        if ready_at >= 4096 {
            assert!(reads.get() * 10 < ready_at + 1);
        }
        println!("completion-work={ready_at} tight-ci={} backoff-ci={} observed-work={}",
            ready_at + 1, reads.get(), elapsed.get());
    }
}

#[test]
fn backoff_keeps_timeout_and_checks_latched_errors_after_bounded_relaxation() {
    use std::cell::Cell;
    let reads = Cell::new(0usize);
    let relaxed = Cell::new(0usize);
    let last_status = Cell::new(0usize);
    assert_eq!(completion::wait_with_backoff(|register| match register {
        completion::Register::Issued => { reads.set(reads.get() + 1); 1 }
        completion::Register::Status => {
            assert!(relaxed.get() - last_status.get() <= 16 * 64);
            last_status.set(relaxed.get());
            if reads.get() >= 100 { PORT_IS_TFES } else { 0 }
        }
    }, || true, || {}, |count| relaxed.set(relaxed.get() + count), 200), Err(RequestError::Transport));
    assert_eq!(reads.get(), 112);
    assert_eq!(completion::wait_with_backoff(|register| match register {
        completion::Register::Issued => 1,
        completion::Register::Status => 0,
    }, || true, || panic!("must time out before yield"), |_| panic!("must time out before relaxation"), 1),
        Err(RequestError::Transport));
}

#[test]
fn dma_progress_hint_reduces_busy_mmio_with_bounded_completion_latency() {
    use std::cell::Cell;
    for ready_at in [0, 1, 8, 32, 256, 4096, 65536] {
        let mut results = Vec::new();
        for legacy in [true, false] {
            let elapsed = Cell::new(0usize);
            let ci = Cell::new(0usize);
            let status = Cell::new(0usize);
            completion::wait_with_backoff(|register| match register {
                completion::Register::Issued => {
                    ci.set(ci.get() + 1);
                    let busy = elapsed.get() < ready_at;
                    elapsed.set(elapsed.get() + 1);
                    u32::from(busy)
                }
                completion::Register::Status => { status.set(status.get() + 1); 0 }
            }, || {
                if legacy { return true; }
                let ready = elapsed.get() >= ready_at;
                elapsed.set(elapsed.get() + 1);
                ready
            }, || {}, |count| elapsed.set(elapsed.get() + count), 100_000).unwrap();
            assert!(elapsed.get() >= ready_at && elapsed.get() <= ready_at + 16 * 65 + 1);
            results.push((ci.get(), status.get(), elapsed.get()));
        }
        if ready_at == 0 { assert_eq!(results[0], results[1]); }
        if ready_at >= 4096 { assert!(results[1].0 * 4 < results[0].0); }
    }
}

#[test]
fn stale_dma_hint_cannot_hide_completion_or_extend_the_poll_limit() {
    use std::cell::Cell;
    for ready_at in 0..=65 {
        let laps = Cell::new(0usize);
        let mut ci = 0;
        let mut status = 0;
        completion::wait_with_backoff(|register| match register {
            completion::Register::Issued => { ci += 1; u32::from(laps.get() < ready_at) }
            completion::Register::Status => { status += 1; 0 }
        }, || false, || {}, |_| laps.set(laps.get() + 1), 97).unwrap();
        assert!(laps.get() >= ready_at && laps.get() - ready_at <= 15);
        assert_eq!(ci, laps.get() / 16 + 1);
        assert!(status >= 1);
    }
}

#[test]
fn dma_hint_is_never_completion_authority_or_a_substitute_for_status() {
    use std::cell::Cell;
    for fail_at_completion in [false, true] {
        let ci = Cell::new(0usize);
        let status = Cell::new(0usize);
        let result = completion::wait_with_backoff(|register| match register {
            completion::Register::Issued => { ci.set(ci.get() + 1); u32::from(ci.get() < 65) }
            completion::Register::Status => {
                status.set(status.get() + 1);
                if fail_at_completion && ci.get() == 65 { PORT_IS_TFES } else { 0 }
            }
        }, || true, || {}, |_| {}, 66);
        assert_eq!(result, if fail_at_completion { Err(RequestError::Transport) } else { Ok(()) });
        assert_eq!(ci.get(), 65);
        assert_eq!(status.get(), 5);
    }
}

#[test]
fn absent_dma_hint_keeps_bounded_latched_error_checks_while_ci_is_busy() {
    use std::cell::Cell;
    for error_at in 0..=65 {
        let laps = Cell::new(0usize);
        let mut ci = 0;
        let result = completion::wait_with_backoff(|register| match register {
            completion::Register::Issued => { ci += 1; 1 }
            completion::Register::Status => {
                if laps.get() >= error_at { PORT_IS_TFES } else { 0 }
            }
        }, || false, || {}, |_| laps.set(laps.get() + 1), 100);
        assert_eq!(result, Err(RequestError::Transport));
        assert!(laps.get() >= error_at && laps.get() - error_at <= 15);
        assert_eq!(ci, laps.get() / 16 + 1);
    }
}

#[test]
fn absent_dma_hint_preserves_exact_iteration_timeout_and_yield_bounds() {
    for limit in [1, 15, 16, 17, 16384, 32769] {
        let mut ci = 0;
        let mut status = 0;
        let mut yields = 0;
        let mut relaxations = 0;
        assert_eq!(completion::wait_with_backoff(|register| match register {
            completion::Register::Issued => { ci += 1; 1 }
            completion::Register::Status => { status += 1; 0 }
        }, || false, || yields += 1, |count| {
            assert!((1..=64).contains(&count));
            relaxations += 1;
        }, limit), Err(RequestError::Transport));
        assert_eq!(ci, (limit - 1) / 16 + 1);
        assert_eq!(status, limit / 16);
        assert_eq!(relaxations, limit - 1);
        assert_eq!(yields, (limit - 1) / 16384);
    }
}

#[test]
fn completion_poll_reduces_mmio_but_checks_status_at_every_possible_completion() {
    for busy in [0, 1, 15, 16, 17, 63, 256, 16384] {
        let mut issued_reads = 0;
        let mut status_reads = 0;
        let mut yields = 0;
        completion::wait(|register| match register {
            completion::Register::Issued => {
                issued_reads += 1;
                u32::from(issued_reads <= busy)
            }
            completion::Register::Status => {
                status_reads += 1;
                assert!(issued_reads > 0);
                0
            }
        }, || true, || { yields += 1; }, busy + 1).unwrap();
        assert_eq!(issued_reads, busy + 1);
        assert_eq!(status_reads, busy / 16 + 1);
        assert_eq!(yields, busy / 16384);
        println!("busy={busy} old-mmio={} new-mmio={}", 2 * (busy + 1), issued_reads + status_reads);
    }
}

#[test]
fn completion_never_accepts_latched_error_even_when_ci_already_cleared() {
    for complete_at in 1..=65 {
        let mut issued_reads = 0;
        assert_eq!(completion::wait(|register| match register {
            completion::Register::Issued => {
                issued_reads += 1;
                u32::from(issued_reads < complete_at)
            }
            completion::Register::Status => {
                if issued_reads == complete_at { PORT_IS_TFES } else { 0 }
            }
        }, || true, || {}, 66), Err(RequestError::Transport));
        assert_eq!(issued_reads, complete_at);
    }
}

#[test]
fn busy_command_detects_error_in_bounded_polls_and_retains_timeout_and_yield_bounds() {
    for error_at in 1..=65 {
        let mut reads = 0;
        assert_eq!(completion::wait(|register| match register {
            completion::Register::Issued => { reads += 1; 1 }
            completion::Register::Status => {
                if reads >= error_at { PORT_IS_TFES } else { 0 }
            }
        }, || true, || {}, 100), Err(RequestError::Transport));
        assert!(reads >= error_at && reads - error_at <= 15);
    }
    for limit in [1, 15, 16, 17, 16384, 32769] {
        let mut reads = 0;
        let mut yields = 0;
        assert_eq!(completion::wait(|register| match register {
            completion::Register::Issued => { reads += 1; 1 }
            completion::Register::Status => 0,
        }, || true, || { yields += 1; }, limit), Err(RequestError::Transport));
        assert_eq!(reads, limit);
        assert_eq!(yields, (limit - 1) / 16384);
    }
}

#[test]
fn provider_default_grows_without_overriding_explicit_windows_or_transfer_bounds() {
    assert_eq!(shared_window_bytes(0, DMA_DATA_BYTES), 1024 * 1024);
    assert_eq!(shared_window_bytes(0, DMA_FALLBACK_DATA_BYTES), 256 * 1024);
    for requested in [1, 4096, 64 * 1024, 128 * 1024, usize::MAX] {
        assert_eq!(shared_window_bytes(requested, DMA_DATA_BYTES), requested);
    }
    let blocks = DMA_DATA_BYTES / BLOCK_BYTES;
    assert_eq!(transfer_bytes(blocks, 0, DMA_DATA_BYTES, DMA_DATA_BYTES), Some(DMA_DATA_BYTES));
    assert_eq!(transfer_bytes(64, 0, 64 * 1024, DMA_DATA_BYTES), Some(64 * 1024));
    assert_eq!(transfer_bytes(65, 0, 64 * 1024, DMA_DATA_BYTES), None);
    assert_eq!(transfer_bytes(blocks, BLOCK_BYTES, DMA_DATA_BYTES, DMA_DATA_BYTES), None);
    assert_eq!(transfer_bytes(blocks, BLOCK_BYTES, DMA_DATA_BYTES + BLOCK_BYTES, DMA_DATA_BYTES), Some(DMA_DATA_BYTES));
    for (count, offset, capacity) in [(0, 0, DMA_DATA_BYTES), (blocks + 1, 0, usize::MAX),
        (usize::MAX / BLOCK_BYTES + 1, 0, usize::MAX), (1, usize::MAX, usize::MAX)] {
        assert_eq!(transfer_bytes(count, offset, capacity, DMA_DATA_BYTES), None);
    }
}

#[test]
fn command_buffer_allocation_prefers_megabyte_and_retains_legacy_capacity_after_failure() {
    for fail_large in [false, true] {
        let mut requests = Vec::new();
        let (physical, local, available) = allocate_command_memory(|bytes| {
            requests.push(bytes);
            if fail_large && bytes == DMA_TOTAL_BYTES { Err(RequestError::Transport) }
            else { Ok((0x100000000, 0x800000)) }
        }).unwrap();
        assert_eq!((physical, local), (0x100000000, 0x800000));
        assert_eq!(available, if fail_large { DMA_FALLBACK_DATA_BYTES } else { DMA_DATA_BYTES });
        assert_eq!(requests, if fail_large { vec![DMA_TOTAL_BYTES, DMA_DATA_OFFSET + DMA_FALLBACK_DATA_BYTES] }
            else { vec![DMA_TOTAL_BYTES] });
        assert_eq!(shared_window_bytes(0, available), available);
        assert_eq!(transfer_bytes(available / BLOCK_BYTES, 0, DMA_DATA_BYTES, available), Some(available));
        assert_eq!(transfer_bytes(available / BLOCK_BYTES + 1, 0, DMA_DATA_BYTES + BLOCK_BYTES, available), None);
        let mut storage = vec![0xa5u64; (DMA_DATA_OFFSET + available) / 8];
        let runtime = AhciRuntime { dma_physical: physical, dma_virtual: storage.as_mut_ptr() as Word,
            dma_data_bytes: available };
        let before = storage.clone();
        assert_eq!(prepare_command(&runtime, 0x25, 0, (available / 512 + 1) as u16, available + 512, false),
            Err(RequestError::InvalidArgument));
        assert_eq!(storage, before);
    }
    let mut calls = 0;
    assert_eq!(allocate_command_memory(|_| { calls += 1; Err(RequestError::Transport) }), Err(RequestError::Transport));
    assert_eq!(calls, 2);
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[test]
fn full_window_command_and_dma_completion_stay_inside_the_reserved_allocation() {
    let mut backing = vec![u64::from_le_bytes([0xa5; 8]); DMA_TOTAL_BYTES / 8 + 2];
    let runtime = AhciRuntime { dma_physical: 0x1234_5678_0000, dma_virtual: backing.as_mut_ptr() as usize + 8, dma_data_bytes: DMA_DATA_BYTES };
    let lba = 0x0123_4567_89ab;
    prepare_command(&runtime, 0x25, lba, (DMA_DATA_BYTES / 512) as u16, DMA_DATA_BYTES, false).unwrap();
    let memory = unsafe { core::slice::from_raw_parts_mut(backing.as_mut_ptr().cast::<u8>(), backing.len() * 8) };
    let data = &mut memory[8..8 + DMA_TOTAL_BYTES];
    assert_eq!(word(data, 0), 5 | (1 << 16));
    assert_eq!(word(data, 4), 0);
    let table = u64::from(word(data, 8)) | (u64::from(word(data, 12)) << 32);
    assert_eq!(table, (runtime.dma_physical + DMA_COMMAND_TABLE_OFFSET) as u64);
    let fis = &data[DMA_COMMAND_TABLE_OFFSET..DMA_COMMAND_TABLE_OFFSET + 20];
    assert_eq!(&fis[..4], &[0x27, 0x80, 0x25, 0]);
    assert_eq!([fis[4], fis[5], fis[6], fis[8], fis[9], fis[10]], [0xab, 0x89, 0x67, 0x45, 0x23, 0x01]);
    assert_eq!(u16::from_le_bytes([fis[12], fis[13]]), (DMA_DATA_BYTES / 512) as u16);
    let prdt = DMA_COMMAND_TABLE_OFFSET + COMMAND_TABLE_PRDT_OFFSET;
    let address = u64::from(word(data, prdt)) | (u64::from(word(data, prdt + 4)) << 32);
    assert_eq!(address, (runtime.dma_physical + DMA_DATA_OFFSET) as u64);
    let bytes = (word(data, prdt + 12) & 0x003f_ffff) as usize + 1;
    assert_eq!(bytes, DMA_DATA_BYTES);
    assert_eq!(word(data, prdt + 8), 0);
    assert_ne!(word(data, prdt + 12) & (1 << 31), 0);
    assert!(data[DMA_DATA_OFFSET..].iter().all(|&byte| byte == 0xa5));
    let offset = (address - runtime.dma_physical as u64) as usize;
    data[offset..offset + bytes].fill(0x42);
    assert!(memory[..8].iter().chain(memory[8 + DMA_TOTAL_BYTES..].iter()).all(|&byte| byte == 0xa5));
}

#[test]
fn shorter_commands_and_flush_clear_previous_prdt_and_reject_invalid_lengths_before_stores() {
    let mut backing = vec![0u64; DMA_TOTAL_BYTES / 8];
    let runtime = AhciRuntime { dma_physical: 0x4000_0000, dma_virtual: backing.as_mut_ptr() as usize, dma_data_bytes: DMA_DATA_BYTES };
    for (command, sectors, bytes, write) in [(0x35, (DMA_DATA_BYTES / 512) as u16, DMA_DATA_BYTES, true),
        (0xea, 0, 0, false), (ATA_IDENTIFY_DEVICE, 0, 512, false), (0x25, 2, 1024, false)] {
        prepare_command(&runtime, command, 0, sectors, bytes, write).unwrap();
        let data = unsafe { core::slice::from_raw_parts(backing.as_ptr().cast::<u8>(), DMA_TOTAL_BYTES) };
        assert_eq!(word(data, 0), 5 | if write { 1 << 6 } else { 0 } | if bytes != 0 { 1 << 16 } else { 0 });
        let fis = DMA_COMMAND_TABLE_OFFSET;
        assert_eq!(data[fis + 2], command);
        assert_eq!(u16::from_le_bytes([data[fis + 12], data[fis + 13]]), sectors);
        if bytes == 0 {
            assert!(data[fis + COMMAND_TABLE_PRDT_OFFSET..fis + COMMAND_TABLE_PRDT_OFFSET + 16].iter().all(|&b| b == 0));
        } else {
            assert_eq!((word(data, fis + COMMAND_TABLE_PRDT_OFFSET + 12) & 0x003f_ffff) as usize + 1, bytes);
        }
    }
    let before = backing.clone();
    for (sectors, bytes) in [((DMA_DATA_BYTES / 512 + 1) as u16, DMA_DATA_BYTES + 512), (0, 512)] {
        assert_eq!(prepare_command(&runtime, 0x25, 0, sectors, bytes, false), Err(RequestError::InvalidArgument));
        assert_eq!(backing, before);
    }
}

#[test]
fn command_preparation_only_resets_the_active_header_and_table() {
    let mut backing = vec![u64::from_le_bytes([0xa5; 8]); DMA_TOTAL_BYTES / 8 + 2];
    let runtime = AhciRuntime {
        dma_physical: 0x4000_0000,
        dma_virtual: backing.as_mut_ptr() as usize + 8,
        dma_data_bytes: DMA_DATA_BYTES,
    };
    let metadata_bytes = 32 + COMMAND_TABLE_PRDT_OFFSET + 16;
    assert_eq!(metadata_bytes, 176);
    for (command, sectors, bytes, write) in [
        (0x35, (DMA_DATA_BYTES / 512) as u16, DMA_DATA_BYTES, true),
        (0xea, 0, 0, false),
        (ATA_IDENTIFY_DEVICE, 0, 512, false),
        (0x25, 1, 512, false),
    ] {
        // Model a completed command's device-written byte count. It must be
        // cleared on reuse, including a following zero-data flush.
        let header = (runtime.dma_virtual + DMA_COMMAND_LIST_OFFSET) as *mut u32;
        unsafe { ptr::write_volatile(header.add(1), 0x40000); }
        prepare_command(&runtime, command, 0, sectors, bytes, write).unwrap();
        let memory = unsafe {
            core::slice::from_raw_parts(backing.as_ptr().cast::<u8>(), backing.len() * 8)
        };
        let data = &memory[8..8 + DMA_TOTAL_BYTES];
        assert_eq!(word(data, 4), 0);
        assert!(data[16..32].iter().all(|&byte| byte == 0));
        let table = DMA_COMMAND_TABLE_OFFSET;
        assert!(data[table + 20..table + COMMAND_TABLE_PRDT_OFFSET].iter().all(|&byte| byte == 0));
        if bytes == 0 {
            assert!(data[table + COMMAND_TABLE_PRDT_OFFSET..table + COMMAND_TABLE_PRDT_OFFSET + 16]
                .iter().all(|&byte| byte == 0));
        }
        assert_eq!(&data[table..table + 3], &[0x27, 0x80, command]);
        for (offset, &byte) in data.iter().enumerate() {
            if offset < 32 || (table..table + COMMAND_TABLE_PRDT_OFFSET + 16).contains(&offset) {
                continue;
            }
            assert_eq!(byte, 0xa5, "command={command:#x} offset={offset:#x}");
        }
        assert!(memory[..8].iter().chain(memory[8 + DMA_TOTAL_BYTES..].iter())
            .all(|&byte| byte == 0xa5));
    }
    println!("AHCI-COMMAND-PREP zeroed_before={DMA_DATA_OFFSET} zeroed_after={metadata_bytes}");
}

#[test]
fn shared_dma_target_encodes_the_provider_address_and_rejects_invalid_spans() {
    let mut backing = vec![0u64; DMA_TOTAL_BYTES / 8];
    let runtime = AhciRuntime { dma_physical: 0x4000_0000, dma_virtual: backing.as_mut_ptr() as Word, dma_data_bytes: DMA_DATA_BYTES };
    let target = 0x0000_1234_5678_0800;
    prepare_command_to(&runtime, 0x25, 7, 128, 65536, false, target).unwrap();
    let bytes = unsafe { core::slice::from_raw_parts(backing.as_ptr().cast::<u8>(), DMA_TOTAL_BYTES) };
    let prdt = DMA_COMMAND_TABLE_OFFSET + COMMAND_TABLE_PRDT_OFFSET;
    assert_eq!(u64::from(word(bytes, prdt)) | (u64::from(word(bytes, prdt + 4)) << 32), target as u64);
    let before = backing.clone();
    for invalid in [target + 1, usize::MAX - 511] {
        assert_eq!(prepare_command_to(&runtime, 0x25, 7, 128, 65536, false, invalid),
            Err(RequestError::InvalidArgument));
        assert_eq!(backing, before);
    }
}
