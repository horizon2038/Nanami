//! Exact bucket chains over the existing inode FIFO. Links use entry padding.
use super::{CachedInode, Ext2Inode, EXT2_INODE_CACHE_ENTRIES};

#[cfg(test)]
static CHAIN_COMPARISONS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

pub(super) struct Cache {
    entries: [CachedInode; EXT2_INODE_CACHE_ENTRIES],
    next: usize,
    heads: [u8; 256],
}

impl Cache {
    pub(super) const fn new() -> Self {
        assert!(EXT2_INODE_CACHE_ENTRIES > 0 && EXT2_INODE_CACHE_ENTRIES <= u8::MAX as usize);
        Self { entries: [CachedInode::EMPTY; EXT2_INODE_CACHE_ENTRIES], next: 0, heads: [0; 256] }
    }

    fn bucket(number: u32) -> usize {
        let hash = number.wrapping_mul(0x9e37_79b1);
        ((hash ^ (hash >> 16)) & 255) as usize
    }

    fn find(&self, number: u32) -> Option<usize> {
        let mut link = self.heads[Self::bucket(number)];
        while link != 0 {
            #[cfg(test)]
            CHAIN_COMPARISONS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            let index = usize::from(link) - 1;
            let entry = &self.entries[index];
            if entry.inode_no == number {
                return Some(index);
            }
            link = entry.next_in_bucket;
        }
        None
    }

    #[cfg(test)]
    pub fn take_chain_comparisons(&self) -> usize {
        CHAIN_COMPARISONS.swap(0, core::sync::atomic::Ordering::Relaxed)
    }

    fn unlink(&mut self, index: usize) {
        let previous = self.entries[index].previous_in_bucket;
        let next = self.entries[index].next_in_bucket;
        if previous == 0 {
            self.heads[Self::bucket(self.entries[index].inode_no)] = next;
        } else {
            self.entries[usize::from(previous) - 1].next_in_bucket = next;
        }
        if next != 0 {
            self.entries[usize::from(next) - 1].previous_in_bucket = previous;
        }
        self.entries[index].previous_in_bucket = 0;
        self.entries[index].next_in_bucket = 0;
    }

    pub(super) fn load(&self, number: u32) -> Option<Ext2Inode> {
        self.find(number).map(|index| self.entries[index].inode)
    }

    pub(super) fn store(&mut self, number: u32, inode: Ext2Inode) {
        if let Some(index) = self.find(number) {
            self.entries[index].inode = inode;
            return;
        }
        let index = self.next;
        self.next = (self.next + 1) % self.entries.len();
        if self.entries[index].valid {
            self.unlink(index);
        }
        let bucket = Self::bucket(number);
        let next = self.heads[bucket];
        let link = (index + 1) as u8;
        self.entries[index] = CachedInode { valid: true, inode_no: number, inode,
            previous_in_bucket: 0, next_in_bucket: next };
        if next != 0 { self.entries[usize::from(next) - 1].previous_in_bucket = link; }
        self.heads[bucket] = link;
    }

    pub(super) fn invalidate(&mut self, number: u32) {
        if let Some(index) = self.find(number) {
            self.unlink(index);
            self.entries[index].valid = false;
        }
    }
}
