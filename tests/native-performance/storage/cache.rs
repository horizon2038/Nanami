use super::*;

#[test]
fn cold_read_fill_does_not_repeat_residency_lookups_after_the_gap_scan() {
    let mut r = runtime(vec![]);
    r.block_cache.take_lookup_calls();
    read_blocks(&mut r, 100, 16).unwrap();
    let lookups = r.block_cache.take_lookup_calls();
    assert!(r.scratch.iter().all(|&byte| byte == 0x77));
    FAKE.with(|fake| assert_eq!(fake.borrow().reads, [(100, 16)]));
    for block in 100..116 {
        assert!(r.block_cache.contains(block));
    }
    println!("READ-MISS-FILL blocks=16 residency_lookups={lookups}");
    assert_eq!(lookups, 0);
}

#[test]
fn read_miss_fill_matches_original_cache_state_with_dirty_pressure_and_device_errors() {
    for slots in [1, 4, 16, 128] {
        for seed in 0..16 {
            for failure in [false, true] {
                let mut outcomes = Vec::new();
                let mut caches = Vec::new();
                for optimized in [false, true] {
                    let mut r = runtime(vec![]);
                    r.block_cache = block_cache::BlockCache::new(BLOCK_SIZE, slots * BLOCK_SIZE, BLOCK_SIZE);
                    for index in 0..slots {
                        let block = 100 + (index * 7 + seed) % 24;
                        let dirty = if (index + seed) % 3 == 0 { Some(block_cache::BlockKind::Data) } else { None };
                        if dirty.is_some() && !r.block_cache.has_room_for(block, 1) { continue; }
                        r.block_cache.store(block, &vec![index as u8; BLOCK_SIZE], dirty);
                    }
                    if failure { FAKE.with(|f| f.borrow_mut().read_error = Some(104)); }
                    let result = if optimized { read_blocks(&mut r, 100, 16) } else { read_blocks_reference(&mut r, 100, 16) };
                    outcomes.push((result, r.scratch.to_vec(), FAKE.with(|f| f.borrow().reads.clone())));
                    caches.push(r.block_cache);
                }
                assert_eq!(outcomes[0], outcomes[1], "slots={slots} seed={seed} failure={failure}");
                caches[0].assert_same_cache_for_test(&caches[1]);
            }
        }
    }
}

#[test]
fn block_slot_hints_match_tree_lookup_through_collisions_eviction_and_dirty_pressure() {
    use block_cache::{BlockCache, BlockKind};
    for slots in [1, 4, 8, 64, 257] {
        let mut actual = BlockCache::new(8, slots * 8, 8);
        let mut reference = BlockCache::new(8, slots * 8, 8);
        reference.use_tree_lookup_for_test();
        let mut seed = 0x193851u64;
        for step in 0..4000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            // Four buckets contain37 distinct identities: stale hints cannot
            // substitute another resident block, including a dirty one.
            let block = ((seed >> 23) as usize % 37) * 256 + (seed as usize % 4);
            let payload = seed.to_le_bytes();
            match step % 7 {
                0 | 1 => {
                    let mut dirty = if step < 1000 || step % 11 != 0 { None }
                        else if step % 2 == 0 { Some(BlockKind::Data) }
                        else { Some(BlockKind::Metadata) };
                    assert_eq!(actual.has_room_for(block, 1), reference.has_room_for(block, 1));
                    if dirty.is_some() && !actual.has_room_for(block, 1) {
                        // As in production, a write must reserve capacity.
                        // Keep exercising read-fill bypass under full pressure.
                        dirty = None;
                    }
                    actual.store(block, &payload, dirty);
                    reference.store(block, &payload, dirty);
                }
                2 => {
                    let mut a = [0xa5; 10];
                    let mut b = a;
                    assert_eq!(actual.load(block, &mut a[1..9]), reference.load(block, &mut b[1..9]));
                    assert_eq!(a, b);
                }
                3 => assert_eq!(actual.load_record::<4>(block, 4).copied(), reference.load_record::<4>(block, 4).copied()),
                4 => assert_eq!(actual.load_slice(block, 1, 6), reference.load_slice(block, 1, 6)),
                5 => {
                    let mut a = [0xa5; 6];
                    let mut b = a;
                    assert_eq!(actual.load_range(block, 4, &mut a[1..5]), reference.load_range(block, 4, &mut b[1..5]));
                    assert_eq!(a, b);
                }
                _ => {
                    let mut a = [0xa5; 8];
                    let mut b = a;
                    assert_eq!(actual.load_prefix_and_gap(block, 1, &mut a), reference.load_prefix_and_gap(block, 1, &mut b));
                    assert_eq!(a, b);
                }
            }
            if step % 17 == 0 {
                actual.assert_same_cache_for_test(&reference);
            }
        }
        actual.assert_same_cache_for_test(&reference);
    }
}

