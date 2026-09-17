//! Reusable shared mappings. Reservations own VA and capability slots; physical
//! ownership is still counted by ProcessManager's allocation references.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SharedMemoryReservation {
    pub base_va: usize,
    pub start_slot: usize,
    pub page_count: usize,
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
    pub fn reserve_shared_memory(
        &mut self,
        pid: usize,
        page_count: usize,
        max_frame_slots: usize,
    ) -> Result<SharedMemoryReservation, CapabilityError> {
        let entry = self
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if pid == 0 || page_count == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let space = self.shared_memory.entry(pid).or_default();
        let free_slot = space.slots.find(page_count);
        let free_page = space.pages.find(page_count);
        let start_slot = free_slot.unwrap_or(entry.next_frame_slot);
        let slot_end = start_slot
            .checked_add(page_count)
            .filter(|end| *end <= max_frame_slots)
            .ok_or(CapabilityError::InvalidArgument)?;
        let base_va = free_page
            .map(|page| page * USER_PAGE_SIZE)
            .unwrap_or(entry.user_heap_next_va);
        let bytes = page_count
            .checked_mul(USER_PAGE_SIZE)
            .ok_or(CapabilityError::InvalidArgument)?;
        let end_va = base_va
            .checked_add(bytes)
            .filter(|end| base_va != 0 && *end <= entry.user_heap_limit_va)
            .ok_or(CapabilityError::InvalidArgument)?;

        // Validate both resources before consuming either one.
        if let Some(slot) = free_slot {
            space.slots.take(slot, page_count);
        }
        if let Some(page) = free_page {
            space.pages.take(page, page_count);
        }
        let reservation = SharedMemoryReservation {
            base_va,
            start_slot,
            page_count,
        };
        space.active.insert(base_va, reservation);
        let entry = self.entry_mut_by_pid(pid).unwrap();
        if free_slot.is_none() {
            entry.next_frame_slot = slot_end;
        }
        if free_page.is_none() {
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
        self.shared_memory
            .get(&pid)?
            .active
            .get(&base_va)
            .filter(|reservation| reservation.page_count == page_count)
            .copied()
    }

    // Call only after every mapping and capability in the reservation has been
    // removed (or before any was installed when rolling back a reservation).
    pub fn recycle_shared_memory(&mut self, pid: usize, reservation: SharedMemoryReservation) {
        let space = self.shared_memory.get_mut(&pid).unwrap();
        assert_eq!(space.active.remove(&reservation.base_va), Some(reservation));
        space
            .slots
            .insert(reservation.start_slot, reservation.page_count);
        space
            .pages
            .insert(reservation.base_va / USER_PAGE_SIZE, reservation.page_count);
    }

    pub(super) fn exclude_shared_virtual_range(
        &mut self,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) {
        if let Some(space) = self.shared_memory.get_mut(&pid) {
            space.pages.exclude(base_va / USER_PAGE_SIZE, page_count);
        }
    }
}
