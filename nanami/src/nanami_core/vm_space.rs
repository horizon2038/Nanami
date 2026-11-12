use crate::nanami_utils::avl::AvlTree;
use crate::nanami_utils::static_avl::StaticAvlTree;
use alloc::collections::{BTreeMap, BTreeSet};
use nun::CapabilityDescriptor;

pub const FRAME_WORD_PAGES: usize = 64;
pub const FRAME_WORD_BYTES: usize = FRAME_WORD_PAGES * 4096;

#[cfg(test)]
static FRAME_SCALAR_INSERTIONS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static FRAME_POINT_RECORDS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static PERMISSION_POINT_QUERIES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static IMMUTABLE_PERMISSION_WORDS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static FRAME_POINT_QUERIES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static SHARED_POINT_QUERIES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static SNAPSHOT_SHARED_WORD_QUERIES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

// Bootstrap mappings are recorded before the alpha heap is available, so this
// tracker intentionally remains fixed-size and allocation-free.
const BOOTSTRAP_VM_FRAME_MAPS: usize = 1 << 14;
const BOOTSTRAP_VM_PT_MAPS: usize = 256;
// Alpha alternates between several temporary mapping windows. Remember their
// completed leaf-table regions without another kernel depth query. This is a
// metadata hint, not additional mapped memory or a limit on address coverage.
const BOOTSTRAP_PT_REGIONS: usize = 8;

// Both supported architectures use 4-KiB pages and 512 entries per leaf table.
// Frame unmapping does not remove page tables. Readiness hints are local to
// their tracker and are discarded when that address space is replaced.
const PAGE_TABLE_REGION_SHIFT: u32 = 12 + 9;
const NO_PAGE_TABLE_REGION: usize = usize::MAX;

/// Exact logical permissions. Default7 needs no bits; the three planes record
/// revoked R/W/X bits for 64 aligned pages. Sparse and exceptional keys/values
/// retain the full usize without rounding addresses or truncating permissions.
struct PagePermissions {
    pages: BTreeMap<usize, [u64; 3]>,
    exceptional: BTreeMap<usize, usize>,
}

impl PagePermissions {
    const fn new() -> Self {
        Self { pages: BTreeMap::new(), exceptional: BTreeMap::new() }
    }

    fn get(&self, va: usize) -> Option<usize> {
        if va & 4095 == 0 {
            if let Some(planes) = self.pages.get(&(va >> 18)) {
                let bit = (va >> 12) & 63;
                let revoked = ((planes[0] >> bit) & 1)
                    | (((planes[1] >> bit) & 1) << 1) | (((planes[2] >> bit) & 1) << 2);
                if revoked != 0 { return Some(7 ^ revoked as usize); }
            }
        }
        self.exceptional.get(&va).copied()
    }

    fn remove(&mut self, va: usize) {
        self.exceptional.remove(&va);
        if va & 4095 != 0 { return; }
        if let Some(planes) = self.pages.get_mut(&(va >> 18)) {
            let bit = !(1u64 << ((va >> 12) & 63));
            for plane in planes.iter_mut() { *plane &= bit; }
            if planes.iter().all(|plane| *plane == 0) { self.pages.remove(&(va >> 18)); }
        }
    }

    fn set(&mut self, va: usize, rights: usize) {
        if rights == 7 { self.remove(va); }
        else if va & 4095 != 0 || rights > 7 {
            if let Some(value) = self.exceptional.get_mut(&va) { *value = rights; return; }
            self.remove(va);
            self.exceptional.insert(va, rights);
        } else {
            if let Some(planes) = self.pages.get_mut(&(va >> 18)) {
                self.exceptional.remove(&va);
                let bit = 1u64 << ((va >> 12) & 63);
                for (index, plane) in planes.iter_mut().enumerate() {
                    if (7 ^ rights) & (1 << index) != 0 { *plane |= bit; }
                    else { *plane &= !bit; }
                }
                return;
            }
            use alloc::collections::btree_map::Entry;
            match self.exceptional.entry(va) {
                Entry::Occupied(mut entry) => { entry.insert(rights); return; }
                Entry::Vacant(entry) => { entry.insert(rights); }
            }
            // Preserve isolated-page footprint. Only insertion of a new key
            // can promote a word; overwriting a scalar or stored plane never
            // promotes. Keep promoted words until empty so removal allocates
            // nothing, even when a dense word becomes sparse again.
            let base = va & !((1 << 18) - 1);
            let mut selected = 0u64;
            let mut planes = [0u64; 3];
            for (&key, &value) in self.exceptional.range(base..=(base | ((1 << 18) - 1))) {
                if key & 4095 != 0 || value >= 7 { continue; }
                let bit = 1u64 << ((key >> 12) & 63);
                selected |= bit;
                for (index, plane) in planes.iter_mut().enumerate() {
                    if (7 ^ value) & (1 << index) != 0 { *plane |= bit; }
                }
            }
            if selected.count_ones() < 4 { return; }
            // Install the complete destination before removing scalar keys.
            // No fallible operation or event dispatch follows publication.
            self.pages.insert(va >> 18, planes);
            while selected != 0 {
                let bit = selected.trailing_zeros() as usize;
                selected &= selected - 1;
                self.exceptional.remove(&(base | (bit << 12)));
            }
        }
    }

    fn mask(word: usize, start: usize, end: usize) -> u64 {
        let low = if word == start >> 18 { ((start & ((1 << 18) - 1)) + 4095) >> 12 } else { 0 };
        let high = if word == (end - 1) >> 18 { (((end - 1) & ((1 << 18) - 1)) >> 12) + 1 } else { 64 };
        if low < high { (u64::MAX << low) & (u64::MAX >> (64 - high)) } else { 0 }
    }

    fn allows_aligned(&self, start: usize, end: usize, permission: usize) -> bool {
        debug_assert!(start < end && matches!(permission, 1 | 2));
        let plane = usize::from(permission == 2);
        self.pages.range((start >> 18)..=((end - 1) >> 18))
            .all(|(&word, planes)| planes[plane] & Self::mask(word, start, end) == 0)
            && self.exceptional.range(start..end)
                .all(|(&va, &rights)| va & 4095 != 0 || rights & permission != 0)
    }