#[test]
fn unrepresentable_hint_slots_keep_the_unbounded_tree_fallback() {
    let mut cache = block_cache::BlockCache::new(4, 65537 * 4, 4);
    for block in 0..65537 {
        cache.store(block, &(block as u32).to_le_bytes(), None);
    }
    for block in [0, 1, 65534, 65535, 65536, 65537] {
        let mut bytes = [0xa5; 4];
        assert_eq!(cache.load(block, &mut bytes), block < 65537);
        if block < 65537 {
            assert_eq!(u32::from_le_bytes(bytes), block as u32);
        } else {
            assert_eq!(bytes, [0xa5; 4]);
        }
    }
}

#[test]
fn batched_prefix_reads_match_scalar_io_scratch_failure_prefix_and_clock_under_pressure() {
    fn run(optimized: bool, slots: usize, seed: usize, failure: bool)
        -> (Result<(),RequestError>,Vec<u8>,Vec<(usize,usize)>,Vec<bool>,Vec<(usize,Vec<u8>)>) {
        let mut r = runtime(vec![]);
        r.block_cache = block_cache::BlockCache::new(BLOCK_SIZE, slots*BLOCK_SIZE, BLOCK_SIZE);
        for index in 0..slots {
            let block = 100 + (index * 7 + seed) % 20;
            let dirty = if (index + seed) % 5 == 0 { Some(block_cache::BlockKind::Data) } else { None };
            r.block_cache.store(block, &vec![(index + 1) as u8;BLOCK_SIZE], dirty);
        }
        if failure { FAKE.with(|f| f.borrow_mut().read_error=Some(104)); }
        let result = if optimized { read_blocks(&mut r,100,16) }
            else { read_blocks_reference(&mut r,100,16) };
        let scratch = r.scratch.to_vec();
        let reads = FAKE.with(|f| f.borrow().reads.clone());
        let mut residency = vec![];
        for fresh in 200..208 {
            r.block_cache.store(fresh, &vec![fresh as u8;BLOCK_SIZE], None);
            residency.extend((100..208).map(|block| r.block_cache.contains(block)));
        }
        let mut payload = vec![];
        for block in 100..208 {
            let mut bytes = vec![0xa5;BLOCK_SIZE];
            if r.block_cache.load(block,&mut bytes) { payload.push((block,bytes)); }
        }
        (result,scratch,reads,residency,payload)
    }
    let mut observed_failure = false;
    for slots in [1,4,8,16] {
        for seed in 0..20 {
            for failure in [false,true] {
                let actual = run(true,slots,seed,failure);
                let expected = run(false,slots,seed,failure);
                observed_failure |= expected.0.is_err();
                assert_eq!(actual,expected,
                    "slots={slots} seed={seed} failure={failure}");
            }
        }
    }
    assert!(observed_failure);
}

