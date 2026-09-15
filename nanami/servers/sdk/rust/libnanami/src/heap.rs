use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::{RequestError, Word};

#[path = "heap/pool.rs"]
mod pool;
use pool::Pool;

#[cfg_attr(not(test), global_allocator)]
static GLOBAL_ALLOCATOR: HeapAllocator = HeapAllocator::new();

extern "C" {
    static __heap_start: u8;
    static __heap_end: u8;
}

pub fn linker_heap_range() -> (Word, Word) {
    unsafe {
        (
            (&__heap_start as *const u8) as Word,
            (&__heap_end as *const u8) as Word,
        )
    }
}

/// Add a mapped heap region without invalidating any existing allocations.
pub fn init_heap(size_bytes: Word) -> Result<(Word, Word), RequestError> {
    let _growth = LockGuard::acquire(&GLOBAL_ALLOCATOR.growth_lock);
    GLOBAL_ALLOCATOR.map_region(size_bytes)
}

/// Used/free block bytes (including headers/padding), and their sum. Free bytes
/// are not a guarantee that a single allocation of that size will succeed.
/// This diagnostic walks the pools; alloc/free never do so.
pub fn heap_stats() -> (Word, Word, Word) {
    GLOBAL_ALLOCATOR.with_pool(|pool| pool.stats())
}

struct HeapAllocator {
    pool: UnsafeCell<Pool>,
    lock: AtomicBool,
    growth_lock: AtomicBool,
}

// The pool is accessed only under lock. Growth is separately serialized so
// IPC never holds that lock, and concurrent misses cannot waste heap mappings.
unsafe impl Sync for HeapAllocator {}

impl HeapAllocator {
    const fn new() -> Self {
        Self {
            pool: UnsafeCell::new(Pool::new()),
            lock: AtomicBool::new(false),
            growth_lock: AtomicBool::new(false),
        }
    }

    fn with_pool<R>(&self, operation: impl FnOnce(&mut Pool) -> R) -> R {
        let _guard = LockGuard::acquire(&self.lock);
        operation(unsafe { &mut *self.pool.get() })
    }

    // Caller holds growth_lock. All accepted mappings remain owned by this
    // process until exit; TLSF only recycles memory within them.
    fn map_region(&self, size_bytes: Word) -> Result<(Word, Word), RequestError> {
        if size_bytes == 0 {
            return Err(RequestError::InvalidArgument);
        }
        if !self.with_pool(|pool| pool.can_grow()) {
            return Err(RequestError::Unsupported);
        }
        let (base, size) = crate::request_heap(size_bytes)?;
        if !self.with_pool(|pool| unsafe { pool.add_region(base, size) }) {
            // An invalid/overlapping reply must never cause us to unmap an
            // address range that might contain existing live allocations.
            return Err(RequestError::Protocol);
        }
        Ok((base, size))
    }

    #[cold]
    unsafe fn grow(&self, layout: Layout, previous: Option<NonNull<u8>>) -> *mut u8 {
        let Some(size) = pool::growth_size(layout) else {
            return ptr::null_mut();
        };
        let _growth = LockGuard::acquire(&self.growth_lock);
        let retry = |pool: &mut Pool| match previous {
            Some(pointer) => pool.reallocate(pointer, layout),
            None => pool.allocate(layout),
        };
        // A concurrent free or growth may have satisfied this request while
        // we waited for growth_lock. Recheck before issuing an Alpha request.
        let result = self.with_pool(retry);
        if !result.is_null() {
            return result;
        }
        if self.map_region(size).is_err() {
            return ptr::null_mut();
        }
        self.with_pool(retry)
    }
}

unsafe impl GlobalAlloc for HeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() == 0 {
            return layout.align() as *mut u8;
        }
        let result = self.with_pool(|pool| pool.allocate(layout));
        if !result.is_null() {
            return result;
        }
        self.grow(layout, None)
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if layout.size() == 0 {
            return;
        }
        if let Some(pointer) = NonNull::new(pointer) {
            self.with_pool(|pool| pool.deallocate(pointer, layout.align()));
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
        let result = self.with_pool(|pool| pool.reallocate(pointer, new_layout));
        if !result.is_null() {
            return result;
        }
        self.grow(new_layout, Some(pointer))
    }
}

struct LockGuard<'a>(&'a AtomicBool);

impl<'a> LockGuard<'a> {
    fn acquire(lock: &'a AtomicBool) -> Self {
        loop {
            if lock
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return Self(lock);
            }
            while lock.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
    }
}

impl Drop for LockGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
