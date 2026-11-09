//! Index invariants, independent of the public-API baseline override harness.
#![allow(dead_code)]
extern crate alloc;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static FAIL_BYTES: Cell<usize> = const { Cell::new(0) };
}
fn fail_allocation(bytes: usize) -> bool {
    FAIL_BYTES.with(|fail| {
        if bytes != 0 && fail.get() == bytes {
            fail.set(0);
            true
        } else {
            false
        }
    })
}
struct TestAllocator;
unsafe impl GlobalAlloc for TestAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if fail_allocation(layout.size()) {
            return core::ptr::null_mut();
        }
        System.alloc(layout)
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if fail_allocation(size) {
            return core::ptr::null_mut();
        }
        System.realloc(pointer, layout, size)
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        System.dealloc(pointer, layout);
    }
}
#[global_allocator]
static ALLOCATOR: TestAllocator = TestAllocator;

mod production {
    include!("../../nanami/src/nanami_core/physical_allocator.rs");

    #[cfg(test)]
    mod tests {
        use super::*;
        const BASE: usize = 4096;

        fn status(result: Result<(), PhysicalAllocError>) -> u8 {
            match result {
                Ok(()) => 0,
                Err(PhysicalAllocError::InvalidArgument) => 1,
                Err(PhysicalAllocError::PermissionDenied) => 2,
                Err(PhysicalAllocError::OutOfMemory) => 3,
            }
        }

        // Original complete preflight followed by a fresh first search.
        fn original_allocate(pool: &mut BuddyPool, base: usize, pages: usize)
            -> Result<(), PhysicalAllocError>
        {
            if pages == 0 { return Err(PhysicalAllocError::InvalidArgument); }
            let end = base.checked_add(pages).ok_or(PhysicalAllocError::InvalidArgument)?;
            let mut current = base;
            while current < end {
                let Some((order, _, block)) = pool.find_containing_block(current) else {
                    return Err(PhysicalAllocError::OutOfMemory);
                };
                let count = block_pages(order).map_err(|_| PhysicalAllocError::OutOfMemory)?;
                let limit = block.checked_add(count).ok_or(PhysicalAllocError::OutOfMemory)?;
                current = limit.min(end);
            }
            current = base;
            while current < end {
                let (order, index, block) = pool.find_containing_block(current)
                    .ok_or(PhysicalAllocError::OutOfMemory)?;
                let limit = block.checked_add(block_pages(order)?)
                    .ok_or(PhysicalAllocError::InvalidArgument)?;
                pool.free_lists[order].swap_remove(index);
                if block < current { pool.add_range(block, current - block)?; }
                let allocated_end = limit.min(end);
                if allocated_end < limit { pool.add_range(allocated_end, limit - allocated_end)?; }
                current = allocated_end;
            }
            Ok(())
        }

        #[test]
        fn buddy_range_preflight_and_mutation_match_original_exact_lists() {
            let pools = [
                vec![vec![100, 101], vec![102], vec![104]],
                vec![vec![0, 1, 1], vec![2], vec![4]],
                vec![vec![], vec![], vec![8, 0]],
                vec![vec![usize::MAX], vec![usize::MAX - 1]],
            ];
            let mut cases = 0;
            for lists in pools {
                for base in [0, 1, 3, 8, 99, 100, 101, 102, 105, usize::MAX - 1] {
                    for pages in [0, 1, 2, 3, 8, 16, usize::MAX] {
                        let mut actual = BuddyPool { free_lists: lists.clone() };
                        let mut expected = BuddyPool { free_lists: lists.clone() };
                        let before = actual.free_lists.clone();
                        let a = status(actual.allocate_range(base, pages));
                        let b = status(original_allocate(&mut expected, base, pages));
                        assert_eq!(a, b, "base={base} pages={pages}");
                        assert_eq!(actual.free_lists, expected.free_lists);
                        if a != 0 { assert_eq!(actual.free_lists, before); }
                        cases += 1;
                    }
                }
            }
            println!("BUDDY-VALIDATED exact-list-cases={cases}");
            let mut actual = BuddyPool::new();
            actual.add_range(BASE, 512).unwrap_or_else(|_| panic!("test region"));
            let mut expected = BuddyPool { free_lists: actual.free_lists.clone() };
            let mut live = Vec::new();
            let mut seed = 0x9a72_13f5u32;
            for step in 0..10_000 {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                if seed % 3 == 0 && !live.is_empty() {
                    let (base, pages) = live.swap_remove(seed as usize % live.len());
                    assert_eq!(status(actual.add_range(base, pages)),
                        status(expected.add_range(base, pages)));
                } else {
                    let base = BASE + (seed as usize >> 4) % 512;
                    let pages = 1 + (seed as usize >> 15) % 32;
                    let result = status(actual.allocate_range(base, pages));
                    assert_eq!(result, status(original_allocate(&mut expected, base, pages)));
                    if result == 0 { live.push((base, pages)); }
                }
                assert_eq!(actual.free_lists, expected.free_lists, "step={step}");
            }
            println!("BUDDY-VALIDATED churn-steps=10000");
        }

