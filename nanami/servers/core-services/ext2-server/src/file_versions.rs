//! Bounded content identities, independent of timestamps and pathname aliases.
//! Eviction loses a hit, never freshness: every insertion gets a new token.
const ENTRIES: usize = 256;
const BUCKETS: usize = 128;
const WORD_BITS: usize = usize::BITS as usize;
const HOLE_WORDS: usize = ENTRIES.div_ceil(WORD_BITS);

#[derive(Clone, Copy)]
struct Entry {
    inode: u32,
    // Every published token is nonzero, so zero can represent an empty slot.
    version: u32,
    previous: u16,
    next: u16,
}

impl Entry {
    const EMPTY: Self = Self { inode: 0, version: 0, previous: 0, next: 0 };
}

// Reuse the space of the original Option<(inode, version)> for bucket links.
const _: () = assert!(core::mem::size_of::<Entry>()
    == core::mem::size_of::<Option<(u32, u32)>>());

pub(super) struct Versions {
    entries: [Entry; ENTRIES],
    next: usize,
    serial: u32,
    used: usize,
    heads: [u16; BUCKETS],
    // Only invalidated slots are holes. Unused tail slots must not bypass the
    // original replacement cursor when a previous hole changed that cursor.
    holes: [usize; HOLE_WORDS],
}

impl Versions {
    pub(super) const fn new() -> Self {
        assert!(ENTRIES <= u16::MAX as usize);
        Self {
            entries: [Entry::EMPTY; ENTRIES],
            next: 0,
            serial: 0,
            used: 0,
            heads: [0; BUCKETS],
            holes: [0; HOLE_WORDS],
        }
    }

    fn bucket(inode: u32) -> usize {
        let hash = inode.wrapping_mul(0x9e37_79b1);
        (hash ^ (hash >> 16)) as usize % BUCKETS
    }

    fn find(&self, inode: u32) -> Option<usize> {
        let mut link = self.heads[Self::bucket(inode)];
        while link != 0 {
            let index = usize::from(link) - 1;
            let entry = &self.entries[index];
            if entry.inode == inode { return Some(index); }
            link = entry.next;
        }
        None
    }

    fn unlink(&mut self, index: usize) {
        let entry = self.entries[index];
        if entry.previous == 0 {
            self.heads[Self::bucket(entry.inode)] = entry.next;
        } else {
            self.entries[usize::from(entry.previous) - 1].next = entry.next;
        }
        if entry.next != 0 {
            self.entries[usize::from(entry.next) - 1].previous = entry.previous;
        }
    }

    pub(super) fn get(&mut self, inode: u32) -> u32 {
        if let Some(index) = self.find(inode) {
            return self.entries[index].version;
        }
        // Zero disables caching. Never wrap and accidentally validate old data.
        let Some(version) = self.serial.checked_add(1) else {
            return 0;
        };
        self.serial = version;
        let index = self.holes.iter().enumerate()
            .find_map(|(word, &bits)| (bits != 0).then(|| word * WORD_BITS + bits.trailing_zeros() as usize))
            .unwrap_or(self.next);
        if self.entries[index].version != 0 { self.unlink(index); }
        self.holes[index / WORD_BITS] &= !(1usize << (index % WORD_BITS));
        let bucket = Self::bucket(inode);
        let next = self.heads[bucket];
        let link = (index + 1) as u16;
        self.entries[index] = Entry { inode, version, previous: 0, next };
        if next != 0 { self.entries[usize::from(next) - 1].previous = link; }
        self.heads[bucket] = link;
        self.used = self.used.max(index + 1);
        self.next = (index + 1) % ENTRIES;
        version
    }

