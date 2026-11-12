use super::*;

fn megabyte_runtime(mapping: Vec<u32>, window: usize) -> Ext2Runtime {
    let mut runtime = runtime_with_window(mapping, window, 1024 * 1024);
    runtime.block_count = 4096;
    FAKE.with(|f| {
        let mut f = f.borrow_mut();
        f.disk.resize(4096 * BLOCK_SIZE, 0x77);
        f.durable.resize(4096 * BLOCK_SIZE, 0x77);
    });
    runtime
}

#[test]
fn cold_megabyte_device_and_file_reads_fill_cache_in_one_payload_copy() {
    for file_read in [false, true] {
        let mut r = megabyte_runtime((100..1124).collect(), 1024 * 1024);
        let mut guarded = vec![0xa5u8; 1024 * 1024 + 6];
        r.block_cache.take_store_copies();
        if file_read {
            let session = ClientSession { shm_local: guarded.as_mut_ptr() as Word, shm_size: guarded.len() };
            assert_eq!(read_file(&mut r, session, Ext2Inode { size: 1024 * 1024 }, 0, 1024 * 1024, 3), Ok(1024 * 1024));
            assert!(guarded[3..guarded.len() - 3].iter().all(|&b| b == 0x77));
            assert!(guarded[..3].iter().chain(&guarded[guarded.len() - 3..]).all(|&b| b == 0xa5));
        } else {
            read_blocks(&mut r, 100, 1024).unwrap();
            assert!(r.scratch.iter().all(|&b| b == 0x77));
        }
        assert_eq!(r.block_cache.take_store_copies(), 1);
        FAKE.with(|f| assert_eq!(f.borrow().reads, [(100, 1024)]));
        for block in 100..1124 {
            let mut bytes = [0u8; BLOCK_SIZE];
            assert!(r.block_cache.load(block, &mut bytes));
            assert!(bytes.iter().all(|&b| b == 0x77));
        }
    }
}

#[test]
fn megabyte_window_reads_large_library_payload_in_four_commands_with_exact_edges() {
    let bytes: usize = 3_171_128; // libstdc++ size in the current Tiny Core image.
    let blocks = bytes.div_ceil(BLOCK_SIZE);
    let mut results = Vec::new();
    for window in [256 * 1024, 1024 * 1024] {
        let mut runtime = megabyte_runtime((100..100 + blocks as u32).collect(), window);
        FAKE.with(|fake| {
            let mut fake = fake.borrow_mut();
            for block in 100..100 + blocks {
                fake.disk[block * BLOCK_SIZE..(block + 1) * BLOCK_SIZE].fill((block % 251) as u8);
            }
        });
        let mut output = vec![0xaa; bytes + 8];
        let session = ClientSession { shm_local: output.as_mut_ptr() as Word, shm_size: output.len() };
        let length = bytes - 10;
        assert_eq!(read_file(&mut runtime, session, Ext2Inode { size: bytes as u32 },
            3, length, 2), Ok(length));
        for (index, &byte) in output.iter().enumerate() {
            let expected = if index < 2 || index >= length + 2 { 0xaa }
                else { ((100 + (index - 2 + 3) / BLOCK_SIZE) % 251) as u8 };
            assert_eq!(byte, expected);
        }
        results.push((output, FAKE.with(|f| f.borrow().reads.clone())));
    }
    assert_eq!(results[0].0, results[1].0);
    assert_eq!(results[0].1.len(), 13);
    assert_eq!(results[1].1.len(), 4);
    assert!(results[1].1[..3].iter().all(|(_, count)| *count == 1024));
    println!("AHCI-MEGABYTE-FILE bytes={bytes} commands_before=13 commands_after=4 all_bytes_and_guards_equal=true");
}