#[test]
fn writeback_preserves_entire_staged_scratch_across_dirty_sizes_kinds_and_errors() {
    for count in [1,2,15,16,17,32] {
        for pattern in 0..3 {
            for failure in 0..4 {
                let mut runtime = runtime(vec![]);
                let mut cache = block_cache::BlockCache::new(BLOCK_SIZE, 32 * BLOCK_SIZE, BUFFER_SIZE);
                for index in 0..count {
                    let kind = if pattern == 0 || (pattern == 2 && index % 2 == 0) {
                        block_cache::BlockKind::Data
                    } else {
                        block_cache::BlockKind::Metadata
                    };
                    // Sparse and mixed passes repeatedly reuse the scratch prefix.
                    let block = 100 + index * if pattern == 2 { 2 } else { 1 };
                    cache.store(block, &vec![index as u8; BLOCK_SIZE], Some(kind));
                }
                for (index, byte) in runtime.scratch.iter_mut().enumerate() {
                    *byte = (index.wrapping_mul(71) ^ (index / 31)) as u8;
                }
                let staged = runtime.scratch.to_vec();
                FAKE.with(|f| {
                    let mut f = f.borrow_mut();
                    match failure {
                        1 => f.write_result = Some(Ok(BLOCK_SIZE - 1)),
                        2 => f.write_result = Some(Err(RequestError::Transport)),
                        3 => f.flush_error = Some(RequestError::Transport),
                        _ => {}
                    }
                });
                let expected = match failure {
                    0 => Ok(()),
                    1 => Err(RequestError::Protocol),
                    _ => Err(RequestError::Transport),
                };
                assert_eq!(cache.flush(1, &mut runtime.scratch[..]), expected);
                assert_eq!(runtime.scratch.as_slice(), staged);
                assert_eq!(cache.is_dirty(), failure != 0);
                let events = FAKE.with(|f| f.borrow().events.clone());
                assert_eq!(cache.flush(1, &mut runtime.scratch[..]), expected);
                assert_eq!(runtime.scratch.as_slice(), staged);
                FAKE.with(|f| assert_eq!(f.borrow().events, events));
                for index in 0..count {
                    let mut bytes = vec![0xa5; BLOCK_SIZE];
                    let block = 100 + index * if pattern == 2 { 2 } else { 1 };
                    assert!(cache.load(block, &mut bytes));
                    assert!(bytes.iter().all(|b| *b == index as u8));
                }
            }
        }
    }
}

#[test]
fn write_reservation_matches_old_per_block_predicate_for_sparse_mixed_dirty_states() {
    let blocks = [0,1,2,4,5,7,20,23];
    for step in 0u32..5000 {
        let state = if step == 0 { 0xffff } else { step.wrapping_mul(40503) & 0xffff };
        let mut cache = block_cache::BlockCache::new(4, 8 * 4, 4);
        let mut dirty = std::collections::BTreeSet::new();
        let mut present = std::collections::BTreeMap::new();
        for (i, &block) in blocks.iter().enumerate() {
            let kind = (state >> (i * 2)) & 3;
            if kind == 0 { continue; }
            let data = [block as u8; 4];
            let write = match kind {
                2 => Some(block_cache::BlockKind::Data),
                3 => Some(block_cache::BlockKind::Metadata),
                _ => None,
            };
            if write.is_some() { dirty.insert(block); }
            present.insert(block, data);
            cache.store(block, &data, write);
        }
        for start in [0,1,2,5,7,19,20,21,23,24,usize::MAX - 128] {
            for count in [0,1,2,3,7,8,9,32,128] {
                let expected = (start..start + count).filter(|block| !dirty.contains(block)).count() <= 8 - dirty.len();
                assert_eq!(cache.has_room_for(start, count), expected, "state={state:x} start={start} count={count}");
            }
        }
        // A reservation probe cannot change payload, residency or dirty state.
        for block in 0..25 {
            let mut data = [0xa5; 4];
            assert_eq!(cache.load(block, &mut data), present.contains_key(&block));
            assert_eq!(data, present.get(&block).copied().unwrap_or([0xa5; 4]));
        }
        assert_eq!(cache.is_dirty(), !dirty.is_empty());
    }
    let cache = block_cache::BlockCache::new(4, 32, 4);
    if cfg!(debug_assertions) {
        assert!(std::panic::catch_unwind(|| cache.has_room_for(usize::MAX - 2, 8)).is_err());
    } else {
        assert_eq!(cache.has_room_for(usize::MAX - 2, 8), true);
    }
}

