use core::alloc::{GlobalAlloc, Layout};
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, Ordering};

struct BumpState {
    start: usize,
    end: usize,
    current: usize,
    initialized: bool,
}

impl BumpState {
    const fn new() -> Self {
        Self {
            start: 0,
            end: 0,
            current: 0,
            initialized: false,
        }
    }
}

pub struct LockedBumpAllocator {
    lock: AtomicBool,
    state: core::cell::UnsafeCell<BumpState>,
}

unsafe impl Sync for LockedBumpAllocator {}

impl LockedBumpAllocator {
    pub const fn new() -> Self {
        Self {
            lock: AtomicBool::new(false),
            state: core::cell::UnsafeCell::new(BumpState::new()),
        }
    }

    fn lock(&self) {
        while self
            .lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }

    fn unlock(&self) {
        self.lock.store(false, Ordering::Release);
    }

    pub unsafe fn init(&self, heap_start: usize, heap_size: usize) {
        self.lock();
        let state = &mut *self.state.get();
        state.start = heap_start;
        state.end = heap_start.saturating_add(heap_size);
        state.current = heap_start;
        state.initialized = true;
        self.unlock();
    }

    pub fn is_initialized(&self) -> bool {
        self.lock();
        let init = unsafe { (*self.state.get()).initialized };
        self.unlock();
        init
    }
}

unsafe impl GlobalAlloc for LockedBumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.lock();
        let state = &mut *self.state.get();
        if !state.initialized {
            self.unlock();
            return null_mut();
        }

        let align = layout.align().max(1);
        let aligned = (state.current + align - 1) & !(align - 1);
        let next = aligned.saturating_add(layout.size());

        if next > state.end {
            self.unlock();
            return null_mut();
        }

        state.current = next;
        self.unlock();
        aligned as *mut u8
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
pub static GLOBAL_ALLOCATOR: LockedBumpAllocator = LockedBumpAllocator::new();

pub unsafe fn init_global_heap(heap_start: usize, heap_size: usize) {
    GLOBAL_ALLOCATOR.init(heap_start, heap_size);
}