        #[test]
        fn buddy_range_search_work_keeps_full_preflight_before_any_mutation() {
            let mut counts = Vec::new();
            for (base, pages) in [(105, 1), (100, 8), (100, 9)] {
                let mut pool = BuddyPool { free_lists: vec![vec![100, 101], vec![102], vec![104]] };
                let before = pool.free_lists.clone();
                take_containing_searches();
                let result = status(pool.allocate_range(base, pages));
                counts.push(take_containing_searches());
                if pages == 9 {
                    assert_eq!(result, 3);
                    assert_eq!(pool.free_lists, before);
                } else {
                    assert_eq!(result, 0);
                }
            }
            println!("BUDDY-VALIDATED single/multi/hole-searches={counts:?}");
            assert_eq!(counts, [1, 7, 5]);
        }

        fn allocator(pages: usize) -> PhysicalAllocator {
            let mut allocator = PhysicalAllocator::new();
            assert!(allocator
                .add_region(BASE * PAGE_SIZE, pages * PAGE_SIZE, false, false)
                .is_ok());
            allocator
        }
        fn allocate(allocator: &mut PhysicalAllocator, page: usize, pages: usize) {
            assert!(allocator
                .allocate_at(page * PAGE_SIZE, pages * PAGE_SIZE, false)
                .is_ok());
        }
        fn release(allocator: &mut PhysicalAllocator, page: usize, pages: usize) {
            assert!(allocator.free(page * PAGE_SIZE, pages * PAGE_SIZE).is_ok());
        }
        fn check_index(allocator: &PhysicalAllocator) {
            let mut seen = vec![false; allocator.allocations.len()];
            for (bucket, &head) in allocator.allocation_buckets.iter().enumerate() {
                let mut link = head;
                while link != 0 {
                    let index = link as usize - 1;
                    assert!(!seen[index], "duplicate index or cyclic chain");
                    seen[index] = true;
                    let record = allocator.allocations[index];
                    assert_eq!(allocator.record_bucket(record.base_page), bucket);
                    link = record.next;
                }
            }
            for (index, record) in allocator.allocations.iter().enumerate() {
                assert!(seen[index], "record absent from index");
                let expected = allocator.allocations.iter().position(|candidate| {
                    !candidate.reserved
                        && candidate.base_page == record.base_page
                        && candidate.page_count >= record.page_count
                });
                assert_eq!(
                    allocator
                        .find_exact_record(record.base_page, record.base_page + record.page_count),
                    expected
                );
            }
        }

        #[test]
        fn chain_links_use_existing_record_padding_without_an_allocation_limit() {
            assert_eq!(core::mem::size_of::<usize>(), 8);
            assert_eq!(core::mem::size_of::<AllocationRecord>(), 24);
            assert_eq!(PhysicalAllocator::record_link(0), Some(1));
            assert_eq!(
                PhysicalAllocator::record_link(u32::MAX as usize - 1),
                Some(u32::MAX)
            );
            assert_eq!(PhysicalAllocator::record_link(u32::MAX as usize), None);
            assert_eq!(PhysicalAllocator::record_link(usize::MAX), None);
            assert_eq!(MAX_INDEX_BYTES, 256 * 1024);
        }

        #[test]
        fn growth_and_interleaved_swap_removes_keep_every_record_indexed() {
            let mut allocator = allocator(2048);
            for page in BASE..BASE + 512 {
                allocate(&mut allocator, page, 1);
                check_index(&allocator);
            }
            assert_eq!(allocator.allocation_buckets.len(), 256);
            let mut random = 7u64;
            for _ in 0..512 {
                random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                let index = (random >> 32) as usize % allocator.allocations.len();
                let record = allocator.allocations[index];
                release(&mut allocator, record.base_page, record.page_count);
                check_index(&allocator);
            }
            assert_eq!(allocator.memory_info().free_pages, 2048);
        }

        #[test]
        fn colliding_head_middle_tail_and_moved_records_remain_reachable() {
            let mut allocator = allocator(2048);
            // Keep one small bucket table while filling a deliberately long chain.
            allocator
                .allocation_buckets
                .resize(INITIAL_INDEX_BUCKETS, 0);
            let bucket = allocator.record_bucket(BASE);
            let pages: Vec<_> = (BASE..BASE + 2048)
                .filter(|&page| allocator.record_bucket(page) == bucket)
                .take(12)
                .collect();
            assert_eq!(pages.len(), 12);
            for &page in &pages {
                allocate(&mut allocator, page, 1);
            }
            check_index(&allocator);
            for offset in [5, 11, 0, 6, 3, 9, 1, 10, 2, 8, 4, 7] {
                release(&mut allocator, pages[offset], 1);
                check_index(&allocator);
            }
        }

