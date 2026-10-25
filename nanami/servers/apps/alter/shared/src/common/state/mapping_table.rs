//! Lazy VMA metadata. Runtime owns the allocation; process snapshots only borrow it.
use super::{ProcessMapping, Word, ALTER_PROCESS_MAPPING_MAX};
use core::{
    alloc::Layout,
    ops::{Deref, DerefMut},
};

#[derive(Clone, Copy)]
pub struct MappingTable {
    address: Word,
}

impl MappingTable {
    pub const EMPTY: Self = Self { address: 0 };

    pub(super) fn ensure(&mut self) -> bool {
        if self.address == 0 {
            // ProcessMapping consists only of words; zero is its EMPTY value.
            self.address = unsafe {
                alloc::alloc::alloc_zeroed(
                    Layout::new::<[ProcessMapping; ALTER_PROCESS_MAPPING_MAX]>(),
                )
            } as Word;
        }
        self.address != 0
    }

    pub(super) fn release(&mut self) {
        if self.address != 0 {
            unsafe {
                alloc::alloc::dealloc(
                    self.address as *mut u8,
                    Layout::new::<[ProcessMapping; ALTER_PROCESS_MAPPING_MAX]>(),
                );
            }
            self.address = 0;
        }
    }
}

impl Deref for MappingTable {
    type Target = [ProcessMapping];
    fn deref(&self) -> &Self::Target {
        if self.address == 0 {
            return &[];
        }
        unsafe {
            core::slice::from_raw_parts(
                self.address as *const ProcessMapping,
                ALTER_PROCESS_MAPPING_MAX,
            )
        }
    }
}

impl DerefMut for MappingTable {
    fn deref_mut(&mut self) -> &mut Self::Target {
        if self.address == 0 {
            return &mut [];
        }
        unsafe {
            core::slice::from_raw_parts_mut(
                self.address as *mut ProcessMapping,
                ALTER_PROCESS_MAPPING_MAX,
            )
        }
    }
}
