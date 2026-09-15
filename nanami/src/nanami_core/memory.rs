use alloc::{collections::BTreeMap, vec::Vec};

mod directory;
mod frames;
mod sources;

use directory::{CapabilityDirectory, NodePool};
use frames::FrameRange;
use sources::PhysicalSource;

use crate::nanami_core::kernel_object::{self, KernelObjectKind};
use crate::nanami_core::physical_allocator::{
    PhysicalAllocError, PhysicalAllocator, PhysicalMemoryInfo,
};
use crate::nanami_core::vm_space::VmTracker;
use crate::nanami_utils::descriptor::{make_child_slot_descriptor, make_root_slot_descriptor};
use nun::{
    arch, capability_call::address_space::Attribute, CapabilityDescriptor, CapabilityError,
    CapabilityResult, CapabilityType, InitInfo, Word,
};

const PAGE_BITS: usize = 12;
const PAGE_SIZE: usize = 1 << PAGE_BITS;

// A materialized range contains at most 8 MiB. Neither the range index nor
// its capability directory has a fixed entry count tied to installed RAM.
const PHYSICAL_LEAF_RADIX: usize = 11;
const PHYSICAL_CHUNK_RADIX: usize = PAGE_BITS + PHYSICAL_LEAF_RADIX;
const PHYSICAL_CHUNK_SIZE: usize = 1 << PHYSICAL_CHUNK_RADIX;
pub(super) const FRAME_LEAF_POOL_RADIX: usize = PHYSICAL_CHUNK_RADIX;
pub(super) const PHYSICAL_DIRECTORY_RADIX: usize = directory::DIRECTORY_RADIX;
const GENERIC_NODE_RADIX: usize = 7;
const PAGE_TABLE_POOL_RADIX: usize = 10;
const PAGE_TABLE_POOL_SLOTS: usize = 1 << PAGE_TABLE_POOL_RADIX;
const KERNEL_OBJECT_POOL_RADIX: usize = 27;
const PROCESS_ARENA_DIRECTORY_RADIX: usize = 12;
const PROCESS_ARENA_DIRECTORY_SLOTS: usize = 1 << PROCESS_ARENA_DIRECTORY_RADIX;
const PROCESS_ARENA_RADIX: usize = 21;

const PHYSICAL_GENERIC_DIRECTORY_SLOT_CANDIDATES: [usize; 4] = [1024, 1025, 1026, 1027];
const KERNEL_OBJECT_POOL_SLOT_CANDIDATES: [usize; 4] = [1028, 1029, 1030, 1031];
const INITIAL_FRAME_LEAF_POOL_SLOT_CANDIDATES: [usize; 4] = [1040, 1041, 1042, 1043];
const PHYSICAL_FRAME_DIRECTORY_SLOT_CANDIDATES: [usize; 4] = [1100, 1101, 1102, 1103];
const PAGE_TABLE_POOL_SLOT_CANDIDATES: [usize; 4] = [1200, 1201, 1202, 1203];
const PROCESS_ARENA_DIRECTORY_SLOT_CANDIDATES: [usize; 4] = [1210, 1211, 1212, 1213];
const INITIAL_GENERIC_CAPACITY: usize = 128;

pub struct MemoryManager {
    pub root_descriptor: CapabilityDescriptor,
    pub root_radix: usize,
    kernel_object_generic: CapabilityDescriptor,
    capability_directory: CapabilityDirectory,
    node_pool: NodePool,
    bootstrap_frames: Option<FrameRange>,
    physical_frames: BTreeMap<usize, FrameRange>,
    physical_sources: BTreeMap<(usize, usize), PhysicalSource>,
    page_table_pool_node: CapabilityDescriptor,
    next_page_table_slot: usize,
    process_arena_directory: CapabilityDescriptor,
    process_arenas_ready: [bool; PROCESS_ARENA_DIRECTORY_SLOTS],
    physical_allocator: Option<PhysicalAllocator>,
    initial_generics: [nun::GenericDescriptor; INITIAL_GENERIC_CAPACITY],
    initial_generic_count: usize,
    initial_generic_consumed_bytes: [usize; INITIAL_GENERIC_CAPACITY],
    // Delegating a generic does not allocate its RAM. Keep the capability
    // watermark separate from the boot reservations used by the buddy allocator.
    initial_generic_delegated_bytes: [usize; INITIAL_GENERIC_CAPACITY],
}

