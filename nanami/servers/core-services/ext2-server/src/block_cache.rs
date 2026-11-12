//! Bounded, indexed block cache. Dirty entries are never evicted or replayed
//! after an uncertain device error; successful barriers make them evictable.
use alloc::{collections::BTreeMap, vec, vec::Vec};
use nanami_runtime::{RequestError, Word};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BlockKind {
    Data,
    Metadata,
}

#[derive(Clone, Copy)]
struct Entry {
    block: Option<usize>,
    dirty: Option<BlockKind>,
    referenced: bool,
    directory: Option<DirectorySummary>,
}

#[derive(Clone, Copy)]
struct DirectorySummary {
    bytes: usize,
    names: [u64; 4],
}

#[cfg(test)]
static NAME_HASH_BYTES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

fn name_bits(name: &[u8]) -> [usize; 2] {
    #[cfg(test)]
    NAME_HASH_BYTES.fetch_add(name.len(), core::sync::atomic::Ordering::Relaxed);
    let mut hash = 0xcbf29ce484222325u64;
    for byte in name { hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3); }
    [hash as usize & 255, (hash >> 32) as usize & 255]
}

/// One immutable lookup name. Hash only when a resident, extent-matching
/// summary needs it, then reuse the result for the rest of this request.
pub(super) struct DirectoryQuery<'a> {
    name: &'a [u8],
    bits: Option<[usize; 2]>,
}

impl<'a> DirectoryQuery<'a> {
    pub(super) fn new(name: &'a [u8]) -> Self {
        Self { name, bits: None }
    }

    fn bits(&mut self) -> [usize; 2] {
        if let Some(bits) = self.bits {
            return bits;
        }
        let bits = name_bits(self.name);
        self.bits = Some(bits);
        bits
    }
}

pub(super) struct BlockCache {
    index: BTreeMap<usize, usize>,
    hints: [u16; 256],
    #[cfg(test)]
    tree_lookup: bool,
    #[cfg(test)]
    lookup_calls: usize,
    #[cfg(test)]
    range_copies: usize,
    #[cfg(test)]
    store_copies: usize,
    #[cfg(test)]
    pub(super) directory_scans: usize,
    entries: Vec<Entry>,
    data: Vec<u8>,
    scratch_backup: Vec<u8>,
    block_size: usize,
    hand: usize,
    dirty_count: usize,
    error: Option<RequestError>,
}

const INLINE_READ_BLOCKS: usize = 64;
const MISSING_SLOT: usize = usize::MAX;

enum ReadSlots {
    Inline { slots: [usize; INLINE_READ_BLOCKS], count: usize },
    Large(Vec<usize>),
}

/// Request-local residency only. Device reads may fill separate scratch but
/// must not mutate this cache until finish_read has consumed the plan.
pub(super) struct ReadPlan {
    block: usize,
    slots: ReadSlots,
}

impl ReadPlan {
    fn slots(&self) -> &[usize] {
        match &self.slots {
            ReadSlots::Inline { slots, count } => &slots[..*count],
            ReadSlots::Large(slots) => slots,
        }
    }

    #[cfg(not(test))]
    pub(super) fn cached_blocks(&self) -> usize {
        self.slots().iter().filter(|&&slot| slot != MISSING_SLOT).count()
    }

    pub(super) fn missing_runs(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        let slots = self.slots();
        let mut next = 0;
        core::iter::from_fn(move || {
            while next < slots.len() && slots[next] != MISSING_SLOT { next += 1; }
            let first = next;
            while next < slots.len() && slots[next] == MISSING_SLOT { next += 1; }
            (first < next).then_some((first, next - first))
        })
    }
}

impl BlockCache {
    pub(super) fn capacity_bytes(&self) -> usize { self.data.len() }