    pub(super) fn invalidate(&mut self, inode: u32) {
        if let Some(index) = self.find(inode) {
            self.unlink(index);
            self.entries[index] = Entry::EMPTY;
            self.holes[index / WORD_BITS] |= 1usize << (index % WORD_BITS);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Original implementation, including first-hole priority and the cursor
    // after a hole is filled. Compare exact tokens and slots, not just freshness.
    struct Reference {
        entries: [Option<(u32, u32)>; ENTRIES], next: usize, serial: u32, used: usize,
    }
    impl Reference {
        fn new() -> Self { Self { entries: [None; ENTRIES], next: 0, serial: 0, used: 0 } }
        fn get(&mut self, inode: u32) -> u32 {
            if let Some((_, version)) = self.entries[..self.used].iter().flatten().find(|(key, _)| *key == inode) {
                return *version;
            }
            let Some(version) = self.serial.checked_add(1) else { return 0; };
            self.serial = version;
            let index = self.entries[..self.used].iter().position(Option::is_none).unwrap_or(self.next);
            self.entries[index] = Some((inode, version));
            self.used = self.used.max(index + 1);
            self.next = (index + 1) % ENTRIES;
            version
        }
        fn invalidate(&mut self, inode: u32) {
            for entry in &mut self.entries[..self.used] {
                if entry.is_some_and(|(key, _)| key == inode) { *entry = None; return; }
            }
        }
    }

    fn assert_same(actual: &Versions, expected: &Reference) {
        assert_eq!((actual.next, actual.used, actual.serial), (expected.next, expected.used, expected.serial));
        for (index, (entry, expected)) in actual.entries.iter().zip(&expected.entries).enumerate() {
            assert_eq!((entry.version != 0).then_some((entry.inode, entry.version)), *expected);
            if let Some((inode, _)) = expected { assert_eq!(actual.find(*inode), Some(index)); }
            let is_hole = actual.holes[index / WORD_BITS] & (1usize << (index % WORD_BITS)) != 0;
            assert_eq!(is_hole, index < actual.used && expected.is_none());
        }
    }

    #[test]
    fn indexed_versions_match_original_tokens_slots_and_holes_through_churn() {
        assert_eq!(core::mem::size_of::<Versions>(), core::mem::size_of::<Reference>()
            + core::mem::size_of::<[u16; BUCKETS]>() + core::mem::size_of::<[usize; HOLE_WORDS]>());
        let mut actual = Versions::new(); let mut expected = Reference::new();
        let mut rng = 0x57ca_193du32;
        for step in 0..20_000 {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let inode = if step % 37 == 0 { u32::MAX } else { (rng >> 16) % 641 };
            if rng & 3 == 0 { actual.invalidate(inode); expected.invalidate(inode); }
            else { assert_eq!(actual.get(inode), expected.get(inode)); }
            assert_same(&actual, &expected);
        }
    }

    #[test]
    fn holes_preserve_prefix_cursor_even_with_unused_tail_and_cross_word_boundaries() {
        let mut actual = Versions::new(); let mut expected = Reference::new();
        for inode in 0..5 { assert_eq!(actual.get(inode), expected.get(inode)); }
        actual.invalidate(1); expected.invalidate(1);
        for inode in [1000, 1001] { assert_eq!(actual.get(inode), expected.get(inode)); }
        assert_eq!(actual.find(1001), Some(2));
        assert_eq!(actual.used, 5); // Unused slot 5 must not displace original victim 2.
        for inode in 2000..2300 { assert_eq!(actual.get(inode), expected.get(inode)); }
        for index in [0, 63, 64, 127, 128, 191, 192, 255] {
            let inode = actual.entries[index].inode;
            actual.invalidate(inode); expected.invalidate(inode);
        }
        for inode in 3000..3020 {
            assert_eq!(actual.get(inode), expected.get(inode)); assert_same(&actual, &expected);
        }
    }

    #[test]
    fn full_collision_chains_keep_exact_versions_after_deletion_and_cursor_wrap() {
        let keys: std::vec::Vec<_> = (0u32..).filter(|inode| {
            let hash = inode.wrapping_mul(0x9e37_79b1);
            (hash ^ (hash >> 16)) as usize % BUCKETS == BUCKETS - 1
        }).take(512).collect();
        let mut actual = Versions::new(); let mut expected = Reference::new();
        for &inode in &keys[..ENTRIES] { assert_eq!(actual.get(inode), expected.get(inode)); }
        for index in [255, 128, 0, 127, 129, 254, 1, 255] {
            actual.invalidate(keys[index]); expected.invalidate(keys[index]); assert_same(&actual, &expected);
        }
        for step in 0..2048 {
            let inode = keys[(step * 173) % keys.len()];
            if step % 5 == 0 { actual.invalidate(inode); expected.invalidate(inode); }
            else { assert_eq!(actual.get(inode), expected.get(inode)); }
            assert_same(&actual, &expected);
        }
    }

    #[test]
    fn exhausted_serial_keeps_hits_and_leaves_failed_insertions_unchanged() {
        let mut actual = Versions::new(); let mut expected = Reference::new();
        actual.serial = u32::MAX - 2; expected.serial = u32::MAX - 2;
        for inode in [0, u32::MAX, 1, 0, u32::MAX] {
            assert_eq!(actual.get(inode), expected.get(inode)); assert_same(&actual, &expected);
        }
        actual.invalidate(0); expected.invalidate(0);
        for inode in [0, 1, u32::MAX, 2] {
            assert_eq!(actual.get(inode), expected.get(inode)); assert_same(&actual, &expected);
        }
    }

    #[test]
    fn mutation_eviction_and_inode_reuse_never_reuse_a_token() {
        let mut versions = Versions::new();
        let first = versions.get(1);
        assert_ne!(first, 0);
        assert_eq!(versions.get(1), first);
        versions.invalidate(1);
        let changed = versions.get(1);
        assert_ne!(changed, first);
        for inode in 2..=(ENTRIES as u32 + 2) {
            versions.get(inode);
        }
        assert_ne!(versions.get(1), changed);
    }

    #[test]
    fn exhaustion_disables_new_cache_identities_without_wrapping() {
        let mut versions = Versions::new();
        versions.serial = u32::MAX - 1;
        assert_eq!(versions.get(1), u32::MAX);
        assert_eq!(versions.get(2), 0);
        versions.invalidate(1);
        assert_eq!(versions.get(1), 0);
    }
}
