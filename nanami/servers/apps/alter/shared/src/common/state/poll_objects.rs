//! Open-file-description state shared by dup and fork, allocated only on use.
use super::{LinuxFile, Word};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

pub const OBJECT_LIMIT: usize = 256;
pub const WATCH_LIMIT: usize = super::LINUX_FD_MAX;
const TIMER_WORDS: usize = OBJECT_LIMIT.div_ceil(64);

#[cfg(test)]
static TIMER_INSPECTIONS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static EPOLL_INSPECTIONS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static RING_INSPECTIONS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
pub fn take_ring_inspections() -> usize {
    RING_INSPECTIONS.swap(0, core::sync::atomic::Ordering::Relaxed)
}
#[cfg(test)]
pub fn take_epoll_inspections() -> usize {
    EPOLL_INSPECTIONS.swap(0, core::sync::atomic::Ordering::Relaxed)
}
#[cfg(test)]
pub fn take_timer_inspections() -> usize {
    TIMER_INSPECTIONS.swap(0, core::sync::atomic::Ordering::Relaxed)
}

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
    /// Original shmat base; preserved when munmap splits an attachment.
    pub attachment: Option<Word>,
    pub readonly_attachment: bool,
}

pub struct SysvSegment {
    pub scope: Word,
    pub key: u32,
    pub mode: u16,
    pub creator: Word,
    pub last_pid: Word,
    pub created: Word,
    pub attached: Word,
    pub detached: Word,
    pub removed: bool,
}

pub struct MemoryFile {
    pub sysv: Option<SysvSegment>,
    pub anonymous: bool,
    /// A retained POSIX FD for VFS-owned, read-only shared file pages. The FD
    /// outlives every guest alias; normal memfd/anonymous RAM has no backing.
    pub backing: Option<Word>,
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
    // Derived only from armed watch kinds/events, never from live readiness.
    pub source_mask: core::cell::Cell<Option<Word>>,
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
    // Slot hints never establish identity. IDs are monotonic and collisions
    // fall back to the authoritative entries without changing their order.
    slots: [u16; OBJECT_LIMIT],
    // Derived membership only. Generic mutable access may replace any variant,
    // so invalidate before handing out &mut Object; rebuild on the next query.
    timer_slots: core::cell::Cell<Option<[u64; TIMER_WORDS]>>,
    epoll_slots: core::cell::Cell<Option<[u64; TIMER_WORDS]>>,
    ring_slots: core::cell::Cell<Option<[u64; TIMER_WORDS]>>,
    // None: unknown. Some(None): no armed timers. Some(Some(t)): no timer
    // can expire before t. Removal may leave this conservative bound early.
    expiry_bound: Option<Option<Word>>,
}

impl Objects {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_id: 1,
            slots: [u16::MAX; OBJECT_LIMIT],
            timer_slots: core::cell::Cell::new(Some([0; TIMER_WORDS])),
            epoll_slots: core::cell::Cell::new(Some([0; TIMER_WORDS])),
            ring_slots: core::cell::Cell::new(Some([0; TIMER_WORDS])),
            expiry_bound: Some(None),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn timer_slots(&self) -> [u64; TIMER_WORDS] {
        if let Some(slots) = self.timer_slots.get() {
            return slots;
        }
        let mut slots = [0; TIMER_WORDS];
        for (index, (_, object)) in self.entries.iter().enumerate() {
            #[cfg(test)]
            TIMER_INSPECTIONS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if matches!(object, Object::Timer(_)) {
                slots[index / 64] |= 1u64 << (index % 64);
            }
        }
        self.timer_slots.set(Some(slots));
        slots
    }

    fn epoll_slots(&self) -> [u64; TIMER_WORDS] {
        if let Some(slots) = self.epoll_slots.get() { return slots; }
        let mut slots = [0u64; TIMER_WORDS];
        for (index, (_, object)) in self.entries.iter().enumerate() {
            #[cfg(test)]
            EPOLL_INSPECTIONS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if matches!(object, Object::Epoll(_)) { slots[index / 64] |= 1u64 << (index % 64); }
        }
        self.epoll_slots.set(Some(slots));
        slots
    }

    fn ring_slots(&self) -> [u64; TIMER_WORDS] {
        if let Some(slots) = self.ring_slots.get() { return slots; }
        let mut slots = [0u64; TIMER_WORDS];
        for (index, (_, object)) in self.entries.iter().enumerate() {
            #[cfg(test)]
            RING_INSPECTIONS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if matches!(object, Object::Ring(_)) { slots[index / 64] |= 1u64 << (index % 64); }
        }
        self.ring_slots.set(Some(slots));
        slots
    }

