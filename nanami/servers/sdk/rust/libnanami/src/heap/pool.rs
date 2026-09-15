use core::ptr::{self, NonNull};

use rlsf::{Tlsf, GRANULARITY};

pub(super) const REGION_LIMIT: usize = 16;
const SECOND_LEVEL_COUNT: usize = 32;

// On 64-bit targets this covers individual blocks below 128 GiB. Larger
// requests are rejected before asking Alpha for an unusable memory mapping.
type Allocator = Tlsf<'static, u32, u32, 32, SECOND_LEVEL_COUNT>;
const MAX_POOL_SIZE: usize = GRANULARITY << 32;

pub(super) struct Pool {
    allocator: Allocator,
    regions: [Option<NonNull<[u8]>>; REGION_LIMIT],
    region_count: usize,
}

impl Pool {
    pub(super) const fn new() -> Self {
        Self {
            allocator: Allocator::new(),
            regions: [None; REGION_LIMIT],
            region_count: 0,
        }
    }

    pub(super) fn can_grow(&self) -> bool {
        self.region_count < REGION_LIMIT
    }

    /// `base..base+size` must be exclusively owned writable RAM that remains
    /// mapped for this pool's lifetime. Pools are never joined across guards.
    pub(super) unsafe fn add_region(&mut self, base: usize, size: usize) -> bool {
        let Some(end) = base.checked_add(size) else {
            return false;
        };
        if base == 0 || size > isize::MAX as usize || !self.can_grow() {
            return false;
        }
        // Keep exact, aligned ranges for the inspection API. Validate before
        // letting TLSF write any metadata into the mapping.
        let Some(start) = base.checked_add(GRANULARITY - 1) else {
            return false;
        };
        let start = start & !(GRANULARITY - 1);
        let end = end & !(GRANULARITY - 1);
        if end.saturating_sub(start) < GRANULARITY * 2 {
            return false;
        }
        for region in self.regions[..self.region_count].iter().flatten() {
            let other_start = region.as_ptr() as *mut u8 as usize;
            let other_end = other_start + region.len();
            if start < other_end && other_start < end {
                return false;
            }
        }
        let region =
            NonNull::new_unchecked(ptr::slice_from_raw_parts_mut(start as *mut u8, end - start));
        let Some(used) = self.allocator.insert_free_block_ptr(region) else {
            return false;
        };
        self.regions[self.region_count] = Some(NonNull::new_unchecked(
            ptr::slice_from_raw_parts_mut(start as *mut u8, used.get()),
        ));
        self.region_count += 1;
        true
    }

    pub(super) fn allocate(&mut self, layout: core::alloc::Layout) -> *mut u8 {
        self.allocator
            .allocate(layout)
            .map_or(ptr::null_mut(), NonNull::as_ptr)
    }

    /// GlobalAlloc's live-pointer and original-layout contract applies.
    pub(super) unsafe fn deallocate(&mut self, pointer: NonNull<u8>, align: usize) {
        self.allocator.deallocate(pointer, align);
    }

    /// `pointer` is a live allocation with `layout.align()`. Failure preserves
    /// that allocation. TLSF first attempts growth/shrinkage in place.
    pub(super) unsafe fn reallocate(
        &mut self,
        pointer: NonNull<u8>,
        layout: core::alloc::Layout,
    ) -> *mut u8 {
        self.allocator
            .reallocate(pointer, layout)
            .map_or(ptr::null_mut(), NonNull::as_ptr)
    }

    // Diagnostic only: used/free include block headers and alignment padding,
    // as in the previous allocator. TLSF's permanent sentinels are excluded.
    pub(super) fn stats(&self) -> (usize, usize, usize) {
        let mut used = 0;
        let mut free = 0;
        for region in self.regions[..self.region_count].iter().flatten() {
            // Every range is exactly the one accepted by insert_free_block_ptr,
            // and the caller holds the allocator lock throughout inspection.
            for block in unsafe { self.allocator.iter_blocks(*region) } {
                if block.is_occupied() {
                    used += block.size();
                } else {
                    free += block.size();
                }
            }
        }
        (used, free, used + free)
    }
}

pub(super) fn growth_size(layout: core::alloc::Layout) -> Option<usize> {
    const PAGE_SIZE: usize = 4096;
    const GROW_CHUNK: usize = 4 * 1024 * 1024;
    // Include alignment, headers/sentinel, and the TLSF size-class rounding
    // slack (at most 1/SECOND_LEVEL_COUNT), not just the requested payload.
    let required = layout
        .size()
        .checked_add(layout.align())?
        .checked_add(GRANULARITY * 2)?;
    let required = required.checked_add(required / SECOND_LEVEL_COUNT)?;
    let size = required.max(GROW_CHUNK).checked_add(PAGE_SIZE - 1)? & !(PAGE_SIZE - 1);
    (size < MAX_POOL_SIZE && size <= isize::MAX as usize).then_some(size)
}
