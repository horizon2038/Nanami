//! The heap bootstrap needs one range record and no allocator. After the heap
//! is online, frame ranges and their capability nodes are created on demand.
use super::*;

/// Request-local conversion ceiling for a known contiguous RAM allocation.
/// This is not ownership or mapping authority; existing Ready ranges prevail.
#[derive(Clone, Copy)]
pub(crate) struct AllocationFrameHint {
    first: usize,
    end: usize,
    max_bits: usize,
}

#[derive(Clone, Copy)]
pub(super) struct FrameRange {
    base_page: usize,
    page_count: usize,
    radix: usize,
    source: CapabilityDescriptor,
    node: Option<CapabilityDescriptor>,
    ready: bool,
    failed: bool,
}

impl FrameRange {
    pub(super) fn node(self) -> Option<CapabilityDescriptor> { self.node }
    pub(super) fn base_page(self) -> usize { self.base_page }
    pub(super) fn logical_descriptor(self, page: usize) -> Option<CapabilityDescriptor> {
        (self.contains(page)).then(|| self.node
            .map(|node| make_child_slot_descriptor(node, self.radix, page - self.base_page))).flatten()
    }
    pub(super) fn private_quarantined(self) -> bool { self.failed && self.source == 0 }
    pub(super) fn contains_descriptor(self, frame: CapabilityDescriptor) -> bool {
        self.node.is_some_and(|node| {
            crate::nanami_utils::descriptor::is_within_node_namespace(frame, node)
        })
    }

    fn contains(self, page: usize) -> bool {
        page.checked_sub(self.base_page)
            .is_some_and(|offset| offset < self.page_count)
    }

    pub(super) fn descriptor(self, page: usize) -> Option<CapabilityDescriptor> {
        (self.ready && self.contains(page)).then(|| {
            make_child_slot_descriptor(self.node.unwrap(), self.radix, page - self.base_page)
        })
    }
}

impl MemoryManager {
    pub(super) fn record_private_large_range(&mut self, first: usize,
        source: CapabilityDescriptor, node: Option<CapabilityDescriptor>, ready: bool, failed: bool) {
        self.ready_frame_range.set(None);
        self.ready_frame_range_other.set(None);
        self.physical_frames.insert(first, FrameRange { base_page: first, page_count: 512,
            radix: 9, source, node, ready, failed });
    }
    pub(super) fn record_bootstrap_prefix(&mut self, address: usize, pages: usize,
        source: CapabilityDescriptor, node: CapabilityDescriptor, radix: usize) {
        self.bootstrap_prefix_frames = Some(FrameRange { base_page: address >> PAGE_BITS,
            page_count: pages, radix, source, node: Some(node), ready: true, failed: false });
    }
    pub(super) fn record_bootstrap_frame_range(
        &mut self, address: usize, pages: usize, source: CapabilityDescriptor,
        node: CapabilityDescriptor, radix: usize,
    ) {
        self.bootstrap_frames = Some(FrameRange { base_page: address >> PAGE_BITS,
            page_count: pages, radix, source, node: Some(node), ready: true, failed: false });
    }

    pub(super) fn bootstrap_frame_node(&self, address: usize, pages: usize)
        -> Option<(CapabilityDescriptor, usize)>
    {
        let range = self.bootstrap_frames?;
        (range.ready && range.base_page == address >> PAGE_BITS && range.page_count == pages)
            .then(|| (range.node.unwrap(), range.radix))
    }

    pub(super) fn bootstrap_frame_extent(&self) -> Option<(usize, usize)> {
        let range = self.bootstrap_frames?;
        (range.ready && !range.failed).then_some((range.base_page * PAGE_SIZE, range.page_count))
    }

