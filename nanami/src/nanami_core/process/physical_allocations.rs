//! Recycled metadata keeps process/reference index churn and heap traffic bounded.
use super::{PhysicalPageReference, ProcessPhysicalAllocation};
use alloc::vec::Vec;

#[derive(Default)]
pub(super) struct PhysicalAllocations {
    active: Index<usize>,
    deferred: Index<usize>,
    references: ReferenceCounts,
    nodes: Vec<Node>,
    free: Option<usize>,
}

#[derive(Clone, Copy)]
pub(super) enum AllocationState {
    Active,
    Deferred,
}

#[derive(Clone, Copy)]
struct Node {
    allocation: ProcessPhysicalAllocation,
    next: Option<usize>,
}

const REFERENCE_PARTITION_BITS: u32 = 4;
const REFERENCE_PARTITIONS: usize = 1 << REFERENCE_PARTITION_BITS;

/// Count active and deferred owners together without resizing the entire index.
/// High hash bits select a partition; its open-addressed index uses low bits.
#[derive(Default)]
struct ReferenceCounts {
    partitions: [Index<(usize, usize)>; REFERENCE_PARTITIONS],
}

impl ReferenceCounts {
    fn partition(key: (usize, usize)) -> usize {
        key.hash() >> (usize::BITS - REFERENCE_PARTITION_BITS)
    }

    fn get(&self, key: (usize, usize)) -> Option<usize> {
        self.partitions[Self::partition(key)].get(key)
    }

    fn insert(&mut self, key: (usize, usize), count: usize) {
        self.partitions[Self::partition(key)].insert(key, count);
    }

    fn remove(&mut self, key: (usize, usize)) -> Option<usize> {
        self.partitions[Self::partition(key)].remove(key)
    }
}

impl PhysicalAllocations {
    /// Canonicalize a uniquely owned extent before installing page aliases.
    /// Reserve all metadata first: an allocation failure must leave its single
    /// whole-extent reference intact. No physical memory is released here.
    pub(super) fn split_unique_extent(
        &mut self,
        pid: usize,
        allocation: ProcessPhysicalAllocation,
    ) -> Result<(), ()> {
        let pages = allocation.page_count;
        if pages == 0
            || allocation.base_va % super::USER_PAGE_SIZE != 0
            || pages
                .checked_mul(super::USER_PAGE_SIZE)
                .and_then(|bytes| allocation.base_va.checked_add(bytes))
                .is_none()
            || allocation.start_slot.checked_add(pages).is_none()
            || allocation.base_page.checked_add(pages).is_none()
            || self.find(AllocationState::Active, pid, allocation.base_va, pages)
                != Some(allocation)
            || self.count(allocation) != 1
        {
            return Err(());
        }
        if pages == 1 {
            return Ok(());
        }
        let mut additional = [0usize; REFERENCE_PARTITIONS];
        for offset in 0..pages {
            let key = (allocation.base_page + offset, 1);
            if self.references.get(key).is_some() {
                return Err(());
            }
            additional[ReferenceCounts::partition(key)] += 1;
        }
        self.nodes.try_reserve(pages - 1).map_err(|_| ())?;
        for (partition, count) in self.references.partitions.iter_mut().zip(additional) {
            partition.try_reserve(count)?;
        }
        // The existing active PID index already has capacity for this owner.
        // Request-local PhysicalPageReference handles must not survive this
        // rewrite; callers collect their page references after canonicalizing.
        self.take(AllocationState::Active, pid, allocation.base_va, pages)
            .ok_or(())?;
        for offset in 0..pages {
            self.insert(
                AllocationState::Active,
                pid,
                ProcessPhysicalAllocation {
                    base_va: allocation.base_va + offset * super::USER_PAGE_SIZE,
                    start_slot: allocation.start_slot + offset,
                    base_page: allocation.base_page + offset,
                    page_count: 1,
                },
            );
        }
        Ok(())
    }

