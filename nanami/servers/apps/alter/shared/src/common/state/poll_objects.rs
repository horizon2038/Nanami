//! Open-file-description state shared by dup and fork, allocated only on use.
use super::{LinuxFile, Word};
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

pub enum Object {
    Counter(EventCounter),
    Epoll(Epoll),
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
