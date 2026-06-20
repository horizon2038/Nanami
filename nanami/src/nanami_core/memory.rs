use crate::nanami_core::kernel_object::{self, KernelObjectKind};
use crate::nanami_core::physical_allocator::{PhysicalAllocError, PhysicalAllocator};
use crate::nanami_core::vm_space::VmTracker;
use crate::nanami_utils::descriptor::{make_child_slot_descriptor, make_root_slot_descriptor};
use nun::{
    arch, CapabilityDescriptor, CapabilityError, CapabilityResult, CapabilityType, InitInfo, Word,
};

const PAGE_BITS: usize = 12;
const PAGE_SIZE: usize = 1 << PAGE_BITS;

const PHYSICAL_NODE_RADIX: usize = 21;
const GENERIC_NODE_RADIX: usize = 7;
const PAGE_TABLE_POOL_RADIX: usize = 7;
const PAGE_TABLE_POOL_SLOTS: usize = 1 << PAGE_TABLE_POOL_RADIX;

const PHYSICAL_GENERIC_NODE_SLOT_CANDIDATES: [usize; 4] = [1024, 1025, 1026, 1027];
const PHYSICAL_FRAME_NODE_SLOT_CANDIDATES: [usize; 4] = [1100, 1101, 1102, 1103];
const PAGE_TABLE_POOL_SLOT_CANDIDATES: [usize; 4] = [1200, 1201, 1202, 1203];
const INITIAL_GENERIC_CAPACITY: usize = 128;

pub struct MemoryManager {
    pub root_descriptor: CapabilityDescriptor,
    pub root_radix: usize,
    pub bootstrap_generic: CapabilityDescriptor,
    pub physical_generic_node: CapabilityDescriptor,
    pub physical_frame_node: CapabilityDescriptor,
    page_table_pool_node: CapabilityDescriptor,
    next_page_table_slot: usize,
    next_process_frame_index: usize,
    physical_allocator: Option<PhysicalAllocator>,
    initial_generics: [nun::GenericDescriptor; INITIAL_GENERIC_CAPACITY],
    initial_generic_count: usize,
    initial_generic_consumed_bytes: [usize; INITIAL_GENERIC_CAPACITY],
}

impl MemoryManager {
    pub fn bootstrap(
        init_info: &InitInfo,
        root_descriptor: CapabilityDescriptor,
        root_radix: usize,
        bootstrap_generic: CapabilityDescriptor,
    ) -> Result<Self, CapabilityError> {
        let mut initial_generic_consumed_bytes = [0usize; INITIAL_GENERIC_CAPACITY];

        crate::info!("memory: create physical generic node");
        let physical_generic_node = create_root_node_from_initial_generics(
            init_info,
            root_descriptor,
            root_radix,
            PHYSICAL_NODE_RADIX,
            &PHYSICAL_GENERIC_NODE_SLOT_CANDIDATES,
            &mut initial_generic_consumed_bytes,
        )?;
        crate::info!(
            "physical generic node={:#018x}",
            physical_generic_node.descriptor
        );
        crate::info!("memory: create physical frame node");
        let physical_frame_node = create_root_node_from_initial_generics(
            init_info,
            root_descriptor,
            root_radix,
            PHYSICAL_NODE_RADIX,
            &PHYSICAL_FRAME_NODE_SLOT_CANDIDATES,
            &mut initial_generic_consumed_bytes,
        )?;
        crate::info!(
            "physical frame node={:#018x}",
            physical_frame_node.descriptor
        );
        crate::info!("memory: create page-table pool node");
        let page_table_pool_node = create_root_node_from_initial_generics(
            init_info,
            root_descriptor,
            root_radix,
            PAGE_TABLE_POOL_RADIX,
            &PAGE_TABLE_POOL_SLOT_CANDIDATES,
            &mut initial_generic_consumed_bytes,
        )?;
        crate::info!(
            "page-table pool node={:#018x}",
            page_table_pool_node.descriptor
        );

        let mut manager = Self {
            root_descriptor,
            root_radix,
            bootstrap_generic,
            physical_generic_node: physical_generic_node.descriptor,
            physical_frame_node: physical_frame_node.descriptor,
            page_table_pool_node: page_table_pool_node.descriptor,
            next_page_table_slot: 0,
            next_process_frame_index: 0,
            physical_allocator: None,
            initial_generics: init_info.generic_list,
            initial_generic_count: init_info.generic_list_count as usize,
            initial_generic_consumed_bytes,
        };

        crate::info!("memory: split initial generics into 4KiB generic caps");
        manager.split_all_initial_generics(init_info)?;
        crate::info!("memory: split complete");

        Ok(manager)
    }

