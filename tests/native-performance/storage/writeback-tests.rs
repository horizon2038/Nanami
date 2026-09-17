use super::*;

#[test]
fn writes_coalesce_until_sync_and_data_barrier_precedes_metadata() {
    let mut runtime = runtime(vec![]);
    runtime.scratch[..BLOCK_SIZE].fill(0x11);
    write_block(&mut runtime, 5).unwrap();
    for value in [0x22, 0x33, 0x44] {
        runtime.scratch[..2 * BLOCK_SIZE].fill(value);
        write_blocks(&mut runtime, 100, 2).unwrap();
    }
    FAKE.with(|fake| assert!(fake.borrow().events.is_empty()));
    runtime.scratch.fill(0xcc);
    flush_blocks(&mut runtime).unwrap();
    assert!(runtime.scratch.iter().all(|b| *b == 0xcc));
    assert!(!runtime.block_cache.is_dirty());
    flush_blocks(&mut runtime).unwrap(); // No redundant barrier after a clean sync.
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(
            fake.events,
            [('W', 100, 2), ('F', 0, 0), ('W', 5, 1), ('F', 0, 0)]
        );
        assert!(fake.durable[100 * BLOCK_SIZE..102 * BLOCK_SIZE]
            .iter()
            .all(|b| *b == 0x44));
        assert!(fake.durable[5 * BLOCK_SIZE..6 * BLOCK_SIZE]
            .iter()
            .all(|b| *b == 0x11));
    });
}

#[test]
fn mixed_multiblock_reads_preserve_dirty_hits_and_batch_misses_at_their_offsets() {
    let mut runtime = runtime(vec![]);
    for (block, value) in [(100, 0x11), (103, 0x33)] {
        runtime.scratch[..BLOCK_SIZE].fill(value);
        write_blocks(&mut runtime, block, 1).unwrap();
    }
    read_blocks(&mut runtime, 100, 6).unwrap();
    for (index, value) in [0x11, 0x77, 0x77, 0x33, 0x77, 0x77].into_iter().enumerate() {
        assert!(
            runtime.scratch[index * BLOCK_SIZE..(index + 1) * BLOCK_SIZE]
                .iter()
                .all(|b| *b == value)
        );
    }
    read_blocks(&mut runtime, 100, 6).unwrap();
    FAKE.with(|fake| {
        assert_eq!(fake.borrow().reads, [(101, 2), (104, 2)]);
        assert!(fake.borrow().writes.is_empty());
    });
}

#[test]
fn pressure_flush_preserves_pending_payload_and_never_evicts_dirty_on_reads() {
    let mut runtime = runtime(vec![]);
    runtime.block_cache = block_cache::BlockCache::new(BLOCK_SIZE, BUFFER_SIZE, BUFFER_SIZE);
    runtime.scratch.fill(0x42);
    write_blocks(&mut runtime, 100, 16).unwrap();
    read_blocks(&mut runtime, 200, 16).unwrap(); // A full dirty cache bypasses clean insertion.
    read_blocks(&mut runtime, 100, 16).unwrap();
    assert!(runtime.scratch.iter().all(|b| *b == 0x42));
    runtime.scratch.fill(0x24);
    write_blocks(&mut runtime, 200, 16).unwrap(); // Flush old batch, keep staging intact.
    assert!(runtime.scratch.iter().all(|b| *b == 0x24));
    flush_blocks(&mut runtime).unwrap();
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(
            fake.events,
            [('W', 100, 16), ('F', 0, 0), ('W', 200, 16), ('F', 0, 0)]
        );
        assert!(fake.durable[200 * BLOCK_SIZE..216 * BLOCK_SIZE]
            .iter()
            .all(|b| *b == 0x24));
    });
}

