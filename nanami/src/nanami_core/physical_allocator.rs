use alloc::vec::Vec;

const PAGE_BITS: usize = 12;
const PAGE_SIZE: usize = 1 << PAGE_BITS;
const INITIAL_INDEX_BUCKETS: usize = 32;
const MAX_INDEX_BYTES: usize = 256 * 1024;
const INDEX_BUCKET_LOAD: usize = 4;

#[cfg(test)]
static CONTAINING_SEARCHES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
fn take_containing_searches() -> usize {
    CONTAINING_SEARCHES.swap(0, core::sync::atomic::Ordering::Relaxed)
}

#[derive(Clone, Copy)]
pub struct PhysicalAllocation {
    pub base_page: usize,
    pub page_count: usize,
    pub is_device: bool,
}

#[derive(Clone, Copy)]
pub struct PhysicalMemoryInfo {
    pub total_pages: usize,
    pub free_pages: usize,
}

#[derive(Clone, Copy)]
pub enum PhysicalAllocError {
    InvalidArgument,
    PermissionDenied,
    OutOfMemory,
}

#[derive(Clone, Copy)]
struct AllocationRecord {
    base_page: usize,
    page_count: usize,
    is_device: bool,
    reserved: bool,
    // One-based Vec index; zero ends the chain. Fits existing 64-bit padding.
    next: u32,
}

struct BuddyPool {
    free_lists: Vec<Vec<usize>>,
}

impl BuddyPool {
    const fn new() -> Self {
        Self {
            free_lists: Vec::new(),
        }
    }

    fn add_range(
        &mut self,
        mut base_page: usize,
        mut page_count: usize,
    ) -> Result<(), PhysicalAllocError> {
        if page_count == 0 {
            return Ok(());
        }

        while page_count != 0 {
            let order = largest_aligned_order(base_page, page_count)?;
            self.add_block(base_page, order)?;
            let count = block_pages(order)?;
            base_page = base_page
                .checked_add(count)
                .ok_or(PhysicalAllocError::InvalidArgument)?;
            page_count -= count;
        }
        Ok(())
    }

    fn add_block(
        &mut self,
        mut base_page: usize,
        mut order: usize,
    ) -> Result<(), PhysicalAllocError> {
        loop {
            self.ensure_order(order);
            let count = block_pages(order)?;
            if base_page % count != 0 {
                return Err(PhysicalAllocError::InvalidArgument);
            }

            let buddy = base_page ^ count;
            // A missing buddy previously required a second scan for this
            // block. Remember duplicates during the buddy scan instead. A
            // buddy still takes priority even if a duplicate appeared first.
            let mut already_free = false;
            let buddy_index = self.free_lists[order].iter().position(|&page| {
                already_free |= page == base_page;
                page == buddy
            });
            if let Some(index) = buddy_index {
                self.free_lists[order].swap_remove(index);
                if buddy < base_page {
                    base_page = buddy;
                }
                order = order
                    .checked_add(1)
                    .ok_or(PhysicalAllocError::InvalidArgument)?;
                continue;
            }

            if !already_free {
                self.free_lists[order].push(base_page);
            }
            return Ok(());
        }
    }

    fn allocate_range(
        &mut self,
        base_page: usize,
        page_count: usize,
    ) -> Result<(), PhysicalAllocError> {
        if page_count == 0 {
            return Err(PhysicalAllocError::InvalidArgument);
        }
        let _ = base_page
            .checked_add(page_count)
            .ok_or(PhysicalAllocError::InvalidArgument)?;

        let first = self.validated_first_block(base_page, page_count)
            .ok_or(PhysicalAllocError::OutOfMemory)?;
        // Complete preflight is read-only. This first location is still exact
        // until carving starts; later locations must be found after mutation.
        self.carve_range(base_page, page_count, first)
    }

    fn contains_range(&self, base_page: usize, page_count: usize) -> bool {
        self.validated_first_block(base_page, page_count).is_some()
    }

    #[inline(always)]
    fn validated_first_block(
        &self, base_page: usize, page_count: usize,
    ) -> Option<(usize, usize, usize)> {
        let end_page = base_page.checked_add(page_count)?;
        if page_count == 0 {
            return None;
        }
        let first = self.find_containing_block(base_page)?;
        let mut block = first;
        loop {
            let (order, _, block_base) = block;
            let block_count = block_pages(order).ok()?;
            let block_end = block_base.checked_add(block_count)?;
            let current = core::cmp::min(block_end, end_page);
            if current == end_page {
                return Some(first);
            }
            block = self.find_containing_block(current)?;
        }
    }

