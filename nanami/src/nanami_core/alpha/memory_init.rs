use super::*;

// Only Alpha-owned physical RAM Frame capabilities, never child aliases. Their
// lifetime is the physical pool's lifetime, so process revoke/reap cannot leave
// a dangling mapping. This caches mappings, NOT page contents: every allocation
// is still zeroed or filled before it is exposed to its new owner.
const BASE: usize = 0x6700_0000;
// Keep the mapping cache within an 8-MiB alias region (32 KiB of tags on
// 64-bit targets), leaving space before the separate image-staging window.
pub(super) const PAGES: usize = 2048;
const _: () = assert!(PAGES.is_power_of_two() && PAGES <= u16::MAX as usize);
const _: () = assert!(BASE + PAGES * PAGE_SIZE <= PROCESS_COPY_TEMP_BASE);

#[derive(Clone, Copy)]
struct Mapping {
    frame: CapabilityDescriptor,
    state: MappingState,
    // Bucket heads are independent of the physical row occupying this slot.
    // Heads and Ready-entry links fit the original row's padding.
    head: u16,
    previous: u16,
    next: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MappingState { Empty, Pending, Ready }

impl Mapping {
    const EMPTY: Self = Self { frame: 0, state: MappingState::Empty, head: 0, previous: 0, next: 0 };
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(core::mem::size_of::<Mapping>() == 16);

pub(super) struct InitializationWindow {
    mappings: [Mapping; PAGES],
}

impl InitializationWindow {
    pub(super) const fn new() -> Self {
        Self {
            mappings: [Mapping::EMPTY; PAGES],
        }
    }

    fn bucket(frame: CapabilityDescriptor) -> usize {
        let mut hash = frame as u64;
        hash ^= hash >> 30;
        hash = hash.wrapping_mul(0xbf58476d1ce4e5b9);
        hash ^= hash >> 27;
        hash = hash.wrapping_mul(0x94d049bb133111eb);
        (hash ^ (hash >> 31)) as usize & (PAGES - 1)
    }

    fn remember(&mut self, frame: CapabilityDescriptor, index: usize) {
        debug_assert!(self.mappings[index].state == MappingState::Ready && self.mappings[index].frame == frame);
        let bucket = Self::bucket(frame);
        let link = (index + 1) as u16;
        if self.mappings[bucket].head == link { return; }
        let mapping = self.mappings[index];
        if mapping.previous != 0 || mapping.next != 0 { self.unlink(index, bucket); }
        let next = self.mappings[bucket].head;
        self.mappings[index].previous = 0;
        self.mappings[index].next = next;
        if next != 0 { self.mappings[usize::from(next) - 1].previous = link; }
        self.mappings[bucket].head = link;
    }

    fn unlink(&mut self, index: usize, bucket: usize) {
        let mapping = self.mappings[index];
        if mapping.previous == 0 {
            self.mappings[bucket].head = mapping.next;
        } else {
            self.mappings[usize::from(mapping.previous) - 1].next = mapping.next;
        }
        if mapping.next != 0 {
            self.mappings[usize::from(mapping.next) - 1].previous = mapping.previous;
        }
        self.mappings[index].previous = 0;
        self.mappings[index].next = 0;
    }

    fn forget(&mut self, index: usize) {
        self.unlink(index, Self::bucket(self.mappings[index].frame));
    }

