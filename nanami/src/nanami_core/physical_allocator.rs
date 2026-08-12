use alloc::vec::Vec;

const PAGE_BITS: usize = 12;
const PAGE_SIZE: usize = 1 << PAGE_BITS;

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
            if let Some(index) = self.find_block_index(order, buddy) {
                self.free_lists[order].swap_remove(index);
                if buddy < base_page {
                    base_page = buddy;
                }
                order = order
                    .checked_add(1)
                    .ok_or(PhysicalAllocError::InvalidArgument)?;
                continue;
            }

            if self.find_block_index(order, base_page).is_none() {
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

        if !self.contains_range(base_page, page_count) {
            return Err(PhysicalAllocError::OutOfMemory);
        }
        self.carve_range(base_page, page_count)
    }

    fn contains_range(&self, base_page: usize, page_count: usize) -> bool {
        let Some(end_page) = base_page.checked_add(page_count) else {
            return false;
        };
        if page_count == 0 {
            return false;
        }

        let mut current = base_page;
        while current < end_page {
            let Some((order, _, block_base)) = self.find_containing_block(current) else {
                return false;
            };
            let Ok(block_count) = block_pages(order) else {
                return false;
            };
            let Some(block_end) = block_base.checked_add(block_count) else {
                return false;
            };
            current = core::cmp::min(block_end, end_page);
        }
        true
    }

    fn carve_range(
        &mut self,
        base_page: usize,
        page_count: usize,
    ) -> Result<(), PhysicalAllocError> {
        let end_page = base_page
            .checked_add(page_count)
            .ok_or(PhysicalAllocError::InvalidArgument)?;
        let mut current = base_page;

        while current < end_page {
            let Some((order, index, block_base)) = self.find_containing_block(current) else {
                return Err(PhysicalAllocError::OutOfMemory);
            };

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
        }

        Ok(())
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

    fn find_block_index(&self, order: usize, base_page: usize) -> Option<usize> {
        if order >= self.free_lists.len() {
            return None;
        }
        let mut index = 0usize;
        while index < self.free_lists[order].len() {
            if self.free_lists[order][index] == base_page {
                return Some(index);
            }
            index += 1;
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
}

impl PhysicalAllocator {
    pub fn new() -> Self {
        Self {
            normal: BuddyPool::new(),
            device: BuddyPool::new(),
            allocations: Vec::new(),
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
            self.allocations.push(AllocationRecord {
                base_page,
                page_count,
                is_device,
                reserved: true,
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

        let index = self
            .allocations
            .iter()
            .position(|allocation| {
                if allocation.reserved {
                    return false;
                }
                let allocation_end = allocation.base_page + allocation.page_count;
                req_base >= allocation.base_page && req_end <= allocation_end
            })
            .ok_or(PhysicalAllocError::InvalidArgument)?;

        let allocation = self.allocations.swap_remove(index);
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
        let mut allocated_pages = 0usize;
        for allocation in self.allocations.iter() {
            if !allocation.is_device {
                allocated_pages = allocated_pages.saturating_add(allocation.page_count);
            }
        }
        PhysicalMemoryInfo {
            total_pages: free_pages.saturating_add(allocated_pages),
            free_pages,
        }
    }

    fn record_allocation(&mut self, base_page: usize, page_count: usize, is_device: bool) {
        self.allocations.push(AllocationRecord {
            base_page,
            page_count,
            is_device,
            reserved: false,
        });
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
    (size_bytes + PAGE_SIZE - 1) / PAGE_SIZE
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