    /// Device I/O borrows the cache immutably. Every resident block stays
    /// available for the final copy, including unflushed dirty data.
    pub(super) fn read_gaps(&self, blocks: core::ops::Range<usize>)
        -> impl Iterator<Item = core::ops::Range<usize>> + '_ {
        let mut next = blocks.start;
        let end = blocks.end;
        let mut cached = self.index.range(blocks);
        core::iter::from_fn(move || {
            for (&block, _) in cached.by_ref() {
                let start = next;
                next = block + 1;
                if start < block { return Some(start..block); }
            }
            if next < end {
                let start = next;
                next = end;
                return Some(start..end);
            }
            None
        })
    }

    /// Copy a streamed run once all its gaps have been read into scratch.
    /// The cache index is still current. Leave the cold stream out of it.
    pub(super) fn finish_streamed_read(
        &mut self, blocks: core::ops::Range<usize>, scratch: &[u8], offset: usize,
        target: &mut [u8],
    ) -> bool {
        let Some(capacity) = blocks.len().checked_mul(self.block_size) else { return false; };
        let Some(end) = offset.checked_add(target.len()) else { return false; };
        if offset >= self.block_size || end > capacity || scratch.len() < capacity {
            return false;
        }
        if target.is_empty() { return true; }
        let first = blocks.start;
        let mut copied = offset;
        let mut copies = 0;
        let mut cached_blocks = 0;
        let mut entries = self.index.range(blocks).peekable();
        while let Some((&block, &slot)) = entries.next() {
            let begin = (block - first) * self.block_size;
            if begin >= end { break; }
            // The immutable range walk keeps slot identities stable. Join
            // only adjacent file blocks held in adjacent cache slots, and
            // stop at the last block intersecting the requested output.
            let mut run = 1;
            while let Some(&(&next_block, &next_slot)) = entries.peek() {
                if next_block != block + run || next_slot != slot + run
                    || (next_block - first) * self.block_size >= end {
                    break;
                }
                entries.next();
                run += 1;
            }
            if copied < begin {
                target[copied - offset..begin - offset].copy_from_slice(&scratch[copied..begin]);
                copies += 1;
            }
            let start = copied.max(begin);
            let finish = (begin + run * self.block_size).min(end);
            let source = slot * self.block_size + (start - begin);
            target[start - offset..finish - offset]
                .copy_from_slice(&self.data[source..source + (finish - start)]);
            for entry in &mut self.entries[slot..slot + run] { entry.referenced = true; }
            copies += 1;
            cached_blocks += run;
            copied = finish;
        }
        if copied < end {
            target[copied - offset..].copy_from_slice(&scratch[copied..end]);
            copies += 1;
        }
        #[cfg(test)]
        { self.range_copies += copies; }
        #[cfg(not(test))]
        if cached_blocks > 1 && option_env!("NANAMI_STREAMED_CACHE_COPY_DIAG") == Some("1") {
            use core::sync::atomic::{AtomicUsize, Ordering};
            static REPORTS: AtomicUsize = AtomicUsize::new(0);
            if REPORTS.fetch_add(1, Ordering::Relaxed) < 16 {
                nanami_runtime::println!("[ext2.stream-copy] bytes={} cached-blocks={} copies={}",
                    target.len(), cached_blocks, copies);
            }
        }
        // Used only for opt-in diagnostics in a production build.
        #[cfg(test)]
        let _ = cached_blocks;
        true
    }

    pub(super) fn plan_read(&self, block: usize, count: usize) -> Option<ReadPlan> {
        let end = block.checked_add(count)?;
        let mut cached = self.index.range(block..end);
        let (&first, &slot) = cached.next()?;
        let mut storage = if count <= INLINE_READ_BLOCKS {
            ReadSlots::Inline { slots: [MISSING_SLOT; INLINE_READ_BLOCKS], count }
        } else {
            ReadSlots::Large(vec![MISSING_SLOT; count])
        };
        let slots = match &mut storage {
            ReadSlots::Inline { slots, count } => &mut slots[..*count],
            ReadSlots::Large(slots) => slots,
        };
        slots[first - block] = slot;
        for (&number, &slot) in cached { slots[number - block] = slot; }
        Some(ReadPlan { block, slots: storage })
    }

    /// All missing runs have completed in scratch. Validate every cached
    /// identity before the first output store, then copy each requested byte
    /// once. Cache fills follow output, so they cannot evict a needed source.
    pub(super) fn finish_read(&mut self, plan: ReadPlan, scratch: &[u8], offset: usize, target: &mut [u8]) -> bool {
        let slots = plan.slots();
        let Some(capacity) = slots.len().checked_mul(self.block_size) else { return false; };
        if offset >= self.block_size || scratch.len() < capacity
            || offset.checked_add(target.len()).is_none_or(|end| end > capacity)
            || slots.iter().enumerate().any(|(index, &slot)|
                slot != MISSING_SLOT && self.entries.get(slot).is_none_or(|entry| entry.block != Some(plan.block + index)))
        { return false; }
        let mut copied = 0;
        let mut index = 0;
        while copied < target.len() {
            let slot = slots[index];
            let mut end = index + 1;
            while end < slots.len() && if slot == MISSING_SLOT {
                slots[end] == MISSING_SLOT
            } else { slots[end] == slot + end - index } { end += 1; }
            let start = if index == 0 { offset } else { 0 };
            let bytes = ((end - index) * self.block_size - start).min(target.len() - copied);
            let source = if slot == MISSING_SLOT {
                &scratch[index * self.block_size + start..index * self.block_size + start + bytes]
            } else {
                for &slot in &slots[index..end] { self.entries[slot].referenced = true; }
                &self.data[slot * self.block_size + start..slot * self.block_size + start + bytes]
            };
            target[copied..copied + bytes].copy_from_slice(source);
            copied += bytes;
            index = end;
        }
        for (first, count) in plan.missing_runs() {
            self.store_read_misses(plan.block + first,
                &scratch[first * self.block_size..(first + count) * self.block_size]);
        }
        true
    }

    #[cfg(test)]
    pub(super) fn take_name_hash_bytes() -> usize {
        NAME_HASH_BYTES.swap(0, core::sync::atomic::Ordering::Relaxed)
    }

    pub(super) fn new(block_size: usize, budget: usize, transfer_bytes: usize) -> Self {
        assert!(block_size != 0 && budget >= transfer_bytes && transfer_bytes >= block_size);
        let capacity = budget / block_size;
        Self {
            index: BTreeMap::new(),
            hints: [u16::MAX; 256],
            #[cfg(test)]
            tree_lookup: false,
            #[cfg(test)]
            lookup_calls: 0,
            #[cfg(test)]
            range_copies: 0,
            #[cfg(test)]
            store_copies: 0,
            #[cfg(test)]
            directory_scans: 0,
            entries: vec![
                Entry {
                    block: None,
                    dirty: None,
                    referenced: false,
                    directory: None,
                };
                capacity
            ],
            data: vec![0; capacity * block_size],
            scratch_backup: vec![0; transfer_bytes],
            block_size,
            hand: 0,
            dirty_count: 0,
            error: None,
        }
    }

    pub(super) fn check_writable(&self) -> Result<(), RequestError> {
        self.error.map_or(Ok(()), Err)
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.dirty_count != 0
    }

    fn lookup(&mut self, block: usize) -> Option<usize> {
        #[cfg(test)]
        { self.lookup_calls += 1; }
        #[cfg(test)]
        if self.tree_lookup {
            return self.index.get(&block).copied();
        }
        let bucket = block % self.hints.len();
        let slot = self.hints[bucket] as usize;
        // Hints confer no residency: eviction/reuse must pass the current
        // full block identity check. The ordered index remains authoritative.
        if slot != u16::MAX as usize
            && self.entries.get(slot).is_some_and(|entry| entry.block == Some(block))
        {
            return Some(slot);
        }
        let slot = *self.index.get(&block)?;
        self.hints[bucket] = u16::try_from(slot).unwrap_or(u16::MAX);
        Some(slot)
    }

    #[cfg(test)]
    pub(super) fn take_lookup_calls(&mut self) -> usize {
        core::mem::take(&mut self.lookup_calls)
    }

    #[cfg(test)]
    pub(super) fn take_range_copies(&mut self) -> usize {
        core::mem::take(&mut self.range_copies)
    }

    #[cfg(test)]
    pub(super) fn take_store_copies(&mut self) -> usize {
        core::mem::take(&mut self.store_copies)
    }

    #[cfg(test)]
    pub(super) fn use_tree_lookup_for_test(&mut self) {
        self.tree_lookup = true;
    }

    #[cfg(test)]
    pub(super) fn assert_same_cache_for_test(&self, other: &Self) {
        assert_eq!(self.index, other.index);
        assert_eq!(self.data, other.data);
        assert_eq!(self.scratch_backup, other.scratch_backup);
        assert_eq!((self.hand, self.dirty_count), (other.hand, other.dirty_count));
        assert_eq!(self.error, other.error);
        for (a, b) in self.entries.iter().zip(&other.entries) {
            assert_eq!((a.block, a.dirty.map(|k| k as u8), a.referenced),
                (b.block, b.dirty.map(|k| k as u8), b.referenced));
        }
    }

    #[cfg(test)]
    pub(super) fn contains(&self, block: usize) -> bool {
        self.index.contains_key(&block)
    }

    /// Copy the resident prefix and locate the end of its following miss run.
    /// The caller supplies a validated whole-block range in separate scratch.
    /// Return block offsets (copied prefix, next resident block or range end).
    pub(super) fn load_prefix_and_gap(&mut self, block: usize, count: usize, target: &mut [u8]) -> (usize, usize) {
        debug_assert_eq!(target.len(), count * self.block_size);
        if count == 0 { return (0, 0); }
        if count == 1 { return (usize::from(self.load(block, target)), 1); }
        let end = block.checked_add(count).expect("validated block range");
        let mut entries = self.index.range(block..end).peekable();
        let mut copied = 0;
        while let Some((&number, &slot)) = entries.next() {
            if number != block + copied { return (copied, number - block); }
            let mut run = 1;
            while let Some(&(&next_block, &next_slot)) = entries.peek() {
                if next_block != number + run || next_slot != slot + run { break; }
                entries.next();
                run += 1;
            }
            let bytes = run * self.block_size;
            let source = slot * self.block_size;
            let output = copied * self.block_size;
            target[output..output + bytes].copy_from_slice(&self.data[source..source + bytes]);
            for entry in &mut self.entries[slot..slot + run] { entry.referenced = true; }
            copied += run;
        }
        (copied, count)
    }

    pub(super) fn load(&mut self, block: usize, target: &mut [u8]) -> bool {
        let Some(slot) = self.lookup(block) else {
            return false;
        };
        self.entries[slot].referenced = true;
        target.copy_from_slice(&self.data[slot * self.block_size..(slot + 1) * self.block_size]);
        true
    }

    /// Borrow one complete cached record. The borrow prevents cache mutation
    /// until the consumer has decoded it; no view survives an eviction/write.
    pub(super) fn load_record<const N: usize>(
        &mut self, block: usize, offset: usize,
    ) -> Option<&[u8; N]> {
        block.checked_add(1)?;
        let end = offset.checked_add(N).filter(|end| *end <= self.block_size)?;
        let slot = self.lookup(block)?;
        self.entries[slot].referenced = true;
        let base = slot * self.block_size;
        self.data[base + offset..base + end].try_into().ok()
    }

    /// Request-local immutable bytes; cache mutation cannot outlive this borrow.
    pub(super) fn load_slice(&mut self, block: usize, offset: usize, bytes: usize) -> Option<&[u8]> {
        block.checked_add(1)?;
        let end = offset.checked_add(bytes).filter(|end| *end <= self.block_size)?;
        let slot = self.lookup(block)?;
        self.entries[slot].referenced = true;
        let base = slot * self.block_size;
        Some(&self.data[base + offset..base + end])
    }

    pub(super) fn record_directory_name(names: &mut [u64; 4], name: &[u8]) {
        for bit in name_bits(name) { names[bit / 64] |= 1u64 << (bit % 64); }
    }

    /// A failed full scan can summarize this exact resident block version.
    /// Later absent names need no dirent scan. A possible match always uses
    /// the original parser, preserving duplicate-name and malformed-tail rules.
    pub(super) fn lookup_directory(
        &mut self, block: usize, bytes: usize, query: &mut DirectoryQuery<'_>,
        find: impl FnOnce(&[u8]) -> Result<Option<u32>, ()>,
        summarize: impl FnOnce(&[u8], &mut [u64; 4]) -> bool,
    ) -> Option<Result<Option<u32>, ()>> {
        block.checked_add(1)?;
        if bytes > self.block_size { return None; }
        let slot = self.lookup(block)?;
        let entry = &mut self.entries[slot];
        entry.referenced = true;
        if let Some(summary) = &entry.directory {
            if summary.bytes == bytes && query.bits().iter().any(|&bit|
                summary.names[bit / 64] & (1u64 << (bit % 64)) == 0)
            {
                return Some(Ok(None));
            }
        }
        let data = &self.data[slot * self.block_size..slot * self.block_size + bytes];
        #[cfg(test)]
        { self.directory_scans += 1; }
        let result = find(data);
        if result == Ok(None) && entry.directory.as_ref().is_none_or(|s| s.bytes != bytes) {
            let mut names = [0; 4];
            if summarize(data, &mut names) {
                entry.directory = Some(DirectorySummary { bytes, names });
            }
        }
        Some(result)
    }

    /// Copy a fully cached byte range without staging it through block scratch.
    /// A miss leaves the destination untouched, including a cached prefix.
    pub(super) fn load_range(&mut self, block: usize, offset: usize, target: &mut [u8]) -> bool {
        if offset >= self.block_size {
            return false;
        }
        if target.is_empty() {
            return true;
        }
        let Some(bytes) = offset.checked_add(target.len()) else {
            return false;
        };
        let count = bytes.div_ceil(self.block_size);
        let Some(end) = block.checked_add(count) else {
            return false;
        };
        if count == 1 {
            let Some(slot) = self.lookup(block) else { return false; };
            let start = slot * self.block_size + offset;
            #[cfg(test)]
            { self.range_copies += 1; }
            target.copy_from_slice(&self.data[start..start + target.len()]);
            self.entries[slot].referenced = true;
            return true;
        }
        // One request-local slot list covers a normal 64-KiB device transfer
        // even with 1-KiB ext2 blocks. The exclusive borrow keeps slots stable.
        // Larger ranges retain the unbounded two-pass path below.
        const RANGE_SLOTS: usize = 64;
        if count <= RANGE_SLOTS {
            let mut slots = [0usize; RANGE_SLOTS];
            let mut present = 0;
            for (&number, &slot) in self.index.range(block..end) {
                if number != block + present { return false; }
                slots[present] = slot;
                present += 1;
            }
            if present != count { return false; }
            let mut copied = 0;
            let mut index = 0;
            while index < count {
                let slot = slots[index];
                let mut end = index + 1;
                while end < count && slots[end] == slots[end - 1] + 1 {
                    end += 1;
                }
                let start = if index == 0 { offset } else { 0 };
                let len = ((end - index) * self.block_size - start).min(target.len() - copied);
                let source = slot * self.block_size + start;
                #[cfg(test)]
                { self.range_copies += 1; }
                target[copied..copied + len].copy_from_slice(&self.data[source..source + len]);
                for &slot in &slots[index..end] { self.entries[slot].referenced = true; }
                copied += len;
                index = end;
            }
            return true;
        }
        // Unique integer keys covering an interval of this length imply that
        // every block is present. Check before touching the caller's output.
        if self.index.range(block..end).count() != count {
            return false;
        }
        let mut copied = 0;
        let mut copies = 0;
        let mut entries = self.index.range(block..end).peekable();
        while let Some((&number, &slot)) = entries.next() {
            let mut run = 1;
            while let Some(&(&next_block, &next_slot)) = entries.peek() {
                if next_block != number + run || next_slot != slot + run { break; }
                entries.next();
                run += 1;
            }
            let start = if number == block { offset } else { 0 };
            let len = (run * self.block_size - start).min(target.len() - copied);
            let source = slot * self.block_size + start;
            target[copied..copied + len].copy_from_slice(&self.data[source..source + len]);
            for entry in &mut self.entries[slot..slot + run] { entry.referenced = true; }
            copied += len;
            copies += 1;
        }
        #[cfg(test)]
        { self.range_copies += copies; }
        #[cfg(not(test))]
        if option_env!("NANAMI_LARGE_CACHE_READ_DIAG") == Some("1") {
            use core::sync::atomic::{AtomicUsize, Ordering};
            static REPORTS: AtomicUsize = AtomicUsize::new(0);
            if REPORTS.fetch_add(1, Ordering::Relaxed) < 16 {
                nanami_runtime::println!("[ext2.cache-range] bytes={} blocks={} copies={}", target.len(), count, copies);
            }
        }
        true
    }

    pub(super) fn has_room_for(&self, block: usize, count: usize) -> bool {
        let end = block + count;
        // Keep the original empty-range behavior (including release-mode
        // wrapping inputs); production callers have already validated the run.
        if end <= block {
            return true;
        }
        let available = self.entries.len() - self.dirty_count;
        if count <= available {
            return true;
        }
        let needed = count - available;
        self.index.range(block..end)
            .filter(|(_, slot)| self.entries[**slot].dirty.is_some())
            .take(needed)
            .count() == needed
    }

    // CLOCK second-chance eviction. Reads can bypass a completely dirty cache;
    // writes reserve room (with writeback if necessary) before calling store.
    pub(super) fn store(&mut self, block: usize, source: &[u8], dirty: Option<BlockKind>) {
        let resident = self.lookup(block);
        self.store_resolved(block, source, dirty, resident);
    }

    /// Only for the missing run just returned by load_prefix_and_gap. The
    /// synchronous device read cannot mutate this cache, and earlier stores
    /// in that run insert distinct blocks. No pending write can exist here.
    pub(super) fn store_read_miss(&mut self, block: usize, source: &[u8]) {
        debug_assert!(!self.index.contains_key(&block));
        self.store_resolved(block, source, None, None);
    }

    /// A just-read missing run in separate device scratch. Reserve each slot
    /// with the same CLOCK operations as scalar fills, then copy consecutive
    /// source/slot extents together. Slot selection reads only metadata: no
    /// cache payload is consumed until the synchronous fill has returned.
    pub(super) fn store_read_misses(&mut self, block: usize, source: &[u8]) {
        debug_assert_eq!(source.len() % self.block_size, 0);
        let mut pending: Option<(usize, usize, usize)> = None;
        for index in 0..source.len() / self.block_size {
            let number = block + index;
            debug_assert!(!self.index.contains_key(&number));
            let slot = self.claim_store_slot(number, None, None);
            if let Some((first, start, count)) = pending {
                if slot == Some(first + count) {
                    pending = Some((first, start, count + 1));
                    continue;
                }
                // Flush before starting the next extent, including a wrap
                // that reused one of these slots under dirty-cache pressure.
                self.copy_store_extent(first, &source[start * self.block_size..(start + count) * self.block_size]);
            }
            pending = slot.map(|slot| (slot, index, 1));
        }
        if let Some((first, start, count)) = pending {
            self.copy_store_extent(first, &source[start * self.block_size..(start + count) * self.block_size]);
        }
    }

    fn store_resolved(&mut self, block: usize, source: &[u8], dirty: Option<BlockKind>, resident: Option<usize>) {
        if let Some(slot) = self.claim_store_slot(block, dirty, resident) {
            assert_eq!(source.len(), self.block_size);
            self.copy_store_extent(slot, source);
        }
    }

    fn copy_store_extent(&mut self, slot: usize, source: &[u8]) {
        #[cfg(test)]
        { self.store_copies += 1; }
        let start = slot * self.block_size;
        self.data[start..start + source.len()].copy_from_slice(source);
    }

    fn claim_store_slot(&mut self, block: usize, dirty: Option<BlockKind>, resident: Option<usize>) -> Option<usize> {
        let slot = if let Some(slot) = resident {
            // An uncached read must never replace a pending write.
            if dirty.is_none() && self.entries[slot].dirty.is_some() {
                return None;
            }
            slot
        } else {
            if self.dirty_count == self.entries.len() {
                assert!(dirty.is_none());
                return None;
            }
            let mut chosen = None;
            for _ in 0..self.entries.len() * 2 {
                let slot = self.hand;
                // The cache is nonempty and hand always names an entry.
                // Advance once and wrap only at the end, without division.
                let next = slot + 1;
                self.hand = if next == self.entries.len() { 0 } else { next };
                let entry = &mut self.entries[slot];
                if entry.dirty.is_some() {
                    continue;
                }
                if entry.referenced {
                    entry.referenced = false;
                } else {
                    chosen = Some(slot);
                    break;
                }
            }
            let Some(slot) = chosen else {
                assert!(dirty.is_none());
                return None;
            };
            if let Some(old) = self.entries[slot].block {
                self.index.remove(&old);
            }
            self.index.insert(block, slot);
            self.hints[block % self.hints.len()] = u16::try_from(slot).unwrap_or(u16::MAX);
            slot
        };
        if dirty.is_some() && self.entries[slot].dirty.is_none() {
            self.dirty_count += 1;
        }
        self.entries[slot] = Entry {
            block: Some(block),
            dirty,
            referenced: true,
            directory: None,
        };
        Some(slot)
    }

    pub(super) fn flush(&mut self, port: Word, scratch: &mut [u8]) -> Result<(), RequestError> {
        self.check_writable()?;
        if !self.is_dirty() {
            return Ok(());
        }
        // Pressure writeback may interrupt an allocation with staged metadata
        // or payload in the shared buffer. Preserve it even on failure.
        assert_eq!(self.scratch_backup.len(), scratch.len());
        // Every batch starts at scratch[0] and contains at most all dirty
        // blocks. The unused tail cannot be touched by either writeback pass.
        let bytes = self.dirty_count.min(scratch.len() / self.block_size) * self.block_size;
        self.scratch_backup[..bytes].copy_from_slice(&scratch[..bytes]);
        let result = self.flush_dirty(port, scratch);
        scratch[..bytes].copy_from_slice(&self.scratch_backup[..bytes]);
        match result {
            Ok(()) => {
                for entry in &mut self.entries {
                    entry.dirty = None;
                }
                self.dirty_count = 0;
            }
            Err(error) => self.error = Some(error),
        }
        result
    }

    fn flush_dirty(&self, port: Word, scratch: &mut [u8]) -> Result<(), RequestError> {
        let max_blocks = scratch.len() / self.block_size;
        // Persist data before publishing metadata. This is ordered writeback,
        // not a journal: interrupted metadata updates can still require fsck.
        for kind in [BlockKind::Data, BlockKind::Metadata] {
            let mut first = 0;
            let mut count = 0;
            let mut wrote = false;
            for (&block, &slot) in &self.index {
                if self.entries[slot].dirty != Some(kind) {
                    continue;
                }
                if count != 0 && (block != first + count || count == max_blocks) {
                    self.write_run(port, first, count)?;
                    count = 0;
                }
                if count == 0 {
                    first = block;
                }
                scratch[count * self.block_size..(count + 1) * self.block_size].copy_from_slice(
                    &self.data[slot * self.block_size..(slot + 1) * self.block_size],
                );
                count += 1;
                wrote = true;
            }
            if count != 0 {
                self.write_run(port, first, count)?;
            }
            if wrote {
                nanami_client::block::block_device_flush(port)?;
            }
        }
        Ok(())
    }

    fn write_run(&self, port: Word, block: usize, count: usize) -> Result<(), RequestError> {
        let written = nanami_client::block::block_device_write(port, block, count, 0)?;
        if written == count * self.block_size {
            Ok(())
        } else {
            Err(RequestError::Protocol)
        }
    }
}