    fn aligned_range_is(&self, start: usize, end: usize, rights: usize) -> bool {
        debug_assert!(start < end && rights <= 7);
        let first = start >> 18;
        let last = (end - 1) >> 18;
        let mut words = self.pages.range(first..=last).peekable();
        let mut exact = self.exceptional.range(start..end)
            .filter(|(va, _)| **va & 4095 == 0).peekable();
        let mut next = first;
        loop {
            let word = match (words.peek().map(|(key, _)| **key), exact.peek().map(|(va, _)| **va >> 18)) {
                (Some(a), Some(b)) => a.min(b),
                (Some(a), None) => a,
                (None, Some(b)) => b,
                (None, None) => break,
            };
            if rights != 7 && word != next { return false; }
            let mut planes = if words.peek().is_some_and(|(key, _)| **key == word) {
                *words.next().unwrap().1
            } else { [0u64; 3] };
            while exact.peek().is_some_and(|(va, _)| **va >> 18 == word) {
                let (&va, &value) = exact.next().unwrap();
                if value > 7 { return false; }
                let bit = 1u64 << ((va >> 12) & 63);
                for (index, plane) in planes.iter_mut().enumerate() {
                    if (7 ^ value) & (1 << index) != 0 { *plane |= bit; }
                }
            }
            let mask = Self::mask(word, start, end);
            for (index, &plane) in planes.iter().enumerate() {
                let expected = if (7 ^ rights) & (1 << index) != 0 { mask } else { 0 };
                if plane & mask != expected { return false; }
            }
            next = word + 1;
        }
        rights == 7 || next == last + 1
    }

    /// Every aligned page has logical rights0 or1. Inspect current revoked
    /// W/X planes once per word; sparse and exceptional entries remain exact.
    fn aligned_range_readonly_or_none(&self, start: usize, end: usize) -> bool {
        debug_assert!(start < end && (start | end) & 4095 == 0);
        let first = start >> 18;
        let last = (end - 1) >> 18;
        let mut words = self.pages.range(first..=last).peekable();
        let mut exact = self.exceptional.range(start..end)
            .filter(|(va, _)| **va & 4095 == 0).peekable();
        let mut next = first;
        loop {
            let word = match (words.peek().map(|(key, _)| **key), exact.peek().map(|(va, _)| **va >> 18)) {
                (Some(a), Some(b)) => a.min(b),
                (Some(a), None) => a,
                (None, Some(b)) => b,
                (None, None) => break,
            };
            // A missing word retains default7, which is not an immutable source.
            if word != next { return false; }
            #[cfg(test)]
            IMMUTABLE_PERMISSION_WORDS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            let mut revoked = if words.peek().is_some_and(|(key, _)| **key == word) {
                let planes = words.next().unwrap().1;
                planes[1] & planes[2]
            } else { 0 };
            while exact.peek().is_some_and(|(va, _)| **va >> 18 == word) {
                let (&va, &rights) = exact.next().unwrap();
                if rights > 1 { return false; }
                revoked |= 1u64 << ((va >> 12) & 63);
            }
            let mask = Self::mask(word, start, end);
            if revoked & mask != mask { return false; }
            next = word + 1;
        }
        next == last + 1
    }

    fn range(&self, start: usize, end: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
        debug_assert!(start < end);
        let mut words = self.pages.range((start >> 18)..=((end - 1) >> 18));
        let mut pending = 0u64;
        let mut planes = [0u64; 3];
        let mut base = 0usize;
        let aligned = core::iter::from_fn(move || loop {
            if pending != 0 {
                let bit = pending.trailing_zeros() as usize;
                pending &= pending - 1;
                let revoked = ((planes[0] >> bit) & 1)
                    | (((planes[1] >> bit) & 1) << 1) | (((planes[2] >> bit) & 1) << 2);
                return Some((base | (bit << 12), 7 ^ revoked as usize));
            }
            let (&word, &value) = words.next()?;
            planes = value;
            base = word << 18;
            pending = (planes[0] | planes[1] | planes[2]) & Self::mask(word, start, end);
        });
        let mut aligned = aligned.peekable();
        let mut exceptional = self.exceptional.range(start..end)
            .map(|(&va, &rights)| (va, rights)).peekable();
        core::iter::from_fn(move || match (aligned.peek().copied(), exceptional.peek().copied()) {
            (Some(a), Some(b)) if a.0 < b.0 => aligned.next(),
            (_, Some(_)) => exceptional.next(),
            _ => aligned.next(),
        })
    }
}

pub trait VmTracker {
    /// Large leaves are restricted to permanent bootstrap mappings. Ordinary
    /// process trackers reject them before any PTE can be installed.
    fn record_bootstrap_frame(&mut self, address: usize, frame: CapabilityDescriptor, leaf_bits: usize)
        -> Result<(), ()>
    {
        if leaf_bits == 12 { self.record_frame(address, frame) } else { Err(()) }
    }
    fn record_frame(
        &mut self,
        virtual_address: usize,
        frame_descriptor: CapabilityDescriptor,
    ) -> Result<(), ()>;
    fn record_page_table(
        &mut self,
        virtual_address: usize,
        page_table_slot_index: usize,
    ) -> Result<(), ()>;
    fn find_frame(&self, virtual_address: usize) -> Option<CapabilityDescriptor>;
    fn find_page_table_slot(&self, virtual_address: usize) -> Option<usize>;
    fn forget_frame(&mut self, virtual_address: usize) -> Option<CapabilityDescriptor>;
    fn page_tables_ready(&self, virtual_address: usize) -> bool;
}

#[derive(Clone, Copy)]
pub struct BootstrapVmSpace {
    frame_by_va: StaticAvlTree<BOOTSTRAP_VM_FRAME_MAPS>,
    page_table_by_va: StaticAvlTree<BOOTSTRAP_VM_PT_MAPS>,
    page_table_regions: [usize; BOOTSTRAP_PT_REGIONS],
    next_page_table_region: usize,
}

impl BootstrapVmSpace {
    pub const fn new() -> Self {
        Self {
            frame_by_va: StaticAvlTree::new(),
            page_table_by_va: StaticAvlTree::new(),
            page_table_regions: [NO_PAGE_TABLE_REGION; BOOTSTRAP_PT_REGIONS],
            next_page_table_region: 0,
        }
    }
}

impl VmTracker for BootstrapVmSpace {
    fn record_bootstrap_frame(&mut self, address: usize, frame: CapabilityDescriptor, leaf_bits: usize)
        -> Result<(), ()>
    {
        match leaf_bits {
            12 => self.record_frame(address, frame),
            21 if address & ((1 << 21) - 1) == 0 => {
                // A PD leaf is not proof that a 4K leaf table exists.
                self.frame_by_va.insert(address, frame)
            }
            _ => Err(()),
        }
    }
    fn record_frame(
        &mut self,
        virtual_address: usize,
        frame_descriptor: CapabilityDescriptor,
    ) -> Result<(), ()> {
        self.frame_by_va.insert(virtual_address, frame_descriptor)?;
        let region = virtual_address >> PAGE_TABLE_REGION_SHIFT;
        if !self.page_table_regions.contains(&region) {
            self.page_table_regions[self.next_page_table_region] = region;
            self.next_page_table_region = (self.next_page_table_region + 1) % BOOTSTRAP_PT_REGIONS;
        }
        Ok(())
    }