    fn carve_range(
        &mut self,
        base_page: usize,
        page_count: usize,
        first: (usize, usize, usize),
    ) -> Result<(), PhysicalAllocError> {
        let end_page = base_page
            .checked_add(page_count)
            .ok_or(PhysicalAllocError::InvalidArgument)?;
        let mut current = base_page;
        let mut block = first;

        loop {
            let (order, index, block_base) = block;

            let block_count = block_pages(order)?;
            let block_end = block_base
                .checked_add(block_count)
                .ok_or(PhysicalAllocError::InvalidArgument)?;
            self.free_lists[order].swap_remove(index);

            if block_base < current {
                self.add_range(block_base, current - block_base)?;
            }

            let allocated_end = if block_end < end_page {
                block_end
            } else {
                end_page
            };
            if allocated_end < block_end {
                self.add_range(allocated_end, block_end - allocated_end)?;
            }
            current = allocated_end;
            if current == end_page {
                return Ok(());
            }
            block = self.find_containing_block(current)
                .ok_or(PhysicalAllocError::OutOfMemory)?;
        }
    }

    fn find_first_fit(&self, page_count: usize) -> Option<usize> {
        if page_count == 0 {
            return None;
        }
        let mut order = ceil_log2(page_count)?;
        while order < self.free_lists.len() {
            if let Some(base_page) = self.free_lists[order].first() {
                return Some(*base_page);
            }
            order += 1;
        }
        None
    }

    fn find_containing_block(&self, page: usize) -> Option<(usize, usize, usize)> {
        #[cfg(test)]
        CONTAINING_SEARCHES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let mut order = 0usize;
        while order < self.free_lists.len() {
            let count = block_pages(order).ok()?;
            let mut index = 0usize;
            while index < self.free_lists[order].len() {
                let base = self.free_lists[order][index];
                let end = base.checked_add(count)?;
                if page >= base && page < end {
                    return Some((order, index, base));
                }
                index += 1;
            }
            order += 1;
        }
        None
    }

    fn ensure_order(&mut self, order: usize) {
        while self.free_lists.len() <= order {
            self.free_lists.push(Vec::new());
        }
    }

    fn free_page_count(&self) -> usize {
        let mut pages = 0usize;
        let mut order = 0usize;
        while order < self.free_lists.len() {
            let block_count = 1usize.checked_shl(order as u32).unwrap_or(0);
            pages = pages.saturating_add(self.free_lists[order].len().saturating_mul(block_count));
            order += 1;
        }
        pages
    }
}

pub struct PhysicalAllocator {
    normal: BuddyPool,
    device: BuddyPool,
    allocations: Vec<AllocationRecord>,
    allocation_buckets: Vec<u32>,
    // Keep the exact ledger sum until the query clamps to usize. A saturated
    // cached usize could not recover correctly after removing a large record.
    // The 32/64-bit platforms' Vec length and record widths bound this sum
    // below u128::MAX, including duplicate reserved-region records.
    allocated_normal_pages: u128,
}

impl PhysicalAllocator {
    pub fn new() -> Self {
        Self {
            normal: BuddyPool::new(),
            device: BuddyPool::new(),
            allocations: Vec::new(),
            allocation_buckets: Vec::new(),
            allocated_normal_pages: 0,
        }
    }

    pub fn add_region(
        &mut self,
        base_address: usize,
        size_bytes: usize,
        is_device: bool,
        used: bool,
    ) -> Result<(), PhysicalAllocError> {
        if !is_page_aligned(base_address) || size_bytes == 0 {
            return Err(PhysicalAllocError::InvalidArgument);
        }
        let page_count = bytes_to_pages(size_bytes);
        if page_count == 0 {
            return Err(PhysicalAllocError::InvalidArgument);
        }
        let base_page = base_address >> PAGE_BITS;

        if used {
            self.push_record(AllocationRecord {
                base_page,
                page_count,
                is_device,
                reserved: true,
                next: 0,
            });
            return Ok(());
        }

        self.pool_mut(is_device).add_range(base_page, page_count)
    }

