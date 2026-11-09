//! Real physical allocator, including recent and interleaved partial frees.
#![allow(dead_code)]
extern crate alloc;
mod production {
    include!(env!("NANAMI_PHYSICAL_ALLOCATOR_SOURCE"));

    // Independently express the first-fit choice and checked address route.
    pub fn address_based_any(a: &mut PhysicalAllocator, bytes: usize)
        -> Result<PhysicalAllocation, PhysicalAllocError> {
        if bytes == 0 { return Err(PhysicalAllocError::InvalidArgument); }
        let count = bytes_to_pages(bytes);
        let wanted = ceil_log2(count).ok_or(PhysicalAllocError::OutOfMemory)?;
        let base = a.normal.free_lists.iter().skip(wanted)
            .find_map(|list| list.first().copied()).ok_or(PhysicalAllocError::OutOfMemory)?;
        a.allocate_at(base << PAGE_BITS, count << PAGE_BITS, false)
    }

    pub fn free_lists(a: &PhysicalAllocator) -> (Vec<Vec<usize>>, Vec<Vec<usize>>) {
        (a.normal.free_lists.clone(), a.device.free_lists.clone())
    }

    fn old_add_block(pool: &mut BuddyPool, mut base: usize, mut order: usize)
        -> Result<(), PhysicalAllocError> {
        loop {
            pool.ensure_order(order);
            let count = block_pages(order)?;
            if base % count != 0 { return Err(PhysicalAllocError::InvalidArgument); }
            let buddy = base ^ count;
            if let Some(index) = pool.free_lists[order].iter().position(|&page| page == buddy) {
                pool.free_lists[order].swap_remove(index);
                base = base.min(buddy);
                order = order.checked_add(1).ok_or(PhysicalAllocError::InvalidArgument)?;
                continue;
            }
            if !pool.free_lists[order].contains(&base) { pool.free_lists[order].push(base); }
            return Ok(());
        }
    }

    fn status(result: Result<(), PhysicalAllocError>) -> u8 {
        match result { Ok(()) => 0, Err(PhysicalAllocError::InvalidArgument) => 1,
            Err(PhysicalAllocError::PermissionDenied) => 2, Err(PhysicalAllocError::OutOfMemory) => 3 }
    }

    #[test]
    fn fused_buddy_scan_preserves_first_buddy_priority_duplicates_and_failure_state() {
        for entries in [vec![], vec![8], vec![9], vec![8, 9], vec![9, 8],
            vec![4, 6, 8, 10], vec![8, 8, 12], vec![8, 4, 9, 9, 8]] {
            for base in [3, 8, 9, 10, usize::MAX] {
                for order in [0, 1, 2] {
                    let lists = vec![entries.clone(), vec![10, 6], vec![12, 4]];
                    let mut fused = BuddyPool { free_lists: lists.clone() };
                    let mut old = BuddyPool { free_lists: lists };
                    assert_eq!(status(fused.add_block(base, order)), status(old_add_block(&mut old, base, order)));
                    assert_eq!(fused.free_lists, old.free_lists);
                }
            }
        }
    }

    #[test]
    fn fused_buddy_scan_matches_two_pass_coalescing_during_fragmented_range_returns() {
        let mut fused = BuddyPool::new();
        let mut old = BuddyPool::new();
        let mut seed = 73u64;
        for _ in 0..6000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let base = (seed >> 24) as usize % 8192;
            let count = 1 + (seed >> 40) as usize % 65;
            let result = fused.add_range(base, count);
            let expected = (|| {
                let mut base = base;
                let mut remaining = count;
                while remaining != 0 {
                    let order = largest_aligned_order(base, remaining)?;
                    old_add_block(&mut old, base, order)?;
                    let pages = block_pages(order)?;
                    base = base.checked_add(pages).ok_or(PhysicalAllocError::InvalidArgument)?;
                    remaining -= pages;
                }
                Ok(())
            })();
            assert_eq!(status(result), status(expected));
            assert_eq!(fused.free_lists, old.free_lists);
        }
    }
}
use production::{PhysicalAllocError, PhysicalAllocator};
const PAGE: usize = 4096;

