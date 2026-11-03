//! Real physical allocator, including recent and interleaved partial frees.
#![allow(dead_code)]
extern crate alloc;
mod production {
    include!(env!("NANAMI_PHYSICAL_ALLOCATOR_SOURCE"));
}
use production::{PhysicalAllocError, PhysicalAllocator};
const PAGE: usize = 4096;
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