    pub(super) fn frame_range(&self, page: usize) -> Option<FrameRange> {
        #[cfg(test)]
        self.frame_range_probes.set(self.frame_range_probes.get() + 1);
        if let Some(range) = self.bootstrap_prefix_frames.filter(|range| range.contains(page)) {
            return Some(range);
        }
        if let Some(range) = self.bootstrap_frames.filter(|range| range.contains(page)) {
            return Some(range);
        }
        let recent = self.ready_frame_range.get();
        if let Some(range) = recent.filter(|range| range.contains(page)) {
            return Some(range);
        }
        // A copy can alternate source and destination pool ranges. Keep both
        // completed identities; neither entry contains page contents or rights.
        if let Some(range) = self.ready_frame_range_other.get().filter(|range| range.contains(page)) {
            return Some(range);
        }
        #[cfg(test)]
        self.frame_range_index_lookups.set(self.frame_range_index_lookups.get() + 1);
        let range = self.physical_frames
            .range(..=page)
            .next_back()
            .map(|(_, range)| *range)
            .filter(|range| range.contains(page));
        // Pending/failed conversions can change or remain quarantined. Only
        // Ready records have permanent node/Frame identity and may bypass the
        // index on later lookups. Misses do not evict a useful completed range.
        if let Some(range) = range.filter(|range| range.ready && !range.failed) {
            self.ready_frame_range_other.set(recent);
            self.ready_frame_range.set(Some(range));
        }
        range
    }

    #[cfg(test)]
    pub fn frame_range_lookup_diagnostics(&self) -> (usize, usize) {
        (self.frame_range_index_lookups.get(), core::mem::size_of_val(&self.ready_frame_range)
            + core::mem::size_of_val(&self.ready_frame_range_other))
    }

    #[cfg(test)]
    pub fn frame_range_probe_count(&self) -> usize {
        self.frame_range_probes.get()
    }

    pub fn prepare_bootstrap_frames(
        &mut self,
        index: usize,
        address: usize,
        pages: usize,
    ) -> Result<(), CapabilityError> {
        if self.bootstrap_frames.is_some() || self.bootstrap_large_heap.is_some() || self.bootstrap_prefix_frames.is_some()
            || pages == 0 || index >= self.initial_generic_count {
            return Err(CapabilityError::InvalidArgument);
        }
        let generic = self.initial_generics[index];
        let base = generic.address as usize;
        let bytes = pages
            .checked_mul(PAGE_SIZE)
            .ok_or(CapabilityError::InvalidArgument)?;
        let end = address
            .checked_add(bytes)
            .ok_or(CapabilityError::InvalidArgument)?;
        let source_end = base
            .checked_add(
                checked_pow2(generic.size_radix as usize)
                    .ok_or(CapabilityError::InvalidArgument)?,
            )
            .ok_or(CapabilityError::InvalidArgument)?;
        if generic.is_device()
            || address & (PAGE_SIZE - 1) != 0
            || address != base + self.initial_generic_delegated_bytes[index]
            || end > source_end
        {
            return Err(CapabilityError::InvalidArgument);
        }
        let radix = pages
            .checked_next_power_of_two()
            .ok_or(CapabilityError::InvalidArgument)?
            .ilog2() as usize;
        let node = create_root_node(
            self.root_descriptor,
            self.root_radix,
            self.kernel_object_generic,
            radix,
            &PHYSICAL_FRAME_DIRECTORY_SLOT_CANDIDATES,
        )?;
        let source = self.generic_descriptor_from_index(index);
        arch::generic::convert(
            source,
            CapabilityType::Frame,
            PAGE_BITS as Word,
            pages as Word,
            node,
            0,
        )?;
        self.initial_generic_delegated_bytes[index] = end - base;
        self.bootstrap_frames = Some(FrameRange {
            base_page: address >> PAGE_BITS,
            page_count: pages,
            radix,
            source,
            node: Some(node),
            ready: true,
            failed: false,
        });
        Ok(())
    }

    pub fn ensure_alpha_frame_at_physical_index(
        &mut self,
        page: usize,
    ) -> Result<(), CapabilityError> {
        self.ensure_alpha_frame_for_allocation(page, None)
    }