#[test]
fn initialization_batch_tail_return_and_fragmented_single_page_fallback() {
    for claimed in [0, 1, 32, 63, 64] {
        for reverse in [false, true] {
            let mut allocator = normal(128);
            let batch = allocator.allocate_any(64 * PAGE).unwrap_or_else(|_| panic!("batch"));
            if claimed < 64 { release(&mut allocator, batch.base_page + claimed, 64 - claimed); }
            let info = allocator.memory_info();
            assert_eq!(info.total_pages - info.free_pages, claimed);
            for offset in 0..claimed {
                let page = if reverse { claimed - offset - 1 } else { offset };
                release(&mut allocator, batch.base_page + page, 1);
            }
            assert_eq!(allocator.memory_info().free_pages, 128);
            allocate(&mut allocator, 1, 128);
        }
    }
    let mut fragmented = normal(128);
    allocate(&mut fragmented, 1, 128);
    for page in (1..=128).step_by(2) { release(&mut fragmented, page, 1); }
    let before = production::free_lists(&fragmented);
    assert!(matches!(fragmented.allocate_any(64 * PAGE), Err(PhysicalAllocError::OutOfMemory)));
    assert_eq!(production::free_lists(&fragmented), before);
    for _ in 0..64 {
        let page = fragmented.allocate_any(PAGE).unwrap_or_else(|_| panic!("fallback"));
        assert_eq!(page.base_page % 2, 1);
    }
    assert_eq!(fragmented.memory_info().free_pages, 0);
    assert_eq!(fragmented.memory_info().total_pages, 128);
}

fn normal(pages: usize) -> PhysicalAllocator {
    let mut allocator = PhysicalAllocator::new();
    allocator
        .add_region(PAGE, pages * PAGE, false, false)
        .unwrap_or_else(|_| panic!("region"));
    allocator
}
fn allocate(allocator: &mut PhysicalAllocator, base: usize, pages: usize) {
    let result = allocator
        .allocate_at(base * PAGE, pages * PAGE, false)
        .unwrap_or_else(|_| panic!("allocate"));
    assert_eq!(
        (result.base_page, result.page_count, result.is_device),
        (base, pages, false)
    );
}
fn release(allocator: &mut PhysicalAllocator, base: usize, pages: usize) {
    allocator
        .free(base * PAGE, pages * PAGE)
        .unwrap_or_else(|_| panic!("free"));
}
fn invalid(result: Result<(), PhysicalAllocError>) {
    assert!(matches!(result, Err(PhysicalAllocError::InvalidArgument)));
}

fn allocation_result(result: Result<production::PhysicalAllocation, PhysicalAllocError>)
    -> Result<(usize, usize, bool), u8> {
    result.map(|a| (a.base_page, a.page_count, a.is_device)).map_err(|error| match error {
        PhysicalAllocError::InvalidArgument => 0,
        PhysicalAllocError::PermissionDenied => 1,
        PhysicalAllocError::OutOfMemory => 2,
    })
}

#[test]
fn first_fit_matches_address_based_selection_and_list_order_under_churn() {
    let mut a = normal(4096);
    let mut reference = normal(4096);
    for allocator in [&mut a, &mut reference] {
        allocator.add_region(8192 * PAGE, 64 * PAGE, true, false).unwrap_or_else(|_| panic!("device"));
    }
    let mut live = Vec::new();
    let mut occupied = [false; 4096];
    let mut random = 0x12345678u64;
    for step in 0..6000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        if step % 5 == 0 && !live.is_empty() {
            let index = (random >> 24) as usize % live.len();
            let (base, count) = live.swap_remove(index);
            let offset = (random >> 32) as usize % count;
            let freed = 1 + (random >> 42) as usize % (count - offset);
            release(&mut a, base + offset, freed);
            release(&mut reference, base + offset, freed);
            occupied[base + offset - 1..base + offset + freed - 1].fill(false);
            if offset != 0 { live.push((base, offset)); }
            if offset + freed != count { live.push((base + offset + freed, count - offset - freed)); }
        } else {
            let count = 1 + (random >> 32) as usize % 65;
            let bytes = count * PAGE - (random as usize & 4095);
            let result = if step % 7 == 0 {
                let base = 1 + (random >> 19) as usize % 4096;
                let result = allocation_result(a.allocate_at(base * PAGE, bytes, false));
                assert_eq!(result, allocation_result(reference.allocate_at(base * PAGE, bytes, false)));
                result
            } else {
                let result = allocation_result(a.allocate_any(bytes));
                assert_eq!(result, allocation_result(production::address_based_any(&mut reference, bytes)));
                result
            };
            if let Ok((base, count, device)) = result {
                assert!(!device);
                assert!(occupied[base - 1..base + count - 1].iter().all(|v| !v));
                occupied[base - 1..base + count - 1].fill(true);
                live.push((base, count));
            }
        }
        assert_eq!(production::free_lists(&a), production::free_lists(&reference));
        assert_eq!(a.memory_info().free_pages, occupied.iter().filter(|v| !**v).count());
    }
    for (base, count) in live { release(&mut a, base, count); release(&mut reference, base, count); }
    assert_eq!(production::free_lists(&a), production::free_lists(&reference));
    assert_eq!(a.memory_info().free_pages, 4096);
}