    /// Move this owner's complete active chain without changing references or
    /// allocating replacement records. Deferred tails may already own aliases.
    pub(super) fn defer_all(&mut self, pid: usize) {
        let Some(head) = self.active.remove(pid) else {
            return;
        };
        let mut tail = head;
        while let Some(next) = self.nodes[tail].next {
            tail = next;
        }
        self.nodes[tail].next = self.deferred.get(pid);
        self.deferred.insert(pid, head);
    }

    pub(super) fn page_references(
        &self,
        pid: usize,
        start: usize,
        end: usize,
    ) -> Vec<PhysicalPageReference> {
        let mut references = Vec::new();
        let mut next = self.active.get(pid);
        while let Some(index) = next {
            let node = self.nodes[index];
            let allocation = node.allocation;
            if allocation.page_count == 1 && allocation.base_va >= start && allocation.base_va < end
            {
                references.push(PhysicalPageReference {
                    allocation,
                    owner: pid,
                    index,
                });
            }
            next = node.next;
        }
        references.sort_by_key(|reference| reference.allocation.base_va);
        references.dedup_by_key(|reference| reference.allocation.base_va);
        references
    }

    pub(super) fn replace_backing(
        &mut self,
        reference: PhysicalPageReference,
        replacement: ProcessPhysicalAllocation,
    ) -> Option<bool> {
        let old = self.nodes.get(reference.index)?.allocation;
        let head = self.deferred.get(reference.owner)?;
        if old != reference.allocation
            || self.nodes[head].allocation != replacement
            || old.base_va != replacement.base_va
            || old.page_count != 1
            || replacement.page_count != 1
        {
            return None;
        }
        // Swap list ownership, then retire the old backing's now-head deferred
        // record. Both physical reference totals remain correct throughout.
        self.nodes[reference.index].allocation = replacement;
        self.nodes[head].allocation = old;
        self.take(AllocationState::Deferred, reference.owner, old.base_va, 1)
            .map(|(_, last)| last)
    }

    fn heads(&self, state: AllocationState) -> &Index<usize> {
        match state {
            AllocationState::Active => &self.active,
            AllocationState::Deferred => &self.deferred,
        }
    }

    fn heads_mut(&mut self, state: AllocationState) -> &mut Index<usize> {
        match state {
            AllocationState::Active => &mut self.active,
            AllocationState::Deferred => &mut self.deferred,
        }
    }

    pub(super) fn insert(
        &mut self,
        state: AllocationState,
        pid: usize,
        allocation: ProcessPhysicalAllocation,
    ) {
        let key = Self::key(allocation);
        let count = self.references.get(key).unwrap_or(0);
        self.references.insert(key, count + 1);
        let node = Node {
            allocation,
            next: self.heads(state).get(pid),
        };
        let index = if let Some(free) = self.free {
            self.free = self.nodes[free].next;
            self.nodes[free] = node;
            free
        } else {
            self.nodes.push(node);
            self.nodes.len() - 1
        };
        self.heads_mut(state).insert(pid, index);
    }

