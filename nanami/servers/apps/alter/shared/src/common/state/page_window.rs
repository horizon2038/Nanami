//! Bounded private RAM leases. Logical VM generations reject stale aliases;
//! Alpha's ordinary ownership references pin backing until confirmed release.
use super::Word;
use core::{cell::RefCell, mem::MaybeUninit};

pub const PAGE: Word = 4096;
pub const MAX_CONTIGUOUS_READ_BYTES: usize = 4 * PAGE;
/// Bounded optional fast path; larger vectors retain the checked-copy service.
pub const MAX_WRITE_BUFFERS: usize = 4;
/// A contiguous owned copy touches at most five distinct consecutive sets.
pub const MAX_CONTIGUOUS_WRITE_BYTES: usize = 4 * PAGE;
const CONTIGUOUS_WRITE_PAGES: usize = MAX_CONTIGUOUS_WRITE_BYTES / PAGE + 1;
const READ_BASE: Word = 0x20_0000_0000;
const WRITE_BASE: Word = READ_BASE + SLOTS * PAGE;
const SETS: usize = 16;
const WAYS: usize = 4;
const SLOTS: usize = SETS * WAYS;
const _: () = assert!(CONTIGUOUS_WRITE_PAGES <= SETS);

#[derive(Clone, Copy)]
struct Lease {
    owner: Word,
    generation: Word,
    page: Word,
    occupied: bool,
    ready: bool,
}
impl Lease {
    const EMPTY: Self = Self {
        owner: 0,
        generation: 0,
        page: 0,
        occupied: false,
        ready: false,
    };
}
struct State {
    leases: [Lease; SLOTS],
    next: [usize; SETS],
}

pub enum GrantError {
    NotInstalled,
    Uncertain,
}

pub trait LeaseEffects {
    fn grant(&mut self, pid: Word, page: Word, local: Word) -> Result<(), GrantError>;
    fn release(&mut self, local: Word) -> Result<(), ()>;
}
pub trait Effects: LeaseEffects {
    fn read(&mut self, local: Word, offset: Word, destination: &mut [MaybeUninit<u8>]);
}
pub trait WriteEffects: LeaseEffects {
    fn write(&mut self, local: Word, offset: Word, source: &[u8]);
}

struct Cache {
    base: Word,
    state: RefCell<State>,
}
impl Cache {
    const fn new(base: Word) -> Self {
        Self {
            base,
            state: RefCell::new(State {
                leases: [Lease::EMPTY; SLOTS],
                next: [0; SETS],
            }),
        }
    }

    fn acquire(
        &self,
        state: &mut State,
        owner: Word,
        generation: Word,
        pid: Word,
        page: Word,
        effects: &mut impl LeaseEffects,
    ) -> Option<Word> {
        let set = ((page / PAGE) ^ generation.rotate_left(7) ^ owner.rotate_right(3)) & (SETS - 1);
        let first = set * WAYS;
        let found = state.leases[first..first + WAYS].iter().position(|lease| {
            lease.ready
                && lease.owner == owner
                && lease.generation == generation
                && lease.page == page
        });
        let index = if let Some(way) = found {
            first + way
        } else {
            let way = state.leases[first..first + WAYS]
                .iter()
                .position(|lease| !lease.occupied)
                .unwrap_or(state.next[set]);
            let index = first + way;
            let local = self.base + index * PAGE;
            if state.leases[index].occupied {
                state.leases[index].ready = false;
                if effects.release(local).is_err() {
                    return None;
                }
                state.leases[index] = Lease::EMPTY;
            }
            // Ambiguous creation must never be followed by reuse of the VA.
            state.leases[index] = Lease {
                owner,
                generation,
                page,
                occupied: true,
                ready: false,
            };
            match effects.grant(pid, page, local) {
                Ok(()) => state.leases[index].ready = true,
                Err(GrantError::NotInstalled) => {
                    state.leases[index] = Lease::EMPTY;
                    return None;
                }
                Err(GrantError::Uncertain) => return None,
            }
            state.next[set] = (way + 1) % WAYS;
            index
        };
        Some(self.base + index * PAGE)
    }

    fn clear_owner(&self, owner: Word, effects: &mut impl LeaseEffects) {
        let mut state = self.state.borrow_mut();
        for (index, lease) in state.leases.iter_mut().enumerate() {
            if lease.occupied && lease.owner == owner {
                lease.ready = false;
                if effects.release(self.base + index * PAGE).is_ok() {
                    *lease = Lease::EMPTY;
                }
            }
        }
    }
}

pub struct ReadWindow(Cache);
impl ReadWindow {
    pub const fn new() -> Self {
        Self(Cache::new(READ_BASE))
    }