#[test]
fn megabyte_mixed_read_preserves_dirty_bytes_and_private_output_after_late_failure() {
    for fail in [false, true] {
        let mut runtime = megabyte_runtime((100..1124).collect(), 1024 * 1024);
        for (block, byte) in [(100, 0x31), (612, 0x32), (1123, 0x33)] {
            runtime.block_cache.store(block, &vec![byte; BLOCK_SIZE], Some(block_cache::BlockKind::Data));
        }
        if fail { FAKE.with(|f| f.borrow_mut().read_error = Some(613)); }
        let mut output = vec![0xaau8; 1024 * BLOCK_SIZE + 4];
        let session = ClientSession { shm_local: output.as_mut_ptr() as Word, shm_size: output.len() };
        let result = read_file(&mut runtime, session, Ext2Inode { size: (1024 * BLOCK_SIZE) as u32 },
            0, 1024 * BLOCK_SIZE, 2);
        if fail {
            assert_eq!(result, Err(OS_RESPONSE_FATAL));
            assert!(output.iter().all(|b| *b == 0xaa));
            assert!((101..612).chain(613..1123).all(|block| !runtime.block_cache.contains(block)));
        } else {
            assert_eq!(result, Ok(1024 * BLOCK_SIZE));
            for (index, &byte) in output[2..2 + 1024 * BLOCK_SIZE].iter().enumerate() {
                assert_eq!(byte, match index / BLOCK_SIZE { 0 => 0x31, 512 => 0x32, 1023 => 0x33, _ => 0x77 });
            }
            assert_eq!(&output[..2], &[0xaa; 2]); assert_eq!(&output[output.len() - 2..], &[0xaa; 2]);
        }
        for (block, byte) in [(100, 0x31), (612, 0x32), (1123, 0x33)] {
            let mut cached = vec![0; BLOCK_SIZE];
            assert!(runtime.block_cache.load(block, &mut cached));
            assert!(cached.iter().all(|b| *b == byte));
        }
        FAKE.with(|f| {
            let f = f.borrow(); assert!(f.writes.is_empty());
            assert_eq!(f.reads[0], (101, 511));
            assert_eq!(f.reads[1..], vec![(613, 510); if fail { BLOCK_READ_RETRY_LIMIT } else { 1 }]);
        });
    }
}

#[test]
fn cold_stream_preserves_hot_metadata_and_avoids_its_followup_device_reads() {
    let mut results = Vec::new();
    for streaming in [false, true] {
        let mut runtime = runtime_with_window((100..612).collect(), 64 * 1024, 128 * 1024);
        for block in 1..9 { runtime.block_cache.store(block, &vec![0x77; BLOCK_SIZE], None); }
        let mut output = vec![0xaa; 512 * BLOCK_SIZE + 4];
        for part in 0..8 {
            let start = 2 + part * 64 * BLOCK_SIZE;
            read_blocks_into(&mut runtime, 100 + part * 64, 64, 0,
                &mut output[start..start + 64 * BLOCK_SIZE], streaming).unwrap();
        }
        assert!(output[..2].iter().chain(output[output.len()-2..].iter()).all(|&b| b == 0xaa));
        assert!(output[2..output.len()-2].iter().all(|&b| b == 0x77));
        let before = FAKE.with(|fake| fake.borrow().reads.len());
        for block in 1..9 {
            assert_eq!(runtime.block_cache.contains(block), streaming);
            read_block(&mut runtime, block).unwrap();
        }
        let extra = FAKE.with(|fake| fake.borrow().reads.len()) - before;
        results.push((output, before, extra));
    }
    assert_eq!(results[0].0, results[1].0);
    assert_eq!((results[0].1, results[0].2), (8, 8));
    assert_eq!((results[1].1, results[1].2), (8, 0));
}

