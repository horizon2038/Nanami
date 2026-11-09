//! Lazily allocated per-task synchronization state (independent of signal state).
use super::Word;
use alloc::vec::Vec;

#[cfg(test)]
static PID_CHECKS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FutexKey {
    pub scope: Word,
    pub address: Word,
    pub shared: bool,
}

#[derive(Clone, Copy)]
pub struct FutexWait {
    pub key: FutexKey,
    pub bitset: u32,
    pub deadline: Option<Word>,
    pub result: Option<isize>,
}

#[derive(Default)]
pub struct Task {
    pub pid: Word,
    pub clear_tid: Word,
    pub robust_list: Word,
    pub wait: Option<FutexWait>,
}

pub struct Tasks {
    entries: Vec<Task>,
}

impl Tasks {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
    pub fn get(&self, pid: Word) -> Option<&Task> {
        self.entries.iter().find(|task| task.pid == pid)
    }
    pub fn ensure(&mut self, pid: Word) -> Option<&mut Task> {
        let index = if let Some(index) = self.entries.iter().position(|task| {
            #[cfg(test)]
            PID_CHECKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            task.pid == pid
        }) {
            index
        } else {
            self.entries.try_reserve(1).ok()?;
            self.entries.push(Task {
                pid,
                ..Task::default()
            });
            self.entries.len() - 1
        };
        self.entries.get_mut(index)
    }
    #[cfg(test)]
    pub fn take_pid_checks(&self) -> usize {
        PID_CHECKS.swap(0, core::sync::atomic::Ordering::Relaxed)
    }
    /// Reuse a request-local slot only when its current PID still matches.
    /// Stale/out-of-range hints retain the normal lookup/creation semantics.
    pub fn ensure_at(&mut self, index: usize, pid: Word) -> Option<&mut Task> {
        if self.entries.get(index).is_some_and(|task| {
            #[cfg(test)]
            PID_CHECKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            task.pid == pid
        }) {
            return self.entries.get_mut(index);
        }
        self.ensure(pid)
    }
    pub fn remove(&mut self, pid: Word) {
        if let Some(index) = self.entries.iter().position(|task| task.pid == pid) {
            self.entries.swap_remove(index);
        }
    }
    pub fn waiters(&self) -> impl Iterator<Item = (Word, FutexWait)> + '_ {
        self.entries
            .iter()
            .filter_map(|task| task.wait.map(|wait| (task.pid, wait)))
    }

    /// Request-local walk in the same slot order as waiters(). The caller may
    /// update existing waits between items, but must not insert/remove tasks.
    pub fn next_waiter(&self, cursor: &mut usize) -> Option<(Word, FutexWait)> {
        while let Some(task) = self.entries.get(*cursor) {
            *cursor += 1;
            if let Some(wait) = task.wait { return Some((task.pid, wait)); }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_slots_fall_back_after_reuse_mismatch_and_out_of_range() {
        let mut tasks = Tasks::new();
        for (pid, value) in [(0, 1), (7, 2), (Word::MAX, 3)] {
            tasks.ensure(pid).unwrap().clear_tid = value;
        }
        tasks.ensure_at(1, 7).unwrap().robust_list = 70;
        tasks.remove(0);
        tasks.ensure_at(0, 7).unwrap().clear_tid = 72;
        assert_eq!(tasks.get(Word::MAX).unwrap().clear_tid, 3);
        assert_eq!((tasks.get(7).unwrap().clear_tid, tasks.get(7).unwrap().robust_list), (72, 70));
        tasks.ensure_at(usize::MAX, Word::MAX).unwrap().robust_list = 33;
        tasks.ensure_at(0, 99).unwrap().clear_tid = 99;
        assert_eq!(tasks.entries.len(), 3);
        tasks.remove(7);
        tasks.ensure_at(1, Word::MAX).unwrap().robust_list = 35;
        assert_eq!(tasks.get(99).unwrap().clear_tid, 99);
        let task = tasks.ensure_at(0, Word::MAX).unwrap();
        assert_eq!((task.clear_tid, task.robust_list), (3, 35));
    }

    #[test]
    fn cursor_preserves_snapshot_order_after_removal_while_clearing_current_waits() {
        let mut tasks = Tasks::new();
        for pid in 1..=40 {
            let task = tasks.ensure(pid).unwrap();
            task.clear_tid = pid * 8;
            task.robust_list = pid * 16;
            if pid % 3 != 0 {
                task.wait = Some(FutexWait {
                    key: FutexKey { scope: pid % 4, address: pid * 4096, shared: pid % 2 == 0 },
                    bitset: 1u32 << (pid % 32), deadline: (pid % 2 == 0).then_some(pid * 100),
                    result: (pid % 5 == 0).then_some(0),
                });
            }
        }
        for pid in [1, 13, 40] { tasks.remove(pid); }
        let expected: Vec<_> = tasks.waiters().collect();
        let mut cursor = 0;
        let mut visited = 0;
        while let Some((pid, wait)) = tasks.next_waiter(&mut cursor) {
            let (old_pid, old_wait) = expected[visited];
            assert_eq!((pid, wait.key, wait.bitset, wait.deadline, wait.result),
                (old_pid, old_wait.key, old_wait.bitset, old_wait.deadline, old_wait.result));
            let task = tasks.ensure(pid).unwrap();
            assert_eq!((task.clear_tid, task.robust_list), (pid * 8, pid * 16));
            task.wait = None;
            visited += 1;
        }
        assert_eq!(visited, expected.len());
        assert!(tasks.waiters().next().is_none());
    }
}
