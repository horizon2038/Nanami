//! Bounded private RAM leases. Logical VM generations reject stale aliases;
//! Alpha's ordinary ownership references pin backing until confirmed release.
use super::Word;
use core::{cell::{Cell, RefCell}, mem::MaybeUninit};

pub const PAGE: Word = 4096;
pub const MAX_CONTIGUOUS_READ_BYTES: usize = 4 * PAGE;
/// Bounded optional fast path; larger vectors retain the checked-copy service.
pub const MAX_WRITE_BUFFERS: usize = 4;
/// Cover one ordinary file-read quantum within the existing lease budget.
pub const MAX_CONTIGUOUS_WRITE_BYTES: usize = 16 * PAGE;
const CONTIGUOUS_WRITE_PAGES: usize = MAX_CONTIGUOUS_WRITE_BYTES / PAGE + 1;
const READ_BASE: Word = 0x20_0000_0000;
const WRITE_BASE: Word = READ_BASE + SLOTS * PAGE;
const SETS: usize = 16;
const WAYS: usize = 4;
const SLOTS: usize = SETS * WAYS;
const _: () = assert!(CONTIGUOUS_WRITE_PAGES <= SLOTS);

#[cfg(not(test))]
static LEASE_DIAGNOSTICS: [core::sync::atomic::AtomicUsize; 8] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; 8];

#[cfg(not(test))]
pub(crate) fn report_lease_diagnostics(phase: &str, owner: Word) {
    if option_env!("NANAMI_LEASE_HASH_DIAG") == Some("1") {
        use core::sync::atomic::Ordering;
        for write in 0..2 {
            let rows = &LEASE_DIAGNOSTICS[write * 4..write * 4 + 4];
            nanami_runtime::println!("[lease.hash] phase={} owner={} write={} grants={} evictions={} same_page_owner={} clears={}",
                phase, owner, write, rows[0].load(Ordering::Relaxed), rows[1].load(Ordering::Relaxed),
                rows[2].load(Ordering::Relaxed), rows[3].load(Ordering::Relaxed));
        }
    }
}

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
    fn set(owner: Word, page: Word) -> usize {
        if option_env!("NANAMI_LEASE_OWNER_HASH") == Some("0") {
            return ((page / PAGE) ^ owner.rotate_right(3)) & (SETS - 1);
        }
        // Mix the whole VM owner: adjacent PIDs must not force their identical
        // stack VAs into one four-way set. Keep page bits separate so each
        // consecutive run of SETS pages still occupies distinct sets. The
        // generation is deliberately excluded for preserve_disjoint's leases.
        let mut hash = owner as u64;
        hash ^= hash >> 30;
        hash = hash.wrapping_mul(0xbf58476d1ce4e5b9);
        hash ^= hash >> 27;
        hash = hash.wrapping_mul(0x94d049bb133111eb);
        ((page / PAGE) ^ (hash ^ (hash >> 31)) as Word) & (SETS - 1)
    }

    #[cfg(not(test))]
    #[inline]
    fn record(&self, event: usize) {
        if option_env!("NANAMI_LEASE_HASH_DIAG") == Some("1") {
            LEASE_DIAGNOSTICS[usize::from(self.base == WRITE_BASE) * 4 + event]
                .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
    }

    fn cached(&self, state: &State, owner: Word, generation: Word, page: Word) -> Option<Word> {
        let set = Self::set(owner, page);
        let first = set * WAYS;
        state.leases[first..first + WAYS].iter().position(|lease| {
            lease.occupied && lease.ready && lease.owner == owner
                && lease.generation == generation && lease.page == page
        }).map(|way| self.base + (first + way) * PAGE)
    }
    fn has_owner(&self, owner: Word) -> bool {
        // Include stale and unacknowledged leases: they still need release.
        self.state.borrow().leases.iter().any(|lease| lease.occupied && lease.owner == owner)
    }
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
        let set = Self::set(owner, page);
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
                #[cfg(not(test))]
                if option_env!("NANAMI_LEASE_HASH_DIAG") == Some("1") {
                    self.record(1);
                    if state.leases[index].owner != owner && state.leases[index].page == page { self.record(2); }
                }
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
            #[cfg(not(test))]
            self.record(0);
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

    // Only a completed, range-confined mapping operation may carry these
    // exact Ready leases into its new metadata generation. The set excludes
    // generation so their existing local VAs remain usable without remapping.
    fn preserve_disjoint(&self, owner: Word, before: Word, after: Word, base: Word, bytes: Word) -> usize {
        if owner == 0 || before == 0 || after == 0 || before == after || bytes == 0 {
            return 0;
        }
        let Some(end) = base.checked_add(bytes) else { return 0; };
        let mut count = 0;
        for lease in &mut self.state.borrow_mut().leases {
            if lease.occupied && lease.ready && lease.owner == owner && lease.generation == before
                && lease.page.checked_add(PAGE).is_some_and(|limit| limit <= base || lease.page >= end)
            {
                lease.generation = after;
                count += 1;
            }
        }
        count
    }

    fn clear_owner(&self, owner: Word, effects: &mut impl LeaseEffects) {
        self.clear_selected(owner, None, effects);
    }

    fn clear_range(&self, owner: Word, base: Word, bytes: Word, effects: &mut impl LeaseEffects) {
        // Invalid/unknown extents retain the full invalidation policy.
        let range = base.checked_add(bytes).filter(|&end| base != 0 && base < end)
            .map(|end| (base, end));
        self.clear_selected(owner, range, effects);
    }

    fn clear_selected(&self, owner: Word, range: Option<(Word, Word)>, effects: &mut impl LeaseEffects) {
        let mut state = self.state.borrow_mut();
        for (index, lease) in state.leases.iter_mut().enumerate() {
            if lease.occupied && lease.owner == owner && range.is_none_or(|(base, end)| {
                lease.page.checked_add(PAGE).is_none_or(|stop| lease.page < end && stop > base)
            }) {
                lease.ready = false;
                #[cfg(not(test))]
                self.record(3);
                if effects.release(self.base + index * PAGE).is_ok() {
                    *lease = Lease::EMPTY;
                }
            }
        }
    }
}