#[test]
fn missing_run_seek_keeps_exact_device_batches_scratch_and_dirty_payload() {
    for mask in 0u32..256 {
        let mut runtime = runtime(vec![]);
        for offset in 0..8 {
            if mask & (1 << offset) != 0 {
                runtime.block_cache.store(100 + offset, &vec![offset as u8; BLOCK_SIZE],
                    if offset % 2 == 0 { Some(block_cache::BlockKind::Metadata) } else { None });
            }
        }
        let mut expected_reads = vec![];
        let mut offset = 0;
        while offset < 8 {
            if mask & (1 << offset) != 0 {
                offset += 1;
                continue;
            }
            let first = offset;
            while offset < 8 && mask & (1 << offset) == 0 { offset += 1; }
            expected_reads.push((100 + first, offset - first));
        }
        read_blocks(&mut runtime, 100, 8).unwrap();
        FAKE.with(|f| assert_eq!(f.borrow().reads, expected_reads));
        for offset in 0..8 {
            let expected = if mask & (1 << offset) != 0 { offset as u8 } else { 0x77 };
            assert!(runtime.scratch[offset * BLOCK_SIZE..(offset + 1) * BLOCK_SIZE].iter().all(|b| *b == expected));
        }
        assert!(runtime.scratch[8 * BLOCK_SIZE..].iter().all(|b| *b == 0xcc));
        let mut dirty = [0xa5; BLOCK_SIZE];
        for offset in 0..8 {
            assert!(runtime.block_cache.load(100 + offset, &mut dirty));
            let expected = if mask & (1 << offset) != 0 { offset as u8 } else { 0x77 };
            assert!(dirty.iter().all(|b| *b == expected));
        }
    }
}

#[test]
fn prefix_and_gap_match_scalar_search_for_every_small_residency_pattern() {
    for mask in 0u32..256 {
        let mut cache = block_cache::BlockCache::new(4, 64, 4);
        for offset in 0..8 {
            if mask & (1 << offset) != 0 {
                cache.store(100 + offset, &[offset as u8; 4],
                    if offset % 2 == 0 { Some(block_cache::BlockKind::Data) } else { None });
            }
        }
        cache.store(90, &[0; 4], None);
        cache.store(110, &[0; 4], None);
        for start in 95..115 {
            for count in 0..20 {
                let prefix = (0..count).take_while(|offset| cache.contains(start + offset)).count();
                let next = (prefix..count).find(|offset| cache.contains(start + offset)).unwrap_or(count);
                let mut target = vec![0xa5; count * 4 + 8];
                assert_eq!(cache.load_prefix_and_gap(start, count, &mut target[4..4+count*4]), (prefix,next),
                    "mask={mask} start={start} count={count}");
                for index in 0..prefix {
                    let number = start + index;
                    let value = if (100..108).contains(&number) { (number - 100) as u8 } else { 0 };
                    assert_eq!(&target[4+index*4..8+index*4], &[value;4]);
                }
                assert!(target[..4].iter().chain(&target[4+prefix*4..]).all(|b| *b==0xa5));
            }
        }
    }
    let mut cache = block_cache::BlockCache::new(4, 16, 4);
    let mut target = [0xa5;32];
    assert_eq!(cache.load_prefix_and_gap(0, 8, &mut target), (0,8));
    cache.store(usize::MAX - 1, &[1; 4], None);
    assert_eq!(cache.load_prefix_and_gap(usize::MAX - 8, 8, &mut target), (0,7));
    assert_eq!(cache.load_prefix_and_gap(usize::MAX, 0, &mut []), (0,0));
}

