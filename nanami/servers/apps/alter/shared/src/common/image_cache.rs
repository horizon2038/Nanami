//! File snapshots with optional parsed ELF metadata, keyed by open-file identity.
use crate::elf::ElfMetadata;
use nanami_runtime::Word;

pub const ENTRIES: usize = 48;
pub const MAX_RETAINED_BYTES: Word = 16 * 1024 * 1024;
// More small-file slots within the original byte budget; large images bypass it.
pub const MAX_IMAGE_BYTES: Word = 2 * 1024 * 1024;
const PAGE_BYTES: Word = 4096;
const KEY_BUCKETS: usize = 64;
const RANGE_BUCKETS: usize = 64;
const RANGE_SHIFT: u32 = 20;

#[cfg(test)]
std::thread_local! {
    static QUERY_COUNTS: core::cell::Cell<(usize, usize)> = const { core::cell::Cell::new((0, 0)) };
    static RETAINED_SLOT_VISITS: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

#[cfg(test)]
pub fn take_query_counts() -> (usize, usize) {
    QUERY_COUNTS.with(|counts| counts.replace((0, 0)))
}
// Dedicated sealed backing lets readonly private mappings share file pages.
// Keep an explicit optout for the ordinary in-place heap policy.
pub const IMMUTABLE_STORAGE: bool = match option_env!("NANAMI_IMMUTABLE_SNAPSHOTS") {
    Some(value) => value.len() == 1 && value.as_bytes()[0] == b'1',
    None => cfg!(target_arch = "x86_64"),
};

#[derive(Clone, Copy)]
pub struct CachedImage {
    pub identity: Word,
    pub address: Word,
    pub size: Word,
    pub capacity: Word,
    pub metadata: Option<ElfMetadata>,
    used: u64,
}

#[cfg(test)]
mod index_tests {
    use super::*;

    fn linear_range(cache: &ImageCache, address: Word, bytes: Word) -> bool {
        IMMUTABLE_STORAGE && bytes != 0 && cache.entries.iter().flatten().any(|image| {
            image.identity != 0 && address >= image.address
                && address.checked_add(bytes).zip(image.address.checked_add(image.capacity))
                    .is_some_and(|(end, limit)| end <= limit)
        })
    }

    #[test]
    fn range_buckets_match_linear_checks_through_overlap_replacement_and_invalidation() {
        let mut cache = ImageCache::new(); let mut rng = 0x8fd1_739bu32;
        let unit = 1usize << RANGE_SHIFT;
        let capacities = [0, 1, 4096, unit - 1, unit, unit + 1, 2 * unit, 63 * unit, 65 * unit, usize::MAX];
        for step in 0..5000 {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let slot = (rng as usize >> 4) % ENTRIES;
            let address = rng as usize % (70 * unit);
            let capacity = capacities[(rng as usize >> 12) % capacities.len()];
            let identity = match step % 3 { 0 => 0, 1 => 1, _ => (1 << 32) | slot };
            match step % 7 {
                0 => cache.invalidate(slot),
                1 => cache.remove(slot),
                _ => cache.insert(slot, identity, address, capacity.min(MAX_IMAGE_BYTES), capacity, None),
            }
            for query in [0, address.saturating_sub(1), address, address | (unit - 1), 64 * unit, usize::MAX - 1, usize::MAX] {
                for bytes in [0, 1, 4096, unit, usize::MAX] {
                    assert_eq!(cache.immutable_range(query, bytes), linear_range(&cache, query, bytes),
                        "step={step} address={query} bytes={bytes}");
                }
            }
        }
    }

    #[test]
    fn region_wrap_overflow_and_adjacent_owners_never_form_a_false_complete_range() {
        let unit = 1usize << RANGE_SHIFT;
        for (address, capacity) in [(0, 1), (unit - 1, 2), (63 * unit + 17, 2 * unit),
            (unit, 65 * unit), (usize::MAX - 4096, 4095), (usize::MAX - 4096, 4097), (0, usize::MAX)] {
            let mut cache = ImageCache::new();
            cache.insert(47, 1 << 32, address, 1, capacity, None);
            for query in [0, 1, unit - 1, unit, 2 * unit - 1, 63 * unit, 64 * unit, 65 * unit, usize::MAX - 1, usize::MAX] {
                for bytes in [0, 1, 2, unit, usize::MAX] {
                    assert_eq!(cache.immutable_range(query, bytes), linear_range(&cache, query, bytes));
                }
            }
        }
        let mut cache = ImageCache::new();
        cache.insert(0, 1 << 32, unit, 4096, 4096, None);
        cache.insert(1, 2 << 32, unit + 4096, 4096, 4096, None);
        assert!(!cache.immutable_range(unit, 8192));
        assert!(!cache.immutable_range(unit + 4095, 2));
        assert_eq!(cache.immutable_range(unit, 4096), IMMUTABLE_STORAGE);
        cache.insert(2, 3 << 32, unit - 1, 8194, 8194, None);
        assert_eq!(cache.immutable_range(unit, 8192), IMMUTABLE_STORAGE);
        cache.invalidate(2);
        assert!(!cache.immutable_range(unit, 8192));
        cache.remove(0);
        assert!(!cache.immutable_range(unit, 4096));
    }

    // Original two-pass eligibility/pressure/minimum selection.
    fn original_replacement(cache: &ImageCache, size: Word, protected: Option<Word>, limit: Word, identity: Word) -> Option<usize> {
        if size == 0 || size > limit { return None; }
        let retained: Word = cache.entries.iter().flatten().map(|image| image.capacity).sum();
        let superseded = |image: &CachedImage| identity >> 32 != 0 && image.identity >> 32 != 0
            && image.identity != identity && image.identity as u32 == identity as u32;
        let candidates = cache.entries.iter().enumerate().filter(|(index, entry)| {
            if entry.is_some_and(|image| Some(image.address) == protected) { return false; }
            retained - entry.map_or(0, |image| image.capacity) + cache.allocation_capacity(*index, size) <= MAX_RETAINED_BYTES
        });
        let pressure = candidates.clone().any(|(_, entry)| entry.as_ref().is_some_and(|e| superseded(e)));
        candidates.min_by_key(|(_, entry)| match entry {
            None => (0, 0),
            Some(image) => (if superseded(image) || (pressure && image.identity == 0) { 1 } else { 2 }, image.used),
        }).map(|(index, _)| index)
    }

    #[test]
    fn single_pass_replacement_matches_original_with_rounding_pressure_and_protection() {
        let mut cache = ImageCache::new(); let mut rng = 0x29b3_175du32;
        let capacities = [4096, 8192, 131072, 524288, MAX_IMAGE_BYTES];
        for step in 0..3000 {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let slot = (rng as usize >> 4) % ENTRIES;
            let identity = (((rng as Word >> 24) % 5) << 32) | ((rng as Word >> 16) % 11);
            let capacity = capacities[(rng as usize >> 12) % capacities.len()];
            match step % 7 {
                0 => cache.invalidate(slot),
                1 => cache.remove(slot),
                _ => cache.insert(slot, identity, 0x100000 + slot * 0x10000, capacity - 1, capacity, None),
            }
            let protected = cache.entry(slot).map(|e| e.address);
            for size in [0, 1, 4095, 4096, 5000, MAX_IMAGE_BYTES, MAX_IMAGE_BYTES + 1, MAX_RETAINED_BYTES, MAX_RETAINED_BYTES + 1] {
                for limit in [MAX_IMAGE_BYTES, MAX_RETAINED_BYTES] {
                    for identity in [0, 3 << 32 | 7, identity] {
                        for protected in [None, protected] {
                            assert_eq!(cache.replacement_with_limit(size, protected, limit, identity),
                                original_replacement(&cache, size, protected, limit, identity),
                                "step={step} size={size} limit={limit} identity={identity} protected={protected:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn equal_lru_ties_promote_failed_owners_only_with_an_eligible_generation_victim() {
        let mut cache = ImageCache::new();
        for slot in 0..ENTRIES {
            let identity = if slot == 47 { 1 << 32 | 7 } else { 1 << 32 | (100 + slot) };
            cache.insert(slot, identity, 0x100000 + slot * 0x10000, 4096, 4096, None);
        }
        cache.invalidate(5);
        for entry in cache.entries.iter_mut().flatten() { entry.used = 7; }
        let identity = 2 << 32 | 7;
        assert_eq!(cache.replacement(4096, None, identity), Some(5));
        assert_eq!(cache.replacement(4096, Some(0x100000 + 47 * 0x10000), identity), Some(0));
        assert_eq!(cache.replacement(4096, Some(0x100000 + 5 * 0x10000), identity), Some(47));
        assert_eq!(cache.replacement(4096, None, 0), Some(0));
        cache.remove(21);
        assert_eq!(cache.replacement(4096, None, identity), Some(21));
    }

    struct Linear { entries: [Option<CachedImage>; ENTRIES], clock: u64 }
    impl Linear {
        fn new() -> Self { Self { entries: [None; ENTRIES], clock: 0 } }
        fn tick(&mut self) -> u64 {
            if self.clock == u64::MAX {
                for entry in self.entries.iter_mut().flatten() { entry.used = 0; }
                self.clock = 0;
            }
            self.clock += 1; self.clock
        }
        fn insert(&mut self, slot: usize, identity: Word, size: Word) {
            let used = self.tick();
            self.entries[slot] = Some(CachedImage { identity, size, address: 0x100000 + slot * 0x10000,
                capacity: size.next_multiple_of(PAGE_BYTES), metadata: None, used });
        }
        fn get(&mut self, identity: Word, size: Word) -> Option<&CachedImage> {
            if identity >> 32 == 0 { return None; }
            let slot = self.entries.iter().position(|entry|
                entry.as_ref().is_some_and(|image| image.identity == identity && image.size == size))?;
            let used = self.tick();
            let entry = self.entries[slot].as_mut()?; entry.used = used; Some(entry)
        }
        fn invalidate(&mut self, slot: usize) {
            if let Some(entry) = self.entries[slot].as_mut() {
                entry.identity = 0; entry.size = 0; entry.metadata = None;
            }
        }
    }
    fn image(entry: Option<&CachedImage>) -> Option<(Word, Word, Word, Word, u64)> {
        entry.map(|e| (e.identity, e.address, e.size, e.capacity, e.used))
    }
    fn insert(actual: &mut ImageCache, linear: &mut Linear, slot: usize, identity: Word, size: Word) {
        actual.insert(slot, identity, 0x100000 + slot * 0x10000, size,
            size.next_multiple_of(PAGE_BYTES), None);
        linear.insert(slot, identity, size);
    }
    fn same(actual: &ImageCache, linear: &Linear) {
        assert_eq!(actual.clock, linear.clock);
        for slot in 0..ENTRIES { assert_eq!(image(actual.entry(slot)), image(linear.entries[slot].as_ref())); }
        assert_eq!(actual.retained_bytes(), linear.entries.iter().flatten().map(|e| e.capacity).sum());
    }
    #[test]
    fn indexed_keys_match_linear_hits_slots_ticks_and_retained_storage_through_churn() {
        RETAINED_SLOT_VISITS.with(|count| count.set(0));
        let mut actual = ImageCache::new(); let mut linear = Linear::new();
        assert_eq!(core::mem::size_of::<ImageCache>(), core::mem::size_of::<Linear>()
            + (KEY_BUCKETS + RANGE_BUCKETS) * 8 + core::mem::size_of::<Word>()
            + core::mem::size_of::<Option<Word>>());
        let mut rng = 0x87ad_119bu32;
        for step in 0..20_000 {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let slot = (rng as usize >> 4) % ENTRIES;
            let identity = (((rng as Word >> 24) % 5) << 32) | ((rng as Word >> 16) % 11);
            let size = ((rng as Word >> 20) % 4) * 4096;
            match rng & 7 {
                0 | 1 => insert(&mut actual, &mut linear, slot, identity, size),
                2 => { actual.invalidate(slot); linear.invalidate(slot); }
                3 => { actual.remove(slot); linear.entries[slot] = None; }
                _ => assert_eq!(image(actual.get(identity, size)), image(linear.get(identity, size))),
            }
            if step % 127 == 0 {
                for slot in 0..ENTRIES {
                    if let Some(entry) = linear.entries[slot] {
                        assert_eq!(image(actual.get(entry.identity, entry.size)), image(linear.get(entry.identity, entry.size)));
                    }
                }
            }
            same(&actual, &linear);
        }
        println!("retained-total slot-visits={}", RETAINED_SLOT_VISITS.with(|count| count.get()));
    }
    #[test]
    fn duplicate_keys_keep_first_slot_and_clock_rollover_preserves_lru() {
        let mut actual = ImageCache::new(); let mut linear = Linear::new();
        let key = 7 << 32 | 41;
        for slot in [47, 21, 0] { insert(&mut actual, &mut linear, slot, key, 4096); }
        assert_eq!(actual.get(key, 4096).unwrap().address, 0x100000);
        linear.get(key, 4096);
        actual.invalidate(0); linear.invalidate(0);
        assert_eq!(image(actual.get(key, 4096)), image(linear.get(key, 4096)));
        actual.remove(21); linear.entries[21] = None;
        assert_eq!(image(actual.get(key, 4096)), image(linear.get(key, 4096)));
        actual.clock = u64::MAX; linear.clock = u64::MAX;
        assert_eq!(image(actual.get(key, 0)), image(linear.get(key, 0))); same(&actual, &linear);
        assert_eq!(image(actual.get(key, 4096)), image(linear.get(key, 4096))); same(&actual, &linear);
        actual.clock = u64::MAX; linear.clock = u64::MAX;
        insert(&mut actual, &mut linear, 0, 0, 4096); same(&actual, &linear);
        assert_eq!(image(actual.get(0, 4096)), image(linear.get(0, 4096))); same(&actual, &linear);
    }
    #[test]
    fn full_bucket_collisions_cannot_hide_duplicates_or_publish_invalidated_entries() {
        let keys: std::vec::Vec<_> = (0usize..).map(|n| (1 << 32) | n)
            .filter(|&key| ImageCache::key_bucket(key, 4096) == KEY_BUCKETS - 1).take(96).collect();
        let mut actual = ImageCache::new(); let mut linear = Linear::new();
        for step in 0..512 {
            let slot = step * 37 % ENTRIES;
            insert(&mut actual, &mut linear, slot, keys[step % keys.len()], 4096);
            if step % 5 == 0 { actual.invalidate(slot); linear.invalidate(slot); }
            for &key in &keys { assert_eq!(image(actual.get(key, 4096)), image(linear.get(key, 4096))); }
            same(&actual, &linear);
        }
    }
}

pub struct ImageCache {
    // Identity of this Alter process, never a guest or a cached permission.
    // Cache entries may be evicted independently; the owning process stays
    // the same for this cache's lifetime. None also preserves query retries.
    pub(super) owner_pid: Option<Word>,
    entries: [Option<CachedImage>; ENTRIES],
    clock: u64,
    // Allocation ownership survives invalidation; only insert/remove change
    // this exact capacity sum. Like the release sum, arithmetic is modulo Word.
    retained_capacity: Word,
    // Bucket bits are slot numbers. Ascending bit iteration preserves the
    // original first-slot choice even if equal keys occur in multiple slots.
    keys: [u64; KEY_BUCKETS],
    // Coarse address regions only select candidates; each query still checks
    // the current identity and complete, checked allocation extent.
    ranges: [u64; RANGE_BUCKETS],
}

impl ImageCache {
    pub const fn new() -> Self {
        assert!(ENTRIES <= u64::BITS as usize);
        Self {
            owner_pid: None,
            entries: [None; ENTRIES],
            clock: 0,
            retained_capacity: 0,
            keys: [0; KEY_BUCKETS],
            ranges: [0; RANGE_BUCKETS],
        }
    }

    fn tick(&mut self) -> u64 {
        if self.clock == u64::MAX {
            for entry in self.entries.iter_mut().flatten() {
                entry.used = 0;
            }
            self.clock = 0;
        }
        self.clock += 1;
        self.clock
    }

    fn key_bucket(identity: Word, size: Word) -> usize {
        let mut hash = (identity as u64) ^ (size as u64).rotate_left(23);
        hash ^= hash >> 33;
        hash = hash.wrapping_mul(0xff51afd7ed558ccd);
        hash ^= hash >> 33;
        hash as usize & (KEY_BUCKETS - 1)
    }

    fn range_buckets(address: Word, capacity: Word) -> Option<(usize, usize)> {
        if !IMMUTABLE_STORAGE || capacity == 0 { return None; }
        let end = address.checked_add(capacity)?;
        let first = address >> RANGE_SHIFT;
        let last = (end - 1) >> RANGE_SHIFT;
        Some((first & (RANGE_BUCKETS - 1), (last - first + 1).min(RANGE_BUCKETS)))
    }

    fn detach_indexes(&mut self, index: usize) {
        if let Some(image) = self.entries[index].as_ref() {
            if image.identity >> 32 != 0 {
                let bucket = Self::key_bucket(image.identity, image.size);
                self.keys[bucket] &= !(1u64 << index);
            }
            if image.identity != 0 {
                if let Some((first, count)) = Self::range_buckets(image.address, image.capacity) {
                    for offset in 0..count {
                        self.ranges[(first + offset) & (RANGE_BUCKETS - 1)] &= !(1u64 << index);
                    }
                }
            }
        }
    }

    pub fn get(&mut self, identity: Word, size: Word) -> Option<&CachedImage> {
        #[cfg(test)]
        QUERY_COUNTS.with(|counts| { let (lookups, selections) = counts.get(); counts.set((lookups + 1, selections)); });
        if identity >> 32 == 0 {
            return None;
        }
        let mut candidates = self.keys[Self::key_bucket(identity, size)];
        let mut found = None;
        while candidates != 0 {
            let index = candidates.trailing_zeros() as usize;
            candidates &= candidates - 1;
            if self.entries[index].as_ref()
                .is_some_and(|image| image.identity == identity && image.size == size)
            {
                found = Some(index);
                break;
            }
        }
        let index = found?;
        let used = self.tick();
        let entry = self.entries[index].as_mut()?;
        entry.used = used;
        Some(entry)
    }

    pub fn retained_bytes(&self) -> Word {
        self.retained_capacity
    }

    pub fn immutable_range(&self, address: Word, bytes: Word) -> bool {
        if !IMMUTABLE_STORAGE || bytes == 0 { return false; }
        let Some(end) = address.checked_add(bytes) else { return false; };
        let mut candidates = self.ranges[(address >> RANGE_SHIFT) & (RANGE_BUCKETS - 1)];
        while candidates != 0 {
            let index = candidates.trailing_zeros() as usize;
            candidates &= candidates - 1;
            if self.entries[index].as_ref().is_some_and(|image| {
                image.identity != 0 && address >= image.address
                    && image.address.checked_add(image.capacity).is_some_and(|limit| end <= limit)
            }) { return true; }
        }
        false
    }

    pub fn allocation_capacity(&self, index: usize, size: Word) -> Word {
        if IMMUTABLE_STORAGE {
            // A published snapshot is retired, never refilled in place. Its
            // reclaimable RAM object needs no geometric heap-growth reserve.
            return size.next_multiple_of(PAGE_BYTES);
        }
        let requested = match self.entry(index) {
            Some(previous) if previous.capacity >= size => return previous.capacity,
            Some(_) => size.next_power_of_two(),
            None => size,
        };
        requested.next_multiple_of(PAGE_BYTES)
    }

    /// Admission is optional. Keep outstanding main ELF bytes intact while
    /// loading PT_INTERP; otherwise a budget-constrained victim could be MRU.
    pub fn replacement(&self, size: Word, protected_address: Option<Word>, identity: Word) -> Option<usize> {
        self.replacement_with_limit(size, protected_address, MAX_IMAGE_BYTES, identity)
    }

    /// Whole-file mmap already requests these bytes. Larger admission is useful
    /// only with immutable backing that can be aliased rather than copied again.
    pub fn replacement_mapping(&self, size: Word, readonly_alias: bool, identity: Word) -> Option<usize> {
        let limit = if IMMUTABLE_STORAGE && readonly_alias { MAX_RETAINED_BYTES } else { MAX_IMAGE_BYTES };
        self.replacement_with_limit(size, None, limit, identity)
    }

    fn replacement_with_limit(
        &self, size: Word, protected_address: Option<Word>, limit: Word, identity: Word,
    ) -> Option<usize> {
        #[cfg(test)]
        QUERY_COUNTS.with(|counts| { let (lookups, selections) = counts.get(); counts.set((lookups, selections + 1)); });
        if size == 0 || size > limit {
            return None;
        }
        let retained = self.retained_bytes();
        let superseded = |image: &CachedImage| {
            identity >> 32 != 0 && image.identity >> 32 != 0
                && image.identity != identity && image.identity as u32 == identity as u32
        };
        let mut oldest: Option<(u64, usize)> = None;
        let mut generation: Option<(u64, usize)> = None;
        let mut failed: Option<(u64, usize)> = None;
        for (index, entry) in self.entries.iter().enumerate() {
            let image = entry.as_ref();
            if image.is_some_and(|image| Some(image.address) == protected_address) { continue; }
            let previous = image.map_or(0, |image| image.capacity);
            if retained - previous + self.allocation_capacity(index, size) > MAX_RETAINED_BYTES { continue; }
            // An eligible empty slot beats every owned allocation. Iteration
            // order also makes this the original first empty slot on ties.
            let Some(image) = image else { return Some(index); };
            let candidate = (image.used, index);
            if oldest.is_none_or(|best| candidate < best) { oldest = Some(candidate); }
            if superseded(image) && generation.is_none_or(|best| candidate < best) {
                generation = Some(candidate);
            }
            if image.identity == 0 && failed.is_none_or(|best| candidate < best) {
                failed = Some(candidate);
            }
        }
        // Failed owners are promoted only when an eligible generation victim
        // exists. Keeping the slot in the tuple preserves first-slot LRU ties.
        match generation {
            Some(victim) => Some(failed.map_or(victim, |owner| victim.min(owner)).1),
            None => oldest.map(|(_, index)| index),
        }
    }

    pub fn entry(&self, index: usize) -> Option<&CachedImage> {
        self.entries[index].as_ref()
    }

    /// Retain ownership after a failed/partial release, but never serve a hit
    /// from a mapping whose publication or retirement has not completed.
    pub fn invalidate(&mut self, index: usize) {
        self.detach_indexes(index);
        if let Some(entry) = self.entries[index].as_mut() {
            entry.identity = 0;
            entry.size = 0;
            entry.metadata = None;
        }
    }

    /// Only after successful release of the recorded allocation.
    pub fn remove(&mut self, index: usize) {
        self.detach_indexes(index);
        self.retained_capacity = self.retained_capacity
            .wrapping_sub(self.entries[index].as_ref().map_or(0, |image| image.capacity));
        self.entries[index] = None;
    }

    pub fn insert(
        &mut self,
        index: usize,
        identity: Word,
        address: Word,
        size: Word,
        capacity: Word,
        metadata: Option<ElfMetadata>,
    ) {
        self.detach_indexes(index);
        self.retained_capacity = self.retained_capacity
            .wrapping_sub(self.entries[index].as_ref().map_or(0, |image| image.capacity))
            .wrapping_add(capacity);
        let used = self.tick();
        self.entries[index] = Some(CachedImage {
            identity,
            address,
            size,
            capacity,
            metadata,
            used,
        });
        if identity >> 32 != 0 {
            self.keys[Self::key_bucket(identity, size)] |= 1u64 << index;
        }
        if identity != 0 {
            if let Some((first, count)) = Self::range_buckets(address, capacity) {
                for offset in 0..count {
                    self.ranges[(first + offset) & (RANGE_BUCKETS - 1)] |= 1u64 << index;
                }
            }
        }
    }
}
