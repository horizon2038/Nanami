use super::{PendingAsyncTimer, MAX_PENDING_ASYNC_TIMERS};

// Fixed-capacity min-heap: no allocation and no full-table scan on expiry.
pub(super) struct Deadlines {
    entries: [PendingAsyncTimer; MAX_PENDING_ASYNC_TIMERS],
    len: usize,
}

impl Deadlines {
    pub(super) const fn new() -> Self {
        Self {
            entries: [PendingAsyncTimer::EMPTY; MAX_PENDING_ASYNC_TIMERS],
            len: 0,
        }
    }

    pub(super) fn next(&self) -> Option<u64> {
        (self.len != 0).then(|| self.entries[0].target_tick)
    }

    pub(super) fn push(&mut self, timer: PendingAsyncTimer) -> bool {
        if self.len == self.entries.len() {
            return false;
        }
        let index = self.len;
        self.entries[index] = timer;
        self.len += 1;
        self.sift_up(index);
        true
    }

    pub(super) fn pop_due(&mut self, now: u64) -> Option<PendingAsyncTimer> {
        if self.next().is_none_or(|deadline| deadline > now) {
            return None;
        }
        Some(self.remove(0))
    }

    pub(super) fn remove_alarm(&mut self, descriptor: usize) {
        if let Some(index) = self.entries[..self.len]
            .iter()
            .position(|timer| timer.alarm && timer.notification_descriptor == descriptor)
        {
            self.remove(index);
        }
    }

    pub(super) fn retire(&mut self, descriptor: usize) {
        // Failure path only. Rebuild because sift-up can move a matching entry
        // into an already visited position during arbitrary removal.
        let mut kept = 0;
        for index in 0..self.len {
            if self.entries[index].notification_descriptor != descriptor {
                self.entries[kept] = self.entries[index];
                kept += 1;
            }
        }
        self.len = kept;
        for index in (0..self.len / 2).rev() {
            self.sift_down(index);
        }
    }

    fn remove(&mut self, index: usize) -> PendingAsyncTimer {
        let timer = self.entries[index];
        self.len -= 1;
        if index != self.len {
            self.entries[index] = self.entries[self.len];
            if index != 0
                && self.entries[index].target_tick < self.entries[(index - 1) / 2].target_tick
            {
                self.sift_up(index);
            } else {
                self.sift_down(index);
            }
        }
        timer
    }

    fn sift_up(&mut self, mut index: usize) {
        while index != 0 {
            let parent = (index - 1) / 2;
            if self.entries[parent].target_tick <= self.entries[index].target_tick {
                break;
            }
            self.entries.swap(parent, index);
            index = parent;
        }
    }

    fn sift_down(&mut self, mut index: usize) {
        loop {
            let left = index * 2 + 1;
            if left >= self.len {
                break;
            }
            let right = left + 1;
            let child = if right < self.len
                && self.entries[right].target_tick < self.entries[left].target_tick
            {
                right
            } else {
                left
            };
            if self.entries[index].target_tick <= self.entries[child].target_tick {
                break;
            }
            self.entries.swap(index, child);
            index = child;
        }
    }
}
