//! Permanent Alpha heap/stack leaves. Never split, exported or reaped.
use super::*;

const LARGE_BITS: usize = 21;

#[derive(Clone, Copy)]
pub(crate) struct BootstrapHeapFrames {
    physical_base: usize,
    node: CapabilityDescriptor,
    node_bits: usize,
    first: usize,
    leaf_bits: usize,
    leaves: usize,
}

impl BootstrapHeapFrames {
    pub(super) fn overlaps(&self, address: usize, bytes: usize) -> bool {
        bytes != 0 && address.checked_add(bytes).is_none_or(|end| {
            address < self.physical_base + self.leaves * self.leaf_bytes() && self.physical_base < end
        })
    }
    pub(crate) fn physical_base(&self) -> usize { self.physical_base }
    pub(crate) fn leaf_bits(&self) -> usize { self.leaf_bits }
    pub(crate) fn leaf_bytes(&self) -> usize { 1 << self.leaf_bits }
    pub(crate) fn leaves(&self) -> usize { self.leaves }
    pub(crate) fn slice_pages(&self, first: usize, pages: usize) -> Option<Self> {
        let pages_per_leaf = 1 << (self.leaf_bits - PAGE_BITS);
        let end = first.checked_add(pages)?;
        if pages == 0 || first % pages_per_leaf != 0 || pages % pages_per_leaf != 0
            || end > self.leaves.checked_mul(pages_per_leaf)? { return None; }
        Some(Self { physical_base: self.physical_base + first * PAGE_SIZE,
            first: self.first + first / pages_per_leaf, leaves: pages / pages_per_leaf, ..*self })
    }
    pub(crate) fn frame(&self, index: usize) -> Option<CapabilityDescriptor> {
        (index < self.leaves).then(|| make_child_slot_descriptor(self.node, self.node_bits, self.first + index))
    }
}

impl MemoryManager {
    pub(crate) fn large_bootstrap_heap_eligible(address: usize, pages: usize) -> bool {
        pages.checked_mul(PAGE_SIZE).is_some_and(|bytes| {
            cfg!(target_arch = "x86_64") && bytes >= 1 << LARGE_BITS
                && bytes & ((1 << LARGE_BITS) - 1) == 0 && address & ((1 << LARGE_BITS) - 1) == 0
        })
    }