#[test]
fn oversized_file_read_streams_cold_runs_with_exact_partial_edges_and_guards() {
    let mut runtime = runtime_with_window((100..612).collect(), 64 * 1024, 128 * 1024);
    runtime.block_cache.store(1, &vec![0x77; BLOCK_SIZE], None);
    FAKE.with(|fake| {
        let mut fake = fake.borrow_mut();
        for block in 100..612 { fake.disk[block * BLOCK_SIZE..(block + 1) * BLOCK_SIZE].fill(block as u8); }
    });
    let inode = Ext2Inode { size: (512 * BLOCK_SIZE - 11) as u32 };
    let mut output = vec![0xaa; 512 * BLOCK_SIZE + 4];
    let session = ClientSession { shm_local: output.as_mut_ptr() as Word, shm_size: output.len() };
    let bytes = read_file(&mut runtime, session, inode, 3, 512 * BLOCK_SIZE - 3, 2).unwrap();
    assert_eq!(bytes, 512 * BLOCK_SIZE - 14);
    for (index, &value) in output.iter().enumerate() {
        let expected = if index < 2 || index >= 2 + bytes { 0xaa }
            else { (100 + (index - 2 + 3) / BLOCK_SIZE) as u8 };
        assert_eq!(value, expected, "index={index}");
    }
    assert!(runtime.block_cache.contains(1));
    assert!((100..612).all(|block| !runtime.block_cache.contains(block)));
    FAKE.with(|fake| assert_eq!(fake.borrow().reads.len(), 8));
}

#[test]
fn oversized_stream_never_bypasses_resident_dirty_payload() {
    for dirty in [110, 200, 355] {
        let mut runtime = runtime_with_window((100..356).collect(), 64 * 1024, 128 * 1024);
        runtime.block_cache.store(dirty, &vec![0x31; BLOCK_SIZE], Some(block_cache::BlockKind::Data));
        let mut output = vec![0u8; 256 * BLOCK_SIZE];
        let session = ClientSession { shm_local: output.as_mut_ptr() as Word, shm_size: output.len() };
        assert_eq!(read_file(&mut runtime, session, Ext2Inode { size: output.len() as u32 },
            0, output.len(), 0), Ok(output.len()));
        for (index, &value) in output.iter().enumerate() {
            assert_eq!(value, if 100 + index / BLOCK_SIZE == dirty { 0x31 } else { 0x77 });
        }
        assert!(runtime.block_cache.is_dirty());
        let mut retained = vec![0; BLOCK_SIZE];
        assert!(runtime.block_cache.load(dirty, &mut retained));
        assert!(retained.iter().all(|&b| b == 0x31));
        FAKE.with(|fake| assert!(fake.borrow().writes.is_empty()));
    }
}

#[test]
fn failed_cold_stream_keeps_output_cache_and_retry_contract() {
    let mut runtime = runtime_with_window((100..612).collect(), 64 * 1024, 128 * 1024);
    runtime.block_cache.store(1, &vec![0x77; BLOCK_SIZE], None);
    FAKE.with(|fake| fake.borrow_mut().read_error = Some(100));
    let mut output = vec![0xaa; 512 * BLOCK_SIZE];
    let session = ClientSession { shm_local: output.as_mut_ptr() as Word, shm_size: output.len() };
    assert_eq!(read_file(&mut runtime, session, Ext2Inode { size: output.len() as u32 },
        0, output.len(), 0), Err(OS_RESPONSE_FATAL));
    assert!(output.iter().all(|&b| b == 0xaa));
    assert!(runtime.block_cache.contains(1));
    assert!((100..612).all(|block| !runtime.block_cache.contains(block)));
    FAKE.with(|fake| assert_eq!(fake.borrow().reads, vec![(100, 64); BLOCK_READ_RETRY_LIMIT]));
    assert_eq!(read_blocks_into(&mut runtime, usize::MAX, 64, 0, &mut output[..8], true),
        Err(RequestError::InvalidArgument));
    assert_eq!(read_blocks_into(&mut runtime, 100, 0, 0, &mut output[..8], true),
        Err(RequestError::InvalidArgument));
    assert_eq!(read_blocks_into(&mut runtime, 100, 64, BLOCK_SIZE, &mut output[..8], true),
        Err(RequestError::InvalidArgument));
}

