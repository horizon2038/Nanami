//! The heap bootstrap needs one range record and no allocator. After the heap
//! is online, frame ranges and their capability nodes are created on demand.
use super::*;

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
    pub(super) fn frame_range(&self, page: usize) -> Option<FrameRange> {
        if let Some(range) = self.bootstrap_frames.filter(|range| range.contains(page)) {
            return Some(range);
        }
        self.physical_frames
            .range(..=page)
            .next_back()
            .map(|(_, range)| *range)
            .filter(|range| range.contains(page))
    }

    pub fn prepare_bootstrap_frames(
        &mut self,
        index: usize,
        address: usize,
        pages: usize,
    ) -> Result<(), CapabilityError> {
        if self.bootstrap_frames.is_some() || pages == 0 || index >= self.initial_generic_count {
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
        if generic.is_device
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
        if let Some(range) = self.frame_range(page) {
            return self.complete_frame_range(range);
        }
        let address = page
            .checked_mul(PAGE_SIZE)
            .ok_or(CapabilityError::InvalidArgument)?;
        // Ordinary allocations must not select an overlapping device generic.
        let index = (0..self.initial_generic_count)
            .filter(|&index| {
                !self.initial_generics[index].is_device
                    && self.source_range(index, address).is_some()
            })
            .min_by_key(|&index| self.initial_generics[index].size_radix)
            .ok_or(CapabilityError::InvalidArgument)?;
        self.materialize_frame_range(index, address)
    }

    pub(super) fn materialize_frame_range(
        &mut self,
        index: usize,
        address: usize,
    ) -> Result<(), CapabilityError> {
        if let Some(range) = self.frame_range(address >> PAGE_BITS) {
            return self.complete_frame_range(range);
        }
        self.ensure_metadata_capacity()?;
        let (start, end) = self
            .source_range(index, address)
            .ok_or(CapabilityError::InvalidArgument)?;
        let (base, radix) = sources::containing_block(start, end, address)
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
        self.complete_frame_range(range)
    }

    fn complete_frame_range(&mut self, mut range: FrameRange) -> Result<(), CapabilityError> {
        if range.ready {
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
