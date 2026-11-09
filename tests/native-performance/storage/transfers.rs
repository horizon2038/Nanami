use super::*;

#[test]
fn long_sparse_reads_preserve_data_partial_edges_and_output_guards() {
    for mixed in [false, true] {
        let mut mapping = vec![0; 64];
        if mixed {
            mapping[31] = 100;
            mapping[63] = 101;
        }
        let mut runtime = runtime(mapping.clone());
        let inode = Ext2Inode { size: (64 * BLOCK_SIZE - 11) as u32 };
        let mut output = vec![0xa5u8; 64 * BLOCK_SIZE + 8];
        let session = ClientSession { shm_local: output.as_mut_ptr() as Word, shm_size: output.len() };
        RUN_CALLS.with(|calls| calls.set(0));
        let copied = read_file(&mut runtime, session, inode, 3, 64 * BLOCK_SIZE - 3, 2).unwrap();
        assert_eq!(copied, 64 * BLOCK_SIZE - 14);
        for (index, byte) in output.iter().copied().enumerate() {
            let expected = if index < 2 || index >= 2 + copied {
                0xa5
            } else if mapping[(index - 2 + 3) / BLOCK_SIZE] == 0 {
                0
            } else {
                0x77
            };
            assert_eq!(byte, expected, "mixed={mixed} index={index}");
        }
        let calls = RUN_CALLS.with(|calls| calls.get());
        println!("SPARSE-READ transfer-blocks=64 mixed={mixed} run-queries={calls}");
        assert_eq!(calls, if mixed { 6 } else { 4 });
    }
}

#[test]
fn warm_file_read_copies_dirty_cache_directly_without_staging_payload_in_scratch() {
    let mut runtime = runtime((100..104).collect());
    let mut inode = Ext2Inode { size: (4 * BLOCK_SIZE) as u32 };
    let input: Vec<u8> = (0..4 * BLOCK_SIZE).map(|index| (index * 17 + 3) as u8).collect();
    write(&mut runtime, &mut inode, 0, &input).unwrap();
    let mut output = vec![0xaau8; input.len() + 9];
    let session = ClientSession { shm_local: output.as_mut_ptr() as Word, shm_size: output.len() };
    assert_eq!(read_file(&mut runtime, session, inode, 7, input.len() - 19, 3), Ok(input.len() - 19));
    assert_eq!(&output[3..input.len() - 16], &input[7..input.len() - 12]);
    assert!(output[..3].iter().chain(output[input.len() - 16..].iter()).all(|byte| *byte == 0xaa));
    // The mapping stub leaves 0xbb in scratch. A scratch-staged cache read would
    // replace this with payload despite doing no device I/O.
    assert!(runtime.scratch.iter().all(|byte| *byte == 0xbb));
    FAKE.with(|fake| assert!(fake.borrow().reads.is_empty()));
    assert!(runtime.block_cache.is_dirty());
}

#[test]
fn mixed_cache_file_read_retains_batched_miss_io_and_dirty_data() {
    let mut runtime = runtime((100..104).collect());
    runtime.block_cache.store(100, &vec![0x31; BLOCK_SIZE], Some(block_cache::BlockKind::Data));
    runtime.block_cache.store(102, &vec![0x32; BLOCK_SIZE], Some(block_cache::BlockKind::Data));
    let mut output = vec![0xaau8; 4 * BLOCK_SIZE];
    let session = ClientSession { shm_local: output.as_mut_ptr() as Word, shm_size: output.len() };
    let inode = Ext2Inode { size: output.len() as u32 };
    assert_eq!(read_file(&mut runtime, session, inode, 0, output.len(), 0), Ok(output.len()));
    for (index, byte) in output.iter().copied().enumerate() {
        assert_eq!(byte, match index / BLOCK_SIZE { 0 => 0x31, 2 => 0x32, _ => 0x77 });
    }
    FAKE.with(|fake| assert_eq!(fake.borrow().reads, [(101, 1), (103, 1)]));
}