    /// A false result requires the original checked-copy fallback. This only
    /// reads from leases and writes the supplied owned buffer; no remote write
    /// alias, guest capability, syscall replay or external side effect is added.
    pub fn copy_into(
        &self,
        owner: Word,
        generation: Word,
        pid: Word,
        source: Word,
        destination: &mut [MaybeUninit<u8>],
        effects: &mut impl Effects,
    ) -> bool {
        if generation == 0
            || destination.len() > MAX_CONTIGUOUS_READ_BYTES
            || source.checked_add(destination.len()).is_none()
        {
            return false;
        }
        let mut state = self.0.state.borrow_mut();
        let mut copied = 0;
        while copied < destination.len() {
            let address = source + copied;
            let page = address & !(PAGE - 1);
            let offset = address - page;
            let bytes = (PAGE - offset).min(destination.len() - copied);
            let Some(local) = self
                .0
                .acquire(&mut state, owner, generation, pid, page, effects)
            else {
                return false;
            };
            effects.read(local, offset, &mut destination[copied..copied + bytes]);
            copied += bytes;
        }
        true
    }

    /// Failed cleanup remains tracked, unusable and retryable. Never overwrite
    /// a possibly live PTE or forget its physical ownership reference.
    pub fn clear_owner(&self, owner: Word, effects: &mut impl LeaseEffects) {
        self.0.clear_owner(owner, effects);
    }
}

pub struct WriteWindow(Cache);
#[derive(Clone, Copy, Default)]
struct WriteFragment {
    local: Word,
    page: Word,
    offset: Word,
    bytes: Word,
}
impl WriteWindow {
    pub const fn new() -> Self {
        Self(Cache::new(WRITE_BASE))
    }

    /// Prepare every page before storing a byte. On failure the caller can
    /// use the original checked request without repeating a partial write.
    /// This bounded consecutive range occupies distinct sets, so preparing a
    /// later page cannot evict an earlier one. Sources are owned native buffers.
    pub fn copy_from(
        &self,
        owner: Word,
        generation: Word,
        pid: Word,
        destination: Word,
        source: &[u8],
        effects: &mut impl WriteEffects,
    ) -> bool {
        if generation == 0 || source.len() > MAX_CONTIGUOUS_WRITE_BYTES
            || destination.checked_add(source.len()).is_none()
        {
            return false;
        }
        let mut state = self.0.state.borrow_mut();
        let mut fragments = [(0, 0, 0); CONTIGUOUS_WRITE_PAGES];
        let mut count = 0;
        let mut prepared = 0;
        while prepared < source.len() {
            let address = destination + prepared;
            let page = address & !(PAGE - 1);
            let offset = address - page;
            let bytes = (PAGE - offset).min(source.len() - prepared);
            let Some(local) = self
                .0
                .acquire(&mut state, owner, generation, pid, page, effects)
            else {
                return false;
            };
            fragments[count] = (local, offset, bytes);
            count += 1;
            prepared += bytes;
        }
        let mut copied = 0;
        for &(local, offset, bytes) in &fragments[..count] {
            effects.write(local, offset, &source[copied..copied + bytes]);
            copied += bytes;
        }
        true
    }

    /// Prepare a bounded owned vector before its first store. A later grant
    /// can evict an earlier hit in the same set: verify every prepared lease
    /// again before writing, otherwise leave the original checked batch intact.
    pub fn copy_from_many(
        &self,
        owner: Word,
        generation: Word,
        pid: Word,
        buffers: &[(Word, &[u8])],
        effects: &mut impl WriteEffects,
    ) -> bool {
        if generation == 0 || buffers.len() > MAX_WRITE_BUFFERS {
            return false;
        }
        let mut total = 0usize;
        for &(destination, source) in buffers {
            let Some(next) = total.checked_add(source.len()) else {
                return false;
            };
            if next > PAGE || destination.checked_add(source.len()).is_none() {
                return false;
            }
            total = next;
        }
        let mut state = self.0.state.borrow_mut();
        let mut fragments = [WriteFragment::default(); MAX_WRITE_BUFFERS * 2];
        let mut count = 0;
        for &(destination, source) in buffers {
            let mut prepared = 0;
            while prepared < source.len() {
                let address = destination + prepared;
                let page = address & !(PAGE - 1);
                let offset = address - page;
                let bytes = (PAGE - offset).min(source.len() - prepared);
                let Some(local) = self
                    .0
                    .acquire(&mut state, owner, generation, pid, page, effects)
                else {
                    return false;
                };
                fragments[count] = WriteFragment {
                    local,
                    page,
                    offset,
                    bytes,
                };
                count += 1;
                prepared += bytes;
            }
        }
        for fragment in &fragments[..count] {
            let lease = &state.leases[(fragment.local - self.0.base) / PAGE];
            if !lease.ready
                || lease.owner != owner
                || lease.generation != generation
                || lease.page != fragment.page
            {
                return false;
            }
        }
        let mut fragment_index = 0;
        for &(_, source) in buffers {
            let mut copied = 0;
            while copied < source.len() {
                let fragment = fragments[fragment_index];
                effects.write(
                    fragment.local,
                    fragment.offset,
                    &source[copied..copied + fragment.bytes],
                );
                copied += fragment.bytes;
                fragment_index += 1;
            }
        }
        true
    }

    pub fn clear_owner(&self, owner: Word, effects: &mut impl LeaseEffects) {
        self.0.clear_owner(owner, effects);
    }
}
