//! The actual Alpha allocator, without installing it as the host global allocator.
#![allow(dead_code)]
use std::alloc::{alloc_zeroed, dealloc, GlobalAlloc, Layout};
#[path = "../../nanami/src/nanami_utils/heap.rs"]
mod heap;

struct Region {
    pointer: *mut u8,
    layout: Layout,
}
impl Region {
    fn new(size: usize) -> Self {
        let layout = Layout::from_size_align(size, 4096).unwrap();
        let pointer = unsafe { alloc_zeroed(layout) };
        assert!(!pointer.is_null());
        Self { pointer, layout }
    }
    fn allocator(&self) -> heap::LockedHeapAllocator {
        let allocator = heap::LockedHeapAllocator::new();
        unsafe {
            allocator.init(self.pointer as usize, self.layout.size());
        }
        allocator
    }
}
impl Drop for Region {
    fn drop(&mut self) {
        unsafe {
            dealloc(self.pointer, self.layout);
        }
    }
}

#[test]
fn alpha_reuses_released_fork_scratch_instead_of_exhausting_a_bump_heap() {
    let region = Region::new(256 * 1024);
    let allocator = region.allocator();
    assert!(allocator.is_initialized());
    let layout = Layout::from_size_align(20480, 4096).unwrap();
    for value in 0..20000 {
        unsafe {
            let pointer = allocator.alloc(layout);
            assert!(!pointer.is_null(), "allocation {value}");
            assert_eq!(pointer as usize % layout.align(), 0);
            pointer.write_bytes(value as u8, layout.size());
            assert_eq!(*pointer.add(layout.size() - 1), value as u8);
            allocator.dealloc(pointer, layout);
        }
    }
}

#[test]
fn alpha_realloc_preserves_data_and_failure_keeps_the_old_allocation_live() {
    let region = Region::new(256 * 1024);
    let allocator = region.allocator();
    let layout = Layout::from_size_align(300, 256).unwrap();
    unsafe {
        let pointer = allocator.alloc(layout);
        assert!(!pointer.is_null());
        pointer.write_bytes(0x39, layout.size());
        assert!(allocator.realloc(pointer, layout, 1024 * 1024).is_null());
        assert!((0..layout.size()).all(|index| *pointer.add(index) == 0x39));
        let grown = allocator.realloc(pointer, layout, 9000);
        assert!(!grown.is_null());
        assert_eq!(grown as usize % 256, 0);
        assert!((0..layout.size()).all(|index| *grown.add(index) == 0x39));
        let smaller = allocator.realloc(grown, Layout::from_size_align(9000, 256).unwrap(), 64);
        assert!(!smaller.is_null());
        assert!((0..64).all(|index| *smaller.add(index) == 0x39));
        allocator.dealloc(smaller, Layout::from_size_align(64, 256).unwrap());
    }
    assert!(unsafe { heap::LockedHeapAllocator::new().alloc(layout) }.is_null());
}

#[test]
fn alpha_parallel_allocate_free_coalesces_without_overlapping_live_blocks() {
    let region = Region::new(1024 * 1024);
    let allocator = region.allocator();
    std::thread::scope(|scope| {
        for owner in 1..=4 {
            let allocator = &allocator;
            scope.spawn(move || {
                for turn in 0..2000 {
                    let layout = Layout::from_size_align(127 + turn % 2048, 256).unwrap();
                    unsafe {
                        let first = allocator.alloc(layout);
                        let second = allocator.alloc(layout);
                        assert!(!first.is_null() && !second.is_null() && first != second);
                        first.write_bytes(owner, layout.size());
                        second.write_bytes(owner + 4, layout.size());
                        std::thread::yield_now();
                        assert!((0..layout.size()).all(|index| *first.add(index) == owner));
                        assert!((0..layout.size()).all(|index| *second.add(index) == owner + 4));
                        allocator.dealloc(second, layout);
                        allocator.dealloc(first, layout);
                    }
                }
            });
        }
    });
    let large = Layout::from_size_align(900 * 1024, 4096).unwrap();
    unsafe {
        let pointer = allocator.alloc(large);
        assert!(!pointer.is_null(), "free blocks must coalesce");
        allocator.dealloc(pointer, large);
    }
}