    pub fn physical_page_index_from_address(&self, physical_address: usize) -> Option<usize> {
        let frame_index = physical_address >> PAGE_BITS;
        if frame_index >= (1 << PHYSICAL_NODE_RADIX) {
            return None;
        }
        Some(frame_index)
    }

    pub fn physical_frame_descriptor_from_index(
        &self,
        frame_index: usize,
    ) -> Option<CapabilityDescriptor> {
        if frame_index >= (1 << PHYSICAL_NODE_RADIX) {
            return None;
        }
        Some(make_child_slot_descriptor(
            self.physical_frame_node,
            PHYSICAL_NODE_RADIX,
            frame_index,
        ))
    }

    pub fn frame_descriptor_from_physical(
        &mut self,
        physical_address: usize,
    ) -> Option<CapabilityDescriptor> {
        let frame_index = self.physical_page_index_from_address(physical_address)?;
        if self
            .ensure_alpha_frame_at_physical_index(frame_index)
            .is_err()
        {
            return None;
        }
        self.physical_frame_descriptor_from_index(frame_index)
    }

    pub fn ensure_alpha_frames_from_generic(
        &mut self,
        source_generic: CapabilityDescriptor,
        destination_base_frame_index: usize,
        count: usize,
    ) -> Result<(), CapabilityError> {
        if count == 0 {
            return Ok(());
        }
        if destination_base_frame_index >= (1 << PHYSICAL_NODE_RADIX)
            || destination_base_frame_index + count > (1 << PHYSICAL_NODE_RADIX)
        {
            return Err(CapabilityError::InvalidArgument);
        }

        match arch::generic::convert(
            source_generic,
            CapabilityType::Frame,
            PAGE_BITS as Word,
            count as Word,
            self.physical_frame_node,
            destination_base_frame_index as Word,
        ) {
            Ok(()) => Ok(()),
            Err(CapabilityError::InvalidArgument) | Err(CapabilityError::IllegalOperation) => {
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    pub fn map_frame(
        &mut self,
        address_space: CapabilityDescriptor,
        frame_descriptor: CapabilityDescriptor,
        virtual_address: usize,
        vm_space: &mut impl VmTracker,
    ) -> CapabilityResult {
        self.ensure_page_tables(address_space, virtual_address, vm_space)?;

        match arch::address_space::map(address_space, frame_descriptor, virtual_address, 0) {
            Ok(()) => {
                let frame_index = self
                    .frame_index_from_descriptor(frame_descriptor)
                    .unwrap_or(0);
                vm_space
                    .record_frame(virtual_address, frame_index)
                    .map_err(|_| CapabilityError::InvalidArgument)?;
                Ok(())
            }
            Err(CapabilityError::IllegalOperation) => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub fn frame_node_descriptor(&self) -> CapabilityDescriptor {
        self.physical_frame_node
    }

    pub fn frame_node_radix(&self) -> usize {
        PHYSICAL_NODE_RADIX
    }

    pub fn root_radix(&self) -> usize {
        self.root_radix
    }

    pub fn page_size(&self) -> usize {
        PAGE_SIZE
    }

    pub fn initialize_physical_allocator(
        &mut self,
        init_info: &InitInfo,
    ) -> Result<(), CapabilityError> {
        let mut allocator = PhysicalAllocator::new();
        let count = init_info.generic_list_count as usize;

        for i in 0..count {
            let g = init_info.generic_list[i];
            if g.size_radix < PAGE_BITS as u8 {
                continue;
            }
            let size_bytes = 1usize << g.size_radix;
            if size_bytes < PAGE_SIZE {
                continue;
            }

            let desc = self.generic_descriptor_from_index(i);
            if desc == self.bootstrap_generic {
                allocator
                    .add_region(g.address as usize, size_bytes, g.is_device, true)
                    .map_err(map_physical_alloc_error)?;
                continue;
            }

            let consumed = self
                .initial_generic_consumed_bytes_for_index(i)
                .min(size_bytes);
            if consumed != 0 {
                allocator
                    .add_region(g.address as usize, consumed, g.is_device, true)
                    .map_err(map_physical_alloc_error)?;
            }
            if consumed < size_bytes {
                allocator
                    .add_region(
                        (g.address as usize).saturating_add(consumed),
                        size_bytes - consumed,
                        g.is_device,
                        false,
                    )
                    .map_err(map_physical_alloc_error)?;
            }
        }

        self.physical_allocator = Some(allocator);
        Ok(())
    }

    pub fn allocate_physical_at(
        &mut self,
        physical_address: usize,
        size_bytes: usize,
        allow_device: bool,
    ) -> Result<usize, CapabilityError> {
        let allocator = self
            .physical_allocator
            .as_mut()
            .ok_or(CapabilityError::InvalidArgument)?;
        let allocation = allocator
            .allocate_at(physical_address, size_bytes, allow_device)
            .map_err(map_physical_alloc_error)?;
        Ok(allocation.base_page)
    }

    pub fn allocate_physical_any(&mut self, size_bytes: usize) -> Result<usize, CapabilityError> {
        let allocator = self
            .physical_allocator
            .as_mut()
            .ok_or(CapabilityError::InvalidArgument)?;
        let allocation = allocator
            .allocate_any(size_bytes)
            .map_err(map_physical_alloc_error)?;
        Ok(allocation.base_page)
    }

    pub fn free_physical(
        &mut self,
        physical_address: usize,
        size_bytes: usize,
    ) -> Result<(), CapabilityError> {
        let allocator = self
            .physical_allocator
            .as_mut()
            .ok_or(CapabilityError::InvalidArgument)?;
        allocator
            .free(physical_address, size_bytes)
            .map_err(map_physical_alloc_error)
    }

    pub fn allocate_process_frames(
        &mut self,
        destination_frame_node: CapabilityDescriptor,
        destination_frame_node_radix: usize,
        destination_base_slot: usize,
        count: usize,
    ) -> Result<(), CapabilityError> {
        let max_slots = 1usize << destination_frame_node_radix;
        if destination_base_slot + count > max_slots {
            return Err(CapabilityError::InvalidArgument);
        }

        if self.physical_allocator.is_some() {
            let mut copied = 0usize;
            while copied < count {
                let dst_slot = destination_base_slot + copied;
                let frame_index = self.allocate_physical_any(PAGE_SIZE)?;
                self.copy_alpha_frame_to_process_node(
                    frame_index,
                    destination_frame_node,
                    destination_frame_node_radix,
                    dst_slot,
                )?;
                copied += 1;
            }
            return Ok(());
        }

        let mut copied = 0usize;
        let mut src_index = self.next_process_frame_index;
        let src_limit = 1usize << PHYSICAL_NODE_RADIX;

        while copied < count && src_index < src_limit {
            let dst_slot = destination_base_slot + copied;
            match self.copy_alpha_frame_to_process_node(
                src_index,
                destination_frame_node,
                destination_frame_node_radix,
                dst_slot,
            ) {
                Ok(_) => {
                    copied += 1;
                    src_index += 1;
                }
                Err(CapabilityError::InvalidDescriptor)
                | Err(CapabilityError::PermissionDenied)
                | Err(CapabilityError::InvalidArgument) => {
                    src_index += 1;
                }
                Err(e) => return Err(e),
            }
        }

        if copied != count {
            return Err(CapabilityError::InvalidArgument);
        }
        self.next_process_frame_index = src_index;
        Ok(())
    }

    pub fn copy_alpha_frame_to_process_node(
        &mut self,
        physical_frame_index: usize,
        destination_frame_node: CapabilityDescriptor,
        destination_frame_node_radix: usize,
        destination_slot: usize,
    ) -> Result<CapabilityDescriptor, CapabilityError> {
        if destination_slot >= (1usize << destination_frame_node_radix) {
            return Err(CapabilityError::InvalidArgument);
        }

        self.ensure_alpha_frame_at_physical_index(physical_frame_index)?;
        let source_frame = self
            .physical_frame_descriptor_from_index(physical_frame_index)
            .ok_or(CapabilityError::InvalidArgument)?;

        arch::node::copy(
            destination_frame_node,
            destination_slot as Word,
            source_frame,
        )?;

        Ok(make_child_slot_descriptor(
            destination_frame_node,
            destination_frame_node_radix,
            destination_slot,
        ))
    }

    pub fn ensure_alpha_frame_at_physical_index(
        &mut self,
        frame_index: usize,
    ) -> Result<(), CapabilityError> {
        if frame_index >= (1 << PHYSICAL_NODE_RADIX) {
            return Err(CapabilityError::InvalidArgument);
        }
        let source_generic = make_child_slot_descriptor(
            self.physical_generic_node,
            PHYSICAL_NODE_RADIX,
            frame_index,
        );

        match arch::generic::convert(
            source_generic,
            CapabilityType::Frame,
            PAGE_BITS as Word,
            1,
            self.physical_frame_node,
            frame_index as Word,
        ) {
            Ok(())
            | Err(CapabilityError::InvalidArgument)
            | Err(CapabilityError::IllegalOperation) => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub fn ensure_alpha_frames_for_range_from_initial_generic(
        &mut self,
        physical_address: usize,
        size_bytes: usize,
        prefer_device: bool,
    ) -> Result<(usize, usize, usize), CapabilityError> {
        if size_bytes == 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let page_base = physical_address & !(PAGE_SIZE - 1);
        let offset = physical_address - page_base;
        let total_span = offset + size_bytes;
        let page_count = (total_span + PAGE_SIZE - 1) / PAGE_SIZE;

        let mut selected: Option<(usize, usize, usize)> = None;
        for pass_device_only in [prefer_device, false] {
            let mut i = 0usize;
            while i < self.initial_generic_count {
                let g = self.initial_generics[i];
                if pass_device_only && !g.is_device {
                    i += 1;
                    continue;
                }
                let start = g.address as usize;
                let size = 1usize << g.size_radix;
                let end = start.saturating_add(size);
                let consumed_end =
                    start.saturating_add(self.initial_generic_consumed_bytes_for_index(i));
                let requested_end = physical_address.saturating_add(size_bytes);
                if physical_address >= start
                    && requested_end <= end
                    && physical_address >= consumed_end
                {
                    match selected {
                        None => selected = Some((i, start, size)),
                        Some((_, _, best_size)) if size < best_size => {
                            selected = Some((i, start, size))
                        }
                        _ => {}
                    }
                }
                i += 1;
            }
            if selected.is_some() {
                break;
            }
        }

        let (generic_idx, generic_start, _) = selected.ok_or(CapabilityError::InvalidArgument)?;
        let base_index = self
            .physical_page_index_from_address(generic_start)
            .ok_or(CapabilityError::InvalidArgument)?;
        let skip_pages = (page_base.saturating_sub(generic_start)) / PAGE_SIZE;
        let total_frames = skip_pages.saturating_add(page_count);
        let generic_desc = self.generic_descriptor_from_index(generic_idx);
        self.ensure_alpha_frames_from_generic(generic_desc, base_index, total_frames)?;
        Ok((base_index, skip_pages, page_count))
    }

    fn split_all_initial_generics(&mut self, init_info: &InitInfo) -> Result<(), CapabilityError> {
        let count = init_info.generic_list_count as usize;
        crate::info!("generic_list_count={:>3}", count);

        for i in 0..count {
            let g = init_info.generic_list[i];
            crate::info!(
                "idx={:>3} addr={:#018x} size_radix={:>2} is_device={}",
                i,
                g.address as usize,
                g.size_radix,
                g.is_device
            );

            if g.is_device {
                crate::info!("memory: idx={:>3} reason=device-generic", i);
                continue;
            }

            if g.size_radix < PAGE_BITS as u8 {
                crate::info!("memory: idx={:>3} reason=size<4KiB", i);
                continue;
            }

            let generic_desc = self.generic_descriptor_from_index(i);
            if generic_desc == self.bootstrap_generic {
                crate::info!("memory: idx={:>3} reason=bootstrap-generic", i);
                continue;
            }
            let start = g.address as usize;
            let Some(size_bytes) = checked_pow2(g.size_radix as usize) else {
                crate::info!("memory: idx={:>3} reason=size-overflow", i);
                continue;
            };
            let Some(end) = start.checked_add(size_bytes) else {
                crate::info!("memory: idx={:>3} reason=end-overflow", i);
                continue;
            };
            let consumed_bytes = self
                .initial_generic_consumed_bytes_for_index(i)
                .min(size_bytes);
            let raw_split_start = start.saturating_add(consumed_bytes);
            let Some(split_start) = checked_align_up(raw_split_start, PAGE_SIZE) else {
                crate::info!("memory: idx={:>3} reason=split-start-overflow", i);
                continue;
            };
            if split_start >= end {
                crate::info!(
                    "memory: idx={:>3} reason=fully-consumed consumed_pages={:>6}",
                    i,
                    consumed_bytes >> PAGE_BITS
                );
                continue;
            }
            if consumed_bytes != 0 {
                crate::info!(
                    "memory: idx={:>3} split remainder consumed_pages={:>6}",
                    i,
                    consumed_bytes >> PAGE_BITS
                );
            }

            let page_count = (end - split_start) >> PAGE_BITS;
            let base_index = split_start >> PAGE_BITS;
            if base_index >= (1 << PHYSICAL_NODE_RADIX)
                || base_index + page_count > (1 << PHYSICAL_NODE_RADIX)
            {
                crate::info!(
                    "idx={:>3} reason=out-of-range base_index={:>7} page_count={:>6}",
                    i,
                    base_index,
                    page_count
                );
                continue;
            }

            let mut converted_pages = 0usize;
            for page_offset in 0..page_count {
                let page_index = base_index + page_offset;
                match arch::generic::convert(
                    generic_desc,
                    CapabilityType::Generic,
                    PAGE_BITS as Word,
                    1,
                    self.physical_generic_node,
                    page_index as Word,
                ) {
                    Ok(()) => {
                        converted_pages += 1;
                    }
                    Err(CapabilityError::InvalidArgument)
                    | Err(CapabilityError::IllegalOperation) => {
                        crate::info!(
                            "idx={:>3} convert(Generic) skip err dst_index={:>7}",
                            i,
                            page_index
                        );
                    }
                    Err(e) => {
                        crate::info!(
                            "idx={:>3} convert(Generic) fatal err={:?} desc={:#018x} dst_node={:#018x} dst_index={:>7}",
                            i,
                            e,
                            generic_desc,
                            self.physical_generic_node,
                            page_index
                        );
                        return Err(e);
                    }
                }
            }

            if converted_pages != page_count {
                crate::info!(
                    "idx={:>3} split partial converted={:>6} requested={:>6}",
                    i,
                    converted_pages,
                    page_count
                );
            }
        }

        Ok(())
    }

    fn ensure_page_tables(
        &mut self,
        address_space: CapabilityDescriptor,
        virtual_address: usize,
        vm_space: &mut impl VmTracker,
    ) -> CapabilityResult {
        loop {
            let depth =
                arch::address_space::get_unset_depth(address_space, virtual_address, PAGE_BITS)?;
            if depth == 0 {
                return Ok(());
            }
            if depth > 3 {
                return Err(CapabilityError::InvalidDepth);
            }

            let page_table = self.alloc_page_table(depth)?;
            match arch::address_space::map(address_space, page_table, virtual_address, 0) {
                Ok(()) => {
                    let slot = self
                        .page_table_pool_slot_from_descriptor(page_table)
                        .unwrap_or(0);
                    vm_space
                        .record_page_table(virtual_address, slot)
                        .map_err(|_| CapabilityError::InvalidArgument)?;
                }
                Err(CapabilityError::IllegalOperation) => continue,
                Err(e) => {
                    crate::info!(
                        "[pt.map.err] addr={:#018x} depth={:>2} pt={:#018x} err={:?}",
                        virtual_address,
                        depth,
                        page_table,
                        e
                    );
                    return Err(e);
                }
            }
        }
    }

    fn alloc_page_table(&mut self, depth: usize) -> Result<CapabilityDescriptor, CapabilityError> {
        while self.next_page_table_slot < PAGE_TABLE_POOL_SLOTS {
            let slot = self.next_page_table_slot;
            self.next_page_table_slot += 1;

            match arch::generic::convert(
                self.bootstrap_generic,
                CapabilityType::PageTable,
                depth as Word,
                1,
                self.page_table_pool_node,
                slot as Word,
            ) {
                Ok(()) => {
                    return Ok(make_child_slot_descriptor(
                        self.page_table_pool_node,
                        PAGE_TABLE_POOL_RADIX,
                        slot,
                    ));
                }
                Err(CapabilityError::InvalidArgument) => {
                    crate::info!(
                        "[pt.alloc.warn] slot={:>3} depth={:>2} bootstrap={:#018x} dst_node={:#018x}",
                        slot,
                        depth,
                        self.bootstrap_generic,
                        self.page_table_pool_node
                    );
                    continue;
                }
                Err(e) => {
                    crate::info!(
                        "[pt.alloc.err] slot={:>3} depth={:>2} bootstrap={:#018x} dst_node={:#018x} err={:?}",
                        slot,
                        depth,
                        self.bootstrap_generic,
                        self.page_table_pool_node,
                        e
                    );
                    return Err(e);
                }
            }
        }

        Err(CapabilityError::InvalidArgument)
    }

    fn frame_index_from_descriptor(&self, descriptor: CapabilityDescriptor) -> Option<usize> {
        let depth = crate::nanami_utils::descriptor::descriptor_depth(descriptor);
        if depth < PHYSICAL_NODE_RADIX {
            return None;
        }
        let shift = nun::WORD_BITS - depth;
        let mask = (1usize << PHYSICAL_NODE_RADIX) - 1;
        Some((descriptor >> shift) & mask)
    }

    fn page_table_pool_slot_from_descriptor(
        &self,
        descriptor: CapabilityDescriptor,
    ) -> Option<usize> {
        let depth = crate::nanami_utils::descriptor::descriptor_depth(descriptor);
        if depth < PAGE_TABLE_POOL_RADIX {
            return None;
        }
        let shift = nun::WORD_BITS - depth;
        let mask = (1usize << PAGE_TABLE_POOL_RADIX) - 1;
        Some((descriptor >> shift) & mask)
    }
}

#[derive(Clone, Copy)]
struct RootNodeAllocation {
    descriptor: CapabilityDescriptor,
}

fn create_root_node_from_initial_generics(
    init_info: &InitInfo,
    root_descriptor: CapabilityDescriptor,
    root_radix: usize,
    node_radix: usize,
    slot_candidates: &[usize],
    initial_generic_consumed_bytes: &mut [usize; INITIAL_GENERIC_CAPACITY],
) -> Result<RootNodeAllocation, CapabilityError> {
    let required_size_bits = kernel_object::memory_size_bits(KernelObjectKind::Node, node_radix)
        .ok_or(CapabilityError::InvalidArgument)?;
    let mut generic_index = 0usize;
    while generic_index < init_info.generic_list_count as usize {
        let g = init_info.generic_list[generic_index];
        if g.is_device || (g.size_radix as usize) < required_size_bits {
            generic_index += 1;
            continue;
        }

        let consumed_bytes = initial_generic_consumed_bytes
            .get(generic_index)
            .copied()
            .unwrap_or(0);
        let Some((_allocation_base, new_consumed_bytes)) =
            next_initial_generic_allocation(g, consumed_bytes, required_size_bits)
        else {
            generic_index += 1;
            continue;
        };

        let generic = generic_descriptor_from_index(root_radix, generic_index);
        crate::info!(
            "root-node source generic idx={:>3} addr={:#018x} size_radix={:>2} node_radix={:>2} required_radix={:>2}",
            generic_index,
            g.address as usize,
            g.size_radix,
            node_radix,
            required_size_bits
        );

        match create_root_node(
            root_descriptor,
            root_radix,
            generic,
            node_radix,
            slot_candidates,
        ) {
            Ok(descriptor) => {
                initial_generic_consumed_bytes[generic_index] = new_consumed_bytes;
                return Ok(RootNodeAllocation { descriptor });
            }
            Err(CapabilityError::InvalidArgument) | Err(CapabilityError::IllegalOperation) => {
                generic_index += 1;
                continue;
            }
            Err(e) => return Err(e),
        }
    }

    Err(CapabilityError::InvalidArgument)
}

fn generic_descriptor_from_index(root_radix: usize, index: usize) -> CapabilityDescriptor {
    let generic_node =
        make_root_slot_descriptor(root_radix, nun::InitSlotOffset::GenericNode as usize);
    make_child_slot_descriptor(generic_node, GENERIC_NODE_RADIX, index)
}

impl MemoryManager {
    #[inline(always)]
    fn generic_descriptor_from_index(&self, index: usize) -> CapabilityDescriptor {
        generic_descriptor_from_index(self.root_radix, index)
    }

    fn initial_generic_consumed_bytes_for_index(&self, index: usize) -> usize {
        self.initial_generic_consumed_bytes
            .get(index)
            .copied()
            .unwrap_or(0)
    }
}

fn create_root_node(
    root_descriptor: CapabilityDescriptor,
    root_radix: usize,
    generic: CapabilityDescriptor,
    node_radix: usize,
    slot_candidates: &[usize],
) -> Result<CapabilityDescriptor, CapabilityError> {
    for slot in slot_candidates {
        crate::info!("radix={:>2} slot={:>5}", node_radix, slot);
        match arch::generic::convert(
            generic,
            CapabilityType::Node,
            node_radix as Word,
            1,
            root_descriptor,
            *slot as Word,
        ) {
            Ok(()) => {
                crate::info!("radix={:>2} slot={:>5}", node_radix, slot);
                return Ok(make_root_slot_descriptor(root_radix, *slot));
            }
            Err(CapabilityError::InvalidArgument) => continue,
            Err(e) => return Err(e),
        }
    }

    Err(CapabilityError::InvalidArgument)
}

fn map_physical_alloc_error(error: PhysicalAllocError) -> CapabilityError {
    match error {
        PhysicalAllocError::InvalidArgument => CapabilityError::InvalidArgument,
        PhysicalAllocError::PermissionDenied => CapabilityError::PermissionDenied,
        PhysicalAllocError::OutOfMemory => CapabilityError::InvalidArgument,
    }
}

fn checked_align_up(value: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    value.checked_add(align - 1).map(|v| v & !(align - 1))
}

fn checked_pow2(bits: usize) -> Option<usize> {
    1usize.checked_shl(bits as u32)
}

fn next_initial_generic_allocation(
    generic: nun::GenericDescriptor,
    consumed_bytes: usize,
    required_size_bits: usize,
) -> Option<(usize, usize)> {
    let base = generic.address as usize;
    let size = checked_pow2(generic.size_radix as usize)?;
    let end = base.checked_add(size)?;
    let unit = checked_pow2(required_size_bits)?;
    let current = base.checked_add(consumed_bytes.min(size))?;
    let allocation_base = checked_align_up(current, unit)?;
    let allocation_end = allocation_base.checked_add(unit)?;
    if allocation_end > end {
        return None;
    }
    Some((allocation_base, allocation_end - base))
}