#[test]
fn first_fit_exact_and_non_power_of_two_requests_keep_rounding_failure_and_device_policy() {
    for bytes in [0, 1, PAGE - 1, PAGE, 3 * PAGE - 1, 63 * PAGE, 64 * PAGE,
        65 * PAGE, 511 * PAGE, 512 * PAGE, 513 * PAGE, 1024 * PAGE,
        1024 * PAGE + 1, usize::MAX] {
        let mut a = normal(1024);
        let mut reference = normal(1024);
        assert_eq!(allocation_result(a.allocate_any(bytes)),
            allocation_result(production::address_based_any(&mut reference, bytes)));
        assert_eq!(production::free_lists(&a), production::free_lists(&reference));
    }
    let mut a = PhysicalAllocator::new();
    a.add_region(PAGE, 64 * PAGE, true, false).unwrap_or_else(|_| panic!("device"));
    assert_eq!(allocation_result(a.allocate_any(PAGE)), Err(2));
    assert!(a.allocate_at(PAGE, PAGE, true).unwrap_or_else(|_| panic!("device allocate")).is_device);
    // Existing region registration can admit a malformed interval extending
    // past the byte-address space. Preserve its old address-based fallback.
    let mut a = PhysicalAllocator::new();
    let mut reference = PhysicalAllocator::new();
    for allocator in [&mut a, &mut reference] {
        allocator.add_region(usize::MAX & !(PAGE - 1), 4 * PAGE, false, false)
            .unwrap_or_else(|_| panic!("high region"));
    }
    assert_eq!(allocation_result(a.allocate_any(2 * PAGE)),
        allocation_result(production::address_based_any(&mut reference, 2 * PAGE)));
    assert_eq!(production::free_lists(&a), production::free_lists(&reference));
}

#[test]
fn newest_first_and_oldest_first_releases_preserve_the_pool() {
    for reverse in [false, true] {
        let mut allocator = normal(8192);
        for page in 1..=4096 {
            allocate(&mut allocator, page, 1);
        }
        for offset in 0..4096 {
            let page = if reverse { 4096 - offset } else { offset + 1 };
            release(&mut allocator, page, 1);
        }
        assert_eq!(allocator.memory_info().free_pages, 8192);
        allocate(&mut allocator, 1, 8192);
        assert_eq!(allocator.memory_info().free_pages, 0);
    }
}

#[test]
fn last_extent_supports_head_middle_and_tail_partial_release() {
    let mut allocator = normal(64);
    allocate(&mut allocator, 1, 16);
    release(&mut allocator, 5, 4);
    invalid(allocator.free(5 * PAGE, PAGE));
    invalid(allocator.free(4 * PAGE, 6 * PAGE));
    release(&mut allocator, 1, 2);
    release(&mut allocator, 15, 2);
    release(&mut allocator, 3, 2);
    release(&mut allocator, 9, 6);
    assert_eq!(allocator.memory_info().free_pages, 64);
    allocate(&mut allocator, 1, 64);
}