    pub(super) fn for_pid(
        &self,
        state: AllocationState,
        pid: usize,
    ) -> impl Iterator<Item = ProcessPhysicalAllocation> + '_ {
        let mut next = self.heads(state).get(pid);
        core::iter::from_fn(move || {
            let node = self.nodes[next?];
            next = node.next;
            Some(node.allocation)
        })
    }

    pub(super) fn count(&self, allocation: ProcessPhysicalAllocation) -> usize {
        self.references.get(Self::key(allocation)).unwrap_or(0)
    }

    /// Snapshot RAM whose final references all belong to this retirement stage.
    /// Do not decrement ownership before the caller has released the resource.
    pub(super) fn releasable(
        &self,
        state: AllocationState,
        pid: usize,
    ) -> Vec<ProcessPhysicalAllocation> {
        let mut ready = Vec::new();
        let mut aliases = Vec::new();
        for allocation in self.for_pid(state, pid) {
            if self.count(allocation) == 1 {
                ready.push(allocation);
            } else {
                aliases.push(allocation);
            }
        }
        // Ordinary private pages never allocate this second buffer or sort.
        // Group aliases only on the exceptional shared-physical-range path.
        aliases.sort_unstable_by_key(|allocation| {
            (
                Self::key(*allocation),
                allocation.base_va,
                allocation.start_slot,
            )
        });
        for group in aliases.chunk_by(|a, b| Self::key(*a) == Self::key(*b)) {
            if group.len() == self.count(group[0]) {
                ready.push(group[0]);
            }
        }
        ready
    }

    pub(super) fn find(
        &self,
        state: AllocationState,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) -> Option<ProcessPhysicalAllocation> {
        self.for_pid(state, pid)
            .find(|allocation| allocation.base_va == base_va && allocation.page_count == page_count)
    }

    pub(super) fn take(
        &mut self,
        state: AllocationState,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) -> Option<(ProcessPhysicalAllocation, bool)> {
        let index = self.detach(state, pid, base_va, page_count, false)?;
        Some(self.recycle(index))
    }

    pub(super) fn take_unique_deferred(
        &mut self,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) -> Option<ProcessPhysicalAllocation> {
        let index = self.detach(AllocationState::Deferred, pid, base_va, page_count, true)?;
        Some(self.recycle(index).0)
    }

    /// Change list ownership without dropping or re-adding the RAM reference.
    pub(super) fn defer(
        &mut self,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) -> Option<ProcessPhysicalAllocation> {
        let index = self.detach(AllocationState::Active, pid, base_va, page_count, false)?;
        self.nodes[index].next = self.deferred.get(pid);
        self.deferred.insert(pid, index);
        Some(self.nodes[index].allocation)
    }

    fn detach(
        &mut self,
        state: AllocationState,
        pid: usize,
        base_va: usize,
        page_count: usize,
        require_unique: bool,
    ) -> Option<usize> {
        let mut next = self.heads(state).get(pid);
        let mut previous: Option<usize> = None;
        while let Some(index) = next {
            let node = self.nodes[index];
            if node.allocation.base_va == base_va && node.allocation.page_count == page_count {
                if require_unique && self.count(node.allocation) != 1 {
                    return None;
                }
                if let Some(previous) = previous {
                    self.nodes[previous].next = node.next;
                } else if let Some(head) = node.next {
                    self.heads_mut(state).insert(pid, head);
                } else {
                    self.heads_mut(state).remove(pid);
                }
                return Some(index);
            }
            previous = Some(index);
            next = node.next;
        }
        None
    }

    pub(super) fn remove_pid(
        &mut self,
        state: AllocationState,
        pid: usize,
        mut removed: impl FnMut(ProcessPhysicalAllocation, bool),
    ) {
        let mut next = self.heads_mut(state).remove(pid);
        while let Some(index) = next {
            next = self.nodes[index].next;
            let (allocation, last) = self.recycle(index);
            removed(allocation, last);
        }
    }

    fn key(allocation: ProcessPhysicalAllocation) -> (usize, usize) {
        (allocation.base_page, allocation.page_count)
    }

    /// Retire successful references once, retaining the failed record and suffix.
    pub(super) fn try_remove_pid<E>(
        &mut self,
        state: AllocationState,
        pid: usize,
        mut release: impl FnMut(ProcessPhysicalAllocation, bool) -> Result<(), E>,
    ) -> Result<(), E> {
        let mut next = self.heads_mut(state).remove(pid);
        while let Some(index) = next {
            let node = self.nodes[index];
            if let Err(error) = release(node.allocation, self.count(node.allocation) == 1) {
                self.heads_mut(state).insert(pid, index);
                return Err(error);
            }
            next = node.next;
            self.recycle(index);
        }
        Ok(())
    }

    fn recycle(&mut self, index: usize) -> (ProcessPhysicalAllocation, bool) {
        let allocation = self.nodes[index].allocation;
        let key = Self::key(allocation);
        let count = self
            .references
            .get(key)
            .expect("registered physical allocation");
        if count == 1 {
            self.references.remove(key);
        } else {
            self.references.insert(key, count - 1);
        }
        self.nodes[index].next = self.free;
        self.free = Some(index);
        (allocation, count == 1)
    }
}