    pub(crate) fn allocation_frame_hint(&self, first: usize, pages: usize) -> Option<AllocationFrameHint> {
        // Tiny requests retain the old amortized pool conversion. Large,
        // independently reserved batches bound cap creation without making
        // one capability node for every individual 4K allocation.
        if option_env!("NANAMI_ALLOCATION_FRAME_CEILING") == Some("0") || pages < 256 { return None; }
        let end = first.checked_add(pages)?;
        first.checked_mul(PAGE_SIZE)?;
        (end - 1).checked_mul(PAGE_SIZE)?;
        let max_bits = (PAGE_BITS + (usize::BITS - (first ^ (end - 1)).leading_zeros()) as usize)
            .min(PHYSICAL_CHUNK_RADIX);
        (max_bits < PHYSICAL_CHUNK_RADIX).then_some(AllocationFrameHint { first, end, max_bits })
    }

    /// Prepare the permanent 4K Frames for an already reserved allocation.
    /// Conversion and quarantine still run at the first page of each new
    /// range. A Ready range needs no more work before its end.
    pub(crate) fn ensure_alpha_frames_for_allocation(
        &mut self, first: usize, pages: usize,
    ) -> Result<(), CapabilityError> {
        if pages == 0 {
            return Ok(());
        }
        let end = first.checked_add(pages).ok_or(CapabilityError::InvalidArgument)?;
        (end - 1).checked_mul(PAGE_SIZE).ok_or(CapabilityError::InvalidArgument)?;
        let hint = self.allocation_frame_hint(first, pages);
        let ready = |range: &FrameRange| range.ready && !range.failed;
        let mut page = first;
        while page < end {
            let range = match self.frame_range(page).filter(ready) {
                Some(range) => range,
                None => {
                    self.ensure_alpha_frame_for_allocation(page, hint)?;
                    self.frame_range(page).filter(ready).ok_or(CapabilityError::Fatal)?
                }
            };
            page = end.min(range.base_page + range.page_count);
        }
        Ok(())
    }

    /// Fill a destination word from already prepared pool Frames. Each
    /// conversion range has its own node and radix, including at a boundary.
    pub(crate) fn read_ready_pool_frames(
        &self, first: usize, frames: &mut [CapabilityDescriptor],
    ) -> Result<(), CapabilityError> {
        let address = first.checked_mul(PAGE_SIZE).ok_or(CapabilityError::InvalidArgument)?;
        let bytes = frames.len().checked_mul(PAGE_SIZE).ok_or(CapabilityError::InvalidArgument)?;
        address.checked_add(bytes).ok_or(CapabilityError::InvalidArgument)?;
        if self.bootstrap_large_heap.is_some_and(|heap| heap.overlaps(address, bytes)) {
            return Err(CapabilityError::InvalidArgument);
        }
        let mut offset = 0;
        while offset < frames.len() {
            let page = first + offset;
            let range = self.frame_range(page).filter(|range| range.ready && !range.failed)
                .ok_or(CapabilityError::InvalidArgument)?;
            let node = range.node.ok_or(CapabilityError::InvalidArgument)?;
            let slot = page - range.base_page;
            let count = (range.page_count - slot).min(frames.len() - offset);
            for (index, frame) in frames[offset..offset + count].iter_mut().enumerate() {
                *frame = make_child_slot_descriptor(node, range.radix, slot + index);
            }
            offset += count;
        }
        Ok(())
    }

