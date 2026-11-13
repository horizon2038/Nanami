//! Recycled metadata keeps process/reference index churn and heap traffic bounded.
use super::{PhysicalAllocationLocation, PhysicalPageReference, ProcessPhysicalAllocation};
use alloc::vec::Vec;
use alloc::collections::BTreeMap;

#[derive(Default)]
pub(super) struct PhysicalAllocations {
    active: Index<usize>,
    deferred: Index<usize>,
    references: ReferenceCounts,
    nodes: Vec<Node>,
    // Canonical active node positions ordered by (VA, pages), newest first
    // for equal keys. Empty live-owner rows retain reusable vector capacity.
    exact: BTreeMap<usize, Vec<usize>>,
    free: Option<usize>,
    // Saturation disables located retirement rather than admitting an ABA hit.
    revision: usize,
    #[cfg(test)]
    range_visits: core::cell::Cell<usize>,
    #[cfg(test)]
    point_visits: core::cell::Cell<usize>,
    #[cfg(test)]
    retire_visits: core::cell::Cell<usize>,
    #[cfg(test)]
    index_moves: usize,
}

#[derive(Clone, Copy)]
pub(super) enum AllocationState {
    Active,
    Deferred,
}

#[derive(Clone, Copy)]
enum Lookup { Point, Location, Retirement, Detach }

// Vec node indices cannot reach usize::MAX. Encode index+1 so the empty
// link uses the NonZero niche rather than a second machine-word discriminant.
// This is private metadata, never a capability descriptor or wire value.
#[derive(Clone, Copy)]
struct Link(Option<core::num::NonZeroUsize>);

impl Link {
    fn new(index: Option<usize>) -> Self {
        Self(index.map(|index| {
            core::num::NonZeroUsize::new(index.checked_add(1).expect("node index overflow"))
                .expect("nonzero node link")
        }))
    }

    fn get(self) -> Option<usize> { self.0.map(|index| index.get() - 1) }
}

#[derive(Clone, Copy)]
struct Node {
    allocation: ProcessPhysicalAllocation,
    next: Link,
    previous: Previous,
    // Inclusive bounds of one-page VAs in this active suffix at insertion.
    // Active chains only gain nodes at the head. Unlink/retirement can leave
    // these bounds conservatively wide; backing replacement preserves VA.
    // Deferred-chain links need not preserve bounds and are never queried.
    pages: PageBounds,
}

// Vec<Node> cannot approach isize::MAX elements: its byte allocation is already
// limited to isize::MAX. Reuse the unreachable high bit for active membership.
#[derive(Clone, Copy)]
struct Previous(usize);
impl Previous {
    const ACTIVE: usize = 1usize << (usize::BITS - 1);
    fn new(index: Option<usize>, active: bool) -> Self {
        let link = index.map_or(0, |index| index + 1);
        assert!(link < Self::ACTIVE);
        Self(link | if active { Self::ACTIVE } else { 0 })
    }
    fn get(self) -> Option<usize> {
        (self.0 & !Self::ACTIVE).checked_sub(1)
    }
    fn active(self) -> bool { self.0 & Self::ACTIVE != 0 }
    fn set(&mut self, index: Option<usize>) { *self = Self::new(index, self.active()); }
}

/// Two conservative intervals retain a large gap between an application's
/// low image/stack, growing mmap arena and high interpreter. They summarize
/// addresses only, never the presence, identity or ownership of a page.
#[derive(Clone, Copy)]
struct PageBounds {
    low: usize,
    high: usize,
    upper_low: usize,
    upper_high: usize,
}

impl PageBounds {
    const EMPTY: Self = Self { low: usize::MAX, high: 0, upper_low: usize::MAX, upper_high: 0 };

    fn include(&mut self, va: usize) {
        if self.low > self.high {
            self.low = va; self.high = va;
        } else if self.upper_low > self.upper_high {
            if va < self.low {
                self.upper_low = self.low; self.upper_high = self.high;
                self.low = va; self.high = va;
            } else if va > self.high {
                self.upper_low = va; self.upper_high = va;
            }
        } else if va < self.low {
            if self.low - va > self.upper_low - self.high {
                self.upper_low = self.low;
                self.low = va; self.high = va;
            } else { self.low = va; }
        } else if va > self.upper_high {
            if va - self.upper_high > self.upper_low - self.high {
                self.high = self.upper_high;
                self.upper_low = va; self.upper_high = va;
            } else { self.upper_high = va; }
        } else if va > self.high && va < self.upper_low {
            if va - self.high <= self.upper_low - va { self.high = va; }
            else { self.upper_low = va; }
        }
    }

    fn intersects(self, start: usize, end: usize) -> bool {
        start < end && ((self.low <= self.high && self.low < end && self.high >= start)
            || (self.upper_low <= self.upper_high && self.upper_low < end && self.upper_high >= start))
    }
}

const REFERENCE_PARTITION_BITS: u32 = 4;
const REFERENCE_PARTITIONS: usize = 1 << REFERENCE_PARTITION_BITS;

/// Count active and deferred owners together without resizing the entire index.
/// High hash bits select a partition; its open-addressed index uses low bits.
#[derive(Default)]
struct ReferenceCounts {
    partitions: [Index<(usize, usize)>; REFERENCE_PARTITIONS],
}

// Request-local location: valid only until this reference index is mutated.
struct ReferenceEntry {
    partition: usize,
    slot: usize,
    count: usize,
}

impl ReferenceCounts {
    fn partition(key: (usize, usize)) -> usize {
        key.hash() >> (usize::BITS - REFERENCE_PARTITION_BITS)
    }

    fn get(&self, key: (usize, usize)) -> Option<usize> {
        self.partitions[Self::partition(key)].get(key)
    }

    #[cfg(test)]
    fn insert(&mut self, key: (usize, usize), count: usize) {
        self.partitions[Self::partition(key)].insert(key, count);
    }

    fn increment(&mut self, key: (usize, usize)) {
        self.partitions[Self::partition(key)].update(key, |count| count.unwrap_or(0) + 1);
    }

    fn remove(&mut self, key: (usize, usize)) -> Option<usize> {
        self.partitions[Self::partition(key)].remove(key)
    }

    fn entry(&self, key: (usize, usize)) -> ReferenceEntry {
        let partition = Self::partition(key);
        let index = &self.partitions[partition];
        let slot = index.locate(key).expect("registered physical allocation");
        ReferenceEntry { partition, slot, count: index.slots[slot].unwrap().1 }
    }

    fn retire(&mut self, entry: ReferenceEntry) {
        let index = &mut self.partitions[entry.partition];
        if entry.count == 1 {
            index.remove_at(entry.slot);
        } else {
            index.slots[entry.slot].as_mut().unwrap().1 = entry.count - 1;
        }
    }
}

impl PhysicalAllocations {
    fn advance_revision(&mut self) { self.revision = self.revision.saturating_add(1); }
    fn exact_enabled() -> bool { option_env!("NANAMI_ACTIVE_ALLOCATION_INDEX") != Some("0") }
    fn address_key(allocation: ProcessPhysicalAllocation) -> (usize, usize) {
        (allocation.base_va, allocation.page_count)
    }

    fn index_new_nodes(&mut self, pid: usize, first: usize, count: usize) {
        if !Self::exact_enabled() { return; }
        let rows = self.exact.entry(pid).or_default();
        let nodes = &self.nodes;
        let key = |index: usize| Self::address_key(nodes[index].allocation);
        let first_key = key(first);
        let mut last_key = first_key;
        let mut increasing = true;
        let mut current = first;
        for _ in 1..count {
            current = nodes[current].previous.get().unwrap();
            let next_key = key(current);
            increasing &= last_key < next_key;
            last_key = next_key;
        }
        let position = rows.partition_point(|&index| key(index) < first_key);
        if count == 1 {
            #[cfg(test)]
            { self.index_moves += rows.len() - position; }
            rows.insert(position, first);
        } else if increasing && rows.get(position).is_none_or(|&index| key(index) > last_key) {
            // Ordinary increasing, disjoint construction shifts the old suffix
            // once. No sort scratch or per-page insertion movement is needed.
            let old_len = rows.len();
            #[cfg(test)]
            { self.index_moves += old_len - position; }
            rows.resize(old_len + count, 0);
            rows.copy_within(position..old_len, position + count);
            current = first;
            for offset in 0..count {
                rows[position + offset] = current;
                if offset + 1 != count { current = nodes[current].previous.get().unwrap(); }
            }
        } else {
            // General insertion preserves newest-first ties, including reused
            // node indices and mixed page counts at the same virtual address.
            current = first;
            for offset in 0..count {
                let position = rows.partition_point(|&index| key(index) < key(current));
                #[cfg(test)]
                { self.index_moves += rows.len() - position; }
                rows.insert(position, current);
                if offset + 1 != count { current = nodes[current].previous.get().unwrap(); }
            }
        }
    }

    fn remove_index_node(&mut self, pid: usize, index: usize) {
        if !Self::exact_enabled() { return; }
        let key = Self::address_key(self.nodes[index].allocation);
        let rows = self.exact.get_mut(&pid).expect("active ownership index");
        let start = rows.partition_point(|&row| Self::address_key(self.nodes[row].allocation) < key);
        let offset = rows[start..].iter().take_while(|&&row|
            Self::address_key(self.nodes[row].allocation) == key).position(|&row| row == index)
            .expect("indexed active allocation");
        rows.remove(start + offset);
    }

    fn compact_index(&mut self, pid: usize) {
        if let Some(rows) = self.exact.get_mut(&pid) {
            rows.retain(|&index| self.nodes[index].previous.active());
        }
    }

    fn unlink_node(&mut self, state: AllocationState, pid: usize, index: usize, previous: Option<usize>) {
        let next = self.nodes[index].next;
        if let Some(previous) = previous { self.nodes[previous].next = next; }
        else if let Some(head) = next.get() { self.heads_mut(state).insert(pid, head); }
        else { self.heads_mut(state).remove(pid); }
        if let Some(next) = next.get() { self.nodes[next].previous.set(previous); }
        if matches!(state, AllocationState::Active) { self.remove_index_node(pid, index); }
        self.nodes[index].previous = Previous::new(None, false);
    }

