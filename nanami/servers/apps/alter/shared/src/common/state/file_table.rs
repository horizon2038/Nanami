//! Runtime owns these allocations. Copyable process snapshots borrow a table;
//! CLONE_FILES installs the same table and the last process releases it.
use super::{LinuxFile, Word, LINUX_FD_MAX};
use alloc::boxed::Box;
use core::ops::{Deref, DerefMut};

#[derive(Clone, Copy)]
pub struct FileTable {
    address: Word,
}

impl FileTable {
    pub const EMPTY: Self = Self { address: 0 };

    pub(super) fn new(files: [LinuxFile; LINUX_FD_MAX]) -> Self {
        Self {
            address: Box::into_raw(Box::new(files)) as Word,
        }
    }

    pub fn same_storage(self, other: Self) -> bool {
        self.address != 0 && self.address == other.address
    }

    pub(super) fn release(&mut self) {
        if self.address != 0 {
            drop(unsafe { Box::from_raw(self.address as *mut [LinuxFile; LINUX_FD_MAX]) });
            self.address = 0;
        }
    }
}

impl Deref for FileTable {
    type Target = [LinuxFile];
    fn deref(&self) -> &Self::Target {
        if self.address == 0 {
            return &[];
        }
        unsafe { &*(self.address as *const [LinuxFile; LINUX_FD_MAX]) }
    }
}

impl DerefMut for FileTable {
    fn deref_mut(&mut self) -> &mut Self::Target {
        if self.address == 0 {
            return &mut [];
        }
        unsafe { &mut *(self.address as *mut [LinuxFile; LINUX_FD_MAX]) }
    }
}
