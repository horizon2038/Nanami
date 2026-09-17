use super::*;

#[test]
fn contiguous_overwrite_batches_io_without_reading_old_data_or_rewriting_inode() {
    let mut runtime = runtime((100..164).collect());
    let mut inode = Ext2Inode {
        size: 64 * BLOCK_SIZE as u32,
    };
    let input: Vec<u8> = (0..64 * BLOCK_SIZE).map(|i| (i * 17 + 3) as u8).collect();
    assert_eq!(write(&mut runtime, &mut inode, 0, &input), Ok(input.len()));
    flush_blocks(&mut runtime).unwrap();
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert!(fake.reads.is_empty());
        assert_eq!(fake.writes, [(100, 16), (116, 16), (132, 16), (148, 16)]);
        assert_eq!(fake.inode_writes, 0);
        assert_eq!(&fake.disk[100 * BLOCK_SIZE..164 * BLOCK_SIZE], input);
    });
}

#[test]
fn partial_edges_preserve_surrounding_bytes_and_batch_only_full_blocks() {
    let mut runtime = runtime((100..120).collect());
    let mut inode = Ext2Inode {
        size: 20 * BLOCK_SIZE as u32,
    };
    let input = vec![0x31; 17 * BLOCK_SIZE + 19];
    assert_eq!(write(&mut runtime, &mut inode, 3, &input), Ok(input.len()));
    flush_blocks(&mut runtime).unwrap();
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.reads, [(100, 1), (117, 1)]);
        assert_eq!(fake.writes, [(100, 16), (116, 2)]);
        assert_eq!(
            &fake.disk[100 * BLOCK_SIZE + 3..100 * BLOCK_SIZE + 3 + input.len()],
            input
        );
        assert!(fake.disk[100 * BLOCK_SIZE..100 * BLOCK_SIZE + 3]
            .iter()
            .all(|b| *b == 0x77));
        assert!(
            fake.disk[100 * BLOCK_SIZE + 3 + input.len()..120 * BLOCK_SIZE]
                .iter()
                .all(|b| *b == 0x77)
        );
    });
}

#[test]
fn fragmented_runs_do_not_overwrite_intervening_blocks() {
    let mut runtime = runtime(vec![100, 101, 200, 201]);
    let mut inode = Ext2Inode {
        size: 4 * BLOCK_SIZE as u32,
    };
    write(&mut runtime, &mut inode, 0, &vec![0x12; 4 * BLOCK_SIZE]).unwrap();
    flush_blocks(&mut runtime).unwrap();
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.writes, [(100, 2), (200, 2)]);
        assert!(fake.disk[102 * BLOCK_SIZE..200 * BLOCK_SIZE]
            .iter()
            .all(|b| *b == 0x77));
    });
}

#[test]
fn allocation_inside_existing_size_still_persists_inode_block_accounting() {
    let mut runtime = runtime(vec![100, 0, 200]);
    let mut inode = Ext2Inode {
        size: 3 * BLOCK_SIZE as u32,
    };
    write(&mut runtime, &mut inode, 0, &vec![0x43; 3 * BLOCK_SIZE]).unwrap();
    assert_eq!(runtime.mapping, [100, 512, 200]);
    flush_blocks(&mut runtime).unwrap();
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.inode_writes, 1);
        assert!(fake.reads.is_empty());
        for block in [100, 512, 200] {
            assert!(fake.disk[block * BLOCK_SIZE..(block + 1) * BLOCK_SIZE]
                .iter()
                .all(|b| *b == 0x43));
        }
    });
}

#[test]
fn extending_a_partial_new_block_zeroes_bytes_outside_the_write() {
    let mut runtime = runtime(vec![0]);
    let mut inode = Ext2Inode { size: 0 };
    write(&mut runtime, &mut inode, 7, &[0x21; 9]).unwrap();
    assert_eq!(inode.size, 16);
    flush_blocks(&mut runtime).unwrap();
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.inode_writes, 1);
        assert!(fake.reads.is_empty());
        assert_eq!(fake.writes, [(512, 1)]);
        let block = &fake.disk[512 * BLOCK_SIZE..513 * BLOCK_SIZE];
        assert_eq!(&block[7..16], &[0x21; 9]);
        assert!(block[..7].iter().chain(block[16..].iter()).all(|b| *b == 0));
    });
}

#[test]
fn new_full_blocks_are_written_once_without_a_separate_zero_write() {
    let mut runtime = runtime(vec![0; 3]);
    let mut inode = Ext2Inode { size: 0 };
    let input: Vec<u8> = (0..3 * BLOCK_SIZE).map(|i| (i * 17 + 3) as u8).collect();
    assert_eq!(write(&mut runtime, &mut inode, 0, &input), Ok(input.len()));
    flush_blocks(&mut runtime).unwrap();
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.writes, [(512, 3)]);
        assert!(fake.reads.is_empty());
        assert_eq!(&fake.disk[512 * BLOCK_SIZE..515 * BLOCK_SIZE], input);
        assert_eq!(fake.inode_writes, 1);
    });
}

#[test]
fn delayed_initialization_error_is_reported_by_sync_without_replay() {
    for result in [Err(RequestError::Transport), Ok(BLOCK_SIZE / 2)] {
        let mut runtime = runtime(vec![0]);
        let mut inode = Ext2Inode { size: 0 };
        FAKE.with(|fake| fake.borrow_mut().write_result = Some(result));
        assert_eq!(write(&mut runtime, &mut inode, 7, &[0x21; 9]), Ok(9));
        assert_eq!(runtime.mapping, [512]);
        assert_eq!(inode.size, 16);
        assert!(flush_blocks(&mut runtime).is_err());
        assert!(flush_blocks(&mut runtime).is_err());
        FAKE.with(|fake| assert_eq!(fake.borrow().writes, [(512, 1)]));
    }
}

#[test]
fn unchanged_existing_allocation_still_updates_inode_when_size_grows() {
    let mut runtime = runtime(vec![100]);
    let mut inode = Ext2Inode { size: 8 };
    write(&mut runtime, &mut inode, 0, &vec![0x53; BLOCK_SIZE]).unwrap();
    assert_eq!(inode.size, BLOCK_SIZE as u32);
    FAKE.with(|fake| assert_eq!(fake.borrow().inode_writes, 1));
}

#[test]
fn delayed_full_write_error_is_not_retried_or_reported_as_sync_success() {
    let mut runtime = runtime(vec![100, 101]);
    let mut inode = Ext2Inode {
        size: 2 * BLOCK_SIZE as u32,
    };
    FAKE.with(|fake| fake.borrow_mut().write_result = Some(Err(RequestError::Transport)));
    assert_eq!(
        write(&mut runtime, &mut inode, 0, &vec![0x65; 2 * BLOCK_SIZE]),
        Ok(2 * BLOCK_SIZE)
    );
    assert_eq!(flush_blocks(&mut runtime), Err(RequestError::Transport));
    assert_eq!(flush_blocks(&mut runtime), Err(RequestError::Transport));
    FAKE.with(|fake| assert_eq!(fake.borrow().writes, [(100, 2)]));
}