#[test]
fn mixed_large_stream_reads_only_gaps_and_preserves_hot_cache_after_a_late_failure() {
    let mut cases = Vec::new();
    for (stream, fail) in [(false, false), (true, false), (true, true)] {
        let window = 256 * BLOCK_SIZE;
        let mut runtime = runtime_with_window((100..612).collect(), window, window);
        for (block, byte) in [(100, 0x32), (355, 0x33)] {
            runtime.block_cache.store(block, &vec![byte; BLOCK_SIZE], None);
            FAKE.with(|fake| fake.borrow_mut().disk[block * BLOCK_SIZE..(block + 1) * BLOCK_SIZE].fill(byte));
        }
        runtime.block_cache.store(200, &vec![0x31; BLOCK_SIZE], Some(block_cache::BlockKind::Data));
        for block in (1..=8).chain(700..945) {
            runtime.block_cache.store(block, &vec![0x77; BLOCK_SIZE], None);
        }
        if fail { FAKE.with(|fake| fake.borrow_mut().read_error = Some(201)); }

        let size = 512 * BLOCK_SIZE - 11;
        let mut output = vec![0xaa; size - 3 + 2];
        let session = ClientSession { shm_local: output.as_mut_ptr() as Word, shm_size: output.len() };
        if stream {
            let result = read_file(&mut runtime, session, Ext2Inode { size: size as u32 }, 3, size - 3, 1);
            if fail {
                assert_eq!(result, Err(OS_RESPONSE_FATAL));
                assert!(output.iter().all(|&byte| byte == 0xaa));
                assert!((1..=8).all(|block| runtime.block_cache.contains(block)));
                assert!((101..200).all(|block| !runtime.block_cache.contains(block)));
                assert!(runtime.block_cache.is_dirty());
                FAKE.with(|fake| {
                    let fake = fake.borrow();
                    assert_eq!(fake.reads[0], (101, 99));
                    assert_eq!(fake.reads[1..], vec![(201, 154); BLOCK_READ_RETRY_LIMIT]);
                    assert!(fake.writes.is_empty());
                });
                continue;
            }
            assert_eq!(result, Ok(size - 3));
            assert!(runtime.block_cache.contains(355));
            assert!((101..200).all(|block| !runtime.block_cache.contains(block)));
        } else {
            // The old mixed path fell back to this scratch read above 64 blocks.
            read_blocks(&mut runtime, 100, 256).unwrap();
            let split = 1 + window - 3;
            output[1..split].copy_from_slice(&runtime.scratch[3..window]);
            // The second window was already eligible for cold streaming.
            read_blocks_into(&mut runtime, 356, 256, 0, &mut output[split..size - 2], true).unwrap();
        }
        assert_eq!((output[0], output[output.len() - 1]), (0xaa, 0xaa));
        for (index, &byte) in output[1..output.len() - 1].iter().enumerate() {
            let block = 100 + (index + 3) / BLOCK_SIZE;
            let expected = match block { 100 => 0x32, 200 => 0x31, 355 => 0x33, _ => 0x77 };
            assert_eq!(byte, expected);
        }
        let read = FAKE.with(|fake| fake.borrow().reads.clone());
        for block in 1..=8 { read_block(&mut runtime, block).unwrap(); }
        let extra = FAKE.with(|fake| fake.borrow().reads.len()) - read.len();
        cases.push((output, read, extra));
    }
    assert_eq!(cases[0].0, cases[1].0);
    assert_eq!(cases[0].1, [(101, 99), (201, 155), (356, 256)]);
    assert_eq!(cases[1].1, [(101, 99), (201, 154), (356, 256)]);
    assert_eq!((cases[0].2, cases[1].2), (8, 0));
    println!("MIXED-STREAM 512KiB device-blocks=510->509 followup-reads=8->0");
}

#[test]
fn file_reads_within_cache_budget_still_fill_and_reuse_payload() {
    let mut runtime = runtime_with_window((100..228).collect(), 64 * 1024, 128 * 1024);
    let mut output = vec![0u8; 128 * BLOCK_SIZE];
    let session = ClientSession { shm_local: output.as_mut_ptr() as Word, shm_size: output.len() };
    for _ in 0..2 {
        assert_eq!(read_file(&mut runtime, session, Ext2Inode { size: output.len() as u32 },
            0, output.len(), 0), Ok(output.len()));
        assert!(output.iter().all(|&b| b == 0x77));
    }
    assert!((100..228).all(|block| runtime.block_cache.contains(block)));
    FAKE.with(|fake| assert_eq!(fake.borrow().reads, [(100, 64), (164, 64)]));
}