    fn record_page_table(
        &mut self,
        virtual_address: usize,
        page_table_slot_index: usize,
    ) -> Result<(), ()> {
        self.page_table_by_va
            .insert(virtual_address, page_table_slot_index)
    }

    fn find_frame(&self, virtual_address: usize) -> Option<CapabilityDescriptor> {
        self.frame_by_va.find(virtual_address)
    }

    fn find_page_table_slot(&self, virtual_address: usize) -> Option<usize> {
        self.page_table_by_va.find(virtual_address)
    }

    fn forget_frame(&mut self, virtual_address: usize) -> Option<CapabilityDescriptor> {
        self.frame_by_va.remove(virtual_address)
    }

    fn page_tables_ready(&self, virtual_address: usize) -> bool {
        self.page_table_regions
            .contains(&(virtual_address >> PAGE_TABLE_REGION_SHIFT))
    }
}

// Ordinary VMs have a heap. Pack several exact VA/descriptor entries into
// each tree node instead of allocating a separate Box for every page.
// Bootstrap and the general AVL utility keep their original representations.
struct DenseFrameWord {
    present: u64,
    descriptors: alloc::vec::Vec<CapabilityDescriptor>,
}

struct FrameIndex {
    dense: BTreeMap<usize, DenseFrameWord>,
    sparse: BTreeMap<usize, CapabilityDescriptor>,
}

impl FrameIndex {
    const fn new() -> Self { Self { dense: BTreeMap::new(), sparse: BTreeMap::new() } }

    fn insert(&mut self, va: usize, frame: CapabilityDescriptor) -> Result<(), ()> {
        if va & 4095 == 0 {
            if let Some(word) = self.dense.get_mut(&(va >> 18)) {
                let bit = (va >> 12) & 63;
                word.descriptors[bit] = frame;
                word.present |= 1u64 << bit;
                return Ok(());
            }
        }
        #[cfg(test)]
        FRAME_SCALAR_INSERTIONS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if self.sparse.insert(va, frame).is_some() || va & 4095 != 0 { return Ok(()); }
        let base = va & !((1 << 18) - 1);
        let end = base | ((1 << 18) - 1);
        // Sparse words keep scalar entries. Promote only a half-full aligned
        // word; an optional allocation failure leaves every scalar intact.
        if self.sparse.len() < 32
            || self.sparse.range(base..=end).filter(|(key, _)| **key & 4095 == 0).take(32).count() < 32
        {
            return Ok(());
        }
        self.pack_word(va);
        Ok(())
    }

    fn pack_word(&mut self, va: usize) {
        if self.dense.contains_key(&(va >> 18)) { return; }
        let base = va & !((1 << 18) - 1);
        let end = base | ((1 << 18) - 1);
        let mut descriptors = alloc::vec::Vec::new();
        if descriptors.try_reserve_exact(64).is_err() { return; }
        descriptors.resize(64, 0);
        let mut present = 0u64;
        for (&key, &value) in self.sparse.range(base..=end) {
            if key & 4095 != 0 { continue; }
            let bit = (key >> 12) & 63;
            descriptors[bit] = value;
            present |= 1u64 << bit;
        }
        self.dense.insert(va >> 18, DenseFrameWord { present, descriptors });
        // Publish the complete destination before deleting the scalar rows.
        while present != 0 {
            let bit = present.trailing_zeros() as usize;
            present &= present - 1;
            self.sparse.remove(&(base | (bit << 12)));
        }
    }

    fn insert_full_word(
        &mut self, base: usize, frames: &[CapabilityDescriptor; FRAME_WORD_PAGES],
    ) -> Result<(), ()> {
        self.pack_word(base);
        if let Some(backing) = self.dense.get_mut(&(base >> 18)) {
            backing.descriptors.copy_from_slice(frames);
            backing.present = u64::MAX;
        } else {
            for (index, &frame) in frames.iter().enumerate() {
                self.insert(base + index * 4096, frame)?;
            }
        }
        Ok(())
    }

    fn find(&self, va: usize) -> Option<CapabilityDescriptor> {
        #[cfg(test)]
        FRAME_POINT_QUERIES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if va & 4095 == 0 {
            if let Some(word) = self.dense.get(&(va >> 18)) {
                let bit = (va >> 12) & 63;
                return (word.present & (1u64 << bit) != 0).then(|| word.descriptors[bit]);
            }
        }
        self.sparse.get(&va).copied()
    }

    fn remove(&mut self, va: usize) -> Option<CapabilityDescriptor> {
        if va & 4095 == 0 {
            if let Some(word) = self.dense.get_mut(&(va >> 18)) {
                let bit = (va >> 12) & 63;
                if word.present & (1u64 << bit) == 0 {
                    // A failed first MAP may leave a prepared, empty word.
                    if word.present == 0 { self.dense.remove(&(va >> 18)); }
                    return None;
                }
                let frame = word.descriptors[bit];
                word.present &= !(1u64 << bit);
                // Keep nonempty words packed so deletion/reinsertion never
                // needs a demotion allocation. Empty words release their RAM.
                if word.present == 0 { self.dense.remove(&(va >> 18)); }
                return Some(frame);
            }
        }
        self.sparse.remove(&va)
    }

    fn dense_range(&self, start: usize, end: usize) -> impl Iterator<Item = (usize, CapabilityDescriptor)> + '_ {
        let mut words = self.dense.range((start >> 18)..=((end - 1) >> 18));
        let mut pending = 0u64;
        let mut base = 0;
        let mut descriptors: &[CapabilityDescriptor] = &[];
        core::iter::from_fn(move || loop {
            if pending != 0 {
                let bit = pending.trailing_zeros() as usize;
                pending &= pending - 1;
                let va = base | (bit << 12);
                if va >= start && va < end { return Some((va, descriptors[bit])); }
                continue;
            }
            let (&key, word) = words.next()?;
            base = key << 18;
            pending = word.present;
            descriptors = &word.descriptors;
        })
    }

    fn has_range(&self, start: usize, end: usize) -> bool {
        start < end && (self.sparse.range(start..end).next().is_some()
            || self.dense_range(start, end).next().is_some())
    }

    fn aligned_range_backed(&self, start: usize, end: usize) -> bool {
        debug_assert!(start < end && (start | end) & 4095 == 0);
        for key in (start >> 18)..=((end - 1) >> 18) {
            let required = PagePermissions::mask(key, start, end);
            if let Some(word) = self.dense.get(&key) {
                if word.present & required != required { return false; }
            } else {
                let base = key << 18;
                let low = start.max(base);
                let high = (end - 1).min(base | ((1 << 18) - 1));
                let mut present = 0u64;
                for (&va, _) in self.sparse.range(low..=high) {
                    if va & 4095 == 0 { present |= 1u64 << ((va >> 12) & 63); }
                }
                if present & required != required { return false; }
            }
        }
        true
    }

    // A lower bound for snapshot capacity, not evidence of permissions or
    // mapped PTEs. Count packed presence bits without walking each Frame or
    // scanning unrelated scalar entries. Sparse rows grow the Vec as before.
    fn dense_range_count(&self, start: usize, end: usize) -> usize {
        if start >= end { return 0; }
        self.dense.range((start >> 18)..=((end - 1) >> 18))
            .map(|(&key, word)| (word.present & PagePermissions::mask(key, start, end)).count_ones() as usize)
            .sum()
    }

    fn visit_range(&self, start: usize, end: usize, mut visit: impl FnMut(usize, CapabilityDescriptor)) {
        let _ = self.try_visit_range(start, end, |va, frame| {
            visit(va, frame);
            Ok::<(), core::convert::Infallible>(())
        });
    }

    fn try_visit_range<E>(
        &self, start: usize, end: usize,
        mut visit: impl FnMut(usize, CapabilityDescriptor) -> Result<(), E>,
    ) -> Result<(), E> {
        // The former AVL walk returned no rows for empty/reversed ranges.
        if start >= end { return Ok(()); }
        let mut dense = self.dense_range(start, end).peekable();
        let mut sparse = self.sparse.range(start..end).map(|(&va, &frame)| (va, frame)).peekable();
        while dense.peek().is_some() || sparse.peek().is_some() {
            let take_dense = match (dense.peek(), sparse.peek()) {
                (Some(a), Some(b)) => a.0 < b.0,
                (Some(_), None) => true,
                _ => false,
            };
            let (va, frame) = if take_dense { dense.next().unwrap() } else { sparse.next().unwrap() };
            visit(va, frame)?;
        }
        Ok(())
    }
}