trait Key: Copy + Eq {
    fn hash(self) -> usize;
}
fn mix(mut value: u64) -> usize {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58476d1ce4e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d049bb133111eb);
    (value ^ (value >> 31)) as usize
}
impl Key for usize {
    fn hash(self) -> usize {
        mix(self as u64)
    }
}
impl Key for (usize, usize) {
    fn hash(self) -> usize {
        mix((self.0 as u64).wrapping_add((self.1 as u64).wrapping_mul(0x9e3779b97f4a7c15)))
    }
}

// At most half full. Backward-shift deletion leaves no tombstones, so repeated
// PID/range reuse does not lengthen probes or allocate replacement tree nodes.
// Growth is geometric; all ordinary insert/remove operations reuse storage.
#[derive(Default)]
struct Index<K> {
    slots: Vec<Option<(K, usize)>>,
    len: usize,
}
impl<K: Key> Index<K> {
    fn try_reserve(&mut self, additional: usize) -> Result<(), ()> {
        let required = self
            .len
            .checked_add(additional)
            .and_then(|len| len.checked_mul(2))
            .ok_or(())?;
        if required <= self.slots.len() {
            return Ok(());
        }
        let capacity = required.checked_next_power_of_two().ok_or(())?.max(16);
        let mut slots = Vec::new();
        slots.try_reserve_exact(capacity).map_err(|_| ())?;
        slots.resize(capacity, None);
        let old = core::mem::replace(&mut self.slots, slots);
        self.len = 0;
        for (key, value) in old.into_iter().flatten() {
            self.insert(key, value);
        }
        Ok(())
    }

