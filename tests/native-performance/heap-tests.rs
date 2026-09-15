#![allow(dead_code)]

use std::alloc::{alloc_zeroed, dealloc, Layout};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

pub type Word = usize;
#[derive(Debug)]
pub enum RequestError {
    InvalidArgument,
    Unsupported,
    Transport,
    Protocol,
    Status(Word),
}

#[macro_export]
macro_rules! println {
    ($($arg:tt)*) => { std::eprintln!($($arg)*) };
}

static GROWTH_ENABLED: AtomicBool = AtomicBool::new(false);
static REQUEST_COUNT: AtomicUsize = AtomicUsize::new(0);
static MAPPINGS: Mutex<Vec<Region>> = Mutex::new(Vec::new());

pub fn request_heap(size: Word) -> Result<(Word, Word), RequestError> {
    REQUEST_COUNT.fetch_add(1, Ordering::Relaxed);
    if !GROWTH_ENABLED.load(Ordering::Relaxed) {
        return Err(RequestError::Unsupported);
    }
    let region = Region::new(size);
    let result = (region.start, size);
    MAPPINGS.lock().unwrap().push(region);
    Ok(result)
}

struct Region {
    start: usize,
    layout: Layout,
}

impl Region {
    fn new(size: usize) -> Self {
        let layout = Layout::from_size_align(size, 4096).unwrap();
        let start = unsafe { alloc_zeroed(layout) } as usize;
        assert_ne!(start, 0);
        Self { start, layout }
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        unsafe { dealloc(self.start as *mut u8, self.layout) };
    }
}

#[cfg(test)]
mod heap {
    include!(env!("NANAMI_HEAP_SOURCE"));

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::{Region, GROWTH_ENABLED, MAPPINGS, REQUEST_COUNT};
        use std::hint::black_box;

        struct Growth;

        impl Growth {
            fn enable() -> Self {
                REQUEST_COUNT.store(0, Ordering::Relaxed);
                GROWTH_ENABLED.store(true, Ordering::Relaxed);
                Self
            }
        }

        impl Drop for Growth {
            fn drop(&mut self) {
                GROWTH_ENABLED.store(false, Ordering::Relaxed);
                MAPPINGS.lock().unwrap().clear();
            }
        }

        #[cfg(legacy_heap)]
        type TestAllocator = ImplicitListAllocator;
        #[cfg(not(legacy_heap))]
        type TestAllocator = HeapAllocator;

        fn allocator(region: &Region) -> TestAllocator {
            let heap = TestAllocator::new();
            #[cfg(legacy_heap)]
            unsafe {
                heap.init(region.start, region.layout.size());
            }
            #[cfg(not(legacy_heap))]
            assert!(heap
                .with_pool(|pool| unsafe { pool.add_region(region.start, region.layout.size()) }));
            heap
        }

        unsafe fn check(pointer: *mut u8, size: usize, byte: u8) {
            assert!(!pointer.is_null());
            assert!(std::slice::from_raw_parts(pointer, size)
                .iter()
                .all(|x| *x == byte));
        }

        #[test]
        fn alignment_zeroing_and_reuse() {
            let region = Region::new(16 * 1024 * 1024);
            let heap = allocator(&region);
            for exponent in 0..=20 {
                for size in [1, 15, 16, 17, 31, 32, 33, 127, 1023, 4097] {
                    let layout = Layout::from_size_align(size, 1 << exponent).unwrap();
                    unsafe {
                        let pointer = heap.alloc(layout);
                        assert!(!pointer.is_null());
                        assert_eq!(pointer as usize % layout.align(), 0);
                        pointer.write_bytes(0xa5, size);
                        heap.dealloc(pointer, layout);
                        let zeroed = heap.alloc_zeroed(layout);
                        check(zeroed, size, 0);
                        heap.dealloc(zeroed, layout);
                    }
                }
            }
        }

