//! Lazily allocated per-task synchronization state (independent of signal state).
use super::Word;
use alloc::vec::Vec;

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
        let index = if let Some(index) = self.entries.iter().position(|task| task.pid == pid) {
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
}