    pub fn ring_references_memory(&self, id: Word) -> bool {
        Self::slot_indices(self.ring_slots()).any(|index| {
            #[cfg(test)]
            RING_INSPECTIONS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            matches!(self.entries.get(index), Some((_, Object::Ring(ring)))
                if ring.memory == id || ring.sqes == id)
        })
    }

    fn slot_indices(slots: [u64; TIMER_WORDS]) -> impl Iterator<Item = usize> {
        slots.into_iter().enumerate().flat_map(|(word, mut bits)| {
            core::iter::from_fn(move || {
                if bits == 0 {
                    return None;
                }
                let index = word * 64 + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                Some(index)
            })
        })
    }

    fn timer_indices(slots: [u64; TIMER_WORDS]) -> impl Iterator<Item = usize> {
        Self::slot_indices(slots).map(|index| {
            #[cfg(test)]
            TIMER_INSPECTIONS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            index
        })
    }

    fn epoll_indices(&self) -> impl Iterator<Item = usize> {
        Self::slot_indices(self.epoll_slots()).map(|index| {
            #[cfg(test)]
            EPOLL_INSPECTIONS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            index
        })
    }

    pub fn timer_deadline(&self) -> Option<Word> {
        Self::timer_indices(self.timer_slots())
            .filter_map(|index| match &self.entries[index].1 {
                Object::Timer(timer) => timer.deadline,
                _ => None,
            }).min()
    }

    pub fn expire_timers(&mut self, now: Word) {
        if self.expiry_bound.is_some_and(|bound| bound.is_none_or(|deadline| now < deadline)) {
            return;
        }
        let mut next: Option<Word> = None;
        for index in Self::timer_indices(self.timer_slots()) {
            if let Object::Timer(timer) = &mut self.entries[index].1 {
                timer.expire(now);
                if let Some(deadline) = timer.deadline {
                    next = Some(next.map_or(deadline, |previous| previous.min(deadline)));
                }
            }
        }
        self.expiry_bound = Some(next);
    }

    pub fn insert(&mut self, object: Object) -> Option<Word> {
        if self.entries.len() == OBJECT_LIMIT || self.next_id == Word::MAX {
            return None;
        }
        self.entries.try_reserve(1).ok()?;
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push((id, object));
        let index = self.entries.len() - 1;
        self.slots[id % OBJECT_LIMIT] = index as u16;
        #[cfg(test)]
        TIMER_INSPECTIONS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let is_timer = matches!(&self.entries[index].1, Object::Timer(_));
        if let Some(mut slots) = self.ring_slots.get() {
            let bit = 1u64 << (index % 64);
            slots[index / 64] &= !bit;
            if matches!(&self.entries[index].1, Object::Ring(_)) { slots[index / 64] |= bit; }
            self.ring_slots.set(Some(slots));
        }
        if let Some(mut slots) = self.epoll_slots.get() {
            let bit = 1u64 << (index % 64);
            slots[index / 64] &= !bit;
            if matches!(&self.entries[index].1, Object::Epoll(_)) { slots[index / 64] |= bit; }
            self.epoll_slots.set(Some(slots));
        }
        if is_timer {
            self.expiry_bound = None;
        }
        if let Some(mut slots) = self.timer_slots.get() {
            let bit = 1u64 << (index % 64);
            slots[index / 64] &= !bit;
            if is_timer {
                slots[index / 64] |= bit;
            }
            self.timer_slots.set(Some(slots));
        }
        Some(id)
    }

    fn index(&self, id: Word) -> Option<usize> {
        let slot = self.slots[id % OBJECT_LIMIT] as usize;
        if self.entries.get(slot).is_some_and(|(key, _)| *key == id) {
            return Some(slot);
        }
        self.entries.iter().position(|(key, _)| *key == id)
    }

