use super::*;

// Only Alpha-owned physical RAM Frame capabilities, never child aliases. Their
// lifetime is the physical pool's lifetime, so process revoke/reap cannot leave
// a dangling mapping. This caches mappings, NOT page contents: every allocation
// is still zeroed or filled before it is exposed to its new owner.
const BASE: usize = 0x6700_0000;
// Retain both sides of an 8-MiB input/copy working set. This consumes 64 KiB
// of tags on 64-bit targets, without reserving another byte of payload RAM.
// The compact layout remains available for memory-constrained configurations.
const SMALL: bool = match option_env!("NANAMI_SMALL_INIT_WINDOW") {
    Some(value) => value.len() == 1 && value.as_bytes()[0] == b'1',
    None => false,
};
pub(super) const PAGES: usize = if SMALL { 2048 } else { 4096 };
const _: () = assert!(PAGES.is_power_of_two() && PAGES <= u16::MAX as usize);
const _: () = assert!(BASE + PAGES * PAGE_SIZE <= PROCESS_COPY_TEMP_BASE);

#[cfg(not(test))]
#[repr(C)]
pub(super) struct Diagnostics {
    magic: usize,
    init_maps: core::sync::atomic::AtomicUsize,
    init_unmaps: core::sync::atomic::AtomicUsize,
    temporary_maps: core::sync::atomic::AtomicUsize,
    temporary_unmaps: core::sync::atomic::AtomicUsize,
}

#[cfg(not(test))]
#[no_mangle]
pub(super) static NANAMI_INIT_DIAGNOSTICS: Diagnostics = Diagnostics {
    magic: 0x494e4954,
    init_maps: core::sync::atomic::AtomicUsize::new(0),
    init_unmaps: core::sync::atomic::AtomicUsize::new(0),
    temporary_maps: core::sync::atomic::AtomicUsize::new(0),
    temporary_unmaps: core::sync::atomic::AtomicUsize::new(0),
};

#[cfg(not(test))]
#[inline]
pub(super) fn record_mapping(initializer: bool, mapped: bool) {
    if option_env!("NANAMI_INIT_WINDOW_DIAG") == Some("1") {
        use core::sync::atomic::Ordering;
        let stats = &NANAMI_INIT_DIAGNOSTICS;
        let counter = match (initializer, mapped) {
            (true, true) => &stats.init_maps,
            (true, false) => &stats.init_unmaps,
            (false, true) => &stats.temporary_maps,
            (false, false) => &stats.temporary_unmaps,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

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
        let (address, large_pages) = self.map_initialization_page_span(first)?;
        if large_pages > 1 { return Ok((address, large_pages.min(maximum))); }
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

    /// Borrowing another Ready alias is useful only on an initializer miss.
    /// Pending cleanup must still follow the ordinary error/retry path.
    pub(super) fn initialization_page_needs_mapping(&self, physical_page: usize) -> bool {
        let Some(frame) = self.memory.physical_frame_descriptor_from_index(physical_page) else { return false; };
        let mapping = self.initialization_window.mappings[physical_page % PAGES];
        mapping.state == MappingState::Empty
            || (mapping.state == MappingState::Ready && mapping.frame != frame)
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
        self.map_initialization_page_span(physical_page).map(|span| span.0)
    }

    fn map_initialization_page_span(&mut self, physical_page: usize)
        -> Result<(usize, usize), CapabilityError> {
        let frame = self
            .memory
            .physical_frame_descriptor_from_index(physical_page)
            .ok_or(CapabilityError::InvalidArgument)?;
        if let Some(span) = self.memory.map_private_large_alias(frame, None)? { return Ok(span); }
        let index = physical_page % PAGES;
        let va = self.initialization_alias_address(physical_page);
        let mapping = self.initialization_window.mappings[index];
        if mapping.state != MappingState::Empty {
            if mapping.state == MappingState::Ready && mapping.frame == frame {
                self.initialization_window.remember(frame, index);
                return Ok((va, 1));
            }
            if mapping.state == MappingState::Ready { self.initialization_window.forget(index); }
            self.initialization_window.mappings[index].state = MappingState::Pending;
            // Preserve failed cleanup in the cache; never overwrite a live PTE.
            self.unmap_alpha_frame(mapping.frame, va)?;
            #[cfg(not(test))]
            record_mapping(true, false);
            self.initialization_window.mappings[index].state = MappingState::Empty;
        }
        // A failed MAP may have installed a PTE. Retain it for cleanup, but
        // never reuse the address as a hit until MAP has completed successfully.
        self.initialization_window.mappings[index].frame = frame;
        self.initialization_window.mappings[index].state = MappingState::Pending;
        self.map_alpha_temporary_frame(frame, va)?;
        #[cfg(not(test))]
        record_mapping(true, true);
        self.initialization_window.mappings[index].state = MappingState::Ready;
        self.initialization_window.remember(frame, index);
        Ok((va, 1))
    }
}
