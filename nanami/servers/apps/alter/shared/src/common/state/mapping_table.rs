//! Lazy, compact VMA metadata. Runtime owns the allocation; process snapshots
//! borrow it, and CLONE_VM aliases share its active length as well as its entries.
use super::{ProcessMapping, Word, ALTER_PROCESS_MAPPING_MAX};
use core::{
    alloc::Layout,
    cell::Cell,
    mem::MaybeUninit,
    ops::Deref,
    sync::atomic::{AtomicUsize, Ordering},
};

// Never reuse a cache generation, including when a table allocation is reused
// after exec. Exhaustion returns zero, which disables cached views safely.
static NEXT_GENERATION: AtomicUsize = AtomicUsize::new(1);
#[cfg(test)]
static ENTRY_MOVES: AtomicUsize = AtomicUsize::new(0);
fn next_generation() -> Word {
    NEXT_GENERATION
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
            next.checked_add(1)
        })
        .unwrap_or(0)
}

const MASK_BITS: usize = Word::BITS as usize;
const MASK_WORDS: usize = ALTER_PROCESS_MAPPING_MAX.div_ceil(MASK_BITS);

struct Storage {
    len: usize,
    generation: Cell<Word>,
    // Exact entry positions lacking each Linux R/W/X permission. Shared with
    // CLONE_VM aliases and maintained with the active prefix, never guest state.
    denied: [[Word; MASK_WORDS]; 3],
    // Only 0..len is initialized. Capacity is never exposed through Deref.
    entries: [MaybeUninit<ProcessMapping>; ALTER_PROCESS_MAPPING_MAX],
}

#[derive(Clone, Copy)]
pub struct MappingTable {
    address: Word,
}

impl MappingTable {
    pub const EMPTY: Self = Self { address: 0 };

    pub(super) fn access_allowed(&self, base: Word, end: Word, permission: Word) -> bool {
        if permission & !7 != 0 {
            return self.iter().all(|mapping| {
                mapping.base == 0 || mapping.base >= end
                    || mapping.base.saturating_add(mapping.size) <= base
                    || mapping.prot & permission == permission
            });
        }
        if self.address == 0 {
            return true;
        }
        let storage = unsafe { &*(self.address as *const Storage) };
        for word in 0..storage.len.div_ceil(MASK_BITS) {
            let mut candidates = 0;
            for bit in 0..3 {
                if permission & (1 << bit) != 0 {
                    candidates |= storage.denied[bit][word];
                }
            }
            while candidates != 0 {
                let index = word * MASK_BITS + candidates.trailing_zeros() as usize;
                candidates &= candidates - 1;
                // Denied bits refer only to entries in the initialized prefix.
                let mapping = unsafe { storage.entries[index].assume_init() };
                if mapping.base < end && mapping.base.saturating_add(mapping.size) > base {
                    return false;
                }
            }
        }
        true
    }

    pub fn same_storage(self, other: Self) -> bool {
        self.address != 0 && self.address == other.address
    }

    pub fn generation(&self) -> Word {
        if self.address == 0 {
            return 0;
        }
        unsafe { (*(self.address as *const Storage)).generation.get() }
    }

    /// Also used when private backing changes without changing the VMA list.
    pub fn invalidate_views(&self) {
        if self.address != 0 {
            unsafe {
                (*(self.address as *const Storage))
                    .generation
                    .set(next_generation());
            }
        }
    }

    pub(super) fn ensure(&mut self) -> bool {
        if self.address == 0 {
            let storage = unsafe { alloc::alloc::alloc(Layout::new::<Storage>()) }.cast::<Storage>();
            if storage.is_null() {
                return false;
            }
            // Initialize every live field before publishing the allocation.
            // MaybeUninit capacity needs no initialization until push/fork.
            unsafe {
                core::ptr::addr_of_mut!((*storage).len).write(0);
                core::ptr::addr_of_mut!((*storage).generation).write(Cell::new(next_generation()));
                core::ptr::addr_of_mut!((*storage).denied).write([[0; MASK_WORDS]; 3]);
            }
            self.address = storage as Word;
        }
        self.address != 0
    }

    pub(super) fn release(&mut self) {
        if self.address != 0 {
            unsafe {
                alloc::alloc::dealloc(self.address as *mut u8, Layout::new::<Storage>());
            }
            self.address = 0;
        }
    }

    pub(super) fn capacity(&self) -> usize {
        ALTER_PROCESS_MAPPING_MAX
    }