    pub fn allocate_at(
        &mut self,
        base_address: usize,
        size_bytes: usize,
        allow_device: bool,
    ) -> Result<PhysicalAllocation, PhysicalAllocError> {
        if !is_page_aligned(base_address) || size_bytes == 0 {
            return Err(PhysicalAllocError::InvalidArgument);
        }
        let req_base = base_address >> PAGE_BITS;
        let req_count = bytes_to_pages(size_bytes);

        if self.normal.allocate_range(req_base, req_count).is_ok() {
            self.record_allocation(req_base, req_count, false);
            return Ok(PhysicalAllocation {
                base_page: req_base,
                page_count: req_count,
                is_device: false,
            });
        }

        if self.device.contains_range(req_base, req_count) && !allow_device {
            return Err(PhysicalAllocError::PermissionDenied);
        }
        if allow_device && self.device.allocate_range(req_base, req_count).is_ok() {
            self.record_allocation(req_base, req_count, true);
            return Ok(PhysicalAllocation {
                base_page: req_base,
                page_count: req_count,
                is_device: true,
            });
        }

        Err(PhysicalAllocError::OutOfMemory)
    }

    pub fn allocate_any(
        &mut self,
        size_bytes: usize,
    ) -> Result<PhysicalAllocation, PhysicalAllocError> {
        if size_bytes == 0 {
            return Err(PhysicalAllocError::InvalidArgument);
        }
        let req_count = bytes_to_pages(size_bytes);
        let base_page = self
            .normal
            .find_first_fit(req_count)
            .ok_or(PhysicalAllocError::OutOfMemory)?;

        self.allocate_at(base_page << PAGE_BITS, req_count << PAGE_BITS, false)
    }

    /// Reserve an aligned power-of-two RAM block within an untyped extent.
    /// Search buddy blocks, not every physical page in a potentially huge map.
    pub fn allocate_aligned_in_range(
        &mut self,
        start: usize,
        end: usize,
        bytes: usize,
    ) -> Result<PhysicalAllocation, PhysicalAllocError> {
        if bytes < PAGE_SIZE || !bytes.is_power_of_two() || start >= end {
            return Err(PhysicalAllocError::InvalidArgument);
        }
        let wanted_order = (bytes >> PAGE_BITS).ilog2() as usize;
        let mut selected = None;
        for order in wanted_order..self.normal.free_lists.len() {
            let block_bytes = block_pages(order)?
                .checked_mul(PAGE_SIZE)
                .ok_or(PhysicalAllocError::InvalidArgument)?;
            for &page in &self.normal.free_lists[order] {
                let base = page << PAGE_BITS;
                let Some(block_end) = base.checked_add(block_bytes) else {
                    continue;
                };
                let Some(address) = base
                    .max(start)
                    .checked_add(bytes - 1)
                    .map(|value| value & !(bytes - 1))
                else {
                    continue;
                };
                if address
                    .checked_add(bytes)
                    .is_some_and(|limit| limit <= end.min(block_end))
                    && selected.is_none_or(|best| address < best)
                {
                    selected = Some(address);
                }
            }
        }
        let address = selected.ok_or(PhysicalAllocError::OutOfMemory)?;
        self.allocate_at(address, bytes, false)
    }

    pub fn free(
        &mut self,
        base_address: usize,
        size_bytes: usize,
    ) -> Result<(), PhysicalAllocError> {
        if !is_page_aligned(base_address) || size_bytes == 0 {
            return Err(PhysicalAllocError::InvalidArgument);
        }

        let req_base = base_address >> PAGE_BITS;
        let req_count = bytes_to_pages(size_bytes);
        let req_end = req_base
            .checked_add(req_count)
            .ok_or(PhysicalAllocError::InvalidArgument)?;

        let contains = |allocation: &AllocationRecord| {
            if allocation.reserved {
                return false;
            }
            let allocation_end = allocation.base_page + allocation.page_count;
            req_base >= allocation.base_page && req_end <= allocation_end
        };
        // Process pages are normally released newest-first. Check the last
        // live allocation before scanning unrelated, longer-lived allocations.
        // Interleaved exact-base releases use the compact index; interior
        // partial releases retain the same validated linear fallback.
        let index = self
            .allocations
            .len()
            .checked_sub(1)
            .filter(|&index| contains(&self.allocations[index]))
            .or_else(|| self.find_exact_record(req_base, req_end))
            .or_else(|| self.allocations.iter().position(contains))
            .ok_or(PhysicalAllocError::InvalidArgument)?;

        self.unindex_record(index);
        let last = self.allocations.len() - 1;
        if index != last {
            self.unindex_record(last);
        }
        let allocation = self.allocations.swap_remove(index);
        if !allocation.is_device {
            self.allocated_normal_pages -= allocation.page_count as u128;
        }
        if index != last {
            self.index_record(index);
        }
        let allocation_end = allocation
            .base_page
            .checked_add(allocation.page_count)
            .ok_or(PhysicalAllocError::InvalidArgument)?;

        if allocation.base_page < req_base {
            self.record_allocation(
                allocation.base_page,
                req_base - allocation.base_page,
                allocation.is_device,
            );
        }
        if req_end < allocation_end {
            self.record_allocation(req_end, allocation_end - req_end, allocation.is_device);
        }

        self.pool_mut(allocation.is_device)
            .add_range(req_base, req_count)
    }