#[test]
fn flush_failure_retains_dirty_buffers_and_latches_the_error_without_replay() {
    let mut runtime = runtime(vec![]);
    runtime.scratch[..BLOCK_SIZE].fill(0x22);
    write_blocks(&mut runtime, 100, 1).unwrap();
    zero_block(&mut runtime, 5).unwrap();
    FAKE.with(|fake| fake.borrow_mut().flush_error = Some(RequestError::Transport));
    assert_eq!(flush_blocks(&mut runtime), Err(RequestError::Transport));
    assert!(runtime.block_cache.is_dirty());
    assert_eq!(flush_blocks(&mut runtime), Err(RequestError::Transport));
    assert_eq!(write_block(&mut runtime, 200), Err(RequestError::Transport));
    read_block(&mut runtime, 100).unwrap();
    assert!(runtime.scratch[..BLOCK_SIZE].iter().all(|b| *b == 0x22));
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.events, [('W', 100, 1), ('F', 0, 0)]); // No metadata publication.
        assert!(fake.durable.iter().all(|b| *b == 0x77));
    });
}

#[test]
fn writeback_timer_is_one_shot_and_not_rearmed_by_more_writes_or_an_explicit_sync() {
    let mut runtime = runtime(vec![]);
    writeback::after_request(&mut runtime).unwrap();
    FAKE.with(|fake| assert_eq!(fake.borrow().alarms, 0));
    for block in [100, 101] {
        zero_block(&mut runtime, block).unwrap();
        writeback::after_request(&mut runtime).unwrap();
    }
    flush_blocks(&mut runtime).unwrap();
    zero_block(&mut runtime, 102).unwrap();
    writeback::after_request(&mut runtime).unwrap();
    FAKE.with(|fake| assert_eq!(fake.borrow().alarms, 1));
    // The previously armed alarm is still valid and flushes the new dirties.
    runtime.writeback.armed = false;
    flush_blocks(&mut runtime).unwrap();
    writeback::after_request(&mut runtime).unwrap();
    FAKE.with(|fake| assert_eq!(fake.borrow().alarms, 1));
}

#[test]
fn failed_timer_falls_back_to_synchronous_writeback() {
    let mut runtime = runtime(vec![]);
    FAKE.with(|fake| fake.borrow_mut().timer_error = Some(RequestError::Transport));
    for block in [100, 101] {
        zero_block(&mut runtime, block).unwrap();
        writeback::after_request(&mut runtime).unwrap();
        assert!(!runtime.block_cache.is_dirty());
        assert!(!runtime.writeback.armed);
    }
    FAKE.with(|fake| assert_eq!(fake.borrow().alarms, 1));
}

#[test]
fn fsync_checks_handle_ownership_and_sync_reports_persistence_errors() {
    let mut runtime = runtime(vec![]);
    zero_block(&mut runtime, 100).unwrap();
    for (pid, handle, status) in [
        (17, 0, OS_RESPONSE_INVALID_ARGUMENT),
        (16, 1, OS_RESPONSE_INVALID_DESCRIPTOR),
        (16, usize::MAX, OS_RESPONSE_INVALID_DESCRIPTOR),
    ] {
        assert_eq!(
            writeback::handle_sync(
                ServiceRequest {
                    identifier: pid,
                    code: vfs::VFS_REQUEST_FSYNC,
                    arg0: handle
                },
                &mut runtime
            )
            .0,
            status
        );
    }
    FAKE.with(|fake| assert!(fake.borrow().events.is_empty()));
    assert_eq!(
        writeback::handle_sync(
            ServiceRequest {
                identifier: 16,
                code: vfs::VFS_REQUEST_FSYNC,
                arg0: 0
            },
            &mut runtime
        )
        .0,
        OS_RESPONSE_OK
    );
    zero_block(&mut runtime, 101).unwrap();
    FAKE.with(|fake| fake.borrow_mut().flush_error = Some(RequestError::Protocol));
    assert_eq!(
        writeback::handle_sync(
            ServiceRequest {
                identifier: 16,
                code: vfs::VFS_REQUEST_SYNC,
                arg0: 0
            },
            &mut runtime
        )
        .0,
        OS_RESPONSE_FATAL
    );
}