#[test]
fn pointer_reads_fill_cold_cache_then_read_current_dirty_bytes_without_touching_scratch() {
    let mut runtime=runtime(vec![]);
    let data: Vec<_>=(0..BLOCK_SIZE).map(|i|(i%251) as u8).collect();
    FAKE.with(|f|f.borrow_mut().disk[40*BLOCK_SIZE..41*BLOCK_SIZE].copy_from_slice(&data));
    assert_eq!(read_block_u32(&mut runtime,40,3).unwrap(),u32::from_le_bytes(data[3..7].try_into().unwrap()));
    FAKE.with(|f|assert_eq!(f.borrow().reads,[(40,1)]));
    runtime.scratch.fill(0xa5);
    for offset in [0,3,BLOCK_SIZE-4] {
        assert_eq!(read_block_u32(&mut runtime,40,offset).unwrap(),u32::from_le_bytes(data[offset..offset+4].try_into().unwrap()));
        assert!(runtime.scratch.iter().all(|b|*b==0xa5));
    }
    runtime.block_cache.store(40,&vec![0x36;BLOCK_SIZE],Some(block_cache::BlockKind::Metadata));
    assert_eq!(read_block_u32(&mut runtime,40,12).unwrap(),0x36363636);
    assert!(runtime.block_cache.is_dirty());
    assert!(runtime.scratch.iter().all(|b|*b==0xa5));
    FAKE.with(|f|{let f=f.borrow();assert_eq!(f.reads,[(40,1)]);assert!(f.writes.is_empty());});
}

#[test]
fn pointer_reads_validate_even_cached_invalid_blocks_and_offsets_before_io() {
    let mut runtime=runtime(vec![]);
    runtime.block_cache.store(usize::MAX,&vec![0;BLOCK_SIZE],None);
    runtime.block_cache.store(runtime.block_count,&vec![0;BLOCK_SIZE],None);
    for (block,offset) in [(40,BLOCK_SIZE-3),(40,usize::MAX),(usize::MAX,0),(runtime.block_count,0)] {
        assert_eq!(read_block_u32(&mut runtime,block,offset),Err(RequestError::InvalidArgument));
    }
    assert!(runtime.scratch.iter().all(|b|*b==0xcc));
    let mut out=[0xa5;4];
    assert!(!runtime.block_cache.load_range(usize::MAX,0,&mut out));
    assert_eq!(out,[0xa5;4]);
    FAKE.with(|f|assert!(f.borrow().reads.is_empty()));
}

#[test]
fn single_block_slice_reads_keep_clock_eviction_equivalent_to_full_loads() {
    let mut full=block_cache::BlockCache::new(BLOCK_SIZE,4*BLOCK_SIZE,BLOCK_SIZE);
    let mut slice=block_cache::BlockCache::new(BLOCK_SIZE,4*BLOCK_SIZE,BLOCK_SIZE);
    let mut record=block_cache::BlockCache::new(BLOCK_SIZE,4*BLOCK_SIZE,BLOCK_SIZE);
    let mut view=block_cache::BlockCache::new(BLOCK_SIZE,4*BLOCK_SIZE,BLOCK_SIZE);
    let data=vec![0x37;BLOCK_SIZE];let mut whole=vec![0;BLOCK_SIZE];let mut word=[0;4];
    for i in 0..100 {
        full.store(i,&data,None);slice.store(i,&data,None);record.store(i,&data,None);
        view.store(i,&data,None);
        for old in [i,i.saturating_sub(2),i.saturating_sub(5)] {
            let hit=full.load(old,&mut whole);
            assert_eq!(hit,slice.load_range(old,7,&mut word));
            assert_eq!(hit,record.load_record::<100>(old,7).is_some());
            assert_eq!(hit,view.load_slice(old,7,100).is_some());
        }
        for old in 0..=i {
            assert_eq!(full.contains(old),slice.contains(old));
            assert_eq!(full.contains(old),record.contains(old));
            assert_eq!(full.contains(old),view.contains(old));
        }
    }
}