/// Borrowed only during synchronous source validation. The immutable VM borrow
/// keeps selected word storage and sharing bits current until the walk ends.
/// Physical ownership/refcounts and pool Frame identities remain caller checks.
pub(crate) struct SnapshotPageCursor<'a> {
    vm: &'a VmSpace,
    word: usize,
    frames: Option<&'a DenseFrameWord>,
    sharing_word: usize,
    shared: u64,
    // Read only by native tests and compile-time snapshot diagnostics. Counter
    // updates disappear from the ordinary build, which has no diagnostic use.
    counts: (usize, usize, usize),
}

impl SnapshotPageCursor<'_> {
    pub(crate) fn page_matches(
        &mut self, va: usize, pool: CapabilityDescriptor, child: CapabilityDescriptor, references: usize,
    ) -> bool {
        if references == 0 { return false; }
        if va & 4095 != 0 {
            return (references == 1 || self.vm.private_page_shared(va))
                && self.vm.frame_by_va.find(va).is_some_and(|frame| frame == pool || frame == child);
        }
        let word = va >> 18;
        let bit = (va >> 12) & 63;
        if references != 1 {
            if self.sharing_word != word {
                self.sharing_word = word;
                self.shared = self.vm.shared_private.get(&word).copied().unwrap_or(0);
                #[cfg(test)]
                SNAPSHOT_SHARED_WORD_QUERIES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
            if self.shared & (1u64 << bit) == 0 { return false; }
        }
        if self.word != word {
            self.word = word;
            self.frames = self.vm.frame_by_va.dense.get(&word);
            if cfg!(test) || option_env!("NANAMI_SNAPSHOT_SPAN_DIAG") == Some("1") {
                self.counts.0 += 1;
                self.counts.1 += usize::from(self.frames.is_some());
            }
        }
        let frame = if let Some(frames) = self.frames {
            (frames.present & (1u64 << bit) != 0).then(|| frames.descriptors[bit])
        } else {
            if cfg!(test) || option_env!("NANAMI_SNAPSHOT_SPAN_DIAG") == Some("1") {
                self.counts.2 += 1;
            }
            self.vm.frame_by_va.sparse.get(&va).copied()
        };
        frame.is_some_and(|frame| frame == pool || frame == child)
    }

    pub(crate) fn diagnostics(&self) -> (usize, usize, usize) { self.counts }
}

pub struct VmSpace {
    frame_by_va: FrameIndex,
    page_table_by_va: AvlTree,
    // Successful leaf mappings prove that all table levels exist in their
    // region. Keep several regions when faults alternate between stack/heap.
    // Eviction only restores the kernel depth query, never mapping authority.
    page_table_regions: [usize; BOOTSTRAP_PT_REGIONS],
    next_page_table_region: usize,
    // Only non-default permissions need metadata. PROT_NONE retains its Frame
    // record and allocation reference while the hardware leaf is absent.
    permissions: PagePermissions,
    // Exact aligned pages with a Frame, rights1 and confirmed mapping state.
    // Updated with every Frame/rights/uncertainty mutation; no source admission
    // or capability validity is cached here. One word describes 64 pages.
    readonly_backing: BTreeMap<usize, u64>,
    // Private fork aliases, never packed into opaque descriptors or R/W/X bits.
    shared_private: BTreeMap<usize, u64>,
    // Writable private COW pages retain logical permissions while their leaf
    // lacks WRITE. Kept separate from readonly fork aliases and uncertainty.
    private_cow_write: BTreeMap<usize, u64>,
    // Preserve exact non-page keys for existing callers; real shared pages
    // use one bitmap word per 64 pages rather than one tree entry per page.
    shared_private_unaligned: BTreeSet<usize>,
    // A capability error may follow a PTE update but precede its shootdown ACK.
    // Separate from R/W/X: unknown presence must neither grant checked access
    // nor allow backing retirement. Nodes are allocated only on error.
    uncertain_mappings: BTreeSet<usize>,
    // Initialized snapshot RAM can have logical access before its first PTE.
    // [pending, managed] bits distinguish confirmed absence from uncertainty;
    // managed survives population to accept an already delivered peer fault.
    deferred_snapshots: BTreeMap<usize, [u64; 2]>,
}