    fn locate(&self, key: K) -> Option<usize> {
        if self.slots.is_empty() {
            return None;
        }
        let mask = self.slots.len() - 1;
        let mut index = key.hash() & mask;
        loop {
            match self.slots[index] {
                Some((found, _)) if found == key => return Some(index),
                Some(_) => index = (index + 1) & mask,
                None => return None,
            }
        }
    }
    fn get(&self, key: K) -> Option<usize> {
        self.locate(key).map(|index| self.slots[index].unwrap().1)
    }
    fn insert(&mut self, key: K, value: usize) {
        if let Some(index) = self.locate(key) {
            self.slots[index] = Some((key, value));
            return;
        }
        if (self.len + 1) * 2 > self.slots.len() {
            let capacity = self.slots.len().saturating_mul(2).max(16);
            let old = core::mem::replace(&mut self.slots, alloc::vec![None; capacity]);
            self.len = 0;
            for (key, value) in old.into_iter().flatten() {
                self.insert(key, value);
            }
        }
        let mask = self.slots.len() - 1;
        let mut index = key.hash() & mask;
        while self.slots[index].is_some() {
            index = (index + 1) & mask;
        }
        self.slots[index] = Some((key, value));
        self.len += 1;
    }
    fn remove(&mut self, key: K) -> Option<usize> {
        let mut hole = self.locate(key)?;
        let (_, value) = self.slots[hole].take().unwrap();
        self.len -= 1;
        let mask = self.slots.len() - 1;
        let mut scan = (hole + 1) & mask;
        while let Some((key, _)) = self.slots[scan] {
            let ideal = key.hash() & mask;
            if (scan.wrapping_sub(ideal) & mask) >= (scan.wrapping_sub(hole) & mask) {
                self.slots[hole] = self.slots[scan].take();
                hole = scan;
            }
            scan = (scan + 1) & mask;
        }
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AllocationState::Active;

    fn reference_slots(allocations: &PhysicalAllocations) -> Vec<Option<((usize, usize), usize)>> {
        allocations
            .references
            .partitions
            .iter()
            .flat_map(|index| index.slots.iter().copied())
            .collect()
    }

    fn reference_count(allocations: &PhysicalAllocations) -> usize {
        allocations
            .references
            .partitions
            .iter()
            .map(|index| index.len)
            .sum()
    }
    #[derive(Clone, Copy, Default, PartialEq, Eq)]
    struct Colliding(usize);
    impl Key for Colliding {
        fn hash(self) -> usize {
            15
        }
    }

    #[test]
    fn collision_clusters_wrap_delete_and_reinsert_without_tombstones() {
        let mut index = Index::<Colliding>::default();
        for key in 0..256 {
            index.insert(Colliding(key), key + 1000);
        }
        for key in (0..256).step_by(3) {
            assert_eq!(index.remove(Colliding(key)), Some(key + 1000));
        }
        for key in 0..256 {
            assert_eq!(
                index.get(Colliding(key)),
                (key % 3 != 0).then_some(key + 1000)
            );
        }
        for key in 0..256 {
            index.insert(Colliding(key), key + 2000);
        }
        for key in (0..256).rev() {
            assert_eq!(index.remove(Colliding(key)), Some(key + 2000));
        }
        assert_eq!(index.len, 0);
        assert!(index.slots.iter().all(Option::is_none));
    }

    #[test]
    fn repeated_pid_and_range_churn_reuses_all_metadata_storage() {
        let mut allocations = PhysicalAllocations::default();
        for iteration in 0..10000 {
            let value = ProcessPhysicalAllocation {
                base_va: 4096,
                start_slot: 1,
                base_page: iteration + 1,
                page_count: 1,
            };
            allocations.insert(Active, iteration + 1, value);
            assert_eq!(allocations.count(value), 1);
            allocations.remove_pid(Active, iteration + 1, |_, last| assert!(last));
            assert_eq!(allocations.nodes.len(), 1);
            assert_eq!(allocations.active.slots.len(), 16);
            assert!(reference_slots(&allocations).len() <= 16 * 16);
            assert_eq!(reference_count(&allocations), 0);
        }
    }

    #[test]
    fn randomized_index_matches_ordered_map() {
        let mut index = Index::<usize>::default();
        let mut reference = alloc::collections::BTreeMap::new();
        let mut seed = 73u64;
        for step in 0..30000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let key = (seed >> 19) as usize % 4096;
            if seed % 3 == 0 {
                assert_eq!(index.remove(key), reference.remove(&key));
            } else {
                index.insert(key, step);
                reference.insert(key, step);
            }
            assert_eq!(index.len, reference.len());
            assert_eq!(index.get(key), reference.get(&key).copied());
        }
        for key in 0..4096 {
            assert_eq!(index.get(key), reference.get(&key).copied());
        }
    }

    fn sample(va: usize, physical: usize, pages: usize) -> ProcessPhysicalAllocation {
        ProcessPhysicalAllocation {
            base_va: va,
            start_slot: physical + 1,
            base_page: physical,
            page_count: pages,
        }
    }

    #[test]
    fn splitting_unique_extent_preserves_every_page_without_releasing_backing() {
        for pages in [1, 2, 512] {
            let mut allocations = PhysicalAllocations::default();
            let original = sample(4096, 100, pages);
            let neighbor = sample(4096 * (pages + 1), 100 + pages, 1);
            allocations.insert(Active, 1, neighbor);
            allocations.insert(Active, 1, original);
            allocations.split_unique_extent(1, original).unwrap();
            let references = allocations.page_references(1, 4096, 4096 * (pages + 1));
            assert_eq!(references.len(), pages);
            for (offset, reference) in references.iter().enumerate() {
                let page = reference.allocation;
                assert_eq!(
                    fields(page),
                    [
                        4096 + offset * 4096,
                        original.start_slot + offset,
                        100 + offset,
                        1
                    ]
                );
                assert_eq!(allocations.count(page), 1);
            }
            assert_eq!(
                allocations.find(Active, 1, neighbor.base_va, 1).map(fields),
                Some(fields(neighbor))
            );
            // A peer's one-page reference prevents source-first final release.
            allocations.insert(Active, 2, references[0].allocation);
            assert!(!allocations.take(Active, 1, 4096, 1).unwrap().1);
            assert!(allocations.take(Active, 2, 4096, 1).unwrap().1);
        }
    }