#[test]
fn cached_record_views_bound_the_range_and_observe_replacement_and_dirty_updates() {
    let mut r=runtime(vec![]);let data:Vec<_>=(0..BLOCK_SIZE).map(|i|(i%251) as u8).collect();
    r.block_cache.store(40,&data,None);
    assert_eq!(r.block_cache.load_record::<100>(40,13).unwrap(),&data[13..113]);
    assert!(r.block_cache.load_record::<100>(40,BLOCK_SIZE-99).is_none());
    assert!(r.block_cache.load_record::<100>(40,usize::MAX).is_none());
    assert!(r.block_cache.load_record::<100>(41,0).is_none());
    r.block_cache.store(usize::MAX,&data,None);
    assert!(r.block_cache.load_record::<100>(usize::MAX,0).is_none());
    r.block_cache.store(40,&vec![0x36;BLOCK_SIZE],Some(block_cache::BlockKind::Metadata));
    assert_eq!(r.block_cache.load_record::<100>(40,BLOCK_SIZE-100).unwrap(),&[0x36;100]);
    assert!(r.block_cache.is_dirty());assert!(r.scratch.iter().all(|b|*b==0xcc));
}

#[test]
fn cached_range_orders_physical_blocks_and_copies_only_requested_bytes() {
    let mut runtime = runtime(vec![]);
    for block in [103, 100, 102, 101] {
        runtime.block_cache.store(block, &vec![block as u8; BLOCK_SIZE], None);
    }
    let mut output = vec![0xaa; 3 * BLOCK_SIZE + 19];
    assert!(runtime.block_cache.load_range(100, 7, &mut output[3..3 * BLOCK_SIZE + 11]));
    for (index, byte) in output.iter().copied().enumerate() {
        let expected = if (3..3 * BLOCK_SIZE + 11).contains(&index) {
            (100 + (index - 3 + 7) / BLOCK_SIZE) as u8
        } else {
            0xaa
        };
        assert_eq!(byte, expected, "offset {index}");
    }
}

#[test]
fn a_missing_cached_block_leaves_the_entire_destination_untouched() {
    for missing in 100..104 {
        let mut runtime = runtime(vec![]);
        for block in 100..104 {
            if block != missing {
                runtime.block_cache.store(block, &vec![block as u8; BLOCK_SIZE], None);
            }
        }
        let mut output = vec![0xaa; 4 * BLOCK_SIZE - 14];
        assert!(!runtime.block_cache.load_range(100, 7, &mut output));
        assert!(output.iter().all(|byte| *byte == 0xaa));
    }
}

#[test]
fn cached_range_rejects_invalid_offsets_and_block_overflow_without_copying() {
    let mut runtime = runtime(vec![]);
    runtime.block_cache.store(100, &vec![0x38; BLOCK_SIZE], None);
    let mut output = vec![0xaa; 2 * BLOCK_SIZE];
    assert!(!runtime.block_cache.load_range(100, BLOCK_SIZE, &mut output));
    assert!(!runtime.block_cache.load_range(usize::MAX, 0, &mut output));
    assert!(output.iter().all(|byte| *byte == 0xaa));
    assert!(runtime.block_cache.load_range(100, 3, &mut []));
}

#[test]
fn bounded_and_large_cached_ranges_keep_physical_order_partial_edges_and_current_dirty_bytes() {
    for blocks in [2, 3, 16, 63, 64, 65, 129] {
        let mut cache = block_cache::BlockCache::new(BLOCK_SIZE, (blocks + 8) * BLOCK_SIZE, BLOCK_SIZE);
        let payload = |block: usize| -> Vec<u8> {
            (0..BLOCK_SIZE).map(|i| ((block * 7 + i * 3) % 251) as u8).collect()
        };
        for block in (100..100 + blocks).rev() { cache.store(block, &payload(block), None); }
        let changed = 100 + blocks / 2;
        cache.store(changed, &vec![0x37; BLOCK_SIZE], Some(block_cache::BlockKind::Data));
        let len = blocks * BLOCK_SIZE - 21;
        let mut output = vec![0xa5; len + 6];
        assert!(cache.load_range(100, 7, &mut output[3..3 + len]));
        assert_eq!(&output[..3], &[0xa5; 3]);
        assert_eq!(&output[3 + len..], &[0xa5; 3]);
        for (i, &byte) in output[3..3 + len].iter().enumerate() {
            let block = 100 + (i + 7) / BLOCK_SIZE;
            let offset = (i + 7) % BLOCK_SIZE;
            let expected = if block == changed { 0x37 } else { ((block * 7 + offset * 3) % 251) as u8 };
            assert_eq!(byte, expected);
        }
        assert!(cache.is_dirty());
    }
}