impl VmSpace {
    pub(crate) fn snapshot_page_cursor(&self) -> SnapshotPageCursor<'_> {
        SnapshotPageCursor { vm: self, word: usize::MAX, frames: None, sharing_word: usize::MAX,
                             shared: 0, counts: (0, 0, 0) }
    }

    #[cfg(test)]
    pub fn take_snapshot_point_queries() -> (usize, usize) {
        use core::sync::atomic::Ordering::Relaxed;
        (FRAME_POINT_QUERIES.swap(0, Relaxed), SHARED_POINT_QUERIES.swap(0, Relaxed))
    }

    #[cfg(test)]
    pub fn take_snapshot_shared_word_queries() -> usize {
        SNAPSHOT_SHARED_WORD_QUERIES.swap(0, core::sync::atomic::Ordering::Relaxed)
    }
    /// A large PDE is not evidence of a 4K leaf table. Drop only readiness
    /// hints when splitting or restoring it; table ownership remains in Memory.
    pub fn invalidate_page_table_region(&mut self, address: usize) {
        let region = address >> PAGE_TABLE_REGION_SHIFT;
        for entry in &mut self.page_table_regions {
            if *entry == region { *entry = NO_PAGE_TABLE_REGION; }
        }
    }

    pub fn forget_page_table_slot(&mut self, address: usize) {
        self.page_table_by_va.remove(address);
        self.invalidate_page_table_region(address);
    }
    #[cfg(test)]
    pub fn take_frame_scalar_insertions() -> usize {
        FRAME_SCALAR_INSERTIONS.swap(0, core::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(test)]
    pub fn take_frame_point_records() -> usize {
        FRAME_POINT_RECORDS.swap(0, core::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(test)]
    pub fn take_immutable_permission_counts() -> (usize, usize) {
        use core::sync::atomic::Ordering::Relaxed;
        (PERMISSION_POINT_QUERIES.swap(0, Relaxed), IMMUTABLE_PERMISSION_WORDS.swap(0, Relaxed))
    }
    #[cfg(test)]
    pub fn frame_storage_counts(&self) -> (usize, usize) {
        (self.frame_by_va.dense.len(), self.frame_by_va.sparse.len())
    }
    /// Optional metadata only, called for the current bulk-map word after RAM
    /// ownership/initialization. No Frame, permissions or page-table proof is
    /// published. Failure falls back to ordinary per-page scalar insertion.
    pub fn prepare_frame_word(&mut self, va: usize, remaining_pages: usize) {
        if va & 4095 == 0 && remaining_pages.min(64 - ((va >> 12) & 63)) >= 32 {
            self.frame_by_va.pack_word(va);
        }
    }

    /// Record a complete word of initialized read-only backing with no PTEs.
    /// The caller already owns every Frame and has excluded live mappings.
    pub fn record_deferred_frame_word(
        &mut self, base: usize, frames: &[CapabilityDescriptor; FRAME_WORD_PAGES], shared: bool,
    ) -> Result<(), ()> {
        let end = base.checked_add(FRAME_WORD_BYTES).ok_or(())?;
        if base & (FRAME_WORD_BYTES - 1) != 0 || self.has_backed_pages(base, end) {
            return Err(());
        }
        let word = base >> 18;
        self.frame_by_va.insert_full_word(base, frames)?;
        self.deferred_snapshots.insert(word, [u64::MAX; 2]);
        if shared {
            self.shared_private.insert(word, u64::MAX);
        }
        self.invalidate_page_table_region(base);
        self.set_permission_words(base, end, 1);
        Ok(())
    }

    /// Record initialized private backing before its huge-leaf MAP. No 4K
    /// page-table readiness or PTE presence is inferred from these slice keys.
    /// Keep all ownership, sharing and uncertainty flags independent of this
    /// insertion; the caller still owns MAP failure and whole-range cleanup.
    pub fn record_unmapped_frame_word(
        &mut self, base: usize, frames: &[CapabilityDescriptor; FRAME_WORD_PAGES],
    ) -> Result<(), ()> {
        let end = base.checked_add(FRAME_WORD_BYTES).ok_or(())?;
        if base & (FRAME_WORD_BYTES - 1) != 0 || self.has_backed_aligned_pages(base, end)
            || self.has_uncertain_mappings(base, end)
        { return Err(()); }
        self.frame_by_va.insert_full_word(base, frames)?;
        self.invalidate_page_table_region(base);
        self.set_permission_words(base, end, 0);
        Ok(())
    }

    /// Publish an immutable source identity with confirmed absent PTEs.
    pub fn record_immutable_source_frame_word(
        &mut self, base: usize, frames: &[CapabilityDescriptor; FRAME_WORD_PAGES],
    ) -> Result<(), ()> {
        let end = base.checked_add(FRAME_WORD_BYTES).ok_or(())?;
        if base & (FRAME_WORD_BYTES - 1) != 0 || self.has_backed_aligned_pages(base, end)
            || self.has_uncertain_mappings(base, end)
        {
            return Err(());
        }
        self.frame_by_va.insert_full_word(base, frames)?;
        self.shared_private.insert(base >> 18, u64::MAX);
        self.invalidate_page_table_region(base);
        self.set_permission_words(base, end, 0);
        Ok(())
    }

    pub const fn new() -> Self {
        Self {
            frame_by_va: FrameIndex::new(),
            page_table_by_va: AvlTree::new(),
            page_table_regions: [NO_PAGE_TABLE_REGION; BOOTSTRAP_PT_REGIONS],
            next_page_table_region: 0,
            permissions: PagePermissions::new(),
            readonly_backing: BTreeMap::new(),
            shared_private: BTreeMap::new(),
            private_cow_write: BTreeMap::new(),
            shared_private_unaligned: BTreeSet::new(),
            uncertain_mappings: BTreeSet::new(),
            deferred_snapshots: BTreeMap::new(),
        }
    }

    pub fn forget_frame(&mut self, virtual_address: usize) -> Option<CapabilityDescriptor> {
        self.forget_deferred_snapshot(virtual_address);
        self.clear_readonly_backing(virtual_address);
        self.permissions.remove(virtual_address);
        self.mark_private_page_shared(virtual_address, false);
        self.uncertain_mappings.remove(&virtual_address);
        self.frame_by_va.remove(virtual_address)
    }

    pub fn private_page_shared(&self, va: usize) -> bool {
        #[cfg(test)]
        SHARED_POINT_QUERIES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if va & 4095 != 0 { return self.shared_private_unaligned.contains(&va); }
        self.shared_private.get(&(va >> 18))
            .is_some_and(|bits| bits & (1u64 << ((va >> 12) & 63)) != 0)
    }

    pub fn mark_private_page_shared(&mut self, va: usize, shared: bool) {
        if !shared && va & 4095 == 0 { self.mark_private_page_cow_write(va, false); }
        if va & 4095 != 0 {
            if shared { self.shared_private_unaligned.insert(va); }
            else { self.shared_private_unaligned.remove(&va); }
        } else if shared {
            *self.shared_private.entry(va >> 18).or_default() |= 1u64 << ((va >> 12) & 63);
        } else if let Some(bits) = self.shared_private.get_mut(&(va >> 18)) {
            *bits &= !(1u64 << ((va >> 12) & 63));
            if *bits == 0 { self.shared_private.remove(&(va >> 18)); }
        }
    }

    pub fn mark_private_range_shared(&mut self, start: usize, end: usize) {
        assert!(start < end && (start | end) & 4095 == 0);
        for word in (start >> 18)..=((end - 1) >> 18) {
            *self.shared_private.entry(word).or_default() |= PagePermissions::mask(word, start, end);
        }
    }

    pub fn private_page_cow_write(&self, va: usize) -> bool {
        va & 4095 == 0 && self.private_cow_write.get(&(va >> 18))
            .is_some_and(|bits| bits & (1u64 << ((va >> 12) & 63)) != 0)
    }

    /// Caller must establish the read-only leaf under VM quiescence before
    /// publishing a writable COW alias. No PTE or ownership proof is implied.
    pub fn mark_private_page_cow_write(&mut self, va: usize, shared: bool) {
        assert_eq!(va & 4095, 0);
        if shared {
            self.mark_private_page_shared(va, true);
            *self.private_cow_write.entry(va >> 18).or_default() |= 1u64 << ((va >> 12) & 63);
        } else if let Some(bits) = self.private_cow_write.get_mut(&(va >> 18)) {
            *bits &= !(1u64 << ((va >> 12) & 63));
            if *bits == 0 { self.private_cow_write.remove(&(va >> 18)); }
        }
    }

    pub fn hardware_permissions(&self, va: usize) -> usize {
        if self.mapping_deferred(va) { return 0; }
        let permissions = self.permissions(va);
        if self.private_page_cow_write(va) { permissions & !2 } else { permissions }
    }

    pub fn mapping_deferred(&self, va: usize) -> bool {
        va & 4095 == 0 && self.deferred_snapshots.get(&(va >> 18))
            .is_some_and(|bits| bits[0] & (1u64 << ((va >> 12) & 63)) != 0)
    }

    pub fn snapshot_page_managed(&self, va: usize) -> bool {
        va & 4095 == 0 && self.deferred_snapshots.get(&(va >> 18))
            .is_some_and(|bits| bits[1] & (1u64 << ((va >> 12) & 63)) != 0)
    }

    pub fn has_deferred_mappings(&self, start: usize, end: usize) -> bool {
        if start >= end || self.deferred_snapshots.is_empty() { return false; }
        self.deferred_snapshots.range((start >> 18)..=((end - 1) >> 18))
            .any(|(&word, bits)| bits[0] & PagePermissions::mask(word, start, end) != 0)
    }

    /// Only for initialized, referenced RAM whose PTE is confirmed absent.
    pub fn mark_mapping_deferred(&mut self, va: usize) {
        assert_eq!(va & 4095, 0);
        let bit = 1u64 << ((va >> 12) & 63);
        let bits = self.deferred_snapshots.entry(va >> 18).or_default();
        bits[0] |= bit; bits[1] |= bit;
        self.invalidate_page_table_region(va);
    }

    fn clear_deferred_pending(&mut self, va: usize) {
        if va & 4095 == 0 {
            if let Some(bits) = self.deferred_snapshots.get_mut(&(va >> 18)) {
                bits[0] &= !(1u64 << ((va >> 12) & 63));
            }
        }
    }

    fn forget_deferred_snapshot(&mut self, va: usize) {
        if va & 4095 == 0 {
            if let Some(bits) = self.deferred_snapshots.get_mut(&(va >> 18)) {
                let mask = !(1u64 << ((va >> 12) & 63));
                bits[0] &= mask; bits[1] &= mask;
                if bits[1] == 0 { self.deferred_snapshots.remove(&(va >> 18)); }
            }
        }
    }

    /// Only after a successful leaf MAP and its acknowledgement.
    pub fn complete_deferred_mapping(&mut self, va: usize) {
        self.clear_deferred_pending(va);
        self.remember_page_table_region(va);
    }

    fn remember_page_table_region(&mut self, va: usize) {
        let region = va >> PAGE_TABLE_REGION_SHIFT;
        if !self.page_table_regions.contains(&region) {
            self.page_table_regions[self.next_page_table_region] = region;
            self.next_page_table_region = (self.next_page_table_region + 1) % BOOTSTRAP_PT_REGIONS;
        }
    }

    pub fn shared_private_pages(&self, start: usize, end: usize) -> alloc::vec::Vec<usize> {
        let mut pages: alloc::vec::Vec<_> = self.shared_private_aligned_range(start, end).collect();
        if self.shared_private_unaligned.range(start..end).next().is_some() {
            pages.extend(self.shared_private_unaligned.range(start..end).copied());
            // The two streams contain disjoint keys. Keep exact old ordering.
            pages.sort_unstable();
        }
        pages
    }

    pub fn private_range_shared(&self, start: usize, end: usize) -> bool {
        self.shared_private_aligned_range(start, end).next().is_some()
            || self.shared_private_unaligned.range(start..end).next().is_some()
    }

    fn shared_private_aligned_range(
        &self, start: usize, end: usize,
    ) -> impl Iterator<Item = usize> + '_ {
        // Preserve the underlying tree's exact reversed-range behavior. In
        // particular, a never-populated BTreeSet bypasses range validation.
        // The exceptional-key tree is checked by the caller in the same way.
        if start > end { let _ = self.shared_private.range(start..end); }
        let first = start >> 18;
        let last = if start < end { (end - 1) >> 18 } else { first };
        let mut words = self.shared_private.range(first..=last);
        let mut pending = 0u64;
        let mut base = 0usize;
        core::iter::from_fn(move || loop {
            if pending != 0 {
                let bit = pending.trailing_zeros() as usize;
                pending &= pending - 1;
                return Some(base | (bit << 12));
            }
            let (&word, &bits) = words.next()?;
            if start >= end { return None; }
            let low = if word == first { ((start & ((1 << 18) - 1)) + 4095) >> 12 } else { 0 };
            let high = if word == last { (((end - 1) & ((1 << 18) - 1)) >> 12) + 1 } else { 64 };
            base = word << 18;
            pending = if low < high {
                bits & (u64::MAX << low) & (u64::MAX >> (64 - high))
            } else { 0 };
        })
    }

    /// Confirmed access only. Zero also covers an uncertain PTE; callers must
    /// use frame_may_be_present rather than infer absence from permissions.
    pub fn permissions(&self, virtual_address: usize) -> usize {
        #[cfg(test)]
        PERMISSION_POINT_QUERIES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if self.mapping_is_uncertain(virtual_address) {
            return 0;
        }
        self.permissions.get(virtual_address).unwrap_or(7)
    }

    /// Current logical read/write permission for every touched 4-KiB page.
    /// Default rights7 need no record. Scan only exceptions and uncertain
    /// mappings instead of repeating tree lookups for each page in a bulk copy.
    /// Backing resolution and private-write preparation still belong to copy.
    pub fn user_range_allowed(&self, start: usize, end: usize, permission: usize) -> bool {
        if start >= end || !matches!(permission, 1 | 2) {
            return false;
        }
        let first = start & !4095;
        if first >> 12 == (end - 1) >> 12 {
            return self.permissions(first) & permission != 0;
        }
        // Match the page-key lookups even if unrelated unaligned metadata keys
        // exist. No state is cached across requests or owner transitions.
        !self.uncertain_mappings.range(first..end).any(|va| va & 4095 == 0)
            && self.permissions.allows_aligned(first, end, permission)
    }

    pub fn mapping_is_uncertain(&self, virtual_address: usize) -> bool {
        self.uncertain_mappings.contains(&virtual_address)
    }

    /// Every page in an aligned snapshot extent has confirmed exact rights1
    /// and a Frame. No persistent seal hint: inspect current metadata each time.
    pub fn readonly_backed_range(&self, start: usize, end: usize) -> bool {
        if start >= end || (start | end) & 4095 != 0 { return false; }
        let first_page = start >> 12;
        let last_page = (end >> 12) - 1;
        let first = first_page >> 6;
        let last = last_page >> 6;
        let mut next = first;
        for (&word, &bits) in self.readonly_backing.range(first..=last) {
            if word != next { return false; }
            let low = if word == first { first_page & 63 } else { 0 };
            let high = if word == last { last_page & 63 } else { 63 };
            let mask = (u64::MAX << low) & (u64::MAX >> (63 - high));
            if bits & mask != mask { return false; }
            next += 1;
        }
        next == last + 1
    }

    /// Confirmed, fully backed aligned range with exact rights, including
    /// default7. This proves current metadata only; COW write guards belong to
    /// the caller, and no permission/Frame result is retained across requests.
    pub fn backed_range_with_permissions(&self, start: usize, end: usize, permissions: usize) -> bool {
        if start >= end || (start | end) & 4095 != 0 || permissions > 7 {
            return false;
        }
        if end - start == 4096 {
            return self.permissions(start) == permissions
                && (permissions != 0 || !self.mapping_is_uncertain(start))
                && self.find_frame(start).is_some();
        }
        if self.uncertain_mappings.range(start..end).any(|va| va & 4095 == 0) {
            return false;
        }
        if !self.permissions.aligned_range_is(start, end, permissions) { return false; }
        // Inspect current presence, not a retained coverage proof. Dense words
        // need one bitmap comparison rather than one callback per descriptor;
        // scalar words retain exact aligned-key checks and early hole failure.
        self.frame_by_va.aligned_range_backed(start, end)
    }

    /// Current Frame coverage only, including zero rights and uncertainty.
    /// The caller must still apply all permission/COW and PTE cleanup rules.
    pub fn fully_backed_aligned_range(&self, start: usize, end: usize) -> bool {
        start < end && (start | end) & 4095 == 0
            && self.frame_by_va.aligned_range_backed(start, end)
    }

    /// Current backing and rights0/1 across an immutable source reservation.
    /// Native read aliases may cover any subset; uncertainty is never absence.
    /// This is a fresh metadata query, not a cached seal or ownership proof.
    pub fn immutable_source_backed_range(&self, start: usize, end: usize) -> bool {
        self.fully_backed_aligned_range(start, end)
            && !self.has_uncertain_mappings(start, end)
            && self.permissions.aligned_range_readonly_or_none(start, end)
    }

    /// Walk current Frame identities in VA order. A missing aligned page
    /// ends the walk. This deliberately supplies no permission or PTE proof.
    pub fn visit_aligned_frame_run(
        &self, start: usize, end: usize, mut visit: impl FnMut(CapabilityDescriptor),
    ) -> bool {
        if start >= end || (start | end) & 4095 != 0 { return false; }
        let mut next = start;
        self.frame_by_va.try_visit_range(start, end, |va, frame| {
            if va & 4095 != 0 { return Ok(()); }
            if va != next { return Err(()); }
            visit(frame);
            next += 4096;
            Ok(())
        }).is_ok() && next == end
    }

    /// Exact confirmed PROT_NONE metadata, including pages without backing.
    /// This proves no access needs revoking, not Frame ownership or coverage.
    pub fn confirmed_zero_permissions(&self, start: usize, end: usize) -> bool {
        start < end && (start | end) & 4095 == 0
            && !self.uncertain_mappings.range(start..end).any(|va| va & 4095 == 0)
            && self.permissions.aligned_range_is(start, end, 0)
    }

    pub fn has_uncertain_mappings(&self, start: usize, end: usize) -> bool {
        self.uncertain_mappings.range(start..end).next().is_some()
    }

    pub fn mark_mapping_uncertain(&mut self, virtual_address: usize) {
        self.clear_readonly_backing(virtual_address);
        self.uncertain_mappings.insert(virtual_address);
    }

    pub fn confirm_unmapped(&mut self, virtual_address: usize) {
        self.uncertain_mappings.remove(&virtual_address);
        self.set_permissions(virtual_address, 0);
    }

    pub fn frame_may_be_present(&self, virtual_address: usize) -> bool {
        self.mapping_is_uncertain(virtual_address)
            || (self.find_frame(virtual_address).is_some()
                && self.permissions.get(virtual_address).unwrap_or(7) != 0
                && !self.mapping_deferred(virtual_address))
    }

    pub fn set_permissions(&mut self, virtual_address: usize, permissions: usize) {
        if permissions == 0 { self.clear_deferred_pending(virtual_address); }
        self.permissions.set(virtual_address, permissions);
        self.update_readonly_backing(virtual_address, permissions);
    }

    /// Apply uniform rights to complete, aligned 64-page words. Use the
    /// point setter when a word needs scalar cleanup or a readonly proof
    /// that cannot be copied from its packed Frame bitmap.
    pub fn set_permission_words(&mut self, start: usize, end: usize, permissions: usize) {
        assert!(start <= end && (start | end) & (FRAME_WORD_BYTES - 1) == 0 && permissions <= 7);
        let denied = 7 ^ permissions;
        let planes = core::array::from_fn(|bit| {
            if denied & (1 << bit) != 0 { u64::MAX } else { 0 }
        });
        for base in (start..end).step_by(FRAME_WORD_BYTES) {
            let word = base >> 18;
            let limit = base + FRAME_WORD_BYTES;
            let readonly = if permissions != 1 {
                Some(0)
            } else if self.has_uncertain_mappings(base, limit) {
                None
            } else {
                self.frame_by_va.dense.get(&word).map(|frames| frames.present)
            };
            let exceptional = self.permissions.exceptional.range(base..limit)
                .any(|(&va, _)| va & 4095 == 0);
            let Some(readonly) = readonly.filter(|_| !exceptional) else {
                for va in (base..limit).step_by(4096) { self.set_permissions(va, permissions); }
                continue;
            };
            if permissions == 7 {
                self.permissions.pages.remove(&word);
            } else {
                self.permissions.pages.insert(word, planes);
            }
            if permissions == 0 {
                if let Some(bits) = self.deferred_snapshots.get_mut(&word) { bits[0] = 0; }
            }
            if readonly == 0 {
                self.readonly_backing.remove(&word);
            } else {
                self.readonly_backing.insert(word, readonly);
            }
        }
    }

    fn clear_readonly_backing(&mut self, va: usize) {
        if va & 4095 != 0 { return; }
        let word = va >> 18;
        if let Some(bits) = self.readonly_backing.get_mut(&word) {
            *bits &= !(1u64 << ((va >> 12) & 63));
            if *bits == 0 { self.readonly_backing.remove(&word); }
        }
    }

    fn update_readonly_backing(&mut self, va: usize, permissions: usize) {
        if va & 4095 != 0 { return; }
        if permissions == 1 && !self.mapping_is_uncertain(va)
            && self.frame_by_va.find(va).is_some()
        {
            self.mark_readonly_backed(va);
        } else {
            self.clear_readonly_backing(va);
        }
    }

    fn mark_readonly_backed(&mut self, va: usize) {
        *self.readonly_backing.entry(va >> 18).or_default() |= 1u64 << ((va >> 12) & 63);
    }

    pub fn backed_pages(&self, start: usize, end: usize) -> alloc::vec::Vec<(usize, usize)> {
        let mut pages = alloc::vec::Vec::new();
        // Reserve only when packed words make counting cheap. Include scalar
        // rows in a mixed range so a nearly full reservation does not trigger
        // another doubling. Pure scalar ranges keep their one-pass growth.
        // Failure preserves incremental growth and the complete metadata merge.
        let dense = self.frame_by_va.dense_range_count(start, end);
        if dense != 0 {
            let count = dense + self.frame_by_va.sparse.range(start..end).count();
            let _ = pages.try_reserve_exact(count);
        }
        self.visit_backed_metadata(start, end, |va, _, rights, uncertain| {
            pages.push((va, if uncertain { 0 } else { rights }));
        });
        pages
    }

    /// Merge the three ordered metadata streams without a fresh permission
    /// and uncertainty tree search per Frame. Include unaligned keys exactly
    /// as ordinary point lookups do; metadata without backing is skipped.
    pub fn visit_backed_metadata(
        &self, start: usize, end: usize,
        mut action: impl FnMut(usize, CapabilityDescriptor, usize, bool),
    ) {
        let _ = self.try_visit_backed_metadata(start, end, |va, frame, rights, uncertain| {
            action(va, frame, rights, uncertain);
            Ok::<(), core::convert::Infallible>(())
        });
    }

    fn try_visit_backed_metadata<E>(
        &self, start: usize, end: usize,
        mut action: impl FnMut(usize, CapabilityDescriptor, usize, bool) -> Result<(), E>,
    ) -> Result<(), E> {
        if start >= end {
            return Ok(());
        }
        let mut permissions = self.permissions.range(start, end).peekable();
        let mut uncertain = self.uncertain_mappings.range(start..end).peekable();
        self.frame_by_va.try_visit_range(start, end, |va, frame| {
            while permissions.peek().is_some_and(|(key, _)| *key < va) {
                permissions.next();
            }
            while uncertain.peek().is_some_and(|key| **key < va) {
                uncertain.next();
            }
            let rights = permissions.peek().filter(|(key, _)| *key == va)
                .map_or(7, |(_, rights)| *rights);
            let pending = uncertain.peek().is_some_and(|key| **key == va);
            action(va, frame, rights, pending)
        })
    }

    /// Same backing predicate as a nonempty backed_pages snapshot, including
    /// PROT_NONE and uncertain Frames, with no permission lookup/allocation.
    pub fn has_backed_pages(&self, start: usize, end: usize) -> bool {
        self.frame_by_va.has_range(start, end)
    }

    /// Equivalent to find_frame at each page of an aligned range, including
    /// PROT_NONE and uncertain Frames. Scalar unaligned keys are not sampled
    /// by that walk and must not create additional collisions here.
    pub fn has_backed_aligned_pages(&self, start: usize, end: usize) -> bool {
        debug_assert!((start | end) & 4095 == 0);
        start < end
            && (self.frame_by_va.dense_range(start, end).next().is_some()
                || self.frame_by_va.sparse.range(start..end)
                    .any(|(&va, _)| va & 4095 == 0))
    }

    /// Visit current Frame metadata whose PTE may be present, in VA order.
    /// The borrow keeps metadata stable during callbacks. The first error
    /// stops both callbacks and the metadata walk. The caller must mark
    /// the failed VA uncertain before any destructive capability/RAM cleanup.
    pub fn try_for_each_present_frame<E>(
        &self, start: usize, end: usize,
        mut action: impl FnMut(usize, CapabilityDescriptor) -> Result<(), E>,
    ) -> Result<(), (usize, E)> {
        self.try_visit_backed_metadata(start, end, |va, frame, rights, uncertain| {
            if !uncertain && (rights == 0 || self.mapping_deferred(va)) { return Ok(()); }
            action(va, frame).map_err(|error| (va, error))
        })
    }

    pub fn all_backed_pages(
        &self,
        start: usize,
        end: usize,
        mut predicate: impl FnMut(usize) -> bool,
    ) -> bool {
        self.frame_by_va.try_visit_range(start, end, |va, _| {
            if predicate(va) { Ok(()) } else { Err(()) }
        }).is_ok()
    }
}