        #[test]
        fn partial_splits_and_reserved_duplicates_preserve_index_and_accounting() {
            let mut allocator = allocator(64);
            for _ in 0..2 {
                assert!(allocator
                    .add_region((BASE + 128) * PAGE_SIZE, 8 * PAGE_SIZE, false, true)
                    .is_ok());
            }
            allocate(&mut allocator, BASE, 16);
            allocate(&mut allocator, BASE + 32, 8);
            release(&mut allocator, BASE + 4, 4);
            check_index(&allocator);
            assert!(allocator.free((BASE + 128) * PAGE_SIZE, PAGE_SIZE).is_err());
            assert!(allocator
                .free((BASE + 3) * PAGE_SIZE, 6 * PAGE_SIZE)
                .is_err());
            release(&mut allocator, BASE, 4);
            release(&mut allocator, BASE + 8, 8);
            release(&mut allocator, BASE + 32, 8);
            check_index(&allocator);
            assert_eq!(allocator.memory_info().free_pages, 64);
            assert_eq!(allocator.memory_info().total_pages, 80);
        }

        #[test]
        fn initial_index_allocation_failure_keeps_valid_allocation_and_fallback() {
            let mut allocator = allocator(128);
            allocator.allocations.reserve(128);
            crate::FAIL_BYTES.with(|fail| fail.set(INITIAL_INDEX_BUCKETS * 4));
            allocate(&mut allocator, BASE, 128);
            assert_eq!(crate::FAIL_BYTES.with(|fail| fail.get()), 0);
            assert!(allocator.allocation_buckets.is_empty());
            release(&mut allocator, BASE + 32, 32);
            check_index(&allocator);
            release(&mut allocator, BASE, 32);
            release(&mut allocator, BASE + 64, 64);
            assert_eq!(allocator.memory_info().free_pages, 128);
        }

        #[test]
        fn growth_failure_keeps_old_chains_and_retries_on_next_allocation() {
            let mut allocator = allocator(256);
            allocator.allocations.reserve(256);
            for page in BASE..BASE + 127 {
                allocate(&mut allocator, page, 1);
            }
            crate::FAIL_BYTES.with(|fail| fail.set(INITIAL_INDEX_BUCKETS * 2 * 4));
            allocate(&mut allocator, BASE + 127, 1);
            assert_eq!(crate::FAIL_BYTES.with(|fail| fail.get()), 0);
            assert_eq!(allocator.allocation_buckets.len(), INITIAL_INDEX_BUCKETS);
            check_index(&allocator);
            allocate(&mut allocator, BASE + 128, 1);
            assert_eq!(
                allocator.allocation_buckets.len(),
                INITIAL_INDEX_BUCKETS * 2
            );
            check_index(&allocator);
            for page in BASE..BASE + 129 {
                release(&mut allocator, page, 1);
            }
            assert_eq!(allocator.memory_info().free_pages, 256);
        }

        #[test]
        fn an_absent_optional_index_rebuilds_all_live_records() {
            let mut allocator = allocator(64);
            for page in BASE..BASE + 16 {
                allocate(&mut allocator, page, 1);
            }
            allocator.allocation_buckets.clear();
            release(&mut allocator, BASE + 3, 1);
            allocate(&mut allocator, BASE + 32, 1);
            check_index(&allocator);
        }

        #[test]
        fn bucket_storage_is_bounded_without_rejecting_further_records() {
            let mut allocator = PhysicalAllocator::new();
            let count = MAX_INDEX_BYTES / core::mem::size_of::<u32>() * INDEX_BUCKET_LOAD + 1;
            for page in 0..count {
                allocator.push_record(AllocationRecord {
                    base_page: page,
                    page_count: 1,
                    is_device: false,
                    reserved: false,
                    next: 0,
                });
            }
            assert_eq!(allocator.allocations.len(), count);
            assert_eq!(
                allocator.allocation_buckets.capacity() * core::mem::size_of::<u32>(),
                MAX_INDEX_BYTES
            );
            let mut seen = vec![false; count];
            for &head in &allocator.allocation_buckets {
                let mut link = head;
                while link != 0 {
                    let index = link as usize - 1;
                    assert!(!seen[index]);
                    seen[index] = true;
                    link = allocator.allocations[index].next;
                }
            }
            assert!(seen.iter().all(|&seen| seen));
            for index in [0, count / 2, count - 1] {
                assert_eq!(allocator.find_exact_record(index, index + 1), Some(index));
            }
        }
    }
}
