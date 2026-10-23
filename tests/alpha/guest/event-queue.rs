//! Test-Service data layout, deliberately outside Alpha and the generic UAPI.
use core::sync::atomic::{AtomicUsize, Ordering};
pub const CAPACITY: usize = 8;

#[repr(C)]
pub struct Queue {
    pub tail: AtomicUsize,
    pub head: AtomicUsize,
    pub total: AtomicUsize,
    pub owner: AtomicUsize,
    pub accepted_calls: AtomicUsize,
    pub reply_release: AtomicUsize,
    entries: [AtomicUsize; CAPACITY],
}

impl Queue {
    pub fn push(&self, value: usize) {
        let tail = self.tail.load(Ordering::Relaxed);
        assert!(tail - self.head.load(Ordering::Acquire) < CAPACITY);
        self.entries[tail % CAPACITY].store(value, Ordering::Relaxed);
        self.tail.store(tail + 1, Ordering::Release);
    }
    pub fn drain(&self, owner: usize) {
        let mut head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        assert!(tail >= head && tail - head <= CAPACITY);
        while head != tail {
            let value = self.entries[head % CAPACITY].load(Ordering::Relaxed);
            self.total.fetch_add(value, Ordering::Relaxed);
            self.owner.store(owner, Ordering::Relaxed);
            head += 1;
            self.head.store(head, Ordering::Release);
        }
    }
}
