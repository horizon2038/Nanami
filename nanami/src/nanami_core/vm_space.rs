use crate::nanami_utils::avl::AvlTree;
use crate::nanami_utils::static_avl::StaticAvlTree;
use alloc::collections::{BTreeMap, BTreeSet};
use nun::CapabilityDescriptor;

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

    fn find(&self, va: usize) -> Option<CapabilityDescriptor> {
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
    // Preserve exact non-page keys for existing callers; real shared pages
    // use one bitmap word per 64 pages rather than one tree entry per page.
    shared_private_unaligned: BTreeSet<usize>,
    // A capability error may follow a PTE update but precede its shootdown ACK.
    // Separate from R/W/X: unknown presence must neither grant checked access
    // nor allow backing retirement. Nodes are allocated only on error.
    uncertain_mappings: BTreeSet<usize>,
}

impl VmSpace {
    /// Optional metadata only, called for the current bulk-map word after RAM
    /// ownership/initialization. No Frame, permissions or page-table proof is
    /// published. Failure falls back to ordinary per-page scalar insertion.
    pub fn prepare_frame_word(&mut self, va: usize, remaining_pages: usize) {
        if va & 4095 == 0 && remaining_pages.min(64 - ((va >> 12) & 63)) >= 32 {
            self.frame_by_va.pack_word(va);
        }
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
            shared_private_unaligned: BTreeSet::new(),
            uncertain_mappings: BTreeSet::new(),
        }
    }

    pub fn forget_frame(&mut self, virtual_address: usize) -> Option<CapabilityDescriptor> {
        self.clear_readonly_backing(virtual_address);
        self.permissions.remove(virtual_address);
        self.mark_private_page_shared(virtual_address, false);
        self.uncertain_mappings.remove(&virtual_address);
        self.frame_by_va.remove(virtual_address)
    }

    pub fn private_page_shared(&self, va: usize) -> bool {
        if va & 4095 != 0 { return self.shared_private_unaligned.contains(&va); }
        self.shared_private.get(&(va >> 18))
            .is_some_and(|bits| bits & (1u64 << ((va >> 12) & 63)) != 0)
    }

    pub fn mark_private_page_shared(&mut self, va: usize, shared: bool) {
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
                && self.permissions.get(virtual_address).unwrap_or(7) != 0)
    }

    pub fn set_permissions(&mut self, virtual_address: usize, permissions: usize) {
        self.permissions.set(virtual_address, permissions);
        self.update_readonly_backing(virtual_address, permissions);
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
            if rights == 0 && !uncertain { return Ok(()); }
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
        VmSpace::forget_frame(self, virtual_address)
    }

    fn page_tables_ready(&self, virtual_address: usize) -> bool {
        self.page_table_regions.contains(&(virtual_address >> PAGE_TABLE_REGION_SHIFT))
    }
}