pub struct ReadWindow(Cache);
impl ReadWindow {
    pub fn clear_range(&self, owner: Word, base: Word, bytes: Word, effects: &mut impl LeaseEffects) {
        self.0.clear_range(owner, base, bytes, effects);
    }
    pub fn preserve_disjoint(&self, owner: Word, before: Word, after: Word, base: Word, bytes: Word) -> usize {
        self.0.preserve_disjoint(owner, before, after, base, bytes)
    }
    pub fn has_owner(&self, owner: Word) -> bool {
        self.0.has_owner(owner)
    }
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

// Admission only: a cold large copy first keeps the checked transfer. Repeated
// identical requests may amortize the view setup. This tuple confers no access
// authority; every actual copy still requires current Ready lease identities.
pub struct WriteWindow(Cache, Cell<[Word; 4]>);
#[derive(Clone, Copy, Default)]
struct WriteFragment {
    local: Word,
    page: Word,
    offset: Word,
    bytes: Word,
}
impl WriteWindow {
    pub fn clear_range(&self, owner: Word, base: Word, bytes: Word, effects: &mut impl LeaseEffects) {
        self.0.clear_range(owner, base, bytes, effects);
    }
    pub fn preserve_disjoint(&self, owner: Word, before: Word, after: Word, base: Word, bytes: Word) -> usize {
        self.0.preserve_disjoint(owner, before, after, base, bytes)
    }
    pub fn has_owner(&self, owner: Word) -> bool {
        self.0.has_owner(owner)
    }
    pub const fn new() -> Self {
        Self(Cache::new(WRITE_BASE), Cell::new([0; 4]))
    }