#[test]
fn an_older_partial_release_and_swapped_record_keep_both_allocations_owned() {
    let mut allocator = normal(64);
    allocate(&mut allocator, 1, 16);
    allocate(&mut allocator, 33, 16);
    release(&mut allocator, 5, 4);
    assert!(allocator.allocate_at(33 * PAGE, PAGE, false).is_err());
    release(&mut allocator, 33, 16);
    assert!(allocator.allocate_at(PAGE, PAGE, false).is_err());
    release(&mut allocator, 1, 4);
    release(&mut allocator, 9, 8);
    assert_eq!(allocator.memory_info().free_pages, 64);
}

#[test]
fn reserved_unknown_zero_misaligned_and_cross_extent_frees_are_rejected() {
    let mut allocator = normal(64);
    allocator
        .add_region(128 * PAGE, 8 * PAGE, false, true)
        .unwrap_or_else(|_| panic!("reserve"));
    allocate(&mut allocator, 1, 4);
    allocate(&mut allocator, 5, 4);
    let free = allocator.memory_info().free_pages;
    for (base, bytes) in [
        (PAGE, 0),
        (PAGE + 1, PAGE),
        (20 * PAGE, PAGE),
        (128 * PAGE, PAGE),
        (4 * PAGE, 2 * PAGE),
        (usize::MAX - PAGE + 1, 2 * PAGE),
    ] {
        invalid(allocator.free(base, bytes));
        assert_eq!(allocator.memory_info().free_pages, free);
    }
    release(&mut allocator, 1, 4);
    invalid(allocator.free(PAGE, PAGE));
    release(&mut allocator, 5, 4);
    assert_eq!(allocator.memory_info().total_pages, 72);
}

#[test]
fn device_and_ram_releases_return_to_their_original_pools() {
    let mut allocator = normal(64);
    allocator
        .add_region(128 * PAGE, 32 * PAGE, true, false)
        .unwrap_or_else(|_| panic!("device"));
    assert!(matches!(
        allocator.allocate_at(128 * PAGE, PAGE, false),
        Err(PhysicalAllocError::PermissionDenied)
    ));
    let device = allocator
        .allocate_at(128 * PAGE, 8 * PAGE, true)
        .unwrap_or_else(|_| panic!("device allocate"));
    assert!(device.is_device);
    allocate(&mut allocator, 1, 8);
    release(&mut allocator, 128, 4);
    release(&mut allocator, 1, 8);
    release(&mut allocator, 132, 4);
    assert_eq!(allocator.memory_info().free_pages, 64);
    assert!(
        allocator
            .allocate_at(128 * PAGE, 32 * PAGE, true)
            .unwrap_or_else(|_| panic!("device reuse"))
            .is_device
    );
}

#[test]
fn mixed_allocate_and_partial_free_matches_an_independent_page_model() {
    const PAGES: usize = 256;
    let mut allocator = normal(PAGES);
    let mut owned = [false; PAGES];
    // A nonzero tag identifies the one live original extent, so an invalid
    // request cannot cross two adjacent independent allocation records.
    let mut tags = [0usize; PAGES];
    let mut tag = 0usize;
    let mut random = 12345u64;
    for step in 0..20000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let page = (random >> 32) as usize % PAGES;
        let count = (1 + (random >> 24) as usize % 8).min(PAGES - page);
        if step % 2 == 0 {
            let result = allocator.allocate_at((page + 1) * PAGE, count * PAGE, false);
            let available = owned[page..page + count].iter().all(|&value| !value);
            assert_eq!(result.is_ok(), available);
            if available {
                tag += 1;
                owned[page..page + count].fill(true);
                tags[page..page + count].fill(tag);
            }
        } else {
            let single_extent = owned[page..page + count].iter().all(|&value| value)
                && tags[page..page + count]
                    .iter()
                    .all(|&value| value == tags[page]);
            let result = allocator.free((page + 1) * PAGE, count * PAGE);
            assert_eq!(result.is_ok(), single_extent);
            if single_extent {
                owned[page..page + count].fill(false);
                tags[page..page + count].fill(0);
                // A partial free splits the original extent into distinct
                // left/right records, which cannot later be freed across a hole.
                if page + count < PAGES && tags[page + count] != 0 {
                    let previous = tags[page + count];
                    tag += 1;
                    for value in &mut tags[page + count..] {
                        if *value != previous {
                            break;
                        }
                        *value = tag;
                    }
                }
            }
        }
        assert_eq!(
            allocator.memory_info().free_pages,
            owned.iter().filter(|&&value| !value).count()
        );
    }
}