impl VmTracker for VmSpace {
    fn record_frame(
        &mut self,
        virtual_address: usize,
        frame_descriptor: CapabilityDescriptor,
    ) -> Result<(), ()> {
        #[cfg(test)]
        FRAME_POINT_RECORDS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        self.frame_by_va.insert(virtual_address, frame_descriptor)?;
        // Successful insertion already proves this exact key has a Frame.
        // Recording never changes rights/uncertainty: their mutators already
        // clear any ineligible readonly bit, so only newly eligible backing
        // needs an update here. Do not re-find the Frame or clear other bits.
        if virtual_address & 4095 == 0
            && self.permissions.get(virtual_address) == Some(1)
            && !self.mapping_is_uncertain(virtual_address)
        {
            self.mark_readonly_backed(virtual_address);
        }
        if !self.mapping_deferred(virtual_address) { self.remember_page_table_region(virtual_address); }
        Ok(())
    }

    fn record_page_table(
        &mut self,
        virtual_address: usize,
        page_table_slot_index: usize,
    ) -> Result<(), ()> {
        self.page_table_by_va
            .insert(virtual_address, page_table_slot_index)
    }

    fn find_frame(&self, virtual_address: usize) -> Option<CapabilityDescriptor> {
        self.frame_by_va.find(virtual_address)
    }

    fn find_page_table_slot(&self, virtual_address: usize) -> Option<usize> {
        self.page_table_by_va.find(virtual_address)
    }

    fn forget_frame(&mut self, virtual_address: usize) -> Option<CapabilityDescriptor> {
        VmSpace::forget_frame(self, virtual_address)
    }

    fn page_tables_ready(&self, virtual_address: usize) -> bool {
        self.page_table_regions.contains(&(virtual_address >> PAGE_TABLE_REGION_SHIFT))
    }
}