    pub(crate) fn ensure_alpha_frame_for_allocation(
        &mut self, page: usize, hint: Option<AllocationFrameHint>,
    ) -> Result<(), CapabilityError> {
        if hint.is_some_and(|hint| page < hint.first || page >= hint.end) {
            return Err(CapabilityError::InvalidArgument);
        }
        if let Some(range) = self.frame_range(page) {
            return self.complete_frame_range(range);
        }
        let address = page
            .checked_mul(PAGE_SIZE)
            .ok_or(CapabilityError::InvalidArgument)?;
        if self.bootstrap_large_heap.is_some_and(|heap| heap.overlaps(address, PAGE_SIZE)) {
            return Err(CapabilityError::PermissionDenied);
        }
        if self.delegated_range(address, PAGE_SIZE) {
            return Err(CapabilityError::PermissionDenied);
        }
        // Ordinary allocations must not select an overlapping device generic.
        let index = (0..self.initial_generic_count)
            .filter(|&index| {
                !self.initial_generics[index].is_device()
                    && self.source_range(index, address).is_some()
            })
            .min_by_key(|&index| self.initial_generics[index].size_radix)
            .ok_or(CapabilityError::InvalidArgument)?;
        self.materialize_frame_range_with_ceiling(index, address, hint)
    }

    pub(super) fn materialize_frame_range(
        &mut self,
        index: usize,
        address: usize,
    ) -> Result<(), CapabilityError> {
        self.materialize_frame_range_with_ceiling(index, address, None)
    }

    fn materialize_frame_range_with_ceiling(
        &mut self, index: usize, address: usize, hint: Option<AllocationFrameHint>,
    ) -> Result<(), CapabilityError> {
        if let Some(range) = self.frame_range(address >> PAGE_BITS) {
            return self.complete_frame_range(range);
        }
        self.ensure_metadata_capacity()?;
        let (start, end) = self
            .source_range(index, address)
            .ok_or(CapabilityError::InvalidArgument)?;
        let (base, radix) = sources::containing_block_with_ceiling(start, end, address,
            hint.map_or(PHYSICAL_CHUNK_RADIX, |hint| hint.max_bits))
            .ok_or(CapabilityError::InvalidArgument)?;
        let source = self.take_physical_source(index, base, radix)?;
        let range = FrameRange {
            base_page: base >> PAGE_BITS,
            page_count: 1 << (radix - PAGE_BITS),
            radix: (radix - PAGE_BITS).max(1),
            source,
            node: None,
            ready: false,
            failed: false,
        };
        self.physical_frames.insert(range.base_page, range);
        self.complete_frame_range(range)?;
        #[cfg(not(test))]
        if option_env!("NANAMI_ALLOCATION_FRAME_DIAG") == Some("1") {
            if let Some(hint) = hint {
                let legacy = sources::containing_block(start, end, address).unwrap().1;
                if radix < legacy {
                    use core::sync::atomic::{AtomicUsize, Ordering};
                    static REPORTS: AtomicUsize = AtomicUsize::new(0);
                    if REPORTS.fetch_add(1, Ordering::Relaxed) < 32 {
                        crate::force_info!("[memory.frames] bounded created={} previous-span={} allocation-pages={}",
                            range.page_count, 1usize << (legacy - PAGE_BITS), hint.end - hint.first);
                    }
                }
            }
        }
        Ok(())
    }

    fn complete_frame_range(&mut self, mut range: FrameRange) -> Result<(), CapabilityError> {
        if range.ready {
            return Ok(());
        }
        if range.node.is_some_and(|node| self.private_large.contains_node(node)) {
            self.complete_unmapped_private_large(range.node.unwrap())?;
            return Ok(());
        }
        if range.failed {
            return Err(CapabilityError::InvalidArgument);
        }
        if range.node.is_none() {
            self.ensure_metadata_capacity()?;
            let slot = self.capability_directory.allocate(&mut self.node_pool)?;
            self.node_pool.create_node(range.radix, slot)?;
            range.node = Some(slot.descriptor());
            self.physical_frames.insert(range.base_page, range);
        }
        // A failed batch may have installed some frames. Quarantine that range
        // instead of retrying the same conversion against occupied slots.
        let result = arch::generic::convert(
            range.source,
            CapabilityType::Frame,
            PAGE_BITS as Word,
            range.page_count as Word,
            range.node.unwrap(),
            0,
        );
        range.ready = result.is_ok();
        range.failed = result.is_err();
        self.physical_frames.insert(range.base_page, range);
        result
    }
}
