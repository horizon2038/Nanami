//! Open-file-description state shared by dup and fork, allocated only on use.
use super::{LinuxFile, Word};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

pub const OBJECT_LIMIT: usize = 256;
pub const WATCH_LIMIT: usize = super::LINUX_FD_MAX;

#[derive(Clone, Copy)]
pub struct EventCounter {
    pub value: u64,
    pub semaphore: bool,
    pub nonblocking: bool,
}

#[derive(Clone, Copy)]
pub struct Timer {
    pub clock: Word,
    pub deadline: Option<Word>,
    pub interval: Word,
    pub expirations: u64,
    pub nonblocking: bool,
}

impl Timer {
    pub fn expire(&mut self, now: Word) {
        let Some(deadline) = self.deadline.filter(|deadline| *deadline <= now) else {
            return;
        };
        let count = if self.interval == 0 {
            1
        } else {
            (now - deadline) / self.interval + 1
        };
        self.expirations = self.expirations.saturating_add(count as u64);
        self.deadline = if self.interval == 0 {
            None
        } else {
            deadline
                .checked_add(count.saturating_mul(self.interval))
                .filter(|next| *next > now)
        };
    }
}

pub struct SignalFd {
    pub mask: u64,
    pub nonblocking: bool,
}

#[derive(Clone, Copy)]
pub struct MemoryMapping {
    pub pid: Word,
    pub base: Word,
    pub bytes: Word,
    pub offset: Word,
    pub prot: Word,
    pub shared: bool,
}

pub struct MemoryFile {
    pub anonymous: bool,
    /// Page index -> owned server mapping. Aliases reference the same RAM.
    pub pages: BTreeMap<Word, Word>,
    pub size: Word,
    pub offset: Word,
    pub status_flags: Word,
    pub seals: Word,
    pub maps: Vec<MemoryMapping>,
}

#[derive(Clone, Copy)]
pub struct Watch {
    pub fd: Word,
    pub file: LinuxFile,
    pub terminal: Word,
    pub events: u32,
    pub data: u64,
    pub armed: bool,
}

pub struct Epoll {
    pub watches: Vec<Watch>,
    pub cursor: usize,
}

pub struct IoUring {
    pub memory: Word,
    pub sqes: Word,
    pub entries: u32,
    pub cq_entries: u32,
    pub cq_offset: Word,
    pub ring_bytes: Word,
    pub sqe_bytes: Word,
    pub array: bool,
    pub head: u32,
    pub tail: u32,
    pub cancel_next: bool,
    pub overflow: alloc::collections::VecDeque<(u64, i32)>,
}

pub enum Object {
    Counter(EventCounter),
    Timer(Timer),
    Signals(SignalFd),
    Memory(MemoryFile),
    Epoll(Epoll),
    Ring(IoUring),
}

pub struct Objects {
    entries: Vec<(Word, Object)>,
    next_id: Word,
}

impl Objects {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_id: 1,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn timer_deadline(&self) -> Option<Word> {
        self.entries
            .iter()
            .filter_map(|(_, object)| match object {
                Object::Timer(timer) => timer.deadline,
                _ => None,
            })
            .min()
    }

    pub fn expire_timers(&mut self, now: Word) {
        for (_, object) in &mut self.entries {
            if let Object::Timer(timer) = object {
                timer.expire(now);
            }
        }
    }

    pub fn insert(&mut self, object: Object) -> Option<Word> {
        if self.entries.len() == OBJECT_LIMIT || self.next_id == Word::MAX {
            return None;
        }
        self.entries.try_reserve(1).ok()?;
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push((id, object));
        Some(id)
    }

    pub fn get(&self, id: Word) -> Option<&Object> {
        self.entries
            .iter()
            .find(|(key, _)| *key == id)
            .map(|(_, object)| object)
    }

    pub fn iter(&self) -> impl Iterator<Item = (Word, &Object)> {
        self.entries.iter().map(|(id, object)| (*id, object))
    }

    pub fn get_mut(&mut self, id: Word) -> Option<&mut Object> {
        self.entries
            .iter_mut()
            .find(|(key, _)| *key == id)
            .map(|(_, object)| object)
    }

    pub fn remove(&mut self, id: Word) {
        if let Some(index) = self.entries.iter().position(|(key, _)| *key == id) {
            self.entries.swap_remove(index);
        }
    }

    pub fn forget_file(&mut self, file: LinuxFile, terminal: Word) {
        for (_, object) in &mut self.entries {
            if let Object::Epoll(epoll) = object {
                epoll.watches.retain(|watch| {
                    watch.file.kind != file.kind
                        || watch.file.posix_fd != file.posix_fd
                        || watch.file.resource != file.resource
                        || (file.kind == super::LinuxFileKind::Terminal
                            && watch.terminal != terminal)
                });
            }
        }
    }
}
