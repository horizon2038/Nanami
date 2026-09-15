use super::*;

#[test]
fn each_block_in_a_batch_gets_its_own_cache_contents() {
    let mut runtime = runtime(vec![]);
    for i in 0..4 {
        runtime.scratch[i * BLOCK_SIZE..(i + 1) * BLOCK_SIZE].fill(i as u8 + 1);
    }
    write_blocks(&mut runtime, 100, 4).unwrap();
    for i in 0..4 {
        runtime.scratch.fill(0);
        read_block(&mut runtime, 100 + i).unwrap();
        assert!(runtime.scratch[..BLOCK_SIZE]
            .iter()
            .all(|b| *b == i as u8 + 1));
    }
    FAKE.with(|fake| assert!(fake.borrow().reads.is_empty()));
}

#[test]
fn short_or_failed_write_invalidates_affected_cache_and_is_not_retried() {
    for result in [Ok(BLOCK_SIZE), Err(RequestError::Transport)] {
        let mut runtime = runtime(vec![]);
        for block in [100, 101, 105] {
            read_block(&mut runtime, block).unwrap();
        }
        FAKE.with(|fake| fake.borrow_mut().write_result = Some(result));
        runtime.scratch[..2 * BLOCK_SIZE].fill(0x32);
        assert!(write_blocks(&mut runtime, 100, 2).is_err());
        assert!(runtime
            .block_cache
            .iter()
            .any(|c| c.valid && c.block == 105));
        assert!(!runtime
            .block_cache
            .iter()
            .any(|c| c.valid && (100..102).contains(&c.block)));
        read_block(&mut runtime, 100).unwrap();
        assert!(runtime.scratch[..BLOCK_SIZE].iter().all(|b| *b == 0x32));
        FAKE.with(|fake| assert_eq!(fake.borrow().writes.len(), 1));
    }
}

#[test]
fn zeroing_never_reads_disk_and_rejects_invalid_targets() {
    let mut runtime = runtime(vec![]);
    zero_block(&mut runtime, 100).unwrap();
    assert_eq!(
        zero_block(&mut runtime, 1024),
        Err(RequestError::InvalidArgument)
    );
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert!(fake.reads.is_empty());
        assert_eq!(fake.writes, [(100, 1)]);
        assert!(fake.disk[100 * BLOCK_SIZE..101 * BLOCK_SIZE]
            .iter()
            .all(|b| *b == 0));
    });
}

#[test]
fn invalid_block_runs_fail_before_ipc() {
    let mut runtime = runtime(vec![]);
    for (block, count) in [
        (100, 0),
        (1023, 2),
        (usize::MAX, 2),
        (100, 17),
        (0, usize::MAX),
    ] {
        assert_eq!(
            write_blocks(&mut runtime, block, count),
            Err(RequestError::InvalidArgument)
        );
    }
    FAKE.with(|fake| assert!(fake.borrow().writes.is_empty()));
}
