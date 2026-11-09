//! Reusable shared mappings. Reservations own VA and capability slots; physical
//! ownership is still counted by ProcessManager's allocation references.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SharedMemoryReservation {
    pub base_va: usize,
    pub start_slot: usize,
    pub page_count: usize,
    references: ReferenceUnit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReferenceUnit {
    Extent,
    Page,
    PoolExtent,
    PoolPage,
}

impl SharedMemoryReservation {
    pub fn has_page_references(self) -> bool {
        matches!(self.references, ReferenceUnit::Page | ReferenceUnit::PoolPage)
    }

    pub fn owns_frame_caps(self) -> bool {
        matches!(self.references, ReferenceUnit::Extent | ReferenceUnit::Page)
    }
}

#[derive(Default)]
pub(super) struct SharedMemorySpace {
    slots: FreeRanges,
    pages: FreeRanges,
    active: BTreeMap<usize, SharedMemoryReservation>,
}

// Address order supports coalescing; size order supports logarithmic best-fit.
// Metadata scales with holes, not with every page or the number of allocations.
#[derive(Default)]
struct FreeRanges {
    by_address: BTreeMap<usize, usize>,
    by_size: alloc::collections::BTreeSet<(usize, usize)>,
}

impl FreeRanges {
    fn find(&self, count: usize) -> Option<usize> {
        self.by_size
            .range((count, 0)..)
            .next()
            .map(|&(_, start)| start)
    }

    fn remove(&mut self, start: usize) -> usize {
        let count = self.by_address.remove(&start).unwrap();
        self.by_size.remove(&(count, start));
        count
    }

    fn insert(&mut self, mut start: usize, mut count: usize) {
        if let Some((&previous, &length)) = self.by_address.range(..start).next_back() {
            debug_assert!(previous + length <= start);
            if previous + length == start {
                self.remove(previous);
                start = previous;
                count += length;
            }
        }
        if let Some((&next, &length)) = self.by_address.range(start..).next() {
            debug_assert!(start + count <= next);
            if start + count == next {
                self.remove(next);
                count += length;
            }
        }
        self.by_address.insert(start, count);
        self.by_size.insert((count, start));
    }

    fn take(&mut self, start: usize, count: usize) {
        let length = self.remove(start);
        if length > count {
            self.insert(start + count, length - count);
        }
    }

    fn exclude(&mut self, start: usize, count: usize) {
        let end = start + count;
        while let Some((&base, &length)) = self.by_address.range(..end).next_back() {
            let range_end = base + length;
            if range_end <= start {
                break;
            }
            self.remove(base);
            if range_end > end {
                self.insert(end, range_end - end);
            }
            if base < start {
                self.insert(base, start - base);
            }
        }
    }
}