impl MemoryManager {
    pub fn bootstrap(
        init_info: &InitInfo,
        root_descriptor: CapabilityDescriptor,
        root_radix: usize,
        bootstrap_generic_index: usize,
        root_generic_index: usize,
        root_generic_consumed_bytes: usize,
    ) -> Result<Self, CapabilityError> {
        let mut initial_generic_consumed_bytes = [0usize; INITIAL_GENERIC_CAPACITY];
        if init_info.generic_list_count as usize > INITIAL_GENERIC_CAPACITY
            || root_generic_index >= init_info.generic_list_count as usize
            || root_generic_index >= INITIAL_GENERIC_CAPACITY
        {
            return Err(CapabilityError::InvalidArgument);
        }
        initial_generic_consumed_bytes[root_generic_index] = root_generic_consumed_bytes;

        crate::info!("memory: create kernel object generic pool");
        let kernel_object_generic = create_generic_from_initial_generic(
            init_info,
            root_descriptor,
            root_radix,
            bootstrap_generic_index,
            KERNEL_OBJECT_POOL_RADIX,
            &KERNEL_OBJECT_POOL_SLOT_CANDIDATES,
            &mut initial_generic_consumed_bytes,
        )?;
        crate::info!("kernel object generic={:#018x}", kernel_object_generic);
        crate::info!("memory: create initial frame leaf pool");
        let initial_frame_leaf_pool = create_generic_from_initial_generic(
            init_info,
            root_descriptor,
            root_radix,
            bootstrap_generic_index,
            FRAME_LEAF_POOL_RADIX,
            &INITIAL_FRAME_LEAF_POOL_SLOT_CANDIDATES,
            &mut initial_generic_consumed_bytes,
        )?;
        crate::info!("initial frame leaf pool={:#018x}", initial_frame_leaf_pool);
        crate::info!("memory: create physical generic directory");
        let physical_generic_directory = create_root_node_from_initial_generic(
            init_info,
            root_descriptor,
            root_radix,
            bootstrap_generic_index,
            PHYSICAL_DIRECTORY_RADIX,
            &PHYSICAL_GENERIC_DIRECTORY_SLOT_CANDIDATES,
            &mut initial_generic_consumed_bytes,
        )?;
        crate::info!(
            "physical generic directory={:#018x}",
            physical_generic_directory.descriptor
        );
        crate::info!("memory: create page-table pool node");
        let page_table_pool_node = create_root_node(
            root_descriptor,
            root_radix,
            kernel_object_generic,
            PAGE_TABLE_POOL_RADIX,
            &PAGE_TABLE_POOL_SLOT_CANDIDATES,
        )?;
        crate::info!("page-table pool node={:#018x}", page_table_pool_node);
        crate::info!("memory: create process arena directory");
        let process_arena_directory = create_root_node(
            root_descriptor,
            root_radix,
            kernel_object_generic,
            PROCESS_ARENA_DIRECTORY_RADIX,
            &PROCESS_ARENA_DIRECTORY_SLOT_CANDIDATES,
        )?;
        crate::info!("process arena directory={:#018x}", process_arena_directory);

        Ok(Self {
            root_descriptor,
            root_radix,
            kernel_object_generic,
            capability_directory: CapabilityDirectory::new(physical_generic_directory.descriptor),
            node_pool: NodePool::new(initial_frame_leaf_pool),
            bootstrap_frames: None,
            physical_frames: BTreeMap::new(),
            physical_sources: BTreeMap::new(),
            page_table_pool_node,
            next_page_table_slot: 0,
            process_arena_directory,
            process_arenas_ready: [false; PROCESS_ARENA_DIRECTORY_SLOTS],
            physical_allocator: None,
            initial_generics: init_info.generic_list,
            initial_generic_count: init_info.generic_list_count as usize,
            initial_generic_consumed_bytes,
            initial_generic_delegated_bytes: initial_generic_consumed_bytes,
        })
    }

    pub fn physical_page_index_from_address(&self, physical_address: usize) -> Option<usize> {
        Some(physical_address >> PAGE_BITS)
    }