#[test]
fn provider_window_batches_a_contiguous_read_without_changing_bytes_or_guards() {
    let mut results = Vec::new();
    for window in [64 * 1024, 256 * 1024] {
        let mut runtime = runtime_with_window((100..356).collect(), window, 1024 * 1024);
        let mut output = vec![0xaau8; 256 * BLOCK_SIZE + 8];
        let session = ClientSession { shm_local: output.as_mut_ptr() as Word, shm_size: output.len() };
        let inode = Ext2Inode { size: (256 * BLOCK_SIZE) as u32 };
        assert_eq!(read_file(&mut runtime, session, inode, 3, 256 * BLOCK_SIZE - 10, 2), Ok(256 * BLOCK_SIZE - 10));
        assert!(output[..2].iter().chain(output[256 * BLOCK_SIZE - 8..].iter()).all(|&b| b == 0xaa));
        assert!(output[2..256 * BLOCK_SIZE - 8].iter().all(|&b| b == 0x77));
        results.push((output, FAKE.with(|fake| fake.borrow().reads.clone())));
    }
    assert_eq!(results[0].0, results[1].0);
    assert_eq!(results[0].1, [(100, 64), (164, 64), (228, 64), (292, 64)]);
    assert_eq!(results[1].1, [(100, 256)]);
}

#[test]
fn larger_window_mixed_reads_keep_dirty_payload_and_leave_output_untouched_on_io_failure() {
    for fail in [false, true] {
        let mut runtime = runtime_with_window((100..356).collect(), 256 * 1024, 1024 * 1024);
        for (block, value) in [(100, 0x31), (355, 0x32)] {
            runtime.block_cache.store(block, &vec![value; BLOCK_SIZE], Some(block_cache::BlockKind::Data));
        }
        if fail { FAKE.with(|fake| fake.borrow_mut().read_error = Some(101)); }
        let mut output = vec![0xaau8; 256 * BLOCK_SIZE];
        let session = ClientSession { shm_local: output.as_mut_ptr() as Word, shm_size: output.len() };
        let inode = Ext2Inode { size: output.len() as u32 };
        let result = read_file(&mut runtime, session, inode, 0, output.len(), 0);
        if fail {
            assert_eq!(result, Err(OS_RESPONSE_FATAL));
            assert!(output.iter().all(|&b| b == 0xaa));
        } else {
            assert_eq!(result, Ok(output.len()));
            for (index, &byte) in output.iter().enumerate() {
                assert_eq!(byte, match index / BLOCK_SIZE { 0 => 0x31, 255 => 0x32, _ => 0x77 });
            }
        }
        FAKE.with(|fake| assert_eq!(fake.borrow().reads,
            vec![(101, 254); if fail { BLOCK_READ_RETRY_LIMIT } else { 1 }]));
        for (block, value) in [(100, 0x31), (355, 0x32)] {
            let mut cached = vec![0; BLOCK_SIZE];
            assert!(runtime.block_cache.load(block, &mut cached));
            assert!(cached.iter().all(|&b| b == value));
        }
    }
}