#[test]
fn missing_blocks_on_either_side_of_slot_bound_leave_output_and_clock_state_unchanged() {
    for blocks in [2, 16, 64, 65, 129] {
        for missing in [0, blocks / 2, blocks - 1] {
            let capacity = blocks + 8;
            let mut range = block_cache::BlockCache::new(BLOCK_SIZE, capacity * BLOCK_SIZE, BLOCK_SIZE);
            let mut untouched = block_cache::BlockCache::new(BLOCK_SIZE, capacity * BLOCK_SIZE, BLOCK_SIZE);
            for block in (100..100 + blocks).rev().filter(|block| *block != 100 + missing) {
                let data = vec![block as u8; BLOCK_SIZE];
                range.store(block, &data, None); untouched.store(block, &data, None);
            }
            let mut output = vec![0xa5; blocks * BLOCK_SIZE - 14];
            assert!(!range.load_range(100, 7, &mut output));
            assert!(output.iter().all(|v| *v == 0xa5));
            // Drive CLOCK after the miss. A cached prefix must not receive
            // second chances that the untouched control never received.
            for block in 1000..1000 + 2 * capacity {
                let data = vec![0x61; BLOCK_SIZE];
                range.store(block, &data, None); untouched.store(block, &data, None);
                for old in (100..100 + blocks).chain(1000..=block) {
                    assert_eq!(range.contains(old), untouched.contains(old));
                }
            }
        }
    }
}

#[test]
fn multi_block_slot_capture_keeps_clock_eviction_equal_to_individual_cached_reads() {
    for blocks in [2, 16, 64, 65] {
      for forward in [false, true] {
        let capacity = blocks + 8;
        let mut range = block_cache::BlockCache::new(BLOCK_SIZE, capacity * BLOCK_SIZE, BLOCK_SIZE);
        let mut scalar = block_cache::BlockCache::new(BLOCK_SIZE, capacity * BLOCK_SIZE, BLOCK_SIZE);
        let mut order: Vec<_> = (100..100 + blocks).collect();
        if !forward { order.reverse(); }
        for block in order {
            let data = vec![block as u8; BLOCK_SIZE];
            range.store(block, &data, None); scalar.store(block, &data, None);
        }
        let mut whole = vec![0; BLOCK_SIZE];
        let mut output = vec![0; blocks * BLOCK_SIZE - 14];
        for round in 0..2 * capacity {
            if range.load_range(100, 7, &mut output) {
                for block in 100..100 + blocks { assert!(scalar.load(block, &mut whole)); }
            }
            let new = 1000 + round;
            let data = vec![round as u8; BLOCK_SIZE];
            range.store(new, &data, None); scalar.store(new, &data, None);
            for old in (100..100 + blocks).chain(1000..=new) {
                assert_eq!(range.contains(old), scalar.contains(old));
            }
        }
      }
    }
}

