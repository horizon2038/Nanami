//! Complete bucket index over the existing FIFO name cache.
use super::{EXT2_DENTRY_CACHE_ENTRIES, EXT2_NAME_LEN_MAX};

#[cfg(test)]
static HASH_BYTES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[derive(Clone, Copy)]
struct Entry {
    valid: bool,
    previous: u8,
    next: u8,
    parent: u32,
    inode: u32,
    len: u8,
    bucket: u8,
    name: [u8; EXT2_NAME_LEN_MAX],
}
impl Entry {
    const EMPTY: Self = Self {
        valid: false, previous: 0, next: 0, parent: 0, inode: 0, len: 0, bucket: 0,
        name: [0; EXT2_NAME_LEN_MAX],
    };
    fn matches(&self, parent: u32, name: &[u8]) -> bool {
        self.valid && self.parent == parent && usize::from(self.len) == name.len()
            && self.name[..usize::from(self.len)] == *name
    }
}

pub(super) struct Query<'a> {
    parent: u32,
    name: &'a [u8],
    bucket: usize,
}
impl<'a> Query<'a> {
    pub(super) fn new(parent: u32, name: &'a [u8]) -> Option<Self> {
        if name.is_empty() || name.len() > EXT2_NAME_LEN_MAX { return None; }
        Some(Self { parent, name, bucket: Cache::bucket(parent, name) })
    }
}

pub(super) struct Cache {
    entries: [Entry; EXT2_DENTRY_CACHE_ENTRIES],
    next: usize,
    // Zero ends a chain; other links are slot+1. Only valid entries are linked.
    heads: [u8; EXT2_DENTRY_CACHE_ENTRIES],
}
impl Cache {
    pub(super) const fn new() -> Self {
        assert!(EXT2_DENTRY_CACHE_ENTRIES > 0 && EXT2_DENTRY_CACHE_ENTRIES <= u8::MAX as usize);
        assert!(EXT2_NAME_LEN_MAX <= u8::MAX as usize);
        Self { entries: [Entry::EMPTY; EXT2_DENTRY_CACHE_ENTRIES], next: 0,
            heads: [0; EXT2_DENTRY_CACHE_ENTRIES] }
    }
    fn bucket(parent: u32, name: &[u8]) -> usize {
        #[cfg(test)]
        HASH_BYTES.fetch_add(name.len(), core::sync::atomic::Ordering::Relaxed);
        let mut hash = parent.wrapping_mul(0x9e3779b1);
        for byte in name { hash = (hash ^ u32::from(*byte)).wrapping_mul(16777619); }
        (hash ^ (hash >> 16)) as usize % EXT2_DENTRY_CACHE_ENTRIES
    }
    #[cfg(test)]
    pub fn take_hash_bytes(&self) -> usize {
        HASH_BYTES.swap(0, core::sync::atomic::Ordering::Relaxed)
    }
    fn find(&self, bucket: usize, parent: u32, name: &[u8]) -> Option<usize> {
        let mut link = self.heads[bucket];
        while link != 0 {
            let index = usize::from(link) - 1;
            if self.entries[index].matches(parent, name) { return Some(index); }
            link = self.entries[index].next;
        }
        None
    }
    fn unlink(&mut self, index: usize) {
        let previous = self.entries[index].previous;
        let next = self.entries[index].next;
        if previous == 0 {
            // Keys never change while linked. Retain their bucket beside the
            // bounded 8-bit name length instead of rehashing a stored name.
            self.heads[usize::from(self.entries[index].bucket)] = next;
        } else {
            self.entries[usize::from(previous) - 1].next = next;
        }
        if next != 0 {
            self.entries[usize::from(next) - 1].previous = previous;
        }
        self.entries[index].previous = 0;
        self.entries[index].next = 0;
    }
    pub(super) fn load(&mut self, parent: u32, name: &[u8]) -> Option<Option<u32>> {
        self.load_query(&Query::new(parent, name)?)
    }
    pub(super) fn load_query(&mut self, query: &Query<'_>) -> Option<Option<u32>> {
        let index = self.find(query.bucket, query.parent, query.name)?;
        Some((self.entries[index].inode != 0).then_some(self.entries[index].inode))
    }
    pub(super) fn store(&mut self, parent: u32, name: &[u8], inode: Option<u32>) {
        if let Some(query) = Query::new(parent, name) { self.store_query(&query, inode); }
    }
    pub(super) fn store_query(&mut self, query: &Query<'_>, inode: Option<u32>) {
        // Recheck current cache contents; the query retains only the hash of
        // its immutable name/parent, never a resident slot or cached result.
        let (bucket, parent, name) = (query.bucket, query.parent, query.name);
        if let Some(index) = self.find(bucket, parent, name) {
            self.entries[index].inode = inode.unwrap_or(0);
            return;
        }
        let index = self.next;
        self.next = (self.next + 1) % self.entries.len();
        if self.entries[index].valid { self.unlink(index); }
        let next = self.heads[bucket];
        let entry = &mut self.entries[index];
        entry.valid = true;
        entry.parent = parent;
        entry.inode = inode.unwrap_or(0);
        entry.len = name.len() as u8;
        entry.bucket = bucket as u8;
        entry.name[..name.len()].copy_from_slice(name);
        entry.previous = 0;
        entry.next = next;
        let link = (index + 1) as u8;
        if next != 0 { self.entries[usize::from(next) - 1].previous = link; }
        self.heads[bucket] = link;
    }
    pub(super) fn invalidate(&mut self, parent: u32, name: &[u8]) {
        if name.is_empty() || name.len() > EXT2_NAME_LEN_MAX { return; }
        if let Some(index) = self.find(Self::bucket(parent, name), parent, name) {
            self.unlink(index);
            self.entries[index].valid = false;
        }
    }
    pub(super) fn invalidate_inode(&mut self, inode: u32) {
        for index in 0..self.entries.len() {
            let entry = &self.entries[index];
            if entry.valid && (entry.parent == inode || entry.inode == inode) {
                // Doubly linked buckets avoid a second search per removal,
                // even when every matching name collides in the same bucket.
                self.unlink(index);
                self.entries[index].valid = false;
            }
        }
    }
}