        #[test]
        fn randomized_alloc_free_realloc_preserve_bytes() {
            let region = Region::new(16 * 1024 * 1024);
            let heap = allocator(&region);
            let mut live: Vec<(*mut u8, Layout, u8)> = Vec::new();
            let mut seed = 73u64;
            let mut peak_live = 0;
            for step in 0..20000 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let index = (seed >> 32) as usize % live.len().max(1);
                let size = (seed as usize >> 8) % 8192 + 1;
                let action = (seed >> 48) % 100;
                unsafe {
                    if live.is_empty() || (live.len() < 256 && action < 45) {
                        let layout =
                            Layout::from_size_align(size, 1 << ((seed >> 24) % 13)).unwrap();
                        let pointer = heap.alloc(layout);
                        assert!(!pointer.is_null());
                        assert_eq!(pointer as usize % layout.align(), 0);
                        let byte = (step % 251) as u8;
                        pointer.write_bytes(byte, size);
                        live.push((pointer, layout, byte));
                        peak_live = peak_live.max(live.len());
                    } else {
                        let (pointer, layout, byte) = live[index];
                        check(pointer, layout.size(), byte);
                        if action >= 70 {
                            let new_pointer = heap.realloc(pointer, layout, size);
                            check(new_pointer, layout.size().min(size), byte);
                            assert_eq!(new_pointer as usize % layout.align(), 0);
                            new_pointer.write_bytes(byte, size);
                            live[index] = (
                                new_pointer,
                                Layout::from_size_align(size, layout.align()).unwrap(),
                                byte,
                            );
                        } else {
                            heap.dealloc(pointer, layout);
                            live.swap_remove(index);
                        }
                    }
                }
            }
            assert!(peak_live > 64);
            for (pointer, layout, byte) in live {
                unsafe {
                    check(pointer, layout.size(), byte);
                    heap.dealloc(pointer, layout);
                }
            }
            #[cfg(not(legacy_heap))]
            assert_eq!(heap.with_pool(|pool| pool.stats()).0, 0);
        }

        #[test]
        #[cfg(not(legacy_heap))]
        fn pool_boundaries_and_growth_size_classes() {
            let region = Region::new(8192);
            unsafe { (region.start as *mut u8).write_bytes(0xab, 8192) };
            let mut pool = pool::Pool::new();
            assert!(unsafe { pool.add_region(region.start + 1, 8190) });
            let layout = Layout::from_size_align(8000, 16).unwrap();
            let pointer = pool.allocate(layout);
            assert!(!pointer.is_null());
            unsafe {
                pointer.write_bytes(0x57, layout.size());
                pool.deallocate(NonNull::new(pointer).unwrap(), layout.align());
                check(region.start as *mut u8, 32, 0xab);
                check((region.start + 8160) as *mut u8, 32, 0xab);
            }
            for align in [1, 64, 4096, 1024 * 1024] {
                for size in [
                    1,
                    31,
                    32,
                    33,
                    1023,
                    1024,
                    1025,
                    4096,
                    (1 << 20) - 32,
                    (1 << 20) + 32,
                    (4 << 20) - 32,
                    (4 << 20) + 1,
                ] {
                    let layout = Layout::from_size_align(size, align).unwrap();
                    let region = Region::new(pool::growth_size(layout).unwrap());
                    let heap = allocator(&region);
                    unsafe {
                        // Use the pool directly so an undersized region cannot
                        // be hidden by another automatic growth request.
                        let pointer = heap.with_pool(|pool| pool.allocate(layout));
                        assert!(!pointer.is_null(), "size={size} align={align}");
                        assert_eq!(pointer as usize % align, 0);
                        pointer.write_bytes(0x61, size);
                        heap.dealloc(pointer, layout);
                    }
                }
            }
        }

        #[test]
        #[cfg(not(legacy_heap))]
        fn coalescing_and_in_place_reallocation() {
            let region = Region::new(1024 * 1024);
            let heap = allocator(&region);
            let layout = Layout::from_size_align(4096, 64).unwrap();
            unsafe {
                let first = heap.alloc(layout);
                let second = heap.alloc(layout);
                let third = heap.alloc(layout);
                first.write_bytes(0x5a, layout.size());
                heap.dealloc(second, layout);
                let grown = heap.realloc(first, layout, 7000);
                assert_eq!(grown, first);
                check(grown, 4096, 0x5a);
                let grown_layout = Layout::from_size_align(7000, 64).unwrap();
                let shrunk = heap.realloc(grown, grown_layout, 32);
                assert_eq!(shrunk, first);
                check(shrunk, 32, 0x5a);
                heap.dealloc(shrunk, Layout::from_size_align(32, 64).unwrap());
                heap.dealloc(third, layout);
                let large = Layout::from_size_align(900 * 1024, 4096).unwrap();
                let pointer = heap.alloc(large);
                assert!(!pointer.is_null());
                heap.dealloc(pointer, large);
            }
            assert_eq!(heap.with_pool(|pool| pool.stats()).0, 0);
        }

        #[test]
        fn failed_reallocation_preserves_original() {
            let region = Region::new(1024 * 1024);
            let heap = allocator(&region);
            let layout = Layout::from_size_align(128, 64).unwrap();
            unsafe {
                let pointer = heap.alloc(layout);
                pointer.write_bytes(0x37, layout.size());
                assert!(heap.realloc(pointer, layout, 32 * 1024 * 1024).is_null());
                check(pointer, layout.size(), 0x37);
                // Valid GlobalAlloc layout, but beyond this allocator's limit.
                let too_large = isize::MAX as usize & !(layout.align() - 1);
                assert!(heap.realloc(pointer, layout, too_large).is_null());
                check(pointer, layout.size(), 0x37);
                heap.dealloc(pointer, layout);
            }
        }

        #[test]
        #[cfg(not(legacy_heap))]
        fn regions_growth_limits_and_existing_allocations() {
            let region = Region::new(4096);
            let heap = allocator(&region);
            assert!(!heap.with_pool(|pool| unsafe { pool.add_region(region.start, 4096) }));
            assert!(!heap.with_pool(|pool| unsafe { pool.add_region(usize::MAX - 15, 4096) }));
            assert!(!heap.with_pool(|pool| unsafe { pool.add_region(0, 4096) }));
            let _growth = Growth::enable();
            unsafe {
                let small = Layout::from_size_align(128, 16).unwrap();
                let pointer = heap.alloc(small);
                pointer.write_bytes(0x19, small.size());
                assert!(heap.map_region(4096).is_ok());
                check(pointer, small.size(), 0x19);
                // Large/aligned growth needs size-class slack, not only headers.
                let large = Layout::from_size_align(8 * 1024 * 1024, 65536).unwrap();
                let grown = heap.alloc(large);
                assert!(!grown.is_null());
                assert_eq!(grown as usize % large.align(), 0);
                heap.dealloc(grown, large);
                heap.dealloc(pointer, small);
                while heap.with_pool(|pool| pool.can_grow()) {
                    assert!(heap.map_region(4096).is_ok());
                }
                let requests = REQUEST_COUNT.load(Ordering::Relaxed);
                assert!(heap.map_region(4096).is_err());
                assert_eq!(REQUEST_COUNT.load(Ordering::Relaxed), requests);
            }
            drop(heap);
        }

        #[test]
        #[cfg(not(legacy_heap))]
        fn reallocation_across_mappings() {
            let _growth = Growth::enable();
            let region = Region::new(4096);
            let heap = allocator(&region);
            let original = Layout::from_size_align(2048, 64).unwrap();
            let expanded = Layout::from_size_align(2 * 1024 * 1024, 64).unwrap();
            unsafe {
                let pointer = heap.alloc(original);
                assert!(!pointer.is_null());
                pointer.write_bytes(0x73, original.size());
                let grown = heap.realloc(pointer, original, expanded.size());
                check(grown, original.size(), 0x73);
                assert_eq!(grown as usize % expanded.align(), 0);
                grown.write_bytes(0x42, expanded.size());
                heap.dealloc(grown, expanded);
            }
            assert_eq!(REQUEST_COUNT.load(Ordering::Relaxed), 1);
            assert_eq!(heap.with_pool(|pool| pool.stats()).0, 0);
        }

        #[test]
        #[cfg(not(legacy_heap))]
        fn concurrent_misses_share_one_growth_request() {
            let _growth = Growth::enable();
            let heap = HeapAllocator::new();
            let start = std::sync::Barrier::new(8);
            let allocated = std::sync::Barrier::new(8);
            std::thread::scope(|scope| {
                for byte in 1..=8 {
                    let (heap, start, allocated) = (&heap, &start, &allocated);
                    scope.spawn(move || unsafe {
                        let layout = Layout::from_size_align(4096, 4096).unwrap();
                        start.wait();
                        let pointer = heap.alloc(layout);
                        assert!(!pointer.is_null());
                        pointer.write_bytes(byte, layout.size());
                        allocated.wait();
                        check(pointer, layout.size(), byte);
                        heap.dealloc(pointer, layout);
                    });
                }
            });
            assert_eq!(REQUEST_COUNT.load(Ordering::Relaxed), 1);
            assert_eq!(heap.with_pool(|pool| pool.stats()).0, 0);
        }

        #[test]
        fn concurrent_allocations() {
            let region = Region::new(16 * 1024 * 1024);
            let heap = allocator(&region);
            std::thread::scope(|scope| {
                for worker in 0..4 {
                    let heap = &heap;
                    scope.spawn(move || {
                        for index in 0..10000 {
                            let layout = Layout::from_size_align(17 + index % 1024, 64).unwrap();
                            unsafe {
                                let pointer = heap.alloc(layout);
                                assert!(!pointer.is_null());
                                pointer.write_bytes(worker + 1, layout.size());
                                check(pointer, layout.size(), worker + 1);
                                heap.dealloc(pointer, layout);
                            }
                        }
                    });
                }
            });
        }

        #[test]
        #[ignore = "timing workload; compare release builds, not correctness test timings"]
        fn heap_benchmark() {
            for count in [128, 1024, 4096] {
                let region = Region::new(16 * 1024 * 1024);
                let heap = allocator(&region);
                let layout = Layout::from_size_align(64, 16).unwrap();
                let live: Vec<_> = (0..count).map(|_| unsafe { heap.alloc(layout) }).collect();
                assert!(live.iter().all(|pointer| !pointer.is_null()));
                let mut samples = Vec::new();
                for _ in 0..5 {
                    let start = std::time::Instant::now();
                    for _ in 0..10000 {
                        unsafe {
                            let pointer = black_box(heap.alloc(black_box(layout)));
                            assert!(!pointer.is_null());
                            heap.dealloc(pointer, layout);
                        }
                    }
                    samples.push(start.elapsed().as_nanos() / 10000);
                }
                samples.sort_unstable();
                std::println!("live={count} alloc/free pair median={} ns", samples[2]);
                for pointer in live {
                    unsafe { heap.dealloc(pointer, layout) };
                }
            }
        }
    }
}