    #[test]
    fn splitting_rejects_shared_deferred_mismatched_and_overflowed_extents() {
        for invalid in 0..9 {
            let mut allocations = PhysicalAllocations::default();
            let original = sample(4096, 100, 2);
            allocations.insert(Active, 1, original);
            let mut requested = original;
            let mut owner = 1;
            match invalid {
                0 => allocations.insert(Active, 2, original),
                1 => {
                    allocations.defer(1, original.base_va, 2).unwrap();
                }
                2 => owner = 2,
                3 => requested.start_slot += 1,
                4 => requested.base_va += 1,
                5 => requested.page_count = 0,
                6 => requested.page_count = usize::MAX,
                7 => requested.base_page = usize::MAX,
                _ => allocations.insert(Active, 2, sample(8192, 101, 1)),
            }
            let refs = reference_slots(&allocations);
            let nodes = allocations.nodes.len();
            assert!(allocations.split_unique_extent(owner, requested).is_err());
            assert_eq!(reference_slots(&allocations), refs);
            assert_eq!(allocations.nodes.len(), nodes);
            assert_eq!(
                allocations.count(original),
                if invalid == 0 { 2 } else { 1 }
            );
        }
    }

    #[test]
    fn impossible_index_reservation_leaves_existing_references_unchanged() {
        let mut index = Index::<usize>::default();
        index.insert(7, 11);
        let before = index.slots.clone();
        assert!(index.try_reserve(usize::MAX).is_err());
        assert_eq!(index.slots, before);
        assert_eq!(index.len, 1);
    }

    #[test]
    fn deferring_moves_the_existing_node_without_changing_physical_references() {
        let mut allocations = PhysicalAllocations::default();
        let value = sample(4096, 100, 1);
        allocations.insert(Active, 1, value);
        allocations.insert(Active, 2, value);
        let references = reference_slots(&allocations);
        let nodes = allocations.nodes.len();
        let free = allocations.free;

        assert_eq!(
            allocations.defer(1, 4096, 1).map(fields),
            Some(fields(value))
        );
        assert!(allocations.find(Active, 1, 4096, 1).is_none());
        assert_eq!(allocations.count(value), 2);
        assert_eq!(reference_slots(&allocations), references);
        assert_eq!(allocations.nodes.len(), nodes);
        assert_eq!(allocations.free, free);
        assert!(allocations.take_unique_deferred(1, 4096, 1).is_none());
        assert_eq!(reference_slots(&allocations), references);
        assert_eq!(allocations.nodes.len(), nodes);
        assert_eq!(allocations.free, free);

        assert_eq!(
            allocations.take(Active, 2, 4096, 1).map(|v| v.1),
            Some(false)
        );
        assert_eq!(
            allocations.take_unique_deferred(1, 4096, 1).map(fields),
            Some(fields(value))
        );
        assert_eq!(allocations.count(value), 0);
        assert_eq!(reference_count(&allocations), 0);
    }

    #[test]
    fn deferring_all_preserves_existing_tail_and_reference_index() {
        let mut allocations = PhysicalAllocations::default();
        let first = sample(4096, 100, 1);
        let second = sample(8192, 101, 1);
        let tail = sample(12288, 100, 1);
        allocations.insert(Active, 1, tail);
        allocations.defer(1, tail.base_va, 1).unwrap();
        allocations.insert(Active, 1, first);
        allocations.insert(Active, 1, second);
        allocations.insert(Active, 2, first);
        let references = reference_slots(&allocations);
        let nodes = allocations.nodes.len();
        let free = allocations.free;

        for _ in 0..2 {
            allocations.defer_all(1);
            assert!(allocations.for_pid(Active, 1).next().is_none());
            assert_eq!(
                allocations
                    .for_pid(AllocationState::Deferred, 1)
                    .map(fields)
                    .collect::<Vec<_>>(),
                [fields(second), fields(first), fields(tail)]
            );
            assert_eq!(allocations.count(first), 3);
            assert_eq!(allocations.count(second), 1);
            assert_eq!(reference_slots(&allocations), references);
            assert_eq!(allocations.nodes.len(), nodes);
            assert_eq!(allocations.free, free);
        }
        allocations.defer_all(999);
        assert_eq!(reference_slots(&allocations), references);
        allocations.remove_pid(AllocationState::Deferred, 1, |value, last| {
            assert_eq!(last, value.base_page == second.base_page);
        });
        assert_eq!(allocations.count(first), 1);
        assert_eq!(allocations.count(second), 0);
        allocations.remove_pid(Active, 2, |_, last| assert!(last));
        assert_eq!(reference_count(&allocations), 0);
    }