    #[cfg(test)]
    pub(super) fn assert_consistent(&self) {
        let mut seen = [false; PAGES];
        for bucket in 0..PAGES {
            let mut link = self.mappings[bucket].head;
            let mut previous = 0;
            while link != 0 {
                let index = usize::from(link) - 1;
                assert!(!seen[index]); seen[index] = true;
                let mapping = self.mappings[index];
                assert!(mapping.state == MappingState::Ready);
                assert_eq!(Self::bucket(mapping.frame), bucket);
                assert_eq!(mapping.previous, previous);
                previous = link; link = mapping.next;
            }
        }
        for (index, mapping) in self.mappings.iter().enumerate() {
            assert_eq!(seen[index], mapping.state == MappingState::Ready);
        }
    }
}

impl Alpha {
    // Map the first page with normal failure/cleanup semantics. Extend only
    // through already Ready, exact-Frame aliases; never evict/map ahead of
    // the caller's zeroing progress, and never cross the alias-window wrap.
    pub(super) fn map_initialization_run(
        &mut self, first: usize, maximum: usize,
    ) -> Result<(usize, usize), CapabilityError> {
        if maximum == 0 { return Err(CapabilityError::InvalidArgument); }
        let address = self.map_initialization_page(first)?;
        let limit = maximum.min(PAGES - first % PAGES);
        let mut count = 1;
        while count < limit {
            let Some(page) = first.checked_add(count) else { break; };
            let Some(frame) = self.memory.physical_frame_descriptor_from_index(page) else { break; };
            let index = page % PAGES;
            let mapping = self.initialization_window.mappings[index];
            if mapping.state != MappingState::Ready || mapping.frame != frame { break; }
            // Preserve the sequential mapper's most-recent lookup order.
            self.initialization_window.remember(frame, index);
            count += 1;
        }
        Ok((address, count))
    }

    pub(super) fn initialization_alias_address(&self, physical_page: usize) -> usize {
        BASE + (physical_page % PAGES) * PAGE_SIZE
    }
    /// Read-only probe: never create/evict an alias while another alias is pinned.
    /// Caller must separately validate current source ownership and Frame identity.
    pub(super) fn cached_initialization_page(&self, physical_page: usize) -> Option<usize> {
        let frame = self.memory.physical_frame_descriptor_from_index(physical_page)?;
        let index = physical_page % PAGES;
        let mapping = self.initialization_window.mappings[index];
        (mapping.state == MappingState::Ready && mapping.frame == frame)
            .then_some(self.initialization_alias_address(physical_page))
    }

    /// Optional read-only lookup by an already resolved CURRENT process Frame.
    /// No mapping changes, ownership inference or retained content snapshot.
    pub(super) fn cached_initialization_frame(&self, frame: CapabilityDescriptor) -> Option<usize> {
        self.cached_initialization_slot(frame).map(|index| BASE + index * PAGE_SIZE)
    }

    pub(super) fn cached_initialization_slot(&self, frame: CapabilityDescriptor) -> Option<usize> {
        let mut link = self.initialization_window.mappings[InitializationWindow::bucket(frame)].head;
        while link != 0 {
            let index = usize::from(link) - 1;
            let mapping = self.initialization_window.mappings[index];
            if mapping.state == MappingState::Ready && mapping.frame == frame {
                return Some(index);
            }
            link = mapping.next;
        }
        None
    }

    pub(super) fn map_initialization_page(
        &mut self,
        physical_page: usize,
    ) -> Result<usize, CapabilityError> {
        let frame = self
            .memory
            .physical_frame_descriptor_from_index(physical_page)
            .ok_or(CapabilityError::InvalidArgument)?;
        let index = physical_page % PAGES;
        let va = self.initialization_alias_address(physical_page);
        let mapping = self.initialization_window.mappings[index];
        if mapping.state != MappingState::Empty {
            if mapping.state == MappingState::Ready && mapping.frame == frame {
                self.initialization_window.remember(frame, index);
                return Ok(va);
            }
            if mapping.state == MappingState::Ready { self.initialization_window.forget(index); }
            self.initialization_window.mappings[index].state = MappingState::Pending;
            // Preserve failed cleanup in the cache; never overwrite a live PTE.
            self.unmap_alpha_frame(mapping.frame, va)?;
            self.initialization_window.mappings[index].state = MappingState::Empty;
        }
        // A failed MAP may have installed a PTE. Retain it for cleanup, but
        // never reuse the address as a hit until MAP has completed successfully.
        self.initialization_window.mappings[index].frame = frame;
        self.initialization_window.mappings[index].state = MappingState::Pending;
        self.map_alpha_temporary_frame(frame, va)?;
        self.initialization_window.mappings[index].state = MappingState::Ready;
        self.initialization_window.remember(frame, index);
        Ok(va)
    }
}