#[test]
fn contiguous_private_batches_remain_independently_releasable_in_any_order() {
    for pages in [3, 63, 512, 515] {
        for reverse in [false, true] {
            let mut allocator = normal(4096);
            allocate(&mut allocator, 1, 32);
            let mut owned = Vec::new();
            while owned.len() < pages {
                let count = (pages - owned.len()).min(512);
                let block = allocator
                    .allocate_any(count * PAGE)
                    .unwrap_or_else(|_| panic!("batch"));
                assert_eq!(block.page_count, count);
                assert!(!block.is_device);
                owned.extend(block.base_page..block.base_page + count);
            }
            assert_eq!(allocator.memory_info().free_pages, 4096 - 32 - pages);
            if reverse {
                owned.reverse();
            } else {
                // Release odd leaves before even leaves, forcing interior splits.
                owned.sort_by_key(|page| (page % 2, *page));
            }
            for page in owned {
                release(&mut allocator, page, 1);
                invalid(allocator.free(page * PAGE, PAGE));
            }
            assert_eq!(allocator.memory_info().free_pages, 4096 - 32);
            assert!(allocator.allocate_at(PAGE, 32 * PAGE, false).is_err());
            release(&mut allocator, 1, 32);
            allocate(&mut allocator, 1, 4096);
        }
    }
}

#[test]
#[ignore = "host physical allocation diagnostic; excludes Frame capabilities and guest execution"]
fn private_page_batch_allocation_cost() {
    use std::{hint::black_box, time::Instant};
    for pages in [64, 512, 1202, 1837] {
        for bulk in [false, true, true, false] {
            let mut allocator = normal(32768);
            for page in 1..=4096 {
                allocate(&mut allocator, page, 1);
            }
            let mut owned = Vec::with_capacity(pages);
            let begin = Instant::now();
            let mut calls = 0;
            while owned.len() < pages {
                let count = if bulk {
                    (pages - owned.len()).min(512)
                } else {
                    1
                };
                let block = black_box(&mut allocator)
                    .allocate_any(black_box(count * PAGE))
                    .unwrap_or_else(|_| panic!("allocate"));
                owned.extend(block.base_page..block.base_page + count);
                calls += 1;
            }
            let allocation_ns = begin.elapsed().as_nanos();
            let begin = Instant::now();
            for page in owned.into_iter().rev() {
                release(black_box(&mut allocator), black_box(page), 1);
            }
            println!(
                "PHYSICAL-BATCH-PERF pages={} bulk={} calls={} allocate-ns={} release-ns={}",
                pages,
                bulk,
                calls,
                allocation_ns,
                begin.elapsed().as_nanos()
            );
            assert_eq!(allocator.memory_info().free_pages, 32768 - 4096);
        }
    }
}

#[test]
#[ignore = "host allocator bookkeeping benchmark; excludes capabilities and IPC"]
fn recent_process_pages_among_long_lived_allocations() {
    use std::{hint::black_box, time::Instant};
    for live in [4096, 16384] {
        for child in [204, 914] {
            for order in ["newest-first", "oldest-first", "alternating-ends"] {
                let offsets: Vec<_> = (0..child)
                    .map(|index| match order {
                        "newest-first" => child - 1 - index,
                        "oldest-first" => index,
                        _ => {
                            if index % 2 == 0 {
                                index / 2
                            } else {
                                child - 1 - index / 2
                            }
                        }
                    })
                    .collect();
                let mut allocator = normal(live + child + 1024);
                for page in 1..=live {
                    allocate(&mut allocator, page, 1);
                }
                for sample in 0..3 {
                    for page in live + 1..=live + child {
                        allocate(&mut allocator, page, 1);
                    }
                    let begin = Instant::now();
                    for &offset in &offsets {
                        release(black_box(&mut allocator), black_box(live + 1 + offset), 1);
                    }
                    println!(
                        "PHYSICAL-FREE-PERF live={} child={} order={} sample={} ns={}",
                        live,
                        child,
                        order,
                        sample,
                        begin.elapsed().as_nanos()
                    );
                    assert_eq!(allocator.memory_info().free_pages, child + 1024);
                }
            }
        }
    }
}