#[test]
fn cached_payload_does_not_bypass_device_or_output_bounds() {
    let mut runtime = runtime(vec![1024]);
    runtime.block_cache.store(1024, &vec![0x31; BLOCK_SIZE], None);
    let mut output = vec![0xaau8; BLOCK_SIZE];
    let session = ClientSession { shm_local: output.as_mut_ptr() as Word, shm_size: output.len() };
    let inode = Ext2Inode { size: BLOCK_SIZE as u32 };
    assert_eq!(read_file(&mut runtime, session, inode, 0, BLOCK_SIZE, 0), Err(OS_RESPONSE_FATAL));
    assert_eq!(read_file(&mut runtime, session, inode, 0, BLOCK_SIZE, 1), Err(OS_RESPONSE_INVALID_ARGUMENT));
    assert!(output.iter().all(|byte| *byte == 0xaa));
    FAKE.with(|fake| assert!(fake.borrow().reads.is_empty()));
}

#[test]
fn pinned_read_pages_receive_each_successful_write_and_growth_is_rejected() {
    let mut runtime = runtime(vec![100, 101]);
    runtime.mapped_pages.pinned = true;
    let mut inode = Ext2Inode { size: (2 * BLOCK_SIZE) as u32 };
    let bytes = [0x38; 64];
    assert_eq!(write(&mut runtime, &mut inode, BLOCK_SIZE - 24, &bytes), Ok(64));
    let updates = runtime.mapped_pages.updates.borrow();
    assert_eq!(updates.len(), 2);
    assert_eq!((updates[0].1, updates[0].2.as_slice()), (BLOCK_SIZE - 24, &bytes[..24]));
    assert_eq!((updates[1].1, updates[1].2.as_slice()), (BLOCK_SIZE, &bytes[24..]));
    drop(updates);
    assert_eq!(write(&mut runtime, &mut inode, 2 * BLOCK_SIZE, &bytes), Err(vfs::VFS_RESPONSE_MAPPING_BUSY));
    assert_eq!(write(&mut runtime, &mut inode, 3 * BLOCK_SIZE, &[]), Ok(0));
    assert_eq!(inode.size, (2 * BLOCK_SIZE) as u32);
    assert_eq!(runtime.mapped_pages.updates.borrow().len(), 2);
}

#[test]
fn sparse_reads_zero_holes_and_continue_to_later_data() {
    let mut runtime = runtime(vec![0, 100, 0, 101, 0]);
    let mut output = vec![0xaau8; 5 * BLOCK_SIZE];
    let session = ClientSession {
        shm_local: output.as_mut_ptr() as Word,
        shm_size: output.len(),
    };
    let inode = Ext2Inode {
        size: (output.len() - 11) as u32,
    };
    assert_eq!(
        data_io::read_file(&mut runtime, session, inode, 3, output.len() - 3, 0),
        Ok(output.len() - 14)
    );
    for (index, byte) in output.iter().copied().enumerate() {
        let expected = if index >= output.len() - 14 {
            0xaa
        } else if matches!((index + 3) / BLOCK_SIZE, 1 | 3) {
            0x77
        } else {
            0
        };
        assert_eq!(byte, expected, "offset {index}");
    }
}

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
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.inode_writes, 1);
        assert_eq!(fake.inode_recounts, 0);
    });
}

#[test]
fn tiny_growth_recounts_only_when_a_new_payload_block_is_allocated() {
    let mut runtime = runtime(vec![0; 3]);
    let mut inode = Ext2Inode { size: 0 };
    for offset in 0..2 * BLOCK_SIZE + 17 {
        write(&mut runtime, &mut inode, offset, &[offset as u8]).unwrap();
    }
    assert_eq!(inode.size, (2 * BLOCK_SIZE + 17) as u32);
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.inode_writes, 2 * BLOCK_SIZE + 17);
        assert_eq!(fake.inode_recounts, 3);
    });
    let mut output = vec![0u8; 3 * BLOCK_SIZE];
    let session = ClientSession { shm_local: output.as_mut_ptr() as Word, shm_size: output.len() };
    let got = read_file(&mut runtime, session, inode, 0, inode.size as usize, 0).unwrap();
    let data = &output[..got];
    assert!(data.iter().enumerate().all(|(offset, &value)| value == offset as u8));
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
