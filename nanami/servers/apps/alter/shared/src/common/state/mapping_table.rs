//! Lazy, compact VMA metadata. Runtime owns the allocation; process snapshots
//! borrow it, and CLONE_VM aliases share its active length as well as its entries.
use super::{ProcessMapping, Word, ALTER_PROCESS_MAPPING_MAX};
use core::{alloc::Layout, ops::Deref};

struct Storage {
    len: usize,
    entries: [ProcessMapping; ALTER_PROCESS_MAPPING_MAX],
}

#[derive(Clone, Copy)]
pub struct MappingTable {
    address: Word,
}

impl MappingTable {
    pub const EMPTY: Self = Self { address: 0 };

    pub fn same_storage(self, other: Self) -> bool {
        self.address != 0 && self.address == other.address
    }

    pub(super) fn ensure(&mut self) -> bool {
        if self.address == 0 {
            // Zero initializes the shared length and the word-only VMA entries.
            self.address = unsafe { alloc::alloc::alloc_zeroed(Layout::new::<Storage>()) } as Word;
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

    pub(super) fn push(&mut self, mapping: ProcessMapping) -> bool {
        if mapping.base == 0 || !self.ensure() {
            return false;
        }
        let storage = unsafe { &mut *(self.address as *mut Storage) };
        if storage.len == ALTER_PROCESS_MAPPING_MAX {
            return false;
        }
        storage.entries[storage.len] = mapping;
        storage.len += 1;
        true
    }

    pub(super) fn remove(&mut self, index: usize) {
        let storage = unsafe { &mut *(self.address as *mut Storage) };
        assert!(index < storage.len);
        // Preserve the relative order of overlapping legacy metadata. Only the
        // active prefix moves; empty capacity is never scanned or copied.
        storage.entries.copy_within(index + 1..storage.len, index);
        storage.len -= 1;
    }

    pub(super) fn clear(&mut self) {
        if self.address != 0 {
            unsafe {
                (*(self.address as *mut Storage)).len = 0;
            }
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
        &storage.entries[..storage.len]
    }
}