#[test]
fn cacheable_window_reads_only_missing_blocks_before_filling_cache() {
    let window = 256 * BLOCK_SIZE;
    let mut reference = None;
    for (legacy, fail) in [(true, false), (false, false), (false, true)] {
        let mut runtime = runtime_with_window((100..356).collect(), window, window);
        for (block, value) in [(100, 0x32), (355, 0x33)] {
            runtime.block_cache.store(block, &vec![value; BLOCK_SIZE], None);
            FAKE.with(|fake| fake.borrow_mut().disk[block * BLOCK_SIZE..(block + 1) * BLOCK_SIZE].fill(value));
        }
        runtime.block_cache.store(200, &vec![0x31; BLOCK_SIZE], Some(block_cache::BlockKind::Data));
        for block in 700..953 {
            runtime.block_cache.store(block, &vec![0x77; BLOCK_SIZE], None);
        }
        if fail { FAKE.with(|fake| fake.borrow_mut().read_error = Some(201)); }

        let bytes = window - 14;
        let mut output = vec![0xaa; bytes + 2];
        if legacy {
            read_blocks(&mut runtime, 100, 256).unwrap();
            output[1..=bytes].copy_from_slice(&runtime.scratch[3..3 + bytes]);
            for (index, value) in [(0, 0x32), (100, 0x31), (255, 0x33)] {
                assert!(runtime.scratch[index * BLOCK_SIZE..(index + 1) * BLOCK_SIZE]
                    .iter().all(|&byte| byte == value));
            }
            FAKE.with(|fake| assert_eq!(fake.borrow().reads, [(101, 99), (201, 155)]));
            reference = Some(output);
            continue;
        }

        let session = ClientSession { shm_local: output.as_mut_ptr() as Word, shm_size: output.len() };
        let inode = Ext2Inode { size: (window - 11) as u32 };
        let result = read_file(&mut runtime, session, inode, 3, bytes + 1, 1);
        if fail {
            assert_eq!(result, Err(OS_RESPONSE_FATAL));
            assert!(output.iter().all(|&byte| byte == 0xaa));
            assert!((700..953).all(|block| runtime.block_cache.contains(block)));
            assert!((101..200).all(|block| !runtime.block_cache.contains(block)));
            assert!(runtime.block_cache.contains(355));
            FAKE.with(|fake| {
                let fake = fake.borrow();
                assert_eq!(fake.reads[0], (101, 99));
                assert_eq!(fake.reads[1..], [(201, 154); BLOCK_READ_RETRY_LIMIT]);
            });
        } else {
            assert_eq!(result, Ok(bytes));
            assert_eq!(reference.as_ref().unwrap(), &output);
            assert!(runtime.block_cache.contains(101));
            assert!(runtime.block_cache.contains(354));
            // The mapping stub primes scratch before read_blocks_into.
            for cached in [0, 100, 255] {
                assert!(runtime.scratch[cached * BLOCK_SIZE..(cached + 1) * BLOCK_SIZE]
                    .iter().all(|&byte| byte == 0xbb));
            }
            FAKE.with(|fake| assert_eq!(fake.borrow().reads, [(101, 99), (201, 154)]));
        }
        assert!(runtime.block_cache.is_dirty());
        FAKE.with(|fake| assert!(fake.borrow().writes.is_empty()));
    }
    println!("CACHEABLE-WINDOW 256KiB device-blocks=254->253 original-resident-scratch-bytes=3072->0");
}

#[test]
fn full_large_window_writeback_keeps_payload_durable_with_one_device_write() {
    let mut runtime = runtime_with_window((100..356).collect(), 256 * 1024, 1024 * 1024);
    let input: Vec<u8> = (0..256 * BLOCK_SIZE).map(|i| (i * 17 + 3) as u8).collect();
    let mut inode = Ext2Inode { size: input.len() as u32 };
    assert_eq!(write(&mut runtime, &mut inode, 0, &input), Ok(input.len()));
    FAKE.with(|fake| assert!(fake.borrow().writes.is_empty()));
    flush_blocks(&mut runtime).unwrap();
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.writes, [(100, 256)]);
        assert_eq!(&fake.durable[100 * BLOCK_SIZE..356 * BLOCK_SIZE], &input);
    });
    assert!(!runtime.block_cache.is_dirty());
}

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
    // Cached payload is copied straight to output, not first through scratch.
    assert!(runtime.scratch[..BLOCK_SIZE].iter().all(|&byte| byte == 0xbb));
    assert!(runtime.scratch[2 * BLOCK_SIZE..3 * BLOCK_SIZE].iter().all(|&byte| byte == 0xbb));
}