    pub fn memory_info(&self) -> PhysicalMemoryInfo {
        let free_pages = self.normal.free_page_count();
        let allocated_pages = self.allocated_normal_pages.min(usize::MAX as u128) as usize;
        PhysicalMemoryInfo {
            total_pages: free_pages.saturating_add(allocated_pages),
            free_pages,
        }
    }

    fn record_allocation(&mut self, base_page: usize, page_count: usize, is_device: bool) {
        self.push_record(AllocationRecord {
            base_page,
            page_count,
            is_device,
            reserved: false,
            next: 0,
        });
    }

    fn push_record(&mut self, record: AllocationRecord) {
        let index = self.allocations.len();
        self.allocations.push(record);
        if !record.is_device {
            self.allocated_normal_pages += record.page_count as u128;
        }
        let buckets = self.allocation_buckets.len();
        let desired = if buckets == 0 {
            INITIAL_INDEX_BUCKETS
        } else if self.allocations.len() / INDEX_BUCKET_LOAD >= buckets {
            (buckets * 2).min(MAX_INDEX_BYTES / core::mem::size_of::<u32>())
        } else {
            buckets
        };
        // This is an optional lookup aid, not allocation authority. An index
        // allocation failure must not fail an otherwise valid RAM allocation.
        if desired > buckets
            && self
                .allocation_buckets
                .try_reserve_exact(desired - buckets)
                .is_ok()
        {
            self.allocation_buckets.resize(desired, 0);
            self.allocation_buckets.fill(0);
            for index in 0..self.allocations.len() {
                self.index_record(index);
            }
        } else {
            self.index_record(index);
        }
    }

    fn record_link(index: usize) -> Option<u32> {
        u32::try_from(index).ok()?.checked_add(1)
    }

    fn record_bucket(&self, base_page: usize) -> usize {
        let mut hash = base_page as u64;
        hash ^= hash >> 30;
        hash = hash.wrapping_mul(0xbf58476d1ce4e5b9);
        hash ^= hash >> 27;
        hash = hash.wrapping_mul(0x94d049bb133111eb);
        hash ^= hash >> 31;
        (hash as usize) & (self.allocation_buckets.len() - 1)
    }

    fn index_record(&mut self, index: usize) {
        let Some(link) = Self::record_link(index) else {
            return;
        };
        if self.allocation_buckets.is_empty() {
            return;
        }
        let bucket = self.record_bucket(self.allocations[index].base_page);
        self.allocations[index].next = self.allocation_buckets[bucket];
        self.allocation_buckets[bucket] = link;
    }

    fn unindex_record(&mut self, index: usize) {
        let Some(target) = Self::record_link(index) else {
            return;
        };
        if self.allocation_buckets.is_empty() {
            return;
        }
        let bucket = self.record_bucket(self.allocations[index].base_page);
        let mut link = self.allocation_buckets[bucket];
        let mut previous: Option<usize> = None;
        while link != 0 {
            let current = link as usize - 1;
            let next = self.allocations[current].next;
            if link == target {
                if let Some(previous) = previous {
                    self.allocations[previous].next = next;
                } else {
                    self.allocation_buckets[bucket] = next;
                }
                return;
            }
            previous = Some(current);
            link = next;
        }
    }

    fn find_exact_record(&self, base_page: usize, end_page: usize) -> Option<usize> {
        if self.allocation_buckets.is_empty() {
            return None;
        }
        let mut link = self.allocation_buckets[self.record_bucket(base_page)];
        let mut found = None;
        while link != 0 {
            let index = link as usize - 1;
            let allocation = self.allocations[index];
            if allocation.base_page == base_page
                && !allocation.reserved
                && end_page <= allocation.base_page + allocation.page_count
            {
                found = Some(found.map_or(index, |previous: usize| previous.min(index)));
            }
            link = allocation.next;
        }
        found
    }

    fn pool_mut(&mut self, is_device: bool) -> &mut BuddyPool {
        if is_device {
            &mut self.device
        } else {
            &mut self.normal
        }
    }
}

fn bytes_to_pages(size_bytes: usize) -> usize {
    // Preserve release-mode rounding for oversized requests: an overflowing
    // alignment addition yields zero pages and allocation rejects the request.
    size_bytes.wrapping_add(PAGE_SIZE - 1) / PAGE_SIZE
}