    pub(crate) fn prepare_bootstrap_heap_frames(
        &mut self, index: usize, address: usize, pages: usize, prefer_large: bool,
    ) -> Result<BootstrapHeapFrames, CapabilityError> {
        if self.bootstrap_frames.is_some() || self.bootstrap_large_heap.is_some() || self.bootstrap_prefix_frames.is_some() {
            return Err(CapabilityError::InvalidArgument);
        }
        if !prefer_large || !Self::large_bootstrap_heap_eligible(address, pages) {
            self.prepare_bootstrap_frames(index, address, pages)?;
            return self.bootstrap_heap_small_frames(address, pages);
        }
        if index >= self.initial_generic_count {
            return Err(CapabilityError::InvalidArgument);
        }
        let generic = self.initial_generics[index];
        let base = generic.address as usize;
        let bytes = pages.checked_mul(PAGE_SIZE).ok_or(CapabilityError::InvalidArgument)?;
        let end = address.checked_add(bytes).ok_or(CapabilityError::InvalidArgument)?;
        let source_end = base.checked_add(checked_pow2(generic.size_radix as usize)
            .ok_or(CapabilityError::InvalidArgument)?).ok_or(CapabilityError::InvalidArgument)?;
        let cursor = base.checked_add(self.initial_generic_delegated_bytes[index])
            .ok_or(CapabilityError::InvalidArgument)?;
        if generic.is_device() || cursor & (PAGE_SIZE - 1) != 0 || address < cursor
            || address - cursor >= 1 << LARGE_BITS || end > source_end
        { return Err(CapabilityError::InvalidArgument); }

        let leaves = bytes >> LARGE_BITS;
        let source = self.generic_descriptor_from_index(index);
        if address != cursor {
            let prefix_pages = (address - cursor) >> PAGE_BITS;
            let radix = prefix_pages.next_power_of_two().ilog2() as usize;
            let prefix = create_root_node(self.root_descriptor, self.root_radix,
                self.kernel_object_generic, radix, &PHYSICAL_FRAME_DIRECTORY_SLOT_CANDIDATES)?;
            arch::generic::convert(source, CapabilityType::Frame, PAGE_BITS as Word,
                prefix_pages as Word, prefix, 0)?;
            self.initial_generic_delegated_bytes[index] = address - base;
            self.record_bootstrap_prefix(cursor, prefix_pages, source, prefix, radix);
        }
        let source_bits = leaves.checked_next_power_of_two()
            .ok_or(CapabilityError::InvalidArgument)?.ilog2() as usize;
        let sources = create_root_node(self.root_descriptor, self.root_radix,
            self.kernel_object_generic, source_bits, &PHYSICAL_FRAME_DIRECTORY_SLOT_CANDIDATES)?;
        // Size the destination for the actual large leaves. Allocate the much
        // larger small-leaf node only after unsupported conversion and cleanup.
        let mut node_bits = source_bits;
        let mut node = create_root_node(self.root_descriptor, self.root_radix,
            self.kernel_object_generic, node_bits, &PHYSICAL_FRAME_DIRECTORY_SLOT_CANDIDATES)?;
        let small_per_leaf = 1 << (LARGE_BITS - PAGE_BITS);
        let mut small = false;
        for i in 0..leaves {
            // Each isolated source needs only 2M alignment, not whole-heap alignment.
            arch::generic::convert(source, CapabilityType::Generic, LARGE_BITS as Word,
                1, sources, i as Word)?;
            self.initial_generic_delegated_bytes[index] = address + ((i + 1) << LARGE_BITS) - base;
            let child = make_child_slot_descriptor(sources, source_bits, i);
            if !small {
                match arch::generic::convert(child, CapabilityType::Frame, LARGE_BITS as Word, 1, node, i as Word) {
                    Ok(()) => continue,
                    Err(CapabilityError::InvalidArgument | CapabilityError::InvalidDepth | CapabilityError::IllegalOperation) => {
                        // Nothing is mapped. Confirm ALL cleanup acknowledgments before
                        // reusing overlapping destination slots or any source cursor.
                        for j in 0..=i { arch::node::revoke(sources, j as Word)?; }
                        node_bits = pages.checked_next_power_of_two()
                            .ok_or(CapabilityError::InvalidArgument)?.ilog2() as usize;
                        node = create_root_node(self.root_descriptor, self.root_radix,
                            self.kernel_object_generic, node_bits, &PHYSICAL_FRAME_DIRECTORY_SLOT_CANDIDATES)?;
                        for j in 0..i {
                            arch::generic::convert(make_child_slot_descriptor(sources, source_bits, j),
                                CapabilityType::Frame, PAGE_BITS as Word, small_per_leaf as Word,
                                node, (j * small_per_leaf) as Word)?;
                        }
                        small = true;
                    }
                    Err(error) => return Err(error),
                }
            }
            arch::generic::convert(child, CapabilityType::Frame, PAGE_BITS as Word,
                small_per_leaf as Word, node, (i * small_per_leaf) as Word)?;
        }
        let frames = BootstrapHeapFrames { physical_base: address, node, node_bits, first: 0,
            leaf_bits: if small { PAGE_BITS } else { LARGE_BITS }, leaves: if small { pages } else { leaves } };
        if small { self.record_bootstrap_frame_range(address, pages, source, node, node_bits); }
        else { self.bootstrap_large_heap = Some(frames); }
        Ok(frames)
    }

    fn bootstrap_heap_small_frames(&self, address: usize, pages: usize)
        -> Result<BootstrapHeapFrames, CapabilityError>
    {
        self.bootstrap_frame_node(address, pages).map(|(node, radix)| BootstrapHeapFrames {
            physical_base: address, node, node_bits: radix, first: 0, leaf_bits: PAGE_BITS, leaves: pages,
        }).ok_or(CapabilityError::InvalidArgument)
    }

    pub(crate) fn bootstrap_stack_frames(&self, heap_pages: usize, pages: usize)
        -> Option<BootstrapHeapFrames>
    {
        let frames = match self.bootstrap_large_heap {
            Some(frames) => frames,
            None => {
                let (address, pages) = self.bootstrap_frame_extent()?;
                self.bootstrap_heap_small_frames(address, pages).ok()?
            }
        };
        frames.slice_pages(heap_pages, pages)
    }

    /// Only permanent Alpha heap/stack use non-4K leaves. Pre-record ownership
    /// before MAP; errors stop bootstrap and must never release or split RAM.
    pub(crate) fn map_bootstrap_heap_frame(
        &mut self, space: CapabilityDescriptor, frame: CapabilityDescriptor,
        address: usize, leaf_bits: usize, vm: &mut impl VmTracker,
    ) -> CapabilityResult {
        if leaf_bits == PAGE_BITS { return self.map_frame(space, frame, address, vm); }
        if leaf_bits != LARGE_BITS || address & ((1 << LARGE_BITS) - 1) != 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        vm.record_bootstrap_frame(address, frame, leaf_bits)
            .map_err(|_| CapabilityError::InvalidArgument)?;
        self.ensure_page_tables_for_leaf(space, address, leaf_bits, vm)?;
        arch::address_space::map(space, frame, address, Attribute::ALL)
    }
}
