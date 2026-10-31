//! Alpha's fixed mapped heap: recycle transient process/VM bookkeeping with TLSF.
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicBool, Ordering};
use rlsf::{Tlsf, GRANULARITY};

struct HeapState {
    allocator: Tlsf<'static, u32, u32, 32, 32>,
    initialized: bool,
}

pub struct LockedHeapAllocator {
    lock: AtomicBool,
    state: UnsafeCell<HeapState>,
}

// All TLSF metadata access is serialized; the mapped region stays owned by Alpha.
unsafe impl Sync for LockedHeapAllocator {}

impl LockedHeapAllocator {
    pub const fn new() -> Self {
        Self {
            lock: AtomicBool::new(false),
            state: UnsafeCell::new(HeapState {
                allocator: Tlsf::new(),
                initialized: false,
            }),
        }
    }

    fn with_state<R>(&self, operation: impl FnOnce(&mut HeapState) -> R) -> R {
        while self
            .lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.lock.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
        let _guard = Unlock(&self.lock);
        operation(unsafe { &mut *self.state.get() })
    }

    /// Initialize once, before allocation. The caller exclusively owns writable
    /// `start..start+size`, mapped for the allocator's entire lifetime.
    pub unsafe fn init(&self, start: usize, size: usize) {
        let end = start.checked_add(size).expect("Alpha heap range overflow") & !(GRANULARITY - 1);
        let aligned = start
            .checked_add(GRANULARITY - 1)
            .expect("Alpha heap alignment overflow")
            & !(GRANULARITY - 1);
        assert!(
            start != 0
                && size <= isize::MAX as usize
                && end.saturating_sub(aligned) >= GRANULARITY * 2
        );
        self.with_state(|state| {
            assert!(!state.initialized, "Alpha heap already initialized");
            let region = NonNull::new_unchecked(ptr::slice_from_raw_parts_mut(
                aligned as *mut u8,
                end - aligned,
            ));
            assert!(
                state.allocator.insert_free_block_ptr(region).is_some(),
                "Alpha heap region rejected"
            );
            state.initialized = true;
        });
    }

    pub fn is_initialized(&self) -> bool {
        self.with_state(|state| state.initialized)
    }
}

unsafe impl GlobalAlloc for LockedHeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() == 0 {
            return layout.align() as *mut u8;
        }
        self.with_state(|state| {
            if !state.initialized {
                return ptr::null_mut();
            }
            state
                .allocator
                .allocate(layout)
                .map_or(ptr::null_mut(), NonNull::as_ptr)
        })
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if layout.size() != 0 {
            if let Some(pointer) = NonNull::new(pointer) {
                self.with_state(|state| state.allocator.deallocate(pointer, layout.align()));
            }
        }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size == 0 {
            self.dealloc(pointer, layout);
            return ptr::null_mut();
        }
        let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) else {
            return ptr::null_mut();
        };
        let Some(pointer) = NonNull::new(pointer).filter(|_| layout.size() != 0) else {
            return self.alloc(new_layout);
        };
        self.with_state(|state| {
            state
                .allocator
                .reallocate(pointer, new_layout)
                .map_or(ptr::null_mut(), NonNull::as_ptr)
        })
    }
}

struct Unlock<'a>(&'a AtomicBool);
impl Drop for Unlock<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[cfg_attr(not(test), global_allocator)]
pub static GLOBAL_ALLOCATOR: LockedHeapAllocator = LockedHeapAllocator::new();

pub unsafe fn init_global_heap(heap_start: usize, heap_size: usize) {
    GLOBAL_ALLOCATOR.init(heap_start, heap_size);
}