    fn find_node(&self, state: AllocationState, pid: usize, base: usize, pages: usize,
        kind: Lookup) -> Option<(usize, Option<usize>)> {
        let indexed = matches!(state, AllocationState::Active) && Self::exact_enabled();
        let mut probes = 0;
        let result = if indexed {
            self.exact.get(&pid).and_then(|rows| {
                let key = (base, pages);
                let position = rows.partition_point(|&index| {
                    probes += 1;
                    Self::address_key(self.nodes[index].allocation) < key
                });
                rows.get(position).and_then(|&index| {
                    probes += 1;
                    let node = &self.nodes[index];
                    (node.previous.active() && Self::address_key(node.allocation) == key)
                        .then_some((index, node.previous.get()))
                })
            })
        } else {
            let mut next = self.heads(state).get(pid);
            let mut previous = None;
            let mut found = None;
            while let Some(index) = next {
                let node = self.nodes[index]; probes += 1;
                if node.allocation.base_va == base && node.allocation.page_count == pages {
                    found = Some((index, previous)); break;
                }
                previous = Some(index); next = node.next.get();
            }
            found
        };
        #[cfg(test)]
        if !matches!(kind, Lookup::Detach) {
            let counter = if matches!(kind, Lookup::Retirement) { &self.retire_visits } else { &self.point_visits };
            counter.set(counter.get() + probes);
        }
        #[cfg(not(test))]
        if option_env!("NANAMI_ACTIVE_ALLOCATION_INDEX_DIAG") == Some("1") {
            let (rows, capacity) = self.exact.get(&pid).map_or((0,0), |rows| (rows.len(), rows.capacity()));
            let name = match kind { Lookup::Point => "point", Lookup::Location => "locate",
                Lookup::Retirement => "retire", Lookup::Detach => "detach" };
            let index_capacity: usize = self.exact.values().map(|rows| rows.capacity()).sum();
            crate::force_info!("[alloc.lookup] owner={} active={} kind={} va={:#x} pages={} index={} probes={} rows={} capacity={} node_capacity={} index_capacity={}",
                pid, usize::from(matches!(state,AllocationState::Active)), name, base, pages,
                usize::from(indexed), probes, rows, capacity, self.nodes.capacity(), index_capacity);
        }
        let _ = (probes, kind);
        result
    }
    #[cfg(test)]
    pub(super) fn take_range_visits(&self) -> usize { self.range_visits.replace(0) }