fn is_page_aligned(address: usize) -> bool {
    address & (PAGE_SIZE - 1) == 0
}

fn block_pages(order: usize) -> Result<usize, PhysicalAllocError> {
    1usize
        .checked_shl(order as u32)
        .ok_or(PhysicalAllocError::InvalidArgument)
}

fn floor_log2(value: usize) -> Option<usize> {
    if value == 0 {
        return None;
    }
    Some(usize::BITS as usize - 1 - value.leading_zeros() as usize)
}

fn ceil_log2(value: usize) -> Option<usize> {
    let floor = floor_log2(value)?;
    if value.is_power_of_two() {
        Some(floor)
    } else {
        floor.checked_add(1)
    }
}

fn largest_aligned_order(base_page: usize, page_count: usize) -> Result<usize, PhysicalAllocError> {
    let mut order = floor_log2(page_count).ok_or(PhysicalAllocError::InvalidArgument)?;
    loop {
        let count = block_pages(order)?;
        if base_page % count == 0 {
            return Ok(order);
        }
        if order == 0 {
            return Ok(0);
        }
        order -= 1;
    }
}

#[cfg(test)]
mod info_tests {
    use super::*;

    fn check(allocator: &PhysicalAllocator) {
        let free = allocator.normal.free_page_count();
        let allocated = allocator.allocations.iter().filter(|record| !record.is_device)
            .fold(0usize, |sum, record| sum.saturating_add(record.page_count));
        let info = allocator.memory_info();
        assert_eq!((info.total_pages, info.free_pages), (free.saturating_add(allocated), free));
    }

    #[test]
    fn info_matches_record_oracle_through_partial_free_reserved_and_device_churn() {
        let mut allocator = PhysicalAllocator::new();
        assert!(allocator.add_region(0x100000, 512 * PAGE_SIZE, false, false).is_ok());
        assert!(allocator.add_region(0x900000, 7 * PAGE_SIZE, false, true).is_ok());
        assert!(allocator.add_region(0x10000000, 32 * PAGE_SIZE, true, false).is_ok());
        assert!(allocator.add_region(0x20000000, 3 * PAGE_SIZE, true, true).is_ok());
        assert!(allocator.allocate_at(0x10000000, 8 * PAGE_SIZE, true).is_ok());
        assert!(allocator.free(0x10002000, 2 * PAGE_SIZE).is_ok());
        let mut live = Vec::new();
        let mut random = 0x1234_5678u64;
        for step in 0..10000 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            if live.is_empty() || random & 3 != 0 {
                if let Ok(allocation) = allocator.allocate_any(((random >> 8) as usize % 8 + 1) * PAGE_SIZE) {
                    live.push((allocation.base_page, allocation.page_count));
                }
            } else {
                let index = (random >> 16) as usize % live.len();
                let (base, pages) = live.swap_remove(index);
                let offset = step % pages;
                assert!(allocator.free((base + offset) * PAGE_SIZE, PAGE_SIZE).is_ok());
                if offset != 0 { live.push((base, offset)); }
                if offset + 1 < pages { live.push((base + offset + 1, pages - offset - 1)); }
            }
            assert!(allocator.free(0x900000, PAGE_SIZE).is_err());
            check(&allocator);
            assert_eq!(allocator.memory_info().total_pages, 519);
        }
        for (base, pages) in live {
            assert!(allocator.free(base * PAGE_SIZE, pages * PAGE_SIZE).is_ok());
            check(&allocator);
        }
        assert_eq!(allocator.memory_info().free_pages, 512);
    }

    #[test]
    fn info_saturation_can_recover_after_record_removal() {
        // Synthetic ledger values isolate saturation from physical map limits.
        // Removing a prefix must recover the exact sum, not subtract from an
        // already clamped usize total and silently lose the overflow excess.
        let mut allocator = PhysicalAllocator::new();
        for (base, pages) in [(0, usize::MAX - 2), (usize::MAX - 8, 7)] {
            allocator.push_record(AllocationRecord {
                base_page: base, page_count: pages, is_device: false, reserved: false, next: 0,
            });
        }
        check(&allocator);
        assert_eq!(allocator.memory_info().total_pages, usize::MAX);
        assert!(allocator.free(0, 6 * PAGE_SIZE).is_ok());
        check(&allocator);
        assert_eq!(allocator.memory_info().free_pages, 6);
        allocator.normal = BuddyPool::new();
        check(&allocator);
        assert_eq!(allocator.memory_info().total_pages, usize::MAX - 1);
    }
}
