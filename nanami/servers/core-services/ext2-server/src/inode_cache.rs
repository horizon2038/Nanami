//! Exact bucket chains with bounded LRU retention for reused inode metadata.
use super::{CachedInode, Ext2Inode, EXT2_INODE_CACHE_ENTRIES};

#[cfg(test)]
static CHAIN_COMPARISONS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

pub(super) struct Cache {
    entries: [CachedInode; EXT2_INODE_CACHE_ENTRIES],
    heads: [u8; 256],
    older: [u8; EXT2_INODE_CACHE_ENTRIES],
    newer: [u8; EXT2_INODE_CACHE_ENTRIES],
    oldest: u8,
    newest: u8,
}

const NONE: u8 = u8::MAX;

impl Cache {
    pub(super) const fn new() -> Self {
        assert!(EXT2_INODE_CACHE_ENTRIES > 0 && EXT2_INODE_CACHE_ENTRIES <= u8::MAX as usize);
        Self { entries: [CachedInode::EMPTY; EXT2_INODE_CACHE_ENTRIES], heads: [0; 256],
            older: [NONE; EXT2_INODE_CACHE_ENTRIES], newer: [NONE; EXT2_INODE_CACHE_ENTRIES],
            oldest: NONE, newest: NONE }
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

    fn unlink_age(&mut self, index: usize) {
        let older = self.older[index];
        let newer = self.newer[index];
        if older == NONE { self.oldest = newer; }
        else { self.newer[usize::from(older)] = newer; }
        if newer == NONE { self.newest = older; }
        else { self.older[usize::from(newer)] = older; }
        self.older[index] = NONE;
        self.newer[index] = NONE;
    }

    fn touch(&mut self, index: usize) {
        if self.newest == index as u8 { return; }
        if self.oldest == index as u8 || self.older[index] != NONE || self.newer[index] != NONE {
            self.unlink_age(index);
        }
        self.older[index] = self.newest;
        if self.newest == NONE { self.oldest = index as u8; }
        else { self.newer[usize::from(self.newest)] = index as u8; }
        self.newest = index as u8;
    }

    pub(super) fn load(&mut self, number: u32) -> Option<Ext2Inode> {
        let index = self.find(number);
        #[cfg(not(test))]
        if option_env!("NANAMI_INODE_CACHE_TRACE") == Some("1") {
            nanami_runtime::println!("[ext2.inode-cache] op=load inode={} hit={}", number, index.is_some());
        }
        let index = index?;
        self.touch(index);
        Some(self.entries[index].inode)
    }

    pub(super) fn store(&mut self, number: u32, inode: Ext2Inode) {
        #[cfg(not(test))]
        if option_env!("NANAMI_INODE_CACHE_TRACE") == Some("1") {
            nanami_runtime::println!("[ext2.inode-cache] op=store inode={}", number);
        }
        if let Some(index) = self.find(number) {
            self.entries[index].inode = inode;
            self.touch(index);
            return;
        }
        let index = self.entries.iter().position(|entry| !entry.valid)
            .unwrap_or(usize::from(self.oldest));
        if self.entries[index].valid {
            self.unlink(index);
            self.unlink_age(index);
        }
        let bucket = Self::bucket(number);
        let next = self.heads[bucket];
        let link = (index + 1) as u8;
        self.entries[index] = CachedInode { valid: true, inode_no: number, inode,
            previous_in_bucket: 0, next_in_bucket: next };
        if next != 0 { self.entries[usize::from(next) - 1].previous_in_bucket = link; }
        self.heads[bucket] = link;
        self.touch(index);
    }

    pub(super) fn invalidate(&mut self, number: u32) {
        #[cfg(not(test))]
        if option_env!("NANAMI_INODE_CACHE_TRACE") == Some("1") {
            nanami_runtime::println!("[ext2.inode-cache] op=invalidate inode={}", number);
        }
        if let Some(index) = self.find(number) {
            self.unlink(index);
            self.unlink_age(index);
            self.entries[index].valid = false;
        }
    }
}