    #[cfg(test)]
    pub(super) fn take_point_visits(&self) -> usize { self.point_visits.replace(0) }
    #[cfg(test)]
    pub(super) fn take_retire_visits(&self) -> usize { self.retire_visits.replace(0) }

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
        if Self::exact_enabled() {
            self.exact.get_mut(&pid).ok_or(())?.try_reserve(pages - 1).map_err(|_| ())?;
        }
        // The existing active PID index already has capacity for this owner.
        // Request-local PhysicalPageReference handles must not survive this
        // rewrite; callers collect their page references after canonicalizing.
        self.take(AllocationState::Active, pid, allocation.base_va, pages)
            .ok_or(())?;
        let nodes = &mut self.nodes;
        let free = &mut self.free;
        let references = &mut self.references;
        // All node/reference capacity was reserved before removing the extent.
        // Build the same newest-first chain without re-searching/publishing its
        // PID head for every page. No fallible work or external callback follows.
        let mut first = None;
        self.active.update(pid, |mut head| {
            for offset in 0..pages {
                let page = ProcessPhysicalAllocation {
                    base_va: allocation.base_va + offset * super::USER_PAGE_SIZE,
                    start_slot: allocation.start_slot + offset,
                    base_page: allocation.base_page + offset,
                    page_count: 1,
                    owns_frame_cap: allocation.owns_frame_cap,
                };
                references.increment(Self::key(page));
                let index = Self::push_node(nodes, free, AllocationState::Active, page, head);
                if first.is_none() { first = Some(index); }
                head = Some(index);
            }
            head.unwrap()
        });
        self.index_new_nodes(pid, first.unwrap(), pages);
        Ok(())
    }

    /// Move this owner's complete active chain without changing references or
    /// allocating replacement records. Deferred tails may already own aliases.
    pub(super) fn defer_all(&mut self, pid: usize) {
        self.exact.remove(&pid);
        let Some(head) = self.active.remove(pid) else {
            return;
        };
        self.advance_revision();
        let mut tail = head;
        self.nodes[tail].previous = Previous::new(None, false);
        while let Some(next) = self.nodes[tail].next.get() {
            self.nodes[next].previous = Previous::new(Some(tail), false);
            tail = next;
        }
        let deferred = self.deferred.get(pid);
        self.nodes[tail].next = Link::new(deferred);
        if let Some(deferred) = deferred { self.nodes[deferred].previous.set(Some(tail)); }
        self.deferred.insert(pid, head);
    }

    pub(super) fn page_references(
        &self,
        pid: usize,
        start: usize,
        end: usize,
    ) -> Vec<PhysicalPageReference> {
        self.page_references_reserved(pid, start, end, 0)
    }

    pub(super) fn page_references_reserved(
        &self, pid: usize, start: usize, end: usize, capacity: usize,
    ) -> Vec<PhysicalPageReference> {
        let mut references = Vec::new();
        // A failed optimization reservation retains the original incremental
        // growth path. The hint is neither a row limit nor ownership proof.
        let _ = references.try_reserve_exact(capacity);
        let mut descending = true;
        let mut previous = None;
        for (index, allocation) in self.page_nodes_in_range(pid, start, end) {
            if previous.is_some_and(|va| va <= allocation.base_va) { descending = false; }
            previous = Some(allocation.base_va);
            references.push(PhysicalPageReference { allocation, owner: pid, index });
        }
        // Strict order proves unique VAs. Duplicates must retain the original
        // stable sort so their newest chain entry continues to win dedup.
        if descending { references.reverse(); }
        else { references.sort_by_key(|reference| reference.allocation.base_va); }
        references.dedup_by_key(|reference| reference.allocation.base_va);
        references
    }

    /// Newest-first active records, borrowed only for this metadata walk.
    /// Consumers must not infer VA order or uniqueness from chain order.
    pub(super) fn page_reference_walk(
        &self, pid: usize, start: usize, end: usize,
    ) -> impl Iterator<Item = PhysicalPageReference> + '_ {
        self.page_nodes_in_range(pid, start, end)
            .map(move |(index, allocation)| PhysicalPageReference { allocation, owner: pid, index })
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
        self.advance_revision();
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
        self.insert_many(state, pid, core::iter::once(allocation));
    }

    pub(super) fn insert_many(
        &mut self,
        state: AllocationState,
        pid: usize,
        mut allocations: impl Iterator<Item = ProcessPhysicalAllocation>,
    ) {
        let Some(first) = allocations.next() else { return; };
        self.advance_revision();
        let heads = match state {
            AllocationState::Active => &mut self.active,
            AllocationState::Deferred => &mut self.deferred,
        };
        let references = &mut self.references;
        let nodes = &mut self.nodes;
        let free = &mut self.free;
        // The head remains borrowed while the disjoint references and nodes
        // change. Preserve per-page reference counts and prepend order.
        let mut first_index = 0;
        let mut count = 1;
        heads.update(pid, |previous| {
            references.increment(Self::key(first));
            let mut head = Self::push_node(nodes, free, state, first, previous);
            first_index = head;
            for allocation in allocations {
                references.increment(Self::key(allocation));
                head = Self::push_node(nodes, free, state, allocation, Some(head));
                count += 1;
            }
            head
        });
        if matches!(state, AllocationState::Active) { self.index_new_nodes(pid, first_index, count); }
    }

    fn push_node(
        nodes: &mut Vec<Node>, free: &mut Option<usize>, state: AllocationState,
        allocation: ProcessPhysicalAllocation, previous: Option<usize>,
    ) -> usize {
        let mut node = Node { allocation, next: Link::new(previous),
            previous: Previous::new(None, matches!(state, AllocationState::Active)), pages: PageBounds::EMPTY };
        if matches!(state, AllocationState::Active) {
            if let Some(next) = previous { node.pages = nodes[next].pages; }
            if allocation.page_count == 1 { node.pages.include(allocation.base_va); }
        }
        let index = if let Some(index) = *free {
            *free = nodes[index].next.get();
            nodes[index] = node;
            index
        } else {
            nodes.push(node);
            nodes.len() - 1
        };
        if let Some(previous) = previous { nodes[previous].previous.set(Some(index)); }
        index
    }

    pub(super) fn for_pid(
        &self,
        state: AllocationState,
        pid: usize,
    ) -> impl Iterator<Item = ProcessPhysicalAllocation> + '_ {
        let mut next = self.heads(state).get(pid);
        core::iter::from_fn(move || {
            let node = self.nodes[next?];
            next = node.next.get();
            Some(node.allocation)
        })
    }

    pub(super) fn count(&self, allocation: ProcessPhysicalAllocation) -> usize {
        self.references.get(Self::key(allocation)).unwrap_or(0)
    }

    fn page_nodes_in_range(
        &self, pid: usize, start: usize, end: usize,
    ) -> impl Iterator<Item = (usize, ProcessPhysicalAllocation)> + '_ {
        let mut next = if start < end { self.active.get(pid) } else { None };
        core::iter::from_fn(move || loop {
            let index = next?;
            let node = &self.nodes[index];
            #[cfg(test)]
            self.range_visits.set(self.range_visits.get() + 1);
            if !node.pages.intersects(start, end) {
                next = None;
                return None;
            }
            next = node.next.get();
            let allocation = node.allocation;
            if allocation.page_count == 1 && allocation.base_va >= start && allocation.base_va < end {
                return Some((index, allocation));
            }
        })
    }

    pub(super) fn pages_in_range(
        &self, pid: usize, start: usize, end: usize,
    ) -> impl Iterator<Item = ProcessPhysicalAllocation> + '_ {
        self.page_nodes_in_range(pid, start, end).map(|(_, allocation)| allocation)
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
            // Every alias already had count != 1 in the immutable pass above.
            // A singleton therefore cannot contain all remaining references;
            // only multi-alias groups need another exact count lookup.
            if group.len() > 1 && group.len() == self.count(group[0]) {
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
        self.find_node(state, pid, base_va, page_count, Lookup::Point)
            .map(|(index, _)| self.nodes[index].allocation)
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

    pub(super) fn locate_active(
        &self, pid: usize, base: usize, pages: usize,
    ) -> Option<PhysicalAllocationLocation> {
        let (index, previous) = self.find_node(AllocationState::Active, pid, base, pages, Lookup::Location)?;
        Some(PhysicalAllocationLocation { allocation: self.nodes[index].allocation,
            owner: pid, index, previous, revision: self.revision })
    }

    pub(super) fn try_take_located<E>(
        &mut self, location: PhysicalAllocationLocation,
        release: impl FnOnce(ProcessPhysicalAllocation, bool) -> Result<(), E>,
    ) -> Result<bool, E> {
        if self.revision == usize::MAX || self.revision != location.revision { return Ok(false); }
        let Some(node) = self.nodes.get(location.index).copied() else { return Ok(false); };
        let linked = if let Some(previous) = location.previous {
            self.nodes.get(previous).is_some_and(|node| node.next.get() == Some(location.index))
        } else { self.active.get(location.owner) == Some(location.index) };
        if !linked || !node.previous.active() || node.allocation != location.allocation { return Ok(false); }
        let reference = self.references.entry(Self::key(node.allocation));
        release(node.allocation, reference.count == 1)?;
        self.advance_revision();
        self.unlink_node(AllocationState::Active, location.owner, location.index, location.previous);
        self.recycle_reference(location.index, reference);
        Ok(true)
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

    /// Keep both list position and RAM reference until release succeeds.
    pub(super) fn try_take<E>(
        &mut self, state: AllocationState, pid: usize, base: usize, pages: usize,
        release: impl FnOnce(ProcessPhysicalAllocation, bool) -> Result<(), E>,
    ) -> Result<Option<ProcessPhysicalAllocation>, E> {
        let Some((index, previous)) = self.find_node(state, pid, base, pages, Lookup::Retirement) else { return Ok(None); };
        let allocation = self.nodes[index].allocation;
        let reference = self.references.entry(Self::key(allocation));
        release(allocation, reference.count == 1)?;
        self.advance_revision();
        self.unlink_node(state, pid, index, previous);
        self.recycle_reference(index, reference);
        Ok(Some(allocation))
    }

    /// Change list ownership without dropping or re-adding the RAM reference.
    pub(super) fn defer(
        &mut self,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) -> Option<ProcessPhysicalAllocation> {
        let index = self.detach(AllocationState::Active, pid, base_va, page_count, false)?;
        let next = self.deferred.get(pid);
        self.nodes[index].next = Link::new(next);
        self.nodes[index].previous = Previous::new(None, false);
        if let Some(next) = next { self.nodes[next].previous.set(Some(index)); }
        self.deferred.insert(pid, index);
        Some(self.nodes[index].allocation)
    }

    /// Preserve reverse-snapshot deferral order without searching the same
    /// newer prefix once per page. Decline before mutation if live list order
    /// or any allocation field differs. References never leave the index.
    pub(super) fn try_defer_pages(
        &mut self, pid: usize, start: usize, end: usize,
        pages: &[ProcessPhysicalAllocation],
    ) -> bool {
        if pages.is_empty() { return true; }
        if !self.page_prefix_matches(pid, start, end, pages) { return false; }
        let selected = |allocation| Self::page_in_range(allocation, start, end);
        // The only potentially new PID-index entry is reserved before edits.
        if self.deferred.get(pid).is_none() && self.deferred.try_reserve(1).is_err() {
            return false;
        }
        self.advance_revision();
        let mut head = self.active.get(pid);
        let mut next = head;
        let mut previous: Option<usize> = None;
        let mut deferred = self.deferred.get(pid);
        let mut remaining = pages.len();
        while let Some(index) = next {
            let node = self.nodes[index];
            if selected(node.allocation) {
                if let Some(previous) = previous {
                    self.nodes[previous].next = Link::new(node.next.get());
                } else {
                    head = node.next.get();
                }
                if let Some(next) = node.next.get() { self.nodes[next].previous.set(previous); }
                self.nodes[index].next = Link::new(deferred);
                self.nodes[index].previous = Previous::new(None, false);
                if let Some(deferred) = deferred { self.nodes[deferred].previous.set(Some(index)); }
                deferred = Some(index);
                remaining -= 1;
                if remaining == 0 { break; }
            } else {
                previous = Some(index);
            }
            next = node.next.get();
        }
        if let Some(head) = head {
            self.active.insert(pid, head);
        } else {
            self.active.remove(pid);
        }
        self.deferred.insert(pid, deferred.unwrap());
        self.compact_index(pid);
        true
    }

    fn page_in_range(allocation: ProcessPhysicalAllocation, start: usize, end: usize) -> bool {
        allocation.page_count == 1 && allocation.base_va >= start
            && allocation.base_va < end
            && (allocation.base_va - start) % super::USER_PAGE_SIZE == 0
    }

    fn page_prefix_matches(
        &self, pid: usize, start: usize, end: usize,
        pages: &[ProcessPhysicalAllocation],
    ) -> bool {
        if pages.is_empty() { return true; }
        let mut remaining = pages.len();
        let mut next = self.active.get(pid);
        while let Some(index) = next {
            let node = self.nodes[index];
            if Self::page_in_range(node.allocation, start, end) {
                if node.allocation != pages[remaining - 1] { return false; }
                remaining -= 1;
                if remaining == 0 { break; }
            }
            next = node.next.get();
        }
        remaining == 0
    }

    /// Invoke fallible release before unlinking each reference. Failure keeps
    /// that record and suffix live; only the successful scalar-order prefix is
    /// recycled. A declined preflight invokes no callbacks and changes nothing.
    pub(super) fn try_retire_pages<E>(
        &mut self, pid: usize, start: usize, end: usize,
        pages: &[ProcessPhysicalAllocation],
        mut release: impl FnMut(ProcessPhysicalAllocation, bool) -> Result<(), E>,
    ) -> Result<bool, E> {
        if pages.is_empty() { return Ok(true); }
        if !self.page_prefix_matches(pid, start, end, pages) { return Ok(false); }
        let Some(head_slot) = self.active.locate(pid) else { return Ok(false); };
        let mut next = Some(self.active.slots[head_slot].unwrap().1);
        // Only this PID's head value changes during the walk. Nodes and RAM
        // references have separate indexes; no head-table resize can occur.
        // Removing the final node ends the walk, so its slot is never reused
        // after backward-shift deletion relocates other PIDs.
        let mut previous: Option<usize> = None;
        let mut remaining = pages.len();
        let result = (|| {
        while let Some(index) = next {
            let node = self.nodes[index];
            if Self::page_in_range(node.allocation, start, end) {
                let reference = self.references.entry(Self::key(node.allocation));
                release(node.allocation, reference.count == 1)?;
                self.advance_revision();
                if let Some(previous) = previous {
                    self.nodes[previous].next = Link::new(node.next.get());
                } else if let Some(head) = node.next.get() {
                    self.active.slots[head_slot].as_mut().unwrap().1 = head;
                } else {
                    self.active.remove_at(head_slot);
                }
                if let Some(next) = node.next.get() { self.nodes[next].previous.set(previous); }
                self.recycle_reference(index, reference);
                remaining -= 1;
                if remaining == 0 { break; }
            } else {
                previous = Some(index);
            }
            next = node.next.get();
        }
        Ok(true)
        })();
        self.compact_index(pid);
        result
    }

    fn detach(
        &mut self,
        state: AllocationState,
        pid: usize,
        base_va: usize,
        page_count: usize,
        require_unique: bool,
    ) -> Option<usize> {
        let (index, previous) = self.find_node(state, pid, base_va, page_count, Lookup::Detach)?;
        if require_unique && self.count(self.nodes[index].allocation) != 1 { return None; }
        self.advance_revision();
        self.unlink_node(state, pid, index, previous);
        Some(index)
    }

    pub(super) fn remove_pid(
        &mut self,
        state: AllocationState,
        pid: usize,
        mut removed: impl FnMut(ProcessPhysicalAllocation, bool),
    ) {
        if matches!(state, AllocationState::Active) { self.exact.remove(&pid); }
        let mut next = self.heads_mut(state).remove(pid);
        if next.is_some() { self.advance_revision(); }
        while let Some(index) = next {
            next = self.nodes[index].next.get();
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
        if next.is_some() { self.advance_revision(); }
        let result = (|| {
        while let Some(index) = next {
            let node = self.nodes[index];
            self.nodes[index].previous.set(None);
            let reference = self.references.entry(Self::key(node.allocation));
            if let Err(error) = release(node.allocation, reference.count == 1) {
                self.heads_mut(state).insert(pid, index);
                return Err(error);
            }
            next = node.next.get();
            if let Some(next) = next { self.nodes[next].previous.set(None); }
            self.recycle_reference(index, reference);
        }
        Ok(())
        })();
        if matches!(state, AllocationState::Active) {
            self.compact_index(pid);
            if self.active.get(pid).is_none() { self.exact.remove(&pid); }
        }
        result
    }

    fn recycle(&mut self, index: usize) -> (ProcessPhysicalAllocation, bool) {
        let reference = self.references.entry(Self::key(self.nodes[index].allocation));
        self.recycle_reference(index, reference)
    }

    // The release callback cannot access this exclusively borrowed index. Reuse
    // its pre-release location/count, but commit only after release succeeds.
    // PID head updates touch separate indexes. Each next alias obtains a fresh
    // location/count after the preceding deletion and any backward shifts.
    fn recycle_reference(&mut self, index: usize, reference: ReferenceEntry) -> (ProcessPhysicalAllocation, bool) {
        let allocation = self.nodes[index].allocation;
        let last = reference.count == 1;
        self.references.retire(reference);
        self.nodes[index].next = Link::new(self.free);
        self.nodes[index].previous = Previous::new(None, false);
        self.free = Some(index);
        (allocation, last)
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
#[cfg(test)]
static INDEX_SLOT_READS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

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
        self.locate_or_vacant(key).ok()
    }

    // Return the first empty slot together with a miss. Until mutation, it is
    // also the insertion location; no second walk through a collision cluster.
    fn locate_or_vacant(&self, key: K) -> Result<usize, usize> {
        if self.slots.is_empty() {
            return Err(0);
        }
        let mask = self.slots.len() - 1;
        let mut index = key.hash() & mask;
        loop {
            #[cfg(test)]
            INDEX_SLOT_READS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            match self.slots[index] {
                Some((found, _)) if found == key => return Ok(index),
                Some(_) => index = (index + 1) & mask,
                None => return Err(index),
            }
        }
    }
    fn get(&self, key: K) -> Option<usize> {
        self.locate(key).map(|index| self.slots[index].unwrap().1)
    }
    fn insert(&mut self, key: K, value: usize) {
        self.update(key, |_| value);
    }

    // Resolve once for read-modify-write as well as plain replacement. The
    // callback runs before any resize, preserving the original value/error
    // ordering. No slot reference survives a table mutation.
    fn update(&mut self, key: K, update: impl FnOnce(Option<usize>) -> usize) {
        let vacant = match self.locate_or_vacant(key) {
            Ok(index) => {
                let value = update(Some(self.slots[index].unwrap().1));
                self.slots[index] = Some((key, value));
                return;
            }
            Err(index) => index,
        };
        let value = update(None);
        let index = if (self.len + 1) * 2 > self.slots.len() {
            let capacity = self.slots.len().saturating_mul(2).max(16);
            let old = core::mem::replace(&mut self.slots, alloc::vec![None; capacity]);
            self.len = 0;
            for (key, value) in old.into_iter().flatten() {
                self.insert(key, value);
            }
            // Growth changes every slot's location; re-resolve only then.
            self.locate_or_vacant(key).expect_err("new index key after growth")
        } else { vacant };
        self.slots[index] = Some((key, value));
        self.len += 1;
    }
    fn remove(&mut self, key: K) -> Option<usize> {
        let hole = self.locate(key)?;
        Some(self.remove_at(hole))
    }

    fn remove_at(&mut self, mut hole: usize) -> usize {
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
        value
    }
}

#[cfg(test)]
mod tests {
    fn consistent(a: &super::PhysicalAllocations) {
        use super::*;
        let mut seen = alloc::vec![false; a.nodes.len()];
        for (state, heads) in [(AllocationState::Active, &a.active), (AllocationState::Deferred, &a.deferred)] {
            for &(pid, head) in heads.slots.iter().flatten() {
                let mut next = Some(head); let mut previous = None; let mut expected = Vec::new();
                while let Some(index) = next {
                    assert!(!seen[index], "duplicate/cyclic node"); seen[index] = true;
                    let node = a.nodes[index];
                    assert_eq!(node.previous.get(), previous);
                    assert_eq!(node.previous.active(), matches!(state, AllocationState::Active));
                    expected.push(index); previous = Some(index); next = node.next.get();
                }
                if matches!(state, AllocationState::Active) && PhysicalAllocations::exact_enabled() {
                    expected.sort_by_key(|&index| PhysicalAllocations::address_key(a.nodes[index].allocation));
                    assert_eq!(a.exact.get(&pid), Some(&expected));
                    for &index in &expected {
                        let row = a.nodes[index].allocation;
                        let oracle = a.for_pid(state, pid).find(|p| p.base_va == row.base_va && p.page_count == row.page_count);
                        assert!(a.find(state, pid, row.base_va, row.page_count) == oracle);
                    }
                }
            }
        }
        for (&pid, rows) in &a.exact {
            if a.active.get(pid).is_none() { assert!(rows.is_empty()); }
        }
        if !PhysicalAllocations::exact_enabled() { assert!(a.exact.is_empty()); }
        let mut next = a.free;
        while let Some(index) = next {
            assert!(!seen[index]); seen[index] = true;
            assert!(!a.nodes[index].previous.active()); assert_eq!(a.nodes[index].previous.get(), None);
            next = a.nodes[index].next.get();
        }
        assert!(seen.iter().all(|&seen| seen), "orphaned node");
        let mut counts = BTreeMap::new();
        for heads in [&a.active, &a.deferred] {
            for &(_, head) in heads.slots.iter().flatten() {
                let mut next = Some(head);
                while let Some(index) = next {
                    *counts.entry(PhysicalAllocations::key(a.nodes[index].allocation)).or_insert(0usize) += 1;
                    next = a.nodes[index].next.get();
                }
            }
        }
        for (key, count) in counts { assert_eq!(a.references.get(key), Some(count)); }
    }

    #[test]
    fn active_index_matches_primary_chains_through_mutation_and_failure_corpus() {
        use super::*;
        let mut a = PhysicalAllocations::default(); let mut seed = 0x1512_4abc_u64;
        for step in 0..1200 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let pid = (seed as usize >> 8) % 7 + 1;
            let va = ((seed as usize >> 16) % 64 + 1) * 4096;
            let rows: Vec<_> = a.for_pid(AllocationState::Active, pid).collect();
            let chosen = rows.get((seed as usize >> 24) % rows.len().max(1)).copied();
            match step % 16 {
                0..=2 => a.insert(AllocationState::Active, pid, sample(va, (seed as usize >> 32) % 64 + 1, step % 3 + 1)),
                3 => { if let Some(row) = chosen { a.defer(pid, row.base_va, row.page_count).unwrap(); } },
                4 => { if let Some(row) = chosen { a.take(AllocationState::Active, pid, row.base_va, row.page_count).unwrap(); } },
                5 => { if let Some(row) = chosen {
                    let expected = rows.iter().copied().find(|p| p.base_va == row.base_va && p.page_count == row.page_count).unwrap();
                    let result = a.try_take(AllocationState::Active, pid, row.base_va, row.page_count, |actual, _| {
                        assert!(actual == expected); if step % 7 == 0 { Err(9) } else { Ok(()) }
                    });
                    assert_eq!(result.is_err(), step % 7 == 0);
                } },
                6 => { let deferred: Vec<_> = a.for_pid(AllocationState::Deferred, pid).collect();
                    if let Some(&row) = deferred.first() {
                        let unique = a.count(row) == 1;
                        assert_eq!(a.take_unique_deferred(pid, row.base_va, row.page_count).is_some(), unique);
                    }
                },
                7 => a.defer_all(pid),
                8 => a.remove_pid(AllocationState::Active, pid, |_, _| {}),
                9 => { let before: Vec<_> = a.for_pid(AllocationState::Active, pid).collect();
                    let fail = step % 5; let mut calls = 0;
                    let result = a.try_remove_pid(AllocationState::Active, pid, |_, _| {
                        calls += 1; if calls > fail { Err(8) } else { Ok(()) }
                    });
                    assert_eq!(result.is_err(), before.len() > fail);
                    assert!(a.for_pid(AllocationState::Active, pid).eq(before.into_iter().skip(fail)));
                },
                10 | 11 => {
                    let base = 0x1000_0000 + step * 16 * 4096;
                    let pages: Vec<_> = (0..8).map(|i| sample(base + i * 4096, 100000 + step * 16 + i, 1)).collect();
                    a.insert_many(AllocationState::Active, pid, pages.iter().copied());
                    consistent(&a);
                    if step % 16 == 10 { assert!(a.try_defer_pages(pid, base, base + 8 * 4096, &pages)); }
                    else { let mut calls = 0;
                        assert_eq!(a.try_retire_pages(pid, base, base + 8 * 4096, &pages, |_, _| {
                            calls += 1; if calls == 5 { Err(7) } else { Ok(()) }
                        }), Err(7));
                    }
                },
                12 => { let extent = sample(0x2000_0000 + step * 16 * 4096, 1000000 + step * 16, 4);
                    a.insert(AllocationState::Active, pid, extent); a.split_unique_extent(pid, extent).unwrap();
                },
                13 => { if let Some(row) = rows.iter().find(|p| p.page_count == 1) {
                    let reference = a.page_references(pid, row.base_va, row.base_va + 4096)[0];
                    let mut replacement = reference.allocation; replacement.base_page = 10000000 + step;
                    a.insert(AllocationState::Deferred, pid, replacement);
                    assert!(a.replace_backing(reference, replacement).is_some());
                } },
                14 => { if let Some(row) = chosen {
                    let location = a.locate_active(pid, row.base_va, row.page_count).unwrap();
                    if step % 3 == 0 { a.insert(AllocationState::Active, 99, sample(4096, 20000000 + step, 1)); }
                    assert_eq!(a.try_take_located(location, |_, _| Ok::<(), ()>(())).unwrap(), step % 3 != 0);
                } },
                _ => { assert!(a.find(AllocationState::Active, pid, 0xffff_0000, 1).is_none()); },
            }
            consistent(&a);
        }
        for pid in (1..=7).chain([99]) {
            a.remove_pid(AllocationState::Active, pid, |_, _| {});
            a.remove_pid(AllocationState::Deferred, pid, |_, _| {});
        }
        consistent(&a); assert!(a.exact.is_empty());
    }

    #[test]
    fn active_index_bulk_insertion_moves_the_old_suffix_once_and_preserves_duplicate_order() {
        use super::*;
        let mut batch = PhysicalAllocations::default(); let mut scalar = PhysicalAllocations::default();
        for a in [&mut batch, &mut scalar] {
            for page in 0..512 { a.insert(AllocationState::Active, 1, sample(0x4000000 + page * 4096, 10000 + page, 1)); }
            a.index_moves = 0;
        }
        let pages: Vec<_> = (0..1024).map(|i| sample((i + 1) * 4096, i, 1)).collect();
        batch.insert_many(AllocationState::Active, 1, pages.iter().copied());
        for page in pages { scalar.insert(AllocationState::Active, 1, page); }
        if PhysicalAllocations::exact_enabled() {
            assert_eq!((batch.index_moves, scalar.index_moves), (512, 512 * 1024));
        }
        assert!(batch.for_pid(AllocationState::Active, 1).eq(scalar.for_pid(AllocationState::Active, 1)));
        let mixed = [sample(4096,90000,2), sample(4096,90001,1), sample(8192,90002,1), sample(4096,90003,1)];
        batch.insert_many(AllocationState::Active, 1, mixed.into_iter());
        assert!(batch.find(AllocationState::Active,1,4096,1) == Some(mixed[3]));
        assert!(batch.find(AllocationState::Active,1,4096,2) == Some(mixed[0]));
        consistent(&batch); consistent(&scalar);
    }

    #[test]
    fn active_index_reuses_live_owner_capacity_and_releases_it_on_owner_removal() {
        use super::*;
        let mut a = PhysicalAllocations::default();
        let pages: Vec<_> = (0..512).map(|i| sample((i + 1) * 4096, 50000 + i, 1)).collect();
        a.insert_many(AllocationState::Active,1,pages.iter().copied());
        let capacity = a.exact.get(&1).map(|rows| rows.capacity());
        let node_capacity = a.nodes.capacity();
        for _ in 0..64 {
            assert!(a.try_retire_pages(1,4096,513*4096,&pages,|_,_|Ok::<(),()>(())).unwrap());
            a.insert_many(AllocationState::Active,1,pages.iter().copied());
            assert_eq!(a.exact.get(&1).map(|rows| rows.capacity()),capacity);
            assert_eq!(a.nodes.capacity(),node_capacity); consistent(&a);
        }
        a.defer_all(1); assert!(a.exact.is_empty()); consistent(&a);
        a.remove_pid(AllocationState::Deferred,1,|_,_|{}); consistent(&a);
        assert_eq!(core::mem::size_of::<Node>(),88);
    }

    #[test]
    fn located_retirement_matches_scalar_with_one_ownership_walk_and_no_rewalk() {
        use super::*;
        fn fixture(shared: bool) -> PhysicalAllocations {
            let mut a = PhysicalAllocations::default();
            for page in 1..=1024 { a.insert(AllocationState::Active, 1, sample(page * 4096, page, 1)); }
            if shared { a.insert(AllocationState::Active, 2, sample(4096, 1, 1)); }
            a
        }
        for shared in [false, true] {
            let mut old = fixture(shared); let mut new = fixture(shared);
            let old_allocation = old.find(AllocationState::Active, 1, 4096, 1).unwrap();
            let mut old_last = None;
            assert!(old.try_take(AllocationState::Active, 1, 4096, 1, |allocation, last| {
                assert!(allocation == old_allocation); old_last = Some(last); Ok::<(), ()>(())
            }).unwrap().is_some());
            let location = new.locate_active(1, 4096, 1).unwrap();
            let mut new_last = None;
            assert!(new.try_take_located(location, |allocation, last| {
                assert!(allocation == old_allocation); new_last = Some(last); Ok::<(), ()>(())
            }).unwrap());
            assert_eq!(old_last, Some(!shared)); assert_eq!(new_last, old_last);
            let (points, retires) = (old.take_point_visits(), old.take_retire_visits());
            if PhysicalAllocations::exact_enabled() { assert!(points <= 12 && retires <= 12); }
            else { assert_eq!((points, retires), (1024, 1024)); }
            assert_eq!((new.take_point_visits(), new.take_retire_visits()), (points, 0));
            assert!(old.for_pid(AllocationState::Active, 1).eq(new.for_pid(AllocationState::Active, 1)));
            assert_eq!(new.count(location.allocation), usize::from(shared));
            assert!(!new.try_take_located(location, |_, _| -> Result<(), ()> { panic!("reused location") }).unwrap());
        }
    }

    #[test]
    fn located_retirement_failure_retains_the_exact_newest_record_and_can_retry() {
        use super::*;
        let mut a = PhysicalAllocations::default();
        let old = sample(4096, 1, 1); let new = sample(4096, 2, 1);
        a.insert(AllocationState::Active, 1, old); a.insert(AllocationState::Active, 1, new);
        let location = a.locate_active(1, 4096, 1).unwrap();
        assert!(location.allocation == new);
        assert_eq!(a.try_take_located(location, |_, _| Err::<(), _>("free failed")), Err("free failed"));
        assert_eq!(a.revision, location.revision); assert_eq!(a.count(new), 1);
        assert!(a.find(AllocationState::Active, 1, 4096, 1) == Some(new));
        assert!(a.try_take_located(location, |_, last| { assert!(last); Ok::<(), ()>(()) }).unwrap());
        assert!(a.find(AllocationState::Active, 1, 4096, 1) == Some(old));
        assert_eq!(a.count(new), 0); assert_eq!(a.count(old), 1);
    }

    #[test]
    fn every_ownership_mutation_invalidates_located_retirement_before_callback() {
        use super::*;
        for change in 0..12 {
            let mut a = PhysicalAllocations::default();
            for page in 1..=4 { a.insert(AllocationState::Active, 1, sample(page * 4096, page, 1)); }
            a.insert(AllocationState::Active, 2, sample(0x200000, 500, 2));
            a.insert(AllocationState::Deferred, 9, sample(0x400000, 900, 1));
            let replacement = sample(4096, 1000, 1);
            a.insert(AllocationState::Deferred, 1, replacement);
            let location = a.locate_active(1, 4096, 1).unwrap();
            match change {
                0 => a.insert(AllocationState::Active, 7, sample(0x500000, 2000, 1)),
                1 => a.defer_all(1),
                2 => { a.defer(1, 4 * 4096, 1).unwrap(); },
                3 => { a.take(AllocationState::Active, 1, 4 * 4096, 1).unwrap(); },
                4 => a.remove_pid(AllocationState::Active, 2, |_, _| {}),
                5 => { a.try_take(AllocationState::Active, 2, 0x200000, 2, |_, _| Ok::<(), ()>(())).unwrap(); },
                6 => { let pages = a.pages_in_range(1, 3 * 4096, 5 * 4096).collect::<Vec<_>>();
                    let mut pages = pages; pages.sort_by_key(|p| p.base_va);
                    assert!(a.try_defer_pages(1, 3 * 4096, 5 * 4096, &pages)); },
                7 => { let mut pages = a.pages_in_range(1, 3 * 4096, 5 * 4096).collect::<Vec<_>>();
                    pages.sort_by_key(|p| p.base_va);
                    assert!(a.try_retire_pages(1, 3 * 4096, 5 * 4096, &pages, |_, _| Ok::<(), ()>(())).unwrap()); },
                8 => { a.take_unique_deferred(9, 0x400000, 1).unwrap(); },
                9 => { let extent = a.find(AllocationState::Active, 2, 0x200000, 2).unwrap(); a.split_unique_extent(2, extent).unwrap(); },
                10 => { let reference = a.page_references(1, 4096, 8192)[0]; assert!(a.replace_backing(reference, replacement).is_some()); },
                _ => { assert_eq!(a.try_remove_pid(AllocationState::Active, 2, |_, _| Err::<(), _>("failed")), Err("failed")); },
            }
            assert!(!a.try_take_located(location, |_, _| -> Result<(), ()> { panic!("stale callback change={change}") }).unwrap());
        }
    }

    #[test]
    fn located_retirement_declines_saturation_and_node_recycling_without_aba() {
        use super::*;
        let mut a = PhysicalAllocations::default(); let allocation = sample(4096, 1, 1);
        a.insert(AllocationState::Active, 1, allocation);
        let location = a.locate_active(1, 4096, 1).unwrap();
        a.remove_pid(AllocationState::Active, 1, |_, _| {});
        a.insert(AllocationState::Active, 2, allocation);
        assert!(!a.try_take_located(location, |_, _| -> Result<(), ()> { panic!("recycled") }).unwrap());
        a.revision = usize::MAX - 1;
        let before_saturation = a.locate_active(2, 4096, 1).unwrap();
        a.insert(AllocationState::Deferred, 3, sample(8192, 4, 1));
        assert_eq!(a.revision, usize::MAX);
        for stale in [before_saturation, a.locate_active(2, 4096, 1).unwrap()] {
            assert!(!a.try_take_located(stale, |_, _| -> Result<(), ()> { panic!("saturated") }).unwrap());
        }
        assert!(a.try_take(AllocationState::Active, 2, 4096, 1, |_, _| Ok::<(), ()>(())).unwrap().is_some());
        assert_eq!(a.revision, usize::MAX);
    }

    #[test]
    fn extent_split_matches_scalar_registration_with_reserved_metadata_and_recycled_nodes() {
        use super::*;
        use core::sync::atomic::Ordering::Relaxed;
        fn fixture(pages:usize,peers:bool)->(PhysicalAllocations,ProcessPhysicalAllocation) {
            let mut a=PhysicalAllocations::default();
            if peers { a.insert(AllocationState::Active,1,sample(4096,4000,1)); }
            let mut extent=sample(0x200000,7000,pages); extent.owns_frame_cap=peers;
            a.insert(AllocationState::Active,1,extent);
            if peers {
                a.insert(AllocationState::Active,1,sample(0x50000000,5000,1));
                a.insert(AllocationState::Active,2,sample(0x60000000,6000,1));
                a.insert(AllocationState::Deferred,99,sample(0x70000000,900000,1));
                a.remove_pid(AllocationState::Deferred,99,|_,_|{});
            }
            (a,extent)
        }
        fn scalar(a:&mut PhysicalAllocations,extent:ProcessPhysicalAllocation) {
            let mut additional=[0;REFERENCE_PARTITIONS];
            for offset in 0..extent.page_count { additional[ReferenceCounts::partition((extent.base_page+offset,1))]+=1; }
            a.nodes.try_reserve(extent.page_count-1).unwrap();
            for (partition,count) in a.references.partitions.iter_mut().zip(additional) { partition.try_reserve(count).unwrap(); }
            a.take(AllocationState::Active,1,extent.base_va,extent.page_count).unwrap();
            for offset in 0..extent.page_count {
                a.insert(AllocationState::Active,1,ProcessPhysicalAllocation {
                    base_va:extent.base_va+offset*4096,start_slot:extent.start_slot+offset,
                    base_page:extent.base_page+offset,page_count:1,owns_frame_cap:extent.owns_frame_cap,
                });
            }
        }
        for pages in [2,3,64,1024] {
            for peers in [false,true] {
                let (mut a,extent)=fixture(pages,peers); let (mut old,_)=fixture(pages,peers);
                INDEX_SLOT_READS.store(0,Relaxed);
                a.split_unique_extent(1,extent).unwrap();
                let reads=INDEX_SLOT_READS.swap(0,Relaxed);
                scalar(&mut old,extent);
                assert_eq!(a.active.slots,old.active.slots); assert_eq!(a.deferred.slots,old.deferred.slots);
                assert_eq!((a.active.len,a.deferred.len,a.free),(old.active.len,old.deferred.len,old.free));
                assert_eq!(a.nodes.len(),old.nodes.len());
                for (a,b) in a.nodes.iter().zip(&old.nodes) {
                    assert!(a.allocation==b.allocation); assert_eq!(a.next.get(),b.next.get());
                    assert_eq!([a.pages.low,a.pages.high,a.pages.upper_low,a.pages.upper_high],
                        [b.pages.low,b.pages.high,b.pages.upper_low,b.pages.upper_high]);
                }
                for p in 0..REFERENCE_PARTITIONS {
                    assert_eq!(a.references.partitions[p].slots,old.references.partitions[p].slots);
                    assert_eq!(a.references.partitions[p].len,old.references.partitions[p].len);
                }
                for offset in 0..pages {
                    let page=a.find(AllocationState::Active,1,extent.base_va+offset*4096,1).unwrap();
                    assert_eq!(page.base_page,extent.base_page+offset);
                    assert_eq!(page.start_slot,extent.start_slot+offset);
                    assert_eq!(page.owns_frame_cap,extent.owns_frame_cap);
                    assert_eq!(a.count(page),1);
                }
                println!("SPLIT-EXTENT-HEAD pages={pages} peers-and-free={peers} probe-reads={reads}");
            }
        }
    }

    #[test]
    fn singleton_aliases_skip_repeated_counts_without_changing_releasable_groups() {
        use super::*;
        use core::sync::atomic::Ordering::Relaxed;
        fn old(a:&PhysicalAllocations,state:AllocationState,pid:usize)->Vec<ProcessPhysicalAllocation> {
            let mut ready=Vec::new(); let mut aliases=Vec::new();
            for allocation in a.for_pid(state,pid) {
                if a.count(allocation)==1 { ready.push(allocation); } else { aliases.push(allocation); }
            }
            aliases.sort_unstable_by_key(|a|(PhysicalAllocations::key(*a),a.base_va,a.start_slot));
            for group in aliases.chunk_by(|a,b|PhysicalAllocations::key(*a)==PhysicalAllocations::key(*b)) {
                if group.len()==a.count(group[0]) { ready.push(group[0]); }
            }
            ready
        }
        for state in [AllocationState::Active,AllocationState::Deferred] {
            let mut a=PhysicalAllocations::default();
            for page in 0..4096 {
                a.insert(state,1,sample((page+1)*4096,10000+page,1));
                a.insert(state,2,sample((page+1)*4096,10000+page,1));
            }
            INDEX_SLOT_READS.store(0,Relaxed);
            let _=a.heads(state).get(1);
            let common=INDEX_SLOT_READS.swap(0,Relaxed);
            INDEX_SLOT_READS.store(0,Relaxed);
            let expected=old(&a,state,1);
            let before=INDEX_SLOT_READS.swap(0,Relaxed);
            let actual=a.releasable(state,1);
            let after=INDEX_SLOT_READS.swap(0,Relaxed);
            assert!(actual==expected && actual.is_empty());
            assert_eq!(before-common,(after-common)*2);
            println!("RELEASABLE-SINGLETON active={} pages=4096 old-probe-reads={before} new-probe-reads={after} common-pid-probes={common}",matches!(state,AllocationState::Active));
            // Complete groups in this stage remain releasable; groups with
            // an outside reference remain held. Keep original result ordering.
            for (pid,va,physical) in [(1,0x20000000,50000),(1,0x20001000,50000),
                (1,0x30000000,60000),(1,0x30001000,60000),(2,0x30000000,60000),
                (1,0x40000000,70000)] {
                a.insert(state,pid,sample(va,physical,1));
            }
            // Also preserve the old rejection of an inconsistent zero count.
            a.references.insert((10000,1),0);
            for pid in [1,2,99] {
                let actual=a.releasable(state,pid); let expected=old(&a,state,pid);
                assert!(actual==expected);
                if pid==1 {
                    assert!(actual.iter().any(|p|p.base_page==50000));
                    assert!(actual.iter().any(|p|p.base_page==70000));
                    assert!(!actual.iter().any(|p|p.base_page==60000));
                }
            }
        }
    }

    #[test]
    fn retirement_reuses_stable_head_slot_and_preserves_failure_prefixes_and_other_pids() {
        use super::*;
        use core::sync::atomic::Ordering::Relaxed;
        fn old_retire<E>(a: &mut PhysicalAllocations,pid:usize,start:usize,end:usize,pages:&[ProcessPhysicalAllocation],
            mut release:impl FnMut(ProcessPhysicalAllocation,bool)->Result<(),E>) -> Result<bool,E> {
            if pages.is_empty() { return Ok(true); }
            if !a.page_prefix_matches(pid,start,end,pages) { return Ok(false); }
            let mut next=a.active.get(pid); let mut previous:Option<usize>=None; let mut remaining=pages.len();
            while let Some(index)=next {
                let node=a.nodes[index];
                if PhysicalAllocations::page_in_range(node.allocation,start,end) {
                    let reference=a.references.entry(PhysicalAllocations::key(node.allocation));
                    if let Err(error) = release(node.allocation,reference.count==1) {
                        a.compact_index(pid); return Err(error);
                    }
                    if let Some(previous)=previous { a.nodes[previous].next=Link::new(node.next.get()); }
                    else if let Some(head)=node.next.get() { a.active.insert(pid,head); }
                    else { a.active.remove(pid); }
                    if let Some(next) = node.next.get() { a.nodes[next].previous.set(previous); }
                    a.recycle_reference(index,reference); remaining-=1;
                    if remaining==0 { break; }
                } else { previous=Some(index); }
                next=node.next.get();
            }
            a.compact_index(pid); Ok(true)
        }
        fn fixture(count:usize,others:bool)->(PhysicalAllocations,Vec<ProcessPhysicalAllocation>) {
            let mut a=PhysicalAllocations::default(); let mut pages=Vec::new();
            for page in 0..count {
                let allocation=sample((page+1)*4096,10000+page,1);
                a.insert(AllocationState::Active,1,allocation); pages.push(allocation);
            }
            if others {
                for pid in 2..64 { a.insert(AllocationState::Active,pid,sample(pid*4096,20000+pid,1)); }
                // Unselected newer head and an alias retained in deferred state.
                a.insert(AllocationState::Active,1,sample(0x70000000,90000,2));
                a.insert(AllocationState::Deferred,1,sample(0x80000000,10063,1));
            }
            (a,pages)
        }
        fn same(a:&PhysicalAllocations,b:&PhysicalAllocations) {
            assert_eq!(a.active.slots,b.active.slots); assert_eq!(a.deferred.slots,b.deferred.slots);
            assert_eq!((a.active.len,a.deferred.len,a.free),(b.active.len,b.deferred.len,b.free));
            assert_eq!(a.nodes.len(),b.nodes.len());
            for (a,b) in a.nodes.iter().zip(&b.nodes) {
                assert!(a.allocation==b.allocation); assert_eq!(a.next.get(),b.next.get());
                assert_eq!([a.pages.low,a.pages.high,a.pages.upper_low,a.pages.upper_high],
                    [b.pages.low,b.pages.high,b.pages.upper_low,b.pages.upper_high]);
            }
            for p in 0..REFERENCE_PARTITIONS { assert_eq!(a.references.partitions[p].slots,b.references.partitions[p].slots); }
        }
        let (mut a,pages)=fixture(4096,false); let (mut old,_)=fixture(4096,false);
        INDEX_SLOT_READS.store(0,Relaxed);
        assert_eq!(old_retire(&mut old,1,4096,4097*4096,&pages,|_,_|Ok::<(),()>(())),Ok(true));
        let before=INDEX_SLOT_READS.swap(0,Relaxed);
        assert_eq!(a.try_retire_pages(1,4096,4097*4096,&pages,|_,_|Ok::<(),()>(())),Ok(true));
        let after=INDEX_SLOT_READS.swap(0,Relaxed);
        assert_eq!(before-after,4096); same(&a,&old);
        println!("RETIREMENT-HEAD-SLOT pages=4096 old-probe-reads={before} new-probe-reads={after} removed-head-lookups=4096");
        for (others,keep_unselected) in [(false,false),(true,false),(true,true)] {
            for fail in [0,31,127,usize::MAX] {
                let (mut a,pages)=fixture(128,others); let (mut old,_)=fixture(128,others);
                if others && !keep_unselected {
                    assert!(a.take(AllocationState::Active,1,0x70000000,2)
                        ==old.take(AllocationState::Active,1,0x70000000,2));
                }
                let mut seen=Vec::new(); let mut previous=Vec::new();
                let expected=old_retire(&mut old,1,4096,129*4096,&pages,|allocation,last| {
                    previous.push((allocation,last)); if previous.len()-1==fail { Err(51) } else { Ok(()) }
                });
                let actual=a.try_retire_pages(1,4096,129*4096,&pages,|allocation,last| {
                    seen.push((allocation,last)); if seen.len()-1==fail { Err(51) } else { Ok(()) }
                });
                assert_eq!(actual,expected); assert!(seen==previous); same(&a,&old);
            }
        }
    }

    #[test]
    fn head_updates_reuse_pid_lookup_and_preserve_node_chains_bounds_and_recycling() {
        use super::*;
        use core::sync::atomic::Ordering::Relaxed;
        fn old_insert(a: &mut PhysicalAllocations, state: AllocationState, pid: usize, allocation: ProcessPhysicalAllocation) {
            a.references.increment(PhysicalAllocations::key(allocation));
            let mut node=Node { allocation,next:Link::new(a.heads(state).get(pid)),
                previous:Previous::new(None,matches!(state,AllocationState::Active)),pages:PageBounds::EMPTY };
            if matches!(state,AllocationState::Active) {
                if let Some(next)=node.next.get() { node.pages=a.nodes[next].pages; }
                if allocation.page_count==1 { node.pages.include(allocation.base_va); }
            }
            let index=if let Some(free)=a.free {
                a.free=a.nodes[free].next.get(); a.nodes[free]=node; free
            } else { a.nodes.push(node); a.nodes.len()-1 };
            if let Some(next) = a.nodes[index].next.get() { a.nodes[next].previous.set(Some(index)); }
            a.heads_mut(state).insert(pid,index);
            if matches!(state,AllocationState::Active) { a.index_new_nodes(pid,index,1); }
        }
        fn same(a: &PhysicalAllocations,b: &PhysicalAllocations) {
            assert_eq!(a.active.slots,b.active.slots); assert_eq!(a.deferred.slots,b.deferred.slots);
            assert_eq!((a.active.len,a.deferred.len,a.free),(b.active.len,b.deferred.len,b.free));
            assert_eq!(a.nodes.len(),b.nodes.len());
            for (a,b) in a.nodes.iter().zip(&b.nodes) {
                assert!(a.allocation==b.allocation); assert_eq!(a.next.get(),b.next.get());
                assert_eq!([a.pages.low,a.pages.high,a.pages.upper_low,a.pages.upper_high],
                    [b.pages.low,b.pages.high,b.pages.upper_low,b.pages.upper_high]);
            }
            for p in 0..REFERENCE_PARTITIONS {
                assert_eq!(a.references.partitions[p].slots,b.references.partitions[p].slots);
                assert_eq!(a.references.partitions[p].len,b.references.partitions[p].len);
            }
        }
        let pids: Vec<_>=(0..usize::MAX).filter(|pid|pid.hash()&63==15).take(16).collect();
        for state in [AllocationState::Active,AllocationState::Deferred] {
            let mut a=PhysicalAllocations::default(); let mut old=PhysicalAllocations::default();
            a.heads_mut(state).try_reserve(32).unwrap(); old.heads_mut(state).try_reserve(32).unwrap();
            a.references.insert((700,1),0); old.references.insert((700,1),0);
            for pass in 0..2 {
                INDEX_SLOT_READS.store(0,Relaxed);
                for (i,&pid) in pids.iter().enumerate() { old_insert(&mut old,state,pid,sample((i+1+pass*16)*4096,700,1)); }
                let before=INDEX_SLOT_READS.swap(0,Relaxed);
                for (i,&pid) in pids.iter().enumerate() { a.insert(state,pid,sample((i+1+pass*16)*4096,700,1)); }
                let after=INDEX_SLOT_READS.swap(0,Relaxed);
                assert_eq!((before,after),(288,152)); same(&a,&old);
                println!("HEAD-UPDATE active={} pass={pass} colliding-pids=16 old-probe-reads={before} new-probe-reads={after}",matches!(state,AllocationState::Active));
            }
            for turn in 0..1000 {
                let pid=(turn*17)%67;
                if turn%9==0 {
                    a.remove_pid(state,pid,|_,_|{}); old.remove_pid(state,pid,|_,_|{});
                } else {
                    let allocation=sample((turn%97)*4096,(turn*7)%101,1+turn%3);
                    a.insert(state,pid,allocation); old_insert(&mut old,state,pid,allocation);
                }
                same(&a,&old);
            }
        }
    }

    #[test]
    fn reference_buffer_hints_and_reverse_path_preserve_original_stable_selection() {
        use super::*;
        for order in [vec![], vec![1, 2, 3, 4, 5], vec![5, 4, 3, 2, 1], vec![2, 5, 1, 2, 4, 3, 5]] {
            let mut allocations = PhysicalAllocations::default();
            for (index, &va) in order.iter().enumerate() {
                let mut allocation = sample(va * 4096, 100 + index, 1);
                allocation.owns_frame_cap = index % 2 == 0;
                allocations.insert(AllocationState::Active, 1, allocation);
            }
            let mut original: Vec<_> = allocations.page_reference_walk(1, 0, usize::MAX).collect();
            original.sort_by_key(|r| r.allocation.base_va);
            original.dedup_by_key(|r| r.allocation.base_va);
            // The overflowing capacity hint must decline without limiting
            // collection. Sparse/duplicate rows keep exact newest handles.
            for capacity in [0, 1, 16, usize::MAX] {
                let actual = allocations.page_references_reserved(1, 0, usize::MAX, capacity);
                assert_eq!(actual.len(), original.len());
                for (actual, original) in actual.iter().zip(&original) {
                    assert!(actual.allocation == original.allocation);
                    assert_eq!((actual.owner, actual.index), (original.owner, original.index));
                }
            }
        }
    }

    #[test]
    fn compact_links_preserve_zero_and_large_indices_in_one_word() {
        use super::Link;
        assert_eq!(core::mem::size_of::<Link>(), core::mem::size_of::<usize>());
        for index in [None, Some(0), Some(1), Some(511), Some(usize::MAX - 1)] {
            assert_eq!(Link::new(index).get(), index);
        }
        // An actual Vec node can never have this index; reject rather than
        // silently encode it as the empty link.
        assert!(std::panic::catch_unwind(|| Link::new(Some(usize::MAX))).is_err());
    }

    #[test]
    fn compact_nodes_recover_cap_flag_overhead_and_preserve_it_through_reuse() {
        use super::*;
        struct OldNode {
            _allocation: ProcessPhysicalAllocation,
            _next: Option<usize>,
            _previous: Previous,
            _pages: PageBounds,
        }
        assert_eq!(core::mem::size_of::<OldNode>() - core::mem::size_of::<Node>(), core::mem::size_of::<usize>());
        if usize::BITS == 64 { assert_eq!(core::mem::size_of::<Node>(), 88); }
        let mut allocations = PhysicalAllocations::default();
        for page in 0..513 {
            let mut allocation = sample((page + 1) * 4096, 100 + page % 17, 1);
            allocation.owns_frame_cap = page % 2 == 0;
            allocations.insert(AllocationState::Active, 1, allocation);
        }
        allocations.defer_all(1);
        let rows: Vec<_> = allocations.for_pid(AllocationState::Deferred, 1).collect();
        assert_eq!(rows.len(), 513);
        for (offset, row) in rows.iter().enumerate() {
            assert_eq!(row.base_va, (513 - offset) * 4096);
            assert_eq!(row.owns_frame_cap, (512 - offset) % 2 == 0);
        }
        let mut last_references = 0;
        allocations.remove_pid(AllocationState::Deferred, 1, |_, last| {
            last_references += usize::from(last);
        });
        assert_eq!(last_references, 17);
        let capacity = allocations.nodes.capacity();
        for turn in 0..1024 {
            let mut allocation = sample(4096, 8000 + turn, 1);
            allocation.owns_frame_cap = turn % 2 == 0;
            allocations.insert(AllocationState::Active, 2, allocation);
            assert_eq!(allocations.for_pid(AllocationState::Active, 2).next().unwrap().owns_frame_cap, allocation.owns_frame_cap);
            allocations.remove_pid(AllocationState::Active, 2, |row, last| {
                assert!(row == allocation);
                assert!(last);
            });
        }
        assert_eq!(allocations.nodes.len(), 513);
        assert_eq!(allocations.nodes.capacity(), capacity);
        println!("OWNERSHIP-LINK old-node={} new-node={} capacity={capacity} saved-bytes={}",
            core::mem::size_of::<OldNode>(), core::mem::size_of::<Node>(),
            capacity * (core::mem::size_of::<OldNode>() - core::mem::size_of::<Node>()));
    }

    use super::*;
    use AllocationState::Active;

    fn assert_range_oracle(a: &PhysicalAllocations) {
        for pid in [1, 2, 999] {
            for (start, end) in [(0, 1), (0, usize::MAX), (4096, 8192),
                (8193, 65537), (65536, 131072), (0x10000000, 0x10200000),
                (usize::MAX - 8192, usize::MAX), (4096, 4096), (8192, 1)] {
                let expected: Vec<_> = a.for_pid(Active, pid).filter(|p|
                    p.page_count == 1 && p.base_va >= start && p.base_va < end)
                    .map(fields).collect();
                let actual: Vec<_> = a.pages_in_range(pid, start, end).map(fields).collect();
                assert_eq!(actual, expected, "owner={pid} range={start}..{end}");
                let mut expected = Vec::new();
                let mut next = a.active.get(pid);
                while let Some(index) = next {
                    let node = a.nodes[index];
                    if node.allocation.page_count == 1 && node.allocation.base_va >= start
                        && node.allocation.base_va < end {
                        expected.push((node.allocation.base_va, fields(node.allocation), index));
                    }
                    next = node.next.get();
                }
                expected.sort_by_key(|row| row.0);
                expected.dedup_by_key(|row| row.0);
                let actual: Vec<_> = a.page_references(pid, start, end).into_iter().map(|p| {
                    assert_eq!(p.owner, pid); (p.allocation.base_va, fields(p.allocation), p.index)
                }).collect();
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn suffix_bounds_skip_disjoint_twenty_thousand_page_ownership_walks() {
        let mut a = PhysicalAllocations::default();
        for i in 1..=20000 { a.insert(Active, 1, sample(i * 4096, i, 1)); }
        for (start, end) in [(0, 1), (20001 * 4096, 30000 * 4096)] {
            a.range_visits.set(0);
            assert_eq!(a.pages_in_range(1, start, end).count(), 0);
            assert_eq!(a.range_visits.get(), 1);
        }
        a.range_visits.set(0);
        let pages: Vec<_> = a.pages_in_range(1, 19985 * 4096, 20001 * 4096).collect();
        assert_eq!(pages.len(), 16);
        assert_eq!(a.range_visits.get(), 17);
        assert_range_oracle(&a);
    }

    #[test]
    fn suffix_bounds_keep_mmap_gap_between_real_image_stack_and_interpreter_addresses() {
        let mut a = PhysicalAllocations::default();
        for (va, physical) in [(0x400000, 50001), (0x0403f000, 50002), (0x60000000, 50003)] {
            a.insert(Active, 1, sample(va, physical, 1));
        }
        let base = 0x10000000;
        for i in 0..20000 { a.insert(Active, 1, sample(base + i * 4096, i + 1, 1)); }
        a.range_visits.set(0);
        assert_eq!(a.pages_in_range(1, base + 20000 * 4096, base + 20016 * 4096).count(), 0);
        assert_eq!(a.range_visits.get(), 1);
        a.range_visits.set(0);
        assert_eq!(a.pages_in_range(1, base + 19984 * 4096, base + 20000 * 4096).count(), 16);
        assert_eq!(a.range_visits.get(), 17);
        assert_range_oracle(&a);
    }

    #[test]
    fn two_band_bounds_never_exclude_an_inserted_address_when_the_gap_moves() {
        for sequence in [vec![0, usize::MAX, 1, usize::MAX - 1, 500, 100, 200, 300],
            vec![500, 501, 502, 1000, 0, 250, 750, 751, 1, usize::MAX]] {
            let mut bounds = PageBounds::EMPTY;
            let mut points = Vec::new();
            for va in sequence {
                points.push(va); bounds.include(va);
                for &point in &points {
                    assert!((bounds.low <= point && point <= bounds.high)
                        || (bounds.upper_low <= point && point <= bounds.upper_high));
                }
            }
        }
        let mut bounds = PageBounds::EMPTY;
        for va in [4096, 8192, 0x0403f000, 0x60000000, 0x10000000, 0x10001000] {
            bounds.include(va);
        }
        assert!(!bounds.intersects(0x10002000, 0x10010000));
        assert!(bounds.intersects(0x60000000, 0x60001000));
    }

    #[test]
    fn suffix_bounds_follow_mixed_unlinks_owner_reuse_and_failed_retirement() {
        let mut a = PhysicalAllocations::default();
        let mut random = 0x51a7e9usize;
        for step in 0..2000 {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            let pid = 1 + ((random >> 16) & 1);
            let va = ((random >> 8) % 32) * 4096 + usize::from(step % 13 == 0);
            match step % 11 {
                0..=4 => a.insert(Active, pid, sample(va, step + 100, 1 + usize::from(step % 7 == 0))),
                5 => { a.defer(pid, va, 1); }
                6 => { a.take(Active, pid, va, 1); }
                7 => { a.take_unique_deferred(pid, va, 1); }
                8 => {
                    let mut calls = 0;
                    let _: Result<(), ()> = a.try_remove_pid(Active, pid, |_, _| {
                        calls += 1; if calls == 2 { Err(()) } else { Ok(()) }
                    });
                }
                9 => { a.defer_all(pid); }
                _ => { a.remove_pid(AllocationState::Deferred, pid, |_, _| {}); }
            }
            assert_range_oracle(&a);
        }
        a.insert(Active, 1, sample(usize::MAX, 99999, 1));
        a.insert(Active, 1, sample(usize::MAX - 4096, 99998, 1));
        assert_range_oracle(&a);
    }

    #[test]
    fn suffix_bounds_preserve_split_replacement_batch_deferral_and_error_prefix() {
        let (mut a, pages) = retirement_fixture();
        let mut calls = 0;
        assert_eq!(a.try_retire_pages(1, 4096, 514 * 4096, &pages, |_, _| {
            calls += 1; if calls == 11 { Err(()) } else { Ok(()) }
        }), Err(()));
        assert_range_oracle(&a);
        let remaining: Vec<_> = a.page_references(1, 4096, 514 * 4096)
            .into_iter().map(|p| p.allocation).collect();
        assert!(a.try_defer_pages(1, 4096, 514 * 4096, &remaining));
        assert_range_oracle(&a);
        let extent = sample(4096, 50000, 32);
        a.insert(Active, 1, extent);
        a.split_unique_extent(1, extent).unwrap();
        assert_range_oracle(&a);
        let reference = a.page_references(1, 4096, 8192)[0];
        let replacement = sample(4096, 60000, 1);
        a.insert(AllocationState::Deferred, 1, replacement);
        assert!(a.replace_backing(reference, replacement).is_some());
        assert_range_oracle(&a);
    }

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
    fn reference_increment_uses_one_probe_and_matches_get_insert_through_growth_and_removal() {
        use core::sync::atomic::Ordering::Relaxed;
        let mut keys=Vec::new();
        for page in 0..usize::MAX {
            let key=(page,1);
            if ReferenceCounts::partition(key)==0 && key.hash()&63==15 { keys.push(key); }
            if keys.len()==16 { break; }
        }
        let mut old=ReferenceCounts::default(); let mut current=ReferenceCounts::default();
        old.partitions[0].try_reserve(32).unwrap(); current.partitions[0].try_reserve(32).unwrap();
        for pass in 0..2 {
            INDEX_SLOT_READS.store(0,Relaxed);
            for &key in &keys { let value=old.get(key).unwrap_or(0); old.insert(key,value+1); }
            let before=INDEX_SLOT_READS.swap(0,Relaxed);
            for &key in &keys { current.increment(key); }
            let after=INDEX_SLOT_READS.swap(0,Relaxed);
            assert_eq!((before,after),(272,136));
            for &key in &keys { assert_eq!(current.get(key),Some(pass+1)); }
            println!("REFERENCE-UPDATE pass={pass} colliding-keys=16 old-slot-reads={before} new-slot-reads={after}");
        }
        for turn in 0..3000 {
            let key=((turn*73)%257,1+turn%5);
            if turn%3==0 { assert_eq!(current.remove(key),old.remove(key)); }
            else { let count=old.get(key).unwrap_or(0); old.insert(key,count+1); current.increment(key); }
            for partition in 0..REFERENCE_PARTITIONS {
                assert_eq!(current.partitions[partition].len,old.partitions[partition].len);
                assert!(current.partitions[partition].slots==old.partitions[partition].slots);
            }
        }
        for key in keys {
            while current.get(key).is_some() {
                let entry=current.entry(key);
                let count=old.get(key).unwrap();
                current.retire(entry);
                if count==1 { old.remove(key); } else { old.insert(key,count-1); }
                assert_eq!(current.get(key),old.get(key));
            }
        }
    }

    #[test]
    fn insertion_reuses_the_probed_vacancy_and_matches_previous_growth_and_churn() {
        use core::sync::atomic::Ordering::Relaxed;
        fn old_insert(index: &mut Index<Colliding>, key: Colliding, value: usize) {
            if let Some(slot) = index.locate(key) { index.slots[slot]=Some((key,value)); return; }
            if (index.len+1)*2>index.slots.len() {
                let capacity=index.slots.len().saturating_mul(2).max(16);
                let old=core::mem::replace(&mut index.slots,alloc::vec![None;capacity]);
                index.len=0;
                for (key,value) in old.into_iter().flatten() { old_insert(index,key,value); }
            }
            let mask=index.slots.len()-1;
            let mut slot=key.hash()&mask;
            loop {
                INDEX_SLOT_READS.fetch_add(1,Relaxed);
                if index.slots[slot].is_none() { break; }
                slot=(slot+1)&mask;
            }
            index.slots[slot]=Some((key,value)); index.len+=1;
        }
        let mut current=Index::<Colliding>::default();
        let mut old=Index::<Colliding>::default();
        current.try_reserve(32).unwrap(); old.try_reserve(32).unwrap();
        INDEX_SLOT_READS.store(0,Relaxed);
        for key in 0..16 { old_insert(&mut old,Colliding(key),key); }
        let before=INDEX_SLOT_READS.swap(0,Relaxed);
        for key in 0..16 { current.insert(Colliding(key),key); }
        let after=INDEX_SLOT_READS.swap(0,Relaxed);
        assert_eq!((before,after),(272,136));
        assert!(current.slots==old.slots);
        println!("PHYSICAL-INDEX-VACANCY colliding-inserts=16 old-slot-reads={before} new-slot-reads={after}");
        for turn in 0..3000 {
            let key=Colliding((turn*73)%257);
            if turn%3==0 { assert_eq!(current.remove(key),old.remove(key)); }
            else { current.insert(key,turn); old_insert(&mut old,key,turn); }
            assert_eq!(current.len,old.len); assert!(current.slots==old.slots);
        }
        for key in 0..257 { assert_eq!(current.get(Colliding(key)),old.get(Colliding(key))); }
        let capacity=current.slots.capacity();
        let keys: Vec<_> = current.slots.iter().flatten().map(|&(key,_)|key).collect();
        for key in keys { current.insert(key,0); old_insert(&mut old,key,0); }
        assert_eq!(current.slots.capacity(),capacity);
        assert!(current.slots==old.slots);
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
                owns_frame_cap: true,
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
            owns_frame_cap: true,
        }
    }

    fn retirement_fixture() -> (PhysicalAllocations, Vec<ProcessPhysicalAllocation>) {
        let mut a = PhysicalAllocations::default();
        let pages: Vec<_> = (1..=513).map(|i| sample(i * 4096, 1000 + i, 1)).collect();
        for (i, &page) in pages.iter().enumerate() {
            a.insert(Active, 1, page);
            if i % 2 == 0 { a.insert(Active, 2, page); }
        }
        for i in 0..512 { a.insert(Active, 1, sample(0x10000000 + i * 4096, 5000 + i, 1)); }
        (a, pages)
    }

    #[test]
    fn batched_retirement_matches_scalar_callbacks_reference_counts_and_remaining_lists() {
        let (mut scalar, pages) = retirement_fixture();
        let (mut batch, _) = retirement_fixture();
        let mut expected = Vec::new();
        for &page in pages.iter().rev() {
            expected.push((page.base_va, scalar.count(page) == 1));
            scalar.take(Active, 1, page.base_va, 1).unwrap();
        }
        let mut actual = Vec::new();
        let result: Result<bool, ()> = batch.try_retire_pages(1, 4096, 514 * 4096, &pages, |page, last| {
            actual.push((page.base_va, last)); Ok(())
        });
        assert_eq!(result, Ok(true)); assert_eq!(actual, expected);
        for pid in [1, 2] {
            assert!(batch.for_pid(Active, pid).collect::<Vec<_>>() == scalar.for_pid(Active, pid).collect::<Vec<_>>());
        }
        assert_eq!(batch.free, scalar.free);
        for page in pages { assert_eq!(batch.count(page), scalar.count(page)); }
    }

    #[test]
    fn batched_retirement_failure_preserves_the_same_scalar_prefix_and_suffix() {
        for fail in [0, 1, 256, 512] {
            let (mut scalar, pages) = retirement_fixture();
            let (mut batch, _) = retirement_fixture();
            let mut expected = Vec::new();
            for (i, &page) in pages.iter().rev().enumerate() {
                expected.push((page.base_va, scalar.count(page) == 1));
                if i == fail { break; }
                scalar.take(Active, 1, page.base_va, 1).unwrap();
            }
            let mut actual = Vec::new();
            let result = batch.try_retire_pages(1, 4096, 514 * 4096, &pages, |page, last| {
                let i = actual.len(); actual.push((page.base_va, last));
                if i == fail { Err(17) } else { Ok(()) }
            });
            assert_eq!(result, Err(17)); assert_eq!(actual, expected);
            for pid in [1, 2] {
                assert!(batch.for_pid(Active, pid).collect::<Vec<_>>() == scalar.for_pid(Active, pid).collect::<Vec<_>>());
            }
            assert_eq!(batch.free, scalar.free);
            for page in pages { assert_eq!(batch.count(page), scalar.count(page)); }
        }
    }

    #[test]
    fn declined_retirement_never_invokes_release_or_changes_ownership() {
        let (mut a, mut pages) = retirement_fixture();
        pages[512].base_page += 1;
        let before = a.for_pid(Active, 1).collect::<Vec<_>>();
        let references = reference_slots(&a);
        let result: Result<bool, ()> = a.try_retire_pages(1, 4096, 514 * 4096, &pages, |_, _| panic!("declined"));
        assert_eq!(result, Ok(false));
        assert!(a.for_pid(Active, 1).collect::<Vec<_>>() == before);
        assert_eq!(reference_slots(&a), references); assert!(a.free.is_none());
    }

    #[test]
    fn retiring_two_aliases_observes_the_last_reference_only_after_the_first_commit() {
        let mut a = PhysicalAllocations::default();
        let low = sample(4096, 100, 1); let high = sample(8192, 100, 1);
        for page in [low, high] { a.insert(Active, 1, page); }
        let mut observed = Vec::new();
        let result: Result<bool, ()> = a.try_retire_pages(1, 4096, 12288, &[low, high], |page, last| {
            observed.push((page.base_va, last)); Ok(())
        });
        assert_eq!(result, Ok(true)); assert_eq!(observed, [(8192, false), (4096, true)]);
        assert_eq!(a.count(low), 0); assert!(a.for_pid(Active, 1).next().is_none());
        // Reuse the recycled nodes, then fail at the final owner. The active
        // head must already reflect the first successful unlink when returning.
        for page in [low, high] { a.insert(Active, 1, page); }
        observed.clear();
        let result = a.try_retire_pages(1, 4096, 12288, &[low, high], |page, last| {
            observed.push((page.base_va, last));
            if last { Err(19) } else { Ok(()) }
        });
        assert_eq!(result, Err(19)); assert_eq!(observed, [(8192, false), (4096, true)]);
        assert_eq!(a.count(low), 1);
        assert!(a.for_pid(Active, 1).collect::<Vec<_>>() == [low]);
    }

    #[test]
    fn owner_retirement_matches_scalar_alias_counts_and_failure_prefix_in_both_states() {
        for state in [Active, AllocationState::Deferred] {
            for fail in [0, 1, 128, 256, 257] {
                let fixture = || {
                    let mut a = PhysicalAllocations::default();
                    for i in 0..257 {
                        let page = sample((i + 1) * 4096, 1000 + i % 31, 1);
                        a.insert(state, 1, page);
                        if i % 17 == 0 { a.insert(Active, 2, page); }
                    }
                    a
                };
                let mut scalar = fixture();
                let mut batch = fixture();
                let pages = scalar.for_pid(state, 1).collect::<Vec<_>>();
                let mut expected = Vec::new();
                for (i, &page) in pages.iter().enumerate() {
                    expected.push((page, scalar.count(page) == 1));
                    if i == fail { break; }
                    scalar.take(state, 1, page.base_va, 1).unwrap();
                }
                let mut observed = Vec::new();
                let result = batch.try_remove_pid(state, 1, |page, last| {
                    let i = observed.len();
                    observed.push((page, last));
                    if i == fail { Err(23) } else { Ok(()) }
                });
                assert_eq!(result, if fail == 257 { Ok(()) } else { Err(23) });
                assert!(observed == expected);
                for owner_state in [Active, AllocationState::Deferred] {
                    for pid in [1, 2] {
                        assert!(batch.for_pid(owner_state, pid).collect::<Vec<_>>() ==
                            scalar.for_pid(owner_state, pid).collect::<Vec<_>>());
                    }
                }
                for page in pages { assert_eq!(batch.count(page), scalar.count(page)); }
                assert_eq!(batch.free, scalar.free);
                // A failed suffix remains usable on a later successful retry.
                let retry: Result<(), ()> = batch.try_remove_pid(state, 1, |_, _| Ok(()));
                assert_eq!(retry, Ok(()));
                assert!(batch.for_pid(state, 1).next().is_none());
                for page in batch.for_pid(Active, 2) { assert!(batch.count(page) > 0); }
            }
        }
    }

    #[test]
    fn batch_deferral_matches_scalar_order_behind_a_large_newer_prefix() {
        fn fixture() -> (PhysicalAllocations, Vec<ProcessPhysicalAllocation>) {
            let mut a = PhysicalAllocations::default();
            a.insert(AllocationState::Deferred, 1, sample(0x9000000, 99999, 1));
            a.insert(Active, 1, sample(0x8000000, 99998, 1));
            let pages: Vec<_> = (0..513).map(|i| sample((i + 1) * 4096, 10000 + i, 1)).collect();
            for &page in &pages {
                a.insert(Active, 1, page);
                a.insert(Active, 2, page);
            }
            for i in 0..512 { a.insert(Active, 1, sample(0x10000000 + i * 4096, 20000 + i, 1)); }
            (a, pages)
        }
        let (mut scalar, pages) = fixture();
        let (mut batch, _) = fixture();
        let nodes = batch.nodes.len(); let free = batch.free;
        for page in pages.iter().rev() { scalar.defer(1, page.base_va, 1).unwrap(); }
        assert!(batch.try_defer_pages(1, 4096, 514 * 4096, &pages));
        for pid in [1, 2] {
            assert!(batch.for_pid(Active, pid).collect::<Vec<_>>() == scalar.for_pid(Active, pid).collect::<Vec<_>>());
            assert!(batch.for_pid(AllocationState::Deferred, pid).collect::<Vec<_>>() == scalar.for_pid(AllocationState::Deferred, pid).collect::<Vec<_>>());
        }
        for page in pages { assert_eq!(batch.count(page), 2); assert_eq!(batch.count(page), scalar.count(page)); }
        assert_eq!(batch.nodes.len(), nodes); assert_eq!(batch.free, free);
    }

    #[test]
    fn batch_deferral_declines_changed_order_or_fields_without_mutation() {
        for changed in [false, true] {
            let mut a = PhysicalAllocations::default();
            let mut pages: Vec<_> = (1..=3).map(|i| sample(i * 4096, 100 + i, 1)).collect();
            let order = if changed { [0, 1, 2] } else { [0, 2, 1] };
            for index in order { a.insert(Active, 1, pages[index]); }
            if changed { pages[2].start_slot += 1; }
            let before = a.for_pid(Active, 1).collect::<Vec<_>>();
            let references = reference_slots(&a);
            assert!(!a.try_defer_pages(1, 4096, 4 * 4096, &pages));
            assert!(a.for_pid(Active, 1).collect::<Vec<_>>() == before);
            assert!(a.for_pid(AllocationState::Deferred, 1).next().is_none());
            assert_eq!(reference_slots(&a), references);
        }
    }

    #[test]
    fn batch_deferral_stops_at_the_snapshot_prefix_and_keeps_older_duplicates() {
        let mut a = PhysicalAllocations::default();
        let older = sample(4096, 100, 1); let newest = sample(4096, 101, 1);
        let high = sample(8192, 102, 1);
        for page in [older, newest, high] { a.insert(Active, 1, page); }
        assert!(a.try_defer_pages(1, 4096, 12288, &[newest, high]));
        assert!(a.for_pid(Active, 1).collect::<Vec<_>>() == [older]);
        assert!(a.for_pid(AllocationState::Deferred, 1).collect::<Vec<_>>() == [newest, high]);
        for page in [older, newest, high] { assert_eq!(a.count(page), 1); }
    }

    #[test]
    fn batch_deferral_preserves_interleaved_nonpages_and_sparse_holes() {
        let mut a = PhysicalAllocations::default();
        let low = sample(4096, 100, 1); let high = sample(12288, 101, 1);
        let whole = sample(8192, 200, 2); let unaligned = sample(8193, 300, 1);
        for page in [low, whole, high, unaligned] { a.insert(Active, 1, page); }
        assert!(a.try_defer_pages(1, 4096, 16384, &[low, high]));
        assert!(a.for_pid(Active, 1).collect::<Vec<_>>() == [unaligned, whole]);
        assert!(a.for_pid(AllocationState::Deferred, 1).collect::<Vec<_>>() == [low, high]);
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