    pub fn get(&self, id: Word) -> Option<&Object> {
        self.index(id).map(|slot| &self.entries[slot].1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (Word, &Object)> {
        self.entries.iter().map(|(id, object)| (*id, object))
    }

    pub fn get_mut(&mut self, id: Word) -> Option<&mut Object> {
        let slot = self.index(id)?;
        self.timer_slots.set(None);
        self.epoll_slots.set(None);
        self.ring_slots.set(None);
        self.expiry_bound = None;
        Some(&mut self.entries[slot].1)
    }

    pub fn remove(&mut self, id: Word) {
        if let Some(index) = self.index(id) {
            let last = self.entries.len() - 1;
            self.entries.swap_remove(index);
            if let Some(mut slots) = self.ring_slots.get() {
                slots[index / 64] &= !(1u64 << (index % 64));
                slots[last / 64] &= !(1u64 << (last % 64));
                if self.entries.get(index).is_some_and(|(_, object)| matches!(object, Object::Ring(_))) {
                    slots[index / 64] |= 1u64 << (index % 64);
                }
                self.ring_slots.set(Some(slots));
            }
            if let Some(mut slots) = self.epoll_slots.get() {
                slots[index / 64] &= !(1u64 << (index % 64));
                slots[last / 64] &= !(1u64 << (last % 64));
                if self.entries.get(index).is_some_and(|(_, object)| matches!(object, Object::Epoll(_))) {
                    slots[index / 64] |= 1u64 << (index % 64);
                }
                self.epoll_slots.set(Some(slots));
            }
            if let Some(mut slots) = self.timer_slots.get() {
                slots[index / 64] &= !(1u64 << (index % 64));
                slots[last / 64] &= !(1u64 << (last % 64));
                if let Some((_, object)) = self.entries.get(index) {
                    #[cfg(test)]
                    TIMER_INSPECTIONS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    if matches!(object, Object::Timer(_)) {
                        slots[index / 64] |= 1u64 << (index % 64);
                    }
                }
                self.timer_slots.set(Some(slots));
            }
            if let Some((moved, _)) = self.entries.get(index) {
                self.slots[*moved % OBJECT_LIMIT] = index as u16;
            }
        }
    }

    // These references cannot replace the enum variant, so timer membership
    // remains valid even when a Timer's deadline or other fields change.
    pub fn counter_mut(&mut self, id: Word) -> Option<&mut EventCounter> {
        let slot = self.index(id)?;
        match &mut self.entries[slot].1 { Object::Counter(value) => Some(value), _ => None }
    }
    pub fn timer_mut(&mut self, id: Word) -> Option<&mut Timer> {
        let slot = self.index(id)?;
        match &mut self.entries[slot].1 {
            Object::Timer(value) => { self.expiry_bound = None; Some(value) },
            _ => None,
        }
    }
    pub fn clear_timer_expirations(&mut self, id: Word) -> bool {
        let Some(slot) = self.index(id) else { return false; };
        match &mut self.entries[slot].1 {
            // Consuming a count cannot move any future expiration earlier.
            Object::Timer(timer) => { timer.expirations = 0; true },
            _ => false,
        }
    }
    pub fn signals_mut(&mut self, id: Word) -> Option<&mut SignalFd> {
        let slot = self.index(id)?;
        match &mut self.entries[slot].1 { Object::Signals(value) => Some(value), _ => None }
    }
    pub fn memory_mut(&mut self, id: Word) -> Option<&mut MemoryFile> {
        let slot = self.index(id)?;
        match &mut self.entries[slot].1 { Object::Memory(value) => Some(value), _ => None }
    }
    pub fn epoll_mut(&mut self, id: Word) -> Option<&mut Epoll> {
        let slot = self.index(id)?;
        match &mut self.entries[slot].1 { Object::Epoll(value) => Some(value), _ => None }
    }
    pub fn ring_mut(&mut self, id: Word) -> Option<&mut IoUring> {
        let slot = self.index(id)?;
        match &mut self.entries[slot].1 { Object::Ring(value) => Some(value), _ => None }
    }
    pub fn set_nonblocking(&mut self, id: Word, value: bool) -> bool {
        let Some(slot) = self.index(id) else { return false; };
        match &mut self.entries[slot].1 {
            Object::Counter(counter) => counter.nonblocking = value,
            Object::Timer(timer) => timer.nonblocking = value,
            Object::Signals(signals) => signals.nonblocking = value,
            _ => return false,
        }
        true
    }

    pub fn has_file_watch(&self, file: LinuxFile, terminal: Word) -> bool {
        self.epoll_indices().any(|index| {
            matches!(self.entries.get(index), Some((_, Object::Epoll(epoll)))
                if epoll.watches.iter().any(|watch| watch_matches_file(watch, file, terminal)))
        })
    }

    pub fn forget_file(&mut self, file: LinuxFile, terminal: Word) {
        let slots = self.epoll_slots();
        for index in Self::slot_indices(slots) {
            #[cfg(test)]
            EPOLL_INSPECTIONS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            let Some((_, object)) = self.entries.get_mut(index) else { continue; };
            if let Object::Epoll(epoll) = object {
                let previous = epoll.watches.len();
                epoll.watches.retain(|watch| !watch_matches_file(watch, file, terminal));
                if epoll.watches.len() != previous {
                    epoll.source_mask.set(None);
                }
            }
        }
    }
}

fn watch_matches_file(watch: &Watch, file: LinuxFile, terminal: Word) -> bool {
    watch.file.kind == file.kind
        && watch.file.posix_fd == file.posix_fd
        && watch.file.resource == file.resource
        && (file.kind != super::LinuxFileKind::Terminal || watch.terminal == terminal)
}