    pub fn physical_frame_descriptor_from_index(
        &self,
        frame_index: usize,
    ) -> Option<CapabilityDescriptor> {
        self.frame_range(frame_index)?.descriptor(frame_index)
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

    pub fn map_frame(
        &mut self,
        address_space: CapabilityDescriptor,
        frame_descriptor: CapabilityDescriptor,
        virtual_address: usize,
        vm_space: &mut impl VmTracker,
    ) -> CapabilityResult {
        if !vm_space.page_tables_ready(virtual_address) {
            self.ensure_page_tables(address_space, virtual_address, vm_space)?;
        }

        let attr = Attribute::ALL;

        match arch::address_space::map(address_space, frame_descriptor, virtual_address, attr) {
            Ok(()) | Err(CapabilityError::IllegalOperation) => {
                vm_space
                    .record_frame(virtual_address, frame_descriptor)
                    .map_err(|_| CapabilityError::InvalidArgument)?;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    pub fn map_frame_strict(
        &mut self,
        address_space: CapabilityDescriptor,
        frame_descriptor: CapabilityDescriptor,
        virtual_address: usize,
        vm_space: &mut impl VmTracker,
    ) -> CapabilityResult {
        if !vm_space.page_tables_ready(virtual_address) {
            self.ensure_page_tables(address_space, virtual_address, vm_space)?;
        }

        let attr = Attribute::ALL;
        match arch::address_space::map(address_space, frame_descriptor, virtual_address, attr) {
            Ok(()) => {
                vm_space
                    .record_frame(virtual_address, frame_descriptor)
                    .map_err(|_| CapabilityError::InvalidArgument)?;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    pub fn root_radix(&self) -> usize {
        self.root_radix
    }

    pub fn page_size(&self) -> usize {
        PAGE_SIZE
    }

    pub fn physical_memory_info(&self) -> Result<PhysicalMemoryInfo, CapabilityError> {
        self.physical_allocator
            .as_ref()
            .map(PhysicalAllocator::memory_info)
            .ok_or(CapabilityError::InvalidArgument)
    }

    pub fn kernel_object_generic(&self) -> CapabilityDescriptor {
        self.kernel_object_generic
    }

    pub fn ensure_process_arena(
        &mut self,
        process_root_slot: usize,
    ) -> Result<CapabilityDescriptor, CapabilityError> {
        if process_root_slot >= PROCESS_ARENA_DIRECTORY_SLOTS {
            return Err(CapabilityError::InvalidArgument);
        }
        if !self.process_arenas_ready[process_root_slot] {
            arch::generic::convert(
                self.kernel_object_generic,
                CapabilityType::Generic,
                PROCESS_ARENA_RADIX as Word,
                1,
                self.process_arena_directory,
                process_root_slot as Word,
            )?;
            self.process_arenas_ready[process_root_slot] = true;
        }
        self.process_arena_descriptor(process_root_slot)
    }

    pub fn process_arena_descriptor(
        &self,
        process_root_slot: usize,
    ) -> Result<CapabilityDescriptor, CapabilityError> {
        if process_root_slot >= PROCESS_ARENA_DIRECTORY_SLOTS
            || !self.process_arenas_ready[process_root_slot]
        {
            return Err(CapabilityError::InvalidArgument);
        }
        Ok(make_child_slot_descriptor(
            self.process_arena_directory,
            PROCESS_ARENA_DIRECTORY_RADIX,
            process_root_slot,
        ))
    }

    pub fn reset_process_arena(&mut self, process_root_slot: usize) -> Result<(), CapabilityError> {
        if process_root_slot >= PROCESS_ARENA_DIRECTORY_SLOTS
            || !self.process_arenas_ready[process_root_slot]
        {
            return Ok(());
        }
        arch::node::revoke(self.process_arena_directory, process_root_slot as Word)
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
            if g.is_device {
                continue;
            }
            let size_bytes = 1usize << g.size_radix;
            if size_bytes < PAGE_SIZE {
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
                let mut free_start = (g.address as usize).saturating_add(consumed);
                let mut free_size = size_bytes - consumed;
                if free_start == 0 {
                    let reserved = PAGE_SIZE.min(free_size);
                    allocator
                        .add_region(0, reserved, g.is_device, true)
                        .map_err(map_physical_alloc_error)?;
                    free_start = free_start.saturating_add(reserved);
                    free_size -= reserved;
                }
                if free_size == 0 {
                    continue;
                }
                allocator
                    .add_region(free_start, free_size, g.is_device, false)
                    .map_err(map_physical_alloc_error)?;
            }
        }

        self.initialize_physical_sources()?;
        self.physical_allocator = Some(allocator);
        Ok(())
    }

    pub fn allocate_physical_at(
        &mut self,
        physical_address: usize,
        size_bytes: usize,
        allow_device: bool,
    ) -> Result<usize, CapabilityError> {
        let allocation_result = {
            let allocator = self
                .physical_allocator
                .as_mut()
                .ok_or(CapabilityError::InvalidArgument)?;
            allocator.allocate_at(physical_address, size_bytes, allow_device)
        };
        match allocation_result {
            Ok(allocation) => Ok(allocation.base_page),
            Err(PhysicalAllocError::OutOfMemory) if allow_device => {
                if self.initial_range_is_device(physical_address, size_bytes) {
                    return self
                        .physical_page_index_from_address(physical_address)
                        .ok_or(CapabilityError::InvalidArgument);
                }
                Err(CapabilityError::InvalidArgument)
            }
            Err(e) => Err(map_physical_alloc_error(e)),
        }
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
    ) -> Result<Vec<usize>, CapabilityError> {
        let max_slots = 1usize << destination_frame_node_radix;
        if destination_base_slot + count > max_slots {
            return Err(CapabilityError::InvalidArgument);
        }

        if self.physical_allocator.is_none() {
            return Err(CapabilityError::InvalidArgument);
        }

        let mut allocated = Vec::with_capacity(count);
        let mut copied = 0usize;
        while copied < count {
            let dst_slot = destination_base_slot + copied;
            let frame_index = self.allocate_physical_any(PAGE_SIZE)?;
            if let Err(e) = self.copy_alpha_frame_to_process_node(
                frame_index,
                destination_frame_node,
                destination_frame_node_radix,
                dst_slot,
            ) {
                crate::info!(
                    "[frame.copy.err] pfn={:#x} dst_node={:#018x} dst_slot={:>6} err={:?}",
                    frame_index,
                    destination_frame_node,
                    dst_slot,
                    e
                );
                let _ = self.free_physical(frame_index * PAGE_SIZE, PAGE_SIZE);
                return Err(e);
            }
            allocated.push(frame_index);
            copied += 1;
        }
        Ok(allocated)
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
        let total_span = offset
            .checked_add(size_bytes)
            .ok_or(CapabilityError::InvalidArgument)?;
        let page_count = total_span
            .checked_add(PAGE_SIZE - 1)
            .ok_or(CapabilityError::InvalidArgument)?
            / PAGE_SIZE;
        let requested_end = page_base
            .checked_add(page_count * PAGE_SIZE)
            .ok_or(CapabilityError::InvalidArgument)?;

        let mut selected: Option<(usize, usize)> = None;
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
                if physical_address >= start && requested_end <= end {
                    match selected {
                        None => selected = Some((i, size)),
                        Some((_, best_size)) if size < best_size => selected = Some((i, size)),
                        _ => {}
                    }
                }
                i += 1;
            }
            if selected.is_some() {
                break;
            }
        }

        let (generic_idx, _) = selected.ok_or(CapabilityError::InvalidArgument)?;
        let requested_base_page = page_base >> PAGE_BITS;
        for offset in 0..page_count {
            if self.frame_range(requested_base_page + offset).is_none() {
                self.materialize_frame_range(
                    generic_idx,
                    (requested_base_page + offset) << PAGE_BITS,
                )?;
            }
            self.ensure_alpha_frame_at_physical_index(requested_base_page + offset)?;
        }
        Ok((requested_base_page, 0, page_count))
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
            let attr = Attribute::ALL;
            match arch::address_space::map(address_space, page_table, virtual_address, attr) {
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
                self.kernel_object_generic,
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
                        self.kernel_object_generic,
                        self.page_table_pool_node
                    );
                    continue;
                }
                Err(e) => {
                    crate::info!(
                        "[pt.alloc.err] slot={:>3} depth={:>2} bootstrap={:#018x} dst_node={:#018x} err={:?}",
                        slot,
                        depth,
                        self.kernel_object_generic,
                        self.page_table_pool_node,
                        e
                    );
                    return Err(e);
                }
            }
        }

        Err(CapabilityError::InvalidArgument)
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

fn create_root_node_from_initial_generic(
    init_info: &InitInfo,
    root_descriptor: CapabilityDescriptor,
    root_radix: usize,
    generic_index: usize,
    node_radix: usize,
    slot_candidates: &[usize],
    initial_generic_consumed_bytes: &mut [usize; INITIAL_GENERIC_CAPACITY],
) -> Result<RootNodeAllocation, CapabilityError> {
    let required_size_bits = kernel_object::memory_size_bits(KernelObjectKind::Node, node_radix)
        .ok_or(CapabilityError::InvalidArgument)?;
    if generic_index >= init_info.generic_list_count as usize
        || generic_index >= INITIAL_GENERIC_CAPACITY
    {
        return Err(CapabilityError::InvalidArgument);
    }
    let g = init_info.generic_list[generic_index];
    if g.is_device || (g.size_radix as usize) < required_size_bits {
        return Err(CapabilityError::InvalidArgument);
    }
    let consumed_bytes = initial_generic_consumed_bytes[generic_index];
    let (_, new_consumed_bytes) =
        next_initial_generic_allocation(g, consumed_bytes, required_size_bits)
            .ok_or(CapabilityError::InvalidArgument)?;
    let generic = generic_descriptor_from_index(root_radix, generic_index);
    crate::info!(
        "root-node source generic idx={:>3} addr={:#018x} size_radix={:>2} node_radix={:>2} required_radix={:>2}",
        generic_index,
        g.address as usize,
        g.size_radix,
        node_radix,
        required_size_bits
    );
    let descriptor = create_root_node(
        root_descriptor,
        root_radix,
        generic,
        node_radix,
        slot_candidates,
    )?;
    initial_generic_consumed_bytes[generic_index] = new_consumed_bytes;
    Ok(RootNodeAllocation { descriptor })
}

fn create_generic_from_initial_generic(
    init_info: &InitInfo,
    root_descriptor: CapabilityDescriptor,
    root_radix: usize,
    generic_index: usize,
    generic_radix: usize,
    slot_candidates: &[usize],
    initial_generic_consumed_bytes: &mut [usize; INITIAL_GENERIC_CAPACITY],
) -> Result<CapabilityDescriptor, CapabilityError> {
    if generic_index >= init_info.generic_list_count as usize
        || generic_index >= INITIAL_GENERIC_CAPACITY
    {
        return Err(CapabilityError::InvalidArgument);
    }
    let g = init_info.generic_list[generic_index];
    if g.is_device || (g.size_radix as usize) < generic_radix {
        return Err(CapabilityError::InvalidArgument);
    }
    let consumed_bytes = initial_generic_consumed_bytes[generic_index];
    let (_, new_consumed_bytes) = next_initial_generic_allocation(g, consumed_bytes, generic_radix)
        .ok_or(CapabilityError::InvalidArgument)?;
    let source = generic_descriptor_from_index(root_radix, generic_index);
    for slot in slot_candidates {
        match arch::generic::convert(
            source,
            CapabilityType::Generic,
            generic_radix as Word,
            1,
            root_descriptor,
            *slot as Word,
        ) {
            Ok(()) => {
                initial_generic_consumed_bytes[generic_index] = new_consumed_bytes;
                return Ok(make_root_slot_descriptor(root_radix, *slot));
            }
            Err(CapabilityError::InvalidArgument) => continue,
            Err(error) => return Err(error),
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

    pub fn initial_generic_consumed_bytes_for_public(&self, index: usize) -> usize {
        self.initial_generic_consumed_bytes_for_index(index)
    }

    fn initial_range_is_device(&self, physical_address: usize, size_bytes: usize) -> bool {
        if size_bytes == 0 {
            return false;
        }
        let requested_end = physical_address.saturating_add(size_bytes);
        let mut i = 0usize;
        while i < self.initial_generic_count {
            let g = self.initial_generics[i];
            let start = g.address as usize;
            let end = start.saturating_add(1usize << g.size_radix);
            if physical_address >= start && requested_end <= end {
                return g.is_device;
            }
            i += 1;
        }
        false
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