    #[test]
    fn active_and_deferred_churn_share_one_recycled_node_pool() {
        let mut allocations = PhysicalAllocations::default();
        for iteration in 0..10000 {
            let value = sample(4096, iteration + 100, 1);
            allocations.insert(Active, iteration + 1, value);
            allocations.insert(Active, iteration + 2, value);
            assert!(allocations.defer(iteration + 1, 4096, 1).is_some());
            assert!(allocations
                .take_unique_deferred(iteration + 1, 4096, 1)
                .is_none());
            allocations.remove_pid(Active, iteration + 2, |_, last| assert!(!last));
            assert!(allocations
                .take_unique_deferred(iteration + 1, 4096, 1)
                .is_some());
            assert_eq!(allocations.nodes.len(), 2);
            assert_eq!(allocations.active.len, 0);
            assert_eq!(allocations.deferred.len, 0);
            assert_eq!(reference_count(&allocations), 0);
            assert_eq!(allocations.active.slots.len(), 16);
            assert_eq!(allocations.deferred.slots.len(), 16);
            assert!(reference_slots(&allocations).len() <= 16 * 16);
        }
    }

    #[test]
    fn large_reference_sets_do_not_require_a_single_multi_mib_index_allocation() {
        let mut references = ReferenceCounts::default();
        for page in 1..=65537 {
            references.insert((page, 1), 1);
        }
        let slot_bytes = core::mem::size_of::<Option<((usize, usize), usize)>>();
        let total_bytes: usize = references
            .partitions
            .iter()
            .map(|index| index.slots.len() * slot_bytes)
            .sum();
        let largest_bytes = references
            .partitions
            .iter()
            .map(|index| index.slots.len() * slot_bytes)
            .max()
            .unwrap();
        assert_eq!(
            references
                .partitions
                .iter()
                .map(|index| index.len)
                .sum::<usize>(),
            65537
        );
        assert!(total_bytes < 8 * 1024 * 1024);
        assert!(largest_bytes <= 512 * 1024);
        for page in (1..=65537).rev() {
            assert_eq!(references.get((page, 1)), Some(1));
            assert_eq!(references.remove((page, 1)), Some(1));
            assert_eq!(references.get((page, 1)), None);
        }
        assert!(references.partitions.iter().all(|index| index.len == 0));
    }

    fn fields(value: ProcessPhysicalAllocation) -> [usize; 4] {
        [
            value.base_va,
            value.start_slot,
            value.base_page,
            value.page_count,
        ]
    }