#[test]
fn mixed_read_late_io_failure_keeps_the_current_run_output_untouched() {
    for failed_block in [101, 103] {
        let mut runtime = runtime((100..104).collect());
        runtime.block_cache.store(100, &vec![0x31; BLOCK_SIZE], Some(block_cache::BlockKind::Data));
        runtime.block_cache.store(102, &vec![0x32; BLOCK_SIZE], Some(block_cache::BlockKind::Data));
        FAKE.with(|fake| fake.borrow_mut().read_error = Some(failed_block));
        let mut output = vec![0xaau8; 4 * BLOCK_SIZE + 8];
        let session = ClientSession { shm_local: output.as_mut_ptr() as Word, shm_size: output.len() };
        let inode = Ext2Inode { size: (4 * BLOCK_SIZE) as u32 };
        assert_eq!(read_file(&mut runtime, session, inode, 7, 4 * BLOCK_SIZE - 19, 3), Err(OS_RESPONSE_FATAL));
        assert!(output.iter().all(|&byte| byte == 0xaa));
        // Dirty cached data survives both early and late failures.
        let mut cached = vec![0; BLOCK_SIZE];
        assert!(runtime.block_cache.load(100, &mut cached));assert!(cached.iter().all(|&byte| byte == 0x31));
        assert!(runtime.block_cache.load(102, &mut cached));assert!(cached.iter().all(|&byte| byte == 0x32));
    }
}

#[test]
fn mixed_reads_preserve_partial_edges_and_cached_sources_under_clean_eviction_pressure() {
    for slots in [2, 3, 4, 16] {
        let mut runtime = runtime((100..116).collect());
        runtime.block_cache = block_cache::BlockCache::new(BLOCK_SIZE, slots * BLOCK_SIZE, BLOCK_SIZE);
        runtime.block_cache.store(100, &vec![0x31; BLOCK_SIZE], None);
        runtime.block_cache.store(115, &vec![0x32; BLOCK_SIZE], None);
        FAKE.with(|fake| {
            let mut fake=fake.borrow_mut();
            fake.disk[100*BLOCK_SIZE..101*BLOCK_SIZE].fill(0x31);
            fake.disk[115*BLOCK_SIZE..116*BLOCK_SIZE].fill(0x32);
        });
        let mut output = vec![0xaau8; 16 * BLOCK_SIZE + 8];
        let session = ClientSession { shm_local: output.as_mut_ptr() as Word, shm_size: output.len() };
        let inode = Ext2Inode { size: (16 * BLOCK_SIZE) as u32 };
        let copied = read_file(&mut runtime, session, inode, 7, 16 * BLOCK_SIZE - 19, 3).unwrap();
        for (index, &byte) in output.iter().enumerate() {
            let expected = if index < 3 || index >= 3 + copied { 0xaa }
                else if index - 3 + 7 < BLOCK_SIZE { 0x31 }
                else if index - 3 + 7 >= 15 * BLOCK_SIZE { 0x32 } else { 0x77 };
            assert_eq!(byte, expected, "slots={slots} index={index}");
        }
        FAKE.with(|fake| assert_eq!(fake.borrow().reads, [(101, 14)]));
    }
}

#[test]
fn delaying_mixed_read_fills_avoids_reloading_a_later_cached_block() {
    let mut results=Vec::new();
    for optimized in [false,true] {
        let mut runtime=runtime((100..116).collect());
        runtime.block_cache=block_cache::BlockCache::new(BLOCK_SIZE,2*BLOCK_SIZE,BLOCK_SIZE);
        for (block,value) in [(100,0x31),(115,0x32)] {
            runtime.block_cache.store(block,&vec![value;BLOCK_SIZE],None);
            FAKE.with(|fake|fake.borrow_mut().disk[block*BLOCK_SIZE..(block+1)*BLOCK_SIZE].fill(value));
        }
        let mut output=vec![0xaau8;16*BLOCK_SIZE];
        if optimized {
            read_blocks_into(&mut runtime,100,16,0,&mut output,false).unwrap();
        } else {
            // Original file-payload miss path, before this optimization.
            read_blocks(&mut runtime,100,16).unwrap();
            output.copy_from_slice(&runtime.scratch[..]);
        }
        results.push((output,FAKE.with(|fake|fake.borrow().reads.clone())));
    }
    assert_eq!(results[0].0,results[1].0);
    assert_eq!(results[0].1,[(101,14),(115,1)]);
    assert_eq!(results[1].1,[(101,14)]);
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