    // An allocated table may belong to CLONE_VM peers even when its active
    // prefix is empty. Only initialize fresh, independently owned fork state.
    pub(super) fn initialize_from(&mut self, source: Self) -> bool {
        if self.address != 0 {
            return false;
        }
        if source.is_empty() {
            return true;
        }
        if !self.ensure() {
            return false;
        }
        let entries = &*source;
        let storage = unsafe { &mut *(self.address as *mut Storage) };
        unsafe {
            // Fresh destination storage cannot alias the source. Publish len
            // only after every copied entry has been initialized.
            core::ptr::copy_nonoverlapping(
                entries.as_ptr(), storage.entries.as_mut_ptr().cast(), entries.len(),
            );
        }
        storage.len = entries.len();
        for (index, mapping) in entries.iter().enumerate() {
            for bit in 0..3 {
                if mapping.prot & (1 << bit) == 0 {
                    storage.denied[bit][index / MASK_BITS] |= 1 << (index % MASK_BITS);
                }
            }
        }
        true
    }

    pub(super) fn push(&mut self, mapping: ProcessMapping) -> bool {
        if mapping.base == 0 || !self.ensure() {
            return false;
        }
        let storage = unsafe { &mut *(self.address as *mut Storage) };
        if storage.len == ALTER_PROCESS_MAPPING_MAX {
            return false;
        }
        storage.entries[storage.len].write(mapping);
        for bit in 0..3 {
            if mapping.prot & (1 << bit) == 0 {
                storage.denied[bit][storage.len / MASK_BITS] |= 1 << (storage.len % MASK_BITS);
            }
        }
        storage.len += 1;
        storage.generation.set(next_generation());
        true
    }

    pub(super) fn remove(&mut self, index: usize) {
        let storage = unsafe { &mut *(self.address as *mut Storage) };
        assert!(index < storage.len);
        #[cfg(test)]
        ENTRY_MOVES.fetch_add(storage.len - index - 1, Ordering::Relaxed);
        // Preserve the relative order of overlapping legacy metadata. Only the
        // active prefix moves; empty capacity is never scanned or copied.
        storage.entries.copy_within(index + 1..storage.len, index);
        let first = index / MASK_BITS;
        let lower = (1usize << (index % MASK_BITS)) - 1;
        for mask in &mut storage.denied {
            for word in first..storage.len.div_ceil(MASK_BITS) {
                let keep = if word == first { lower } else { 0 };
                let carry = mask.get(word + 1).copied().unwrap_or(0) << (MASK_BITS - 1);
                mask[word] = (mask[word] & keep) | ((mask[word] >> 1) & !keep) | carry;
            }
        }
        storage.len -= 1;
        storage.generation.set(next_generation());
    }

    #[cfg(test)]
    pub fn take_entry_moves() -> usize {
        ENTRY_MOVES.swap(0, Ordering::Relaxed)
    }

    // Stable bulk removal for coalescing. The predicate does not dispatch or
    // borrow the table; each original entry is visited exactly once in order.
    pub(super) fn retain(&mut self, mut keep: impl FnMut(ProcessMapping) -> bool) {
        // Keep the common zero/one removal case on the existing fast path.
        // After the first tail move, compact all further removals together.
        let Some(first) = (0..self.len()).find(|&index| !keep(self[index])) else { return; };
        self.remove(first);
        let storage = unsafe { &mut *(self.address as *mut Storage) };
        let mut write = first;
        for read in first..storage.len {
            let mapping = unsafe { storage.entries[read].assume_init() };
            if keep(mapping) {
                if write != read {
                    storage.entries[write].write(mapping);
                    #[cfg(test)]
                    ENTRY_MOVES.fetch_add(1, Ordering::Relaxed);
                }
                write += 1;
            }
        }
        if write == storage.len { return; }
        storage.len = write;
        storage.denied = [[0; MASK_WORDS]; 3];
        for index in 0..write {
            let mapping = unsafe { storage.entries[index].assume_init() };
            for bit in 0..3 {
                if mapping.prot & (1 << bit) == 0 {
                    storage.denied[bit][index / MASK_BITS] |= 1 << (index % MASK_BITS);
                }
            }
        }
        storage.generation.set(next_generation());
    }

    pub(super) fn clear(&mut self) {
        if self.address != 0 {
            unsafe {
                let storage = &mut *(self.address as *mut Storage);
                storage.len = 0;
                storage.denied = [[0; MASK_WORDS]; 3];
            }
            self.invalidate_views();
        }
    }
}

impl Deref for MappingTable {
    type Target = [ProcessMapping];
    fn deref(&self) -> &Self::Target {
        if self.address == 0 {
            return &[];
        }
        let storage = unsafe { &*(self.address as *const Storage) };
        // All writers initialize entries before increasing the shared length;
        // remove preserves that prefix and clear makes it empty.
        unsafe { core::slice::from_raw_parts(storage.entries.as_ptr().cast(), storage.len) }
    }
}