#[test]
fn contiguous_and_split_cache_extents_copy_exact_partial_edges_and_current_dirty_data() {
    for size in [1usize, 4, 1024, 4096] {
        for blocks in [2, 3, 16, 63, 64, 65] {
            let orders: Vec<Vec<usize>> = vec![
                (0..blocks).collect(),
                (0..blocks).rev().collect(),
                (0..blocks).step_by(2).chain((1..blocks).step_by(2)).collect(),
                (0..blocks).collect::<Vec<_>>().chunks(3).rev().flatten().copied().collect(),
            ];
            for order in orders {
                let mut cache = block_cache::BlockCache::new(size, (blocks + 8) * size, size);
                let payload = |block: usize| -> Vec<u8> {
                    (0..size).map(|i| ((block * 7 + i * 3) % 251) as u8).collect()
                };
                for index in order { cache.store(100 + index, &payload(100 + index), None); }
                let changed = 100 + blocks / 2;
                cache.store(changed, &vec![0x37; size], Some(block_cache::BlockKind::Data));
                for offset in [0, size / 2, size - 1] {
                    for trim in [0, size - 1] {
                        let len = blocks * size - offset - trim;
                        let mut output = vec![0xa5; len + 6];
                        assert!(cache.load_range(100, offset, &mut output[3..3 + len]));
                        assert_eq!(&output[..3], &[0xa5; 3]);
                        assert_eq!(&output[3 + len..], &[0xa5; 3]);
                        for (i, &byte) in output[3..3 + len].iter().enumerate() {
                            let block = 100 + (i + offset) / size;
                            let within = (i + offset) % size;
                            let expected = if block == changed { 0x37 } else { ((block * 7 + within * 3) % 251) as u8 };
                            assert_eq!(byte, expected, "size={size} blocks={blocks} offset={offset} trim={trim}");
                        }
                        assert!(cache.is_dirty());
                    }
                }
            }
        }
    }
}

#[test]
fn metadata_write_attempts_invalidate_directory_positions_but_data_writes_do_not() {
    let mut runtime = runtime(vec![]);
    write_block(&mut runtime, 32).unwrap();
    assert_eq!(runtime.directory_invalidations, 1);
    write_blocks(&mut runtime, 40, 2).unwrap();
    assert_eq!(runtime.directory_invalidations, 1);
    assert!(write_block(&mut runtime, usize::MAX).is_err());
    assert_eq!(runtime.directory_invalidations, 2);
}

#[test]
fn regular_inode_table_writes_keep_metadata_ordering_without_invalidating_cursors() {
    let mut runtime = runtime(vec![]);
    runtime.scratch[..BLOCK_SIZE].fill(0x35);
    write_inode_table_block(&mut runtime, 32).unwrap();
    assert_eq!(runtime.directory_invalidations, 0);
    runtime.scratch[..BLOCK_SIZE].fill(0x24);
    write_blocks(&mut runtime, 40, 1).unwrap();
    flush_blocks(&mut runtime).unwrap();
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.writes, [(40, 1), (32, 1)]);
        assert!(fake.durable[32 * BLOCK_SIZE..33 * BLOCK_SIZE].iter().all(|&b| b == 0x35));
    });
    assert!(write_inode_table_block(&mut runtime, usize::MAX).is_err());
    assert_eq!(runtime.directory_invalidations, 0);
    assert!(write_block(&mut runtime, usize::MAX).is_err());
    assert_eq!(runtime.directory_invalidations, 1);
}

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
fn short_or_failed_write_retains_dirty_data_and_is_not_retried() {
    for result in [Ok(BLOCK_SIZE), Err(RequestError::Transport)] {
        let mut runtime = runtime(vec![]);
        FAKE.with(|fake| fake.borrow_mut().write_result = Some(result));
        runtime.scratch[..2 * BLOCK_SIZE].fill(0x32);
        write_blocks(&mut runtime, 100, 2).unwrap();
        assert!(flush_blocks(&mut runtime).is_err());
        assert!(runtime.block_cache.is_dirty());
        assert!(flush_blocks(&mut runtime).is_err());
        assert!(write_block(&mut runtime, 105).is_err());
        read_blocks(&mut runtime, 100, 2).unwrap();
        assert!(runtime.scratch[..2 * BLOCK_SIZE].iter().all(|b| *b == 0x32));
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
    flush_blocks(&mut runtime).unwrap();
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
