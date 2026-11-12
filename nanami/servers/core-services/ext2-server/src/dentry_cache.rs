//! Complete bucket index with bounded LRU retention for reused names.
use super::{EXT2_DENTRY_CACHE_ENTRIES, EXT2_NAME_LEN_MAX};

// Short names need no 255-byte inline reservation. Keep the old maximum of
// 128 long names independently, with full byte-for-byte identity checks.
const INLINE: usize = 32;
const LONG_NAMES: usize = 128;
// Zero keeps empty Entry arrays zero-initialized; long indices are slot+1.
const NO_LONG: u8 = 0;

#[cfg(test)]
static HASH_BYTES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[derive(Clone, Copy)]
struct Entry {
    valid: bool,
    previous: u16,
    next: u16,
    parent: u32,
    inode: u32,
    len: u8,
    bucket: u16,
    long: u8,
    name: [u8; INLINE],
}
impl Entry {
    const EMPTY: Self = Self {
        valid: false, previous: 0, next: 0, parent: 0, inode: 0, len: 0, bucket: 0,
        long: NO_LONG,
        name: [0; INLINE],
    };
    fn matches_prefix(&self, parent: u32, name: &[u8]) -> bool {
        self.valid && self.parent == parent && usize::from(self.len) == name.len()
            && self.name[..name.len().min(INLINE)] == name[..name.len().min(INLINE)]
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
    // Zero ends a chain; other links are slot+1. Only valid entries are linked.
    heads: [u16; EXT2_DENTRY_CACHE_ENTRIES],
    // Invalid entries reuse older[] as a free list, avoiding a whole-cache
    // empty-slot search on every insertion. Valid entries are linked by age.
    older: [u16; EXT2_DENTRY_CACHE_ENTRIES],
    newer: [u16; EXT2_DENTRY_CACHE_ENTRIES],
    oldest: u16,
    newest: u16,
    free: u16,
    suffixes: [[u8; EXT2_NAME_LEN_MAX - INLINE]; LONG_NAMES],
    long_owners: [u16; LONG_NAMES],
}
const NONE: u16 = u16::MAX;

#[cfg(not(test))]
fn trace(op: &str, query: &Query<'_>, result: i64) {
    if option_env!("NANAMI_DENTRY_CACHE_TRACE") != Some("1") { return; }
    // Exact bytes, including non-UTF8 names, in one bounded diagnostic record.
    let mut hex = [0u8; 2 * EXT2_NAME_LEN_MAX];
    for (index, &byte) in query.name.iter().enumerate() {
        hex[index * 2] = b"0123456789abcdef"[usize::from(byte >> 4)];
        hex[index * 2 + 1] = b"0123456789abcdef"[usize::from(byte & 15)];
    }
    nanami_runtime::println!("[ext2.dentry] op={} parent={} name={} result={}",
        op, query.parent, core::str::from_utf8(&hex[..query.name.len() * 2]).unwrap(), result);
}
impl Cache {
    pub(super) const fn new() -> Self {
        assert!(EXT2_DENTRY_CACHE_ENTRIES > 0 && EXT2_DENTRY_CACHE_ENTRIES <= u16::MAX as usize);
        assert!(EXT2_NAME_LEN_MAX > INLINE && EXT2_NAME_LEN_MAX <= u8::MAX as usize);
        let mut older = [NONE; EXT2_DENTRY_CACHE_ENTRIES];
        let mut index = 0;
        while index + 1 < EXT2_DENTRY_CACHE_ENTRIES {
            older[index] = (index + 1) as u16;
            index += 1;
        }
        Self { entries: [Entry::EMPTY; EXT2_DENTRY_CACHE_ENTRIES],
            heads: [0; EXT2_DENTRY_CACHE_ENTRIES],
            older, newer: [NONE; EXT2_DENTRY_CACHE_ENTRIES],
            oldest: NONE, newest: NONE, free: 0,
            suffixes: [[0; EXT2_NAME_LEN_MAX - INLINE]; LONG_NAMES], long_owners: [NONE; LONG_NAMES] }
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
            let entry = &self.entries[index];
            if entry.matches_prefix(parent, name) && (name.len() <= INLINE
                || self.suffixes[usize::from(entry.long - 1)][..name.len() - INLINE] == name[INLINE..])
            { return Some(index); }
            link = self.entries[index].next;
        }
        None
    }
    fn unlink(&mut self, index: usize) {
        let previous = self.entries[index].previous;
        let next = self.entries[index].next;
        if previous == 0 {
            // Keys never change while linked. Retain their bucket beside the
            // bounded name length instead of rehashing a stored name.
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
        let index = self.find(query.bucket, query.parent, query.name);
        #[cfg(not(test))]
        trace("load", query, index.map_or(-1, |index| i64::from(self.entries[index].inode)));
        let index = index?;
        self.touch(index);
        Some((self.entries[index].inode != 0).then_some(self.entries[index].inode))
    }
    pub(super) fn store(&mut self, parent: u32, name: &[u8], inode: Option<u32>) {
        if let Some(query) = Query::new(parent, name) { self.store_query(&query, inode); }
    }
    pub(super) fn store_query(&mut self, query: &Query<'_>, inode: Option<u32>) {
        #[cfg(not(test))]
        trace("store", query, i64::from(inode.unwrap_or(0)));
        // Recheck current cache contents; the query retains only the hash of
        // its immutable name/parent, never a resident slot or cached result.
        let (bucket, parent, name) = (query.bucket, query.parent, query.name);
        if let Some(index) = self.find(bucket, parent, name) {
            self.entries[index].inode = inode.unwrap_or(0);
            self.touch(index);
            return;
        }
        let long = if name.len() > INLINE {
            let available = self.long_owners.iter().position(|&owner| owner == NONE);
            let slot = if let Some(slot) = available { slot } else {
                // Keep short-name churn from consuming the previous 128-name
                // long-name capacity. Evict the least recent long entry only.
                let mut victim = self.oldest;
                while self.entries[usize::from(victim)].long == NO_LONG {
                    victim = self.newer[usize::from(victim)];
                }
                let slot = usize::from(self.entries[usize::from(victim)].long - 1);
                self.remove(usize::from(victim));
                slot
            };
            (slot + 1) as u8
        } else { NO_LONG };
        if self.free == NONE { self.remove(usize::from(self.oldest)); }
        let index = usize::from(self.free);
        self.free = self.older[index];
        self.older[index] = NONE;
        self.newer[index] = NONE;
        if long != NO_LONG {
            let slot = usize::from(long - 1);
            self.long_owners[slot] = index as u16;
            self.suffixes[slot][..name.len() - INLINE].copy_from_slice(&name[INLINE..]);
        }
        let next = self.heads[bucket];
        let entry = &mut self.entries[index];
        entry.valid = true;
        entry.parent = parent;
        entry.inode = inode.unwrap_or(0);
        entry.len = name.len() as u8;
        entry.bucket = bucket as u16;
        entry.long = long;
        entry.name[..name.len().min(INLINE)].copy_from_slice(&name[..name.len().min(INLINE)]);
        entry.previous = 0;
        entry.next = next;
        let link = (index + 1) as u16;
        if next != 0 { self.entries[usize::from(next) - 1].previous = link; }
        self.heads[bucket] = link;
        self.touch(index);
    }
    pub(super) fn invalidate(&mut self, parent: u32, name: &[u8]) {
        let Some(query) = Query::new(parent, name) else { return; };
        #[cfg(not(test))]
        trace("invalidate", &query, 0);
        if let Some(index) = self.find(query.bucket, parent, name) {
            self.remove(index);
        }
    }
    pub(super) fn invalidate_inode(&mut self, inode: u32) {
        #[cfg(not(test))]
        if option_env!("NANAMI_DENTRY_CACHE_TRACE") == Some("1") {
            nanami_runtime::println!("[ext2.dentry] op=invalidate-inode inode={}", inode);
        }
        for index in 0..self.entries.len() {
            let entry = &self.entries[index];
            if entry.valid && (entry.parent == inode || entry.inode == inode) {
                // Doubly linked buckets avoid a second search per removal,
                // even when every matching name collides in the same bucket.
                self.remove(index);
            }
        }
    }

    fn remove(&mut self, index: usize) {
        self.unlink(index);
        self.unlink_age(index);
        let long = self.entries[index].long;
        if long != NO_LONG { self.long_owners[usize::from(long - 1)] = NONE; }
        self.entries[index].long = NO_LONG;
        self.entries[index].valid = false;
        self.older[index] = self.free;
        self.free = index as u16;
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
        if self.newest == index as u16 { return; }
        if self.oldest == index as u16 || self.older[index] != NONE || self.newer[index] != NONE {
            self.unlink_age(index);
        }
        self.older[index] = self.newest;
        if self.newest == NONE { self.oldest = index as u16; }
        else { self.newer[usize::from(self.newest)] = index as u16; }
        self.newest = index as u16;
    }
}