    /// Prepare every page before storing a byte. On failure the caller can
    /// use the original checked request without repeating a partial write.
    /// Up to SETS pages occupy distinct sets. A longer unaligned range can
    /// wrap into an earlier set; revalidate its prepared identities before
    /// writing. Sources are owned native buffers.
    pub fn copy_from(
        &self,
        owner: Word,
        generation: Word,
        pid: Word,
        destination: Word,
        source: &[u8],
        effects: &mut impl WriteEffects,
    ) -> bool {
        self.copy_from_inner::<false>(owner, generation, pid, destination, source, effects)
    }

    /// Only an existing, current writable lease can authorize this copy.
    /// A miss must not grant, evict, release, or store before COW preparation.
    pub fn copy_from_cached(
        &self,
        owner: Word,
        generation: Word,
        pid: Word,
        destination: Word,
        source: &[u8],
        effects: &mut impl WriteEffects,
    ) -> bool {
        self.copy_from_inner::<true>(owner, generation, pid, destination, source, effects)
    }

    fn copy_from_inner<const CACHED: bool>(
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
        if !CACHED && source.len() > 4 * PAGE {
            let hint = [owner, generation, destination, source.len()];
            let first = destination & !(PAGE - 1);
            let pages = ((destination - first) + source.len()).div_ceil(PAGE);
            let ready = (0..pages).all(|index|
                self.0.cached(&state, owner, generation, first + index * PAGE).is_some());
            let previous = self.1.replace(hint);
            if !ready && previous != hint {
                // Sequential one-shot buffers must not pay16/17 grant RPCs
                // in place of one checked copy. No leases or bytes changed.
                return false;
            }
        }
        let mut fragments = [(0, 0, 0); CONTIGUOUS_WRITE_PAGES];
        let mut count = 0;
        let mut prepared = 0;
        while prepared < source.len() {
            let address = destination + prepared;
            let page = address & !(PAGE - 1);
            let offset = address - page;
            let bytes = (PAGE - offset).min(source.len() - prepared);
            let Some(local) = (if CACHED {
                self.0.cached(&state, owner, generation, page)
            } else {
                self.0.acquire(&mut state, owner, generation, pid, page, effects)
            })
            else {
                return false;
            };
            fragments[count] = (local, offset, bytes);
            count += 1;
            prepared += bytes;
        }
        if count > SETS {
            // A Ready hit does not move replacement's cursor. A later miss
            // in the same set can therefore evict that earlier hit even when
            // the range itself fits in the total cache. No output store may
            // use a local VA whose current full lease identity changed.
            let first_page = destination & !(PAGE - 1);
            for (index, &(local, _, _)) in fragments[..count].iter().enumerate() {
                let lease = &state.leases[(local - self.0.base) / PAGE];
                if !lease.occupied || !lease.ready || lease.owner != owner
                    || lease.generation != generation || lease.page != first_page + index * PAGE
                {
                    return false;
                }
            }
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
        self.copy_from_many_inner::<false>(owner, generation, pid, buffers, effects)
    }

    /// A vector hit is authoritative only when every fragment is already Ready.
    /// In particular, a later miss must not commit an earlier field.
    pub fn copy_from_many_cached(
        &self,
        owner: Word,
        generation: Word,
        pid: Word,
        buffers: &[(Word, &[u8])],
        effects: &mut impl WriteEffects,
    ) -> bool {
        self.copy_from_many_inner::<true>(owner, generation, pid, buffers, effects)
    }

    fn copy_from_many_inner<const CACHED: bool>(
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
                let Some(local) = (if CACHED {
                    self.0.cached(&state, owner, generation, page)
                } else {
                    self.0.acquire(&mut state, owner, generation, pid, page, effects)
                })
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
        if self.1.get()[0] == owner { self.1.set([0; 4]); }
        self.0.clear_owner(owner, effects);
    }
}