impl ProcessManager {
    /// Reserve one replacement capability without reserving another VA.
    pub fn reserve_private_replacement_slot(
        &mut self,
        pid: usize,
        max_frame_slots: usize,
    ) -> Result<(CapabilityDescriptor, CapabilityDescriptor, usize), CapabilityError> {
        let pid = self.memory_owner_pid(pid);
        let entry = self
            .memory_entry(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if let Some(space) = self.shared_memory.get_mut(&pid) {
            if let Some(slot) = space.slots.find(1) {
                let count = space.slots.remove(slot);
                if count > 1 {
                    space.slots.insert(slot + 1, count - 1);
                }
                return Ok((entry.root_node, entry.address_space, slot));
            }
        }
        if entry.next_frame_slot >= max_frame_slots {
            return Err(CapabilityError::InvalidArgument);
        }
        self.entry_mut_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?
            .next_frame_slot += 1;
        Ok((entry.root_node, entry.address_space, entry.next_frame_slot))
    }

    /// Only after the old leaf, temporary aliases, capability and physical
    /// ownership record have been removed. This does not free virtual space.
    pub fn recycle_private_frame_slot(&mut self, pid: usize, slot: usize) {
        let pid = self.memory_owner_pid(pid);
        self.shared_memory
            .entry(pid)
            .or_default()
            .slots
            .insert(slot, 1);
    }

    pub fn reserve_shared_memory(
        &mut self,
        pid: usize,
        page_count: usize,
        max_frame_slots: usize,
    ) -> Result<SharedMemoryReservation, CapabilityError> {
        self.reserve_shared_memory_at(pid, page_count, max_frame_slots, 0)
    }

    pub fn reserve_owned_pool_memory(
        &mut self, pid: usize, pages: usize, max_slots: usize,
    ) -> Result<SharedMemoryReservation, CapabilityError> {
        let reservation = self.reserve_shared_memory(pid, pages, max_slots)?;
        let owner = self.memory_owner_pid(pid);
        let stored = self.shared_memory.get_mut(&owner).unwrap().active
            .get_mut(&reservation.base_va).unwrap();
        stored.references = ReferenceUnit::PoolExtent;
        Ok(*stored)
    }

    pub fn reserve_shared_memory_at(
        &mut self,
        pid: usize,
        page_count: usize,
        max_frame_slots: usize,
        requested_va: usize,
    ) -> Result<SharedMemoryReservation, CapabilityError> {
        let pid = self.memory_owner_pid(pid);
        let entry = self
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if pid == 0 || page_count == 0 || requested_va & (USER_PAGE_SIZE - 1) != 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let space = self.shared_memory.entry(pid).or_default();
        let free_slot = space.slots.find(page_count);
        let free_page = if requested_va == 0 {
            space.pages.find(page_count)
        } else {
            None
        };
        let start_slot = free_slot.unwrap_or(entry.next_frame_slot);
        let slot_end = start_slot
            .checked_add(page_count)
            .filter(|end| *end <= max_frame_slots)
            .ok_or(CapabilityError::InvalidArgument)?;
        let base_va = if requested_va != 0 {
            requested_va
        } else {
            free_page
                .map(|page| page * USER_PAGE_SIZE)
                .unwrap_or(entry.user_heap_next_va)
        };
        let bytes = page_count
            .checked_mul(USER_PAGE_SIZE)
            .ok_or(CapabilityError::InvalidArgument)?;
        let end_va = base_va
            .checked_add(bytes)
            .filter(|end| base_va != 0 && (requested_va != 0 || *end <= entry.user_heap_limit_va))
            .ok_or(CapabilityError::InvalidArgument)?;
        if space
            .active
            .range(..end_va)
            .next_back()
            .is_some_and(|(_, reservation)| {
                reservation.base_va + reservation.page_count * USER_PAGE_SIZE > base_va
            })
        {
            return Err(CapabilityError::IllegalOperation);
        }

        // Validate both resources before consuming either one.
        if let Some(slot) = free_slot {
            space.slots.take(slot, page_count);
        }
        if let Some(page) = free_page {
            space.pages.take(page, page_count);
        } else if requested_va != 0 {
            space.pages.exclude(base_va / USER_PAGE_SIZE, page_count);
        }
        let reservation = SharedMemoryReservation {
            base_va,
            start_slot,
            page_count,
            references: ReferenceUnit::Extent,
        };
        space.active.insert(base_va, reservation);
        let entry = self.entry_mut_by_pid(pid).unwrap();
        if free_slot.is_none() {
            entry.next_frame_slot = slot_end;
        }
        if free_page.is_none() && requested_va == 0 {
            entry.user_heap_next_va = end_va;
        }
        Ok(reservation)
    }

    pub fn shared_memory_reservation(
        &self,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) -> Option<SharedMemoryReservation> {
        let pid = self.memory_owner_pid(pid);
        self.shared_memory
            .get(&pid)?
            .active
            .get(&base_va)
            .filter(|reservation| reservation.page_count == page_count)
            .copied()
    }

    /// Find one reservation containing the complete nonempty byte range.
    pub fn shared_memory_containing_range(
        &self,
        pid: usize,
        base_va: usize,
        bytes: usize,
    ) -> Option<SharedMemoryReservation> {
        let end = base_va.checked_add(bytes)?;
        if bytes == 0 {
            return None;
        }
        let pid = self.memory_owner_pid(pid);
        let (_, reservation) = self
            .shared_memory
            .get(&pid)?
            .active
            .range(..=base_va)
            .next_back()?;
        let limit = reservation
            .page_count
            .checked_mul(USER_PAGE_SIZE)
            .and_then(|bytes| reservation.base_va.checked_add(bytes))?;
        (end <= limit).then_some(*reservation)
    }

    /// Find any reservation overlapping a complete nonempty byte range.
    pub fn shared_memory_overlapping_range(
        &self,
        pid: usize,
        base_va: usize,
        bytes: usize,
    ) -> Option<SharedMemoryReservation> {
        let end = base_va.checked_add(bytes)?;
        if bytes == 0 {
            return None;
        }
        let pid = self.memory_owner_pid(pid);
        let (_, reservation) = self
            .shared_memory
            .get(&pid)?
            .active
            .range(..end)
            .next_back()?;
        let limit = reservation
            .page_count
            .checked_mul(USER_PAGE_SIZE)
            .and_then(|bytes| reservation.base_va.checked_add(bytes))?;
        (limit > base_va).then_some(*reservation)
    }

    /// Change a RAM reservation's accounting exactly once, without releasing
    /// backing. The explicit unit survives partial UNMAP/capability cleanup,
    /// so ordinary extent release never has to search for the allocation twice.
    pub fn canonicalize_shared_memory(
        &mut self,
        pid: usize,
        reservation: SharedMemoryReservation,
    ) -> Result<(), CapabilityError> {
        let pid = self.memory_owner_pid(pid);
        if self.shared_memory_reservation(pid, reservation.base_va, reservation.page_count)
            != Some(reservation)
        {
            return Err(CapabilityError::InvalidArgument);
        }
        if reservation.has_page_references() || reservation.page_count == 1 {
            return Ok(());
        }
        let allocation = self
            .physical_allocations
            .find(
                AllocationState::Active,
                pid,
                reservation.base_va,
                reservation.page_count,
            )
            .filter(|allocation| allocation.start_slot == reservation.start_slot
                && allocation.owns_frame_cap == reservation.owns_frame_caps())
            .ok_or(CapabilityError::IllegalOperation)?;
        self.physical_allocations
            .split_unique_extent(pid, allocation)
            .map_err(|_| CapabilityError::IllegalOperation)?;
        self.shared_memory
            .get_mut(&pid)
            .unwrap()
            .active
            .get_mut(&reservation.base_va)
            .unwrap()
            .references = if reservation.owns_frame_caps() { ReferenceUnit::Page } else { ReferenceUnit::PoolPage };
        Ok(())
    }

    // Call only after every mapping and capability in the reservation has been
    // removed (or before any was installed when rolling back a reservation).
    pub fn recycle_shared_memory(&mut self, pid: usize, reservation: SharedMemoryReservation) {
        let pid = self.memory_owner_pid(pid);
        let heap_limit = self.find_entry_by_pid(pid).unwrap().user_heap_limit_va / USER_PAGE_SIZE;
        let space = self.shared_memory.get_mut(&pid).unwrap();
        assert_eq!(space.active.remove(&reservation.base_va), Some(reservation));
        space
            .slots
            .insert(reservation.start_slot, reservation.page_count);
        // Fixed mappings may live beyond the default allocation arena. Their
        // capability slots are reusable, but offering their VA to a default
        // allocation would make an otherwise valid request fail its heap limit.
        let first = reservation.base_va / USER_PAGE_SIZE;
        let end = (first + reservation.page_count).min(heap_limit);
        if first < end {
            space.pages.insert(first, end - first);
        }
    }

    pub(super) fn exclude_shared_virtual_range(
        &mut self,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) {
        let pid = self.memory_owner_pid(pid);
        if let Some(space) = self.shared_memory.get_mut(&pid) {
            space.pages.exclude(base_va / USER_PAGE_SIZE, page_count);
        }
    }
}