    #[test]
    fn mixed_extent_sizes_remain_pid_local_exact_and_newest_first() {
        let mut allocations = PhysicalAllocations::default();
        let one = sample(4096, 100, 1);
        let two = sample(4096, 200, 2);
        let four = sample(4096, 300, 4);
        let newer_four = sample(4096, 400, 4);
        allocations.insert(Active, 1, one);
        allocations.insert(Active, 2, four);
        assert!(allocations.find(Active, 1, 4096, 4).is_none());
        assert!(allocations.take(Active, 1, 4096, 4).is_none());
        assert_eq!(
            allocations.find(Active, 1, 4096, 1).map(fields),
            Some(fields(one))
        );
        allocations.insert(Active, 1, two);
        allocations.insert(Active, 1, four);
        allocations.insert(Active, 1, newer_four);
        assert_eq!(
            allocations.take(Active, 1, 4096, 4).map(|v| fields(v.0)),
            Some(fields(newer_four))
        );
        assert_eq!(
            allocations.take(Active, 1, 4096, 2).map(|v| fields(v.0)),
            Some(fields(two))
        );
        assert_eq!(
            allocations
                .take(Active, 1, 4096, 4)
                .map(|v| (fields(v.0), v.1)),
            Some((fields(four), false))
        );
        assert!(allocations.find(Active, 1, 4096, 4).is_none());
        assert!(allocations.take(Active, 1, 4096, 4).is_none());
        assert_eq!(
            allocations.find(Active, 2, 4096, 4).map(fields),
            Some(fields(four))
        );
        assert_eq!(
            allocations.take(Active, 2, 4096, 4).map(|v| v.1),
            Some(true)
        );
        assert_eq!(
            allocations.take(Active, 1, 4096, 1).map(|v| fields(v.0)),
            Some(fields(one))
        );
        allocations.insert(Active, 1, four);
        assert_eq!(
            allocations.find(Active, 1, 4096, 4).map(fields),
            Some(fields(four))
        );
    }

    #[test]
    fn mixed_size_owner_churn_matches_an_independent_reference_model() {
        let mut allocations = PhysicalAllocations::default();
        let mut model: Vec<(usize, ProcessPhysicalAllocation)> = Vec::new();
        let mut random = 0x123456789abcdefu64;
        for step in 0..20000 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let pid = (random >> 32) as usize % 7;
            let va = ((random >> 24) as usize % 16) * 4096;
            let pages = [0, 1, 2, 5, 16][(random >> 20) as usize % 5];
            match step % 5 {
                0..=2 => {
                    let value = sample(va, (random >> 40) as usize % 32, pages);
                    allocations.insert(Active, pid, value);
                    model.push((pid, value));
                }
                3 => {
                    let expected = model
                        .iter()
                        .rposition(|&(owner, value)| {
                            owner == pid && value.base_va == va && value.page_count == pages
                        })
                        .map(|index| {
                            let (_, value) = model.remove(index);
                            let last = !model.iter().any(|&(_, other)| {
                                other.base_page == value.base_page
                                    && other.page_count == value.page_count
                            });
                            (fields(value), last)
                        });
                    assert_eq!(
                        allocations
                            .take(Active, pid, va, pages)
                            .map(|v| (fields(v.0), v.1)),
                        expected
                    );
                }
                _ => {
                    let mut expected = Vec::new();
                    while let Some(index) = model.iter().rposition(|&(owner, _)| owner == pid) {
                        let (_, value) = model.remove(index);
                        let last = !model.iter().any(|&(_, other)| {
                            other.base_page == value.base_page
                                && other.page_count == value.page_count
                        });
                        expected.push((fields(value), last));
                    }
                    let mut actual = Vec::new();
                    allocations.remove_pid(Active, pid, |value, last| {
                        actual.push((fields(value), last))
                    });
                    assert_eq!(actual, expected);
                }
            }
            for pid in 0..7 {
                let expected: Vec<_> = model
                    .iter()
                    .rev()
                    .filter(|&&(owner, _)| owner == pid)
                    .map(|&(_, value)| fields(value))
                    .collect();
                assert_eq!(
                    allocations
                        .for_pid(Active, pid)
                        .map(fields)
                        .collect::<Vec<_>>(),
                    expected
                );
                for value in expected {
                    let newest = model
                        .iter()
                        .rev()
                        .find(|&&(owner, other)| {
                            owner == pid
                                && other.base_va == value[0]
                                && other.page_count == value[3]
                        })
                        .unwrap()
                        .1;
                    assert_eq!(
                        allocations
                            .find(Active, pid, value[0], value[3])
                            .map(fields),
                        Some(fields(newest))
                    );
                }
            }
        }
    }
}
