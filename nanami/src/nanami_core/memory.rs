use alloc::vec::Vec;

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

const PHYSICAL_DIRECTORY_RADIX: usize = 10;
const PHYSICAL_DIRECTORY_SLOTS: usize = 1 << PHYSICAL_DIRECTORY_RADIX;
const PHYSICAL_LEAF_RADIX: usize = 11;
const PHYSICAL_LEAF_PAGES: usize = 1 << PHYSICAL_LEAF_RADIX;
const PHYSICAL_CHUNK_RADIX: usize = PAGE_BITS + PHYSICAL_LEAF_RADIX;
const PHYSICAL_CHUNK_SIZE: usize = 1 << PHYSICAL_CHUNK_RADIX;
const FRAME_LEAF_POOL_RADIX: usize = PHYSICAL_CHUNK_RADIX;
const FRAME_LEAF_NODE_MEMORY_RADIX: usize =
    kernel_object::node_memory_size_bits(PHYSICAL_LEAF_RADIX);
const FRAME_LEAVES_PER_POOL: usize = 1 << (FRAME_LEAF_POOL_RADIX - FRAME_LEAF_NODE_MEMORY_RADIX);
const GENERIC_NODE_RADIX: usize = 7;
const PAGE_TABLE_POOL_RADIX: usize = 10;
const PAGE_TABLE_POOL_SLOTS: usize = 1 << PAGE_TABLE_POOL_RADIX;
const KERNEL_OBJECT_POOL_RADIX: usize = 27;
const FRAME_POOL_DIRECTORY_RADIX: usize = 9;
const PROCESS_ARENA_DIRECTORY_RADIX: usize = 12;
const PROCESS_ARENA_DIRECTORY_SLOTS: usize = 1 << PROCESS_ARENA_DIRECTORY_RADIX;
const PROCESS_ARENA_RADIX: usize = 21;

const PHYSICAL_GENERIC_DIRECTORY_SLOT_CANDIDATES: [usize; 4] = [1024, 1025, 1026, 1027];
const KERNEL_OBJECT_POOL_SLOT_CANDIDATES: [usize; 4] = [1028, 1029, 1030, 1031];
const INITIAL_FRAME_LEAF_POOL_SLOT_CANDIDATES: [usize; 4] = [1040, 1041, 1042, 1043];
const PHYSICAL_FRAME_DIRECTORY_SLOT_CANDIDATES: [usize; 4] = [1100, 1101, 1102, 1103];
const FRAME_POOL_DIRECTORY_SLOT_CANDIDATES: [usize; 4] = [1120, 1121, 1122, 1123];
const PAGE_TABLE_POOL_SLOT_CANDIDATES: [usize; 4] = [1200, 1201, 1202, 1203];
const PROCESS_ARENA_DIRECTORY_SLOT_CANDIDATES: [usize; 4] = [1210, 1211, 1212, 1213];
const INITIAL_GENERIC_CAPACITY: usize = 128;

pub struct MemoryManager {
    pub root_descriptor: CapabilityDescriptor,
    pub root_radix: usize,
    kernel_object_generic: CapabilityDescriptor,
    physical_generic_directory: CapabilityDescriptor,
    physical_frame_directory: CapabilityDescriptor,
    initial_frame_leaf_pool: CapabilityDescriptor,
    frame_pool_directory: CapabilityDescriptor,
    frame_leaf_pool_count: usize,
    next_frame_leaf_slot: usize,
    page_table_pool_node: CapabilityDescriptor,
    next_page_table_slot: usize,
    process_arena_directory: CapabilityDescriptor,
    process_arenas_ready: [bool; PROCESS_ARENA_DIRECTORY_SLOTS],
    physical_allocator: Option<PhysicalAllocator>,
    initial_generics: [nun::GenericDescriptor; INITIAL_GENERIC_CAPACITY],
    initial_generic_count: usize,
    initial_generic_consumed_bytes: [usize; INITIAL_GENERIC_CAPACITY],
    physical_chunks: [PhysicalChunk; PHYSICAL_DIRECTORY_SLOTS],
    physical_chunk_count: usize,
    physical_chunk_map: [PhysicalChunkMap; PHYSICAL_DIRECTORY_SLOTS],
    physical_chunk_map_count: usize,
}

#[derive(Clone, Copy)]
struct PhysicalChunk {
    physical_base_page: usize,
    page_count: usize,
    is_device: bool,
    source_is_page_node: bool,
    frame_leaf_ready: bool,
    used_as_frame_pool: bool,
}

#[derive(Clone, Copy)]
struct PhysicalChunkMap {
    physical_base_page: usize,
    page_count: usize,
    chunk_index: usize,
}

const EMPTY_PHYSICAL_CHUNK: PhysicalChunk = PhysicalChunk {
    physical_base_page: 0,
    page_count: 0,
    is_device: false,
    source_is_page_node: false,
    frame_leaf_ready: false,
    used_as_frame_pool: false,
};

const EMPTY_PHYSICAL_CHUNK_MAP: PhysicalChunkMap = PhysicalChunkMap {
    physical_base_page: 0,
    page_count: 0,
    chunk_index: 0,
};

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
        if root_generic_index >= init_info.generic_list_count as usize
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
        crate::info!("memory: create physical frame directory");
        let physical_frame_directory = create_root_node_from_initial_generic(
            init_info,
            root_descriptor,
            root_radix,
            bootstrap_generic_index,
            PHYSICAL_DIRECTORY_RADIX,
            &PHYSICAL_FRAME_DIRECTORY_SLOT_CANDIDATES,
            &mut initial_generic_consumed_bytes,
        )?;
        crate::info!(
            "physical frame directory={:#018x}",
            physical_frame_directory.descriptor
        );
        crate::info!("memory: create frame pool directory");
        let frame_pool_directory = create_root_node_from_initial_generic(
            init_info,
            root_descriptor,
            root_radix,
            bootstrap_generic_index,
            FRAME_POOL_DIRECTORY_RADIX,
            &FRAME_POOL_DIRECTORY_SLOT_CANDIDATES,
            &mut initial_generic_consumed_bytes,
        )?;
        crate::info!(
            "frame pool directory={:#018x}",
            frame_pool_directory.descriptor
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

        let mut manager = Self {
            root_descriptor,
            root_radix,
            kernel_object_generic,
            physical_generic_directory: physical_generic_directory.descriptor,
            physical_frame_directory: physical_frame_directory.descriptor,
            initial_frame_leaf_pool,
            frame_pool_directory: frame_pool_directory.descriptor,
            frame_leaf_pool_count: 1,
            next_frame_leaf_slot: 0,
            page_table_pool_node,
            next_page_table_slot: 0,
            process_arena_directory,
            process_arenas_ready: [false; PROCESS_ARENA_DIRECTORY_SLOTS],
            physical_allocator: None,
            initial_generics: init_info.generic_list,
            initial_generic_count: init_info.generic_list_count as usize,
            initial_generic_consumed_bytes,
            physical_chunks: [EMPTY_PHYSICAL_CHUNK; PHYSICAL_DIRECTORY_SLOTS],
            physical_chunk_count: 0,
            physical_chunk_map: [EMPTY_PHYSICAL_CHUNK_MAP; PHYSICAL_DIRECTORY_SLOTS],
            physical_chunk_map_count: 0,
        };

        crate::info!("memory: build hierarchical physical capability sources");
        manager.split_all_initial_generics(init_info)?;
        crate::info!("memory: split complete");

        Ok(manager)
    }

    pub fn physical_page_index_from_address(&self, physical_address: usize) -> Option<usize> {
        Some(physical_address >> PAGE_BITS)
    }

    pub fn physical_frame_descriptor_from_index(
        &self,
        frame_index: usize,
    ) -> Option<CapabilityDescriptor> {
        let (chunk_index, page_offset) = self.physical_location(frame_index)?;
        let chunk = self.physical_chunks.get(chunk_index)?;
        if chunk.used_as_frame_pool {
            return None;
        }
        Some(make_child_slot_descriptor(
            self.physical_frame_leaf_descriptor(chunk_index),
            PHYSICAL_LEAF_RADIX,
            page_offset,
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

    pub fn map_frame(
        &mut self,
        address_space: CapabilityDescriptor,
        frame_descriptor: CapabilityDescriptor,
        virtual_address: usize,
        vm_space: &mut impl VmTracker,
    ) -> CapabilityResult {
        self.ensure_page_tables(address_space, virtual_address, vm_space)?;

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
        self.ensure_page_tables(address_space, virtual_address, vm_space)?;

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

    pub fn frame_node_descriptor(&self) -> CapabilityDescriptor {
        self.physical_frame_directory
    }

    pub fn frame_node_radix(&self) -> usize {
        PHYSICAL_DIRECTORY_RADIX
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

        let mut allocated = Vec::new();
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

    pub fn ensure_alpha_frame_at_physical_index(
        &mut self,
        frame_index: usize,
    ) -> Result<(), CapabilityError> {
        let (chunk_index, _) = self
            .physical_location(frame_index)
            .ok_or(CapabilityError::InvalidArgument)?;
        let chunk = self
            .physical_chunks
            .get(chunk_index)
            .copied()
            .ok_or(CapabilityError::InvalidArgument)?;
        if chunk.used_as_frame_pool {
            return Err(CapabilityError::InvalidArgument);
        }
        if chunk.frame_leaf_ready {
            return Ok(());
        }
        self.materialize_physical_frame_leaf(chunk_index)
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
                let requested_end = physical_address.saturating_add(size_bytes);
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
        let g = self.initial_generics[generic_idx];
        let generic_start = g.address as usize;
        let generic_end = generic_start
            .checked_add(1usize << g.size_radix)
            .ok_or(CapabilityError::InvalidArgument)?;
        let requested_end = page_base
            .checked_add(page_count * PAGE_SIZE)
            .ok_or(CapabilityError::InvalidArgument)?;
        let consumed = self.initial_generic_consumed_bytes_for_index(generic_idx);
        let conversion_start = checked_align_up(
            generic_start
                .checked_add(consumed)
                .ok_or(CapabilityError::InvalidArgument)?,
            PAGE_SIZE,
        )
        .ok_or(CapabilityError::InvalidArgument)?;

        if requested_end > conversion_start {
            let conversion_end = checked_align_up(requested_end, PHYSICAL_CHUNK_SIZE)
                .unwrap_or(requested_end)
                .min(generic_end);
            self.install_physical_source_range(generic_idx, conversion_start, conversion_end)?;
            self.initial_generic_consumed_bytes[generic_idx] = conversion_end - generic_start;
        }

        let requested_base_page = page_base >> PAGE_BITS;
        for offset in 0..page_count {
            self.ensure_alpha_frame_at_physical_index(requested_base_page + offset)?;
        }
        Ok((requested_base_page, 0, page_count))
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

            self.install_physical_source_range(i, split_start, end)?;
        }

        crate::info!(
            "memory: physical chunks={} capacity={} chunk_bytes={:#x}",
            self.physical_chunk_count,
            PHYSICAL_DIRECTORY_SLOTS,
            PHYSICAL_CHUNK_SIZE
        );

        Ok(())
    }

    fn install_physical_source_range(
        &mut self,
        generic_index: usize,
        start: usize,
        end: usize,
    ) -> Result<(), CapabilityError> {
        if start >= end || start & (PAGE_SIZE - 1) != 0 || end & (PAGE_SIZE - 1) != 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let source_generic = self.generic_descriptor_from_index(generic_index);
        let mut cursor = start;
        while cursor < end {
            if self.physical_chunk_count >= PHYSICAL_DIRECTORY_SLOTS {
                return Err(CapabilityError::InvalidArgument);
            }
            let chunk_index = self.physical_chunk_count;
            let remaining = end - cursor;
            let full_chunk =
                cursor & (PHYSICAL_CHUNK_SIZE - 1) == 0 && remaining >= PHYSICAL_CHUNK_SIZE;
            let chunk_bytes = if full_chunk {
                PHYSICAL_CHUNK_SIZE
            } else {
                let until_boundary = PHYSICAL_CHUNK_SIZE - (cursor & (PHYSICAL_CHUNK_SIZE - 1));
                remaining.min(until_boundary)
            };
            let page_count = chunk_bytes >> PAGE_BITS;

            if full_chunk {
                arch::generic::convert(
                    source_generic,
                    CapabilityType::Generic,
                    PHYSICAL_CHUNK_RADIX as Word,
                    1,
                    self.physical_generic_directory,
                    chunk_index as Word,
                )?;
            } else {
                arch::generic::convert(
                    self.kernel_object_generic,
                    CapabilityType::Node,
                    PHYSICAL_LEAF_RADIX as Word,
                    1,
                    self.physical_generic_directory,
                    chunk_index as Word,
                )?;
                let source_page_node = self.physical_source_descriptor(chunk_index);
                arch::generic::convert(
                    source_generic,
                    CapabilityType::Generic,
                    PAGE_BITS as Word,
                    page_count as Word,
                    source_page_node,
                    0,
                )?;
            }

            self.physical_chunks[chunk_index] = PhysicalChunk {
                physical_base_page: cursor >> PAGE_BITS,
                page_count,
                is_device: self.initial_generics[generic_index].is_device,
                source_is_page_node: !full_chunk,
                frame_leaf_ready: false,
                used_as_frame_pool: false,
            };
            self.physical_chunk_count += 1;
            self.insert_physical_chunk_map(PhysicalChunkMap {
                physical_base_page: cursor >> PAGE_BITS,
                page_count,
                chunk_index,
            });
            cursor += chunk_bytes;
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

    fn physical_source_descriptor(&self, chunk_index: usize) -> CapabilityDescriptor {
        make_child_slot_descriptor(
            self.physical_generic_directory,
            PHYSICAL_DIRECTORY_RADIX,
            chunk_index,
        )
    }

    fn physical_frame_leaf_descriptor(&self, chunk_index: usize) -> CapabilityDescriptor {
        make_child_slot_descriptor(
            self.physical_frame_directory,
            PHYSICAL_DIRECTORY_RADIX,
            chunk_index,
        )
    }

    fn frame_leaf_pool_descriptor(&self, pool_index: usize) -> Option<CapabilityDescriptor> {
        if pool_index == 0 {
            return Some(self.initial_frame_leaf_pool);
        }
        let slot = pool_index - 1;
        if slot >= (1 << FRAME_POOL_DIRECTORY_RADIX) {
            return None;
        }
        Some(make_child_slot_descriptor(
            self.frame_pool_directory,
            FRAME_POOL_DIRECTORY_RADIX,
            slot,
        ))
    }

    fn insert_physical_chunk_map(&mut self, mapping: PhysicalChunkMap) {
        let index = self.physical_chunk_map[..self.physical_chunk_map_count]
            .partition_point(|entry| entry.physical_base_page < mapping.physical_base_page);
        self.physical_chunk_map
            .copy_within(index..self.physical_chunk_map_count, index + 1);
        self.physical_chunk_map[index] = mapping;
        self.physical_chunk_map_count += 1;
    }

    fn physical_location(&self, physical_page: usize) -> Option<(usize, usize)> {
        let end = self.physical_chunk_map[..self.physical_chunk_map_count]
            .partition_point(|entry| entry.physical_base_page <= physical_page);
        if end == 0 {
            return None;
        }
        let mapping = self.physical_chunk_map[end - 1];
        let offset = physical_page.checked_sub(mapping.physical_base_page)?;
        (offset < mapping.page_count).then_some((mapping.chunk_index, offset))
    }

    fn materialize_physical_frame_leaf(
        &mut self,
        chunk_index: usize,
    ) -> Result<(), CapabilityError> {
        let chunk = self
            .physical_chunks
            .get(chunk_index)
            .copied()
            .ok_or(CapabilityError::InvalidArgument)?;
        if chunk.frame_leaf_ready {
            return Ok(());
        }
        if chunk.used_as_frame_pool {
            return Err(CapabilityError::InvalidArgument);
        }

        self.ensure_frame_leaf_pool_capacity()?;
        let pool_index = self.next_frame_leaf_slot / FRAME_LEAVES_PER_POOL;
        let pool_slot = self.next_frame_leaf_slot % FRAME_LEAVES_PER_POOL;
        let pool = self
            .frame_leaf_pool_descriptor(pool_index)
            .ok_or(CapabilityError::InvalidArgument)?;
        arch::generic::convert(
            pool,
            CapabilityType::Node,
            PHYSICAL_LEAF_RADIX as Word,
            1,
            self.physical_frame_directory,
            chunk_index as Word,
        )?;
        self.next_frame_leaf_slot += 1;

        let frame_leaf = self.physical_frame_leaf_descriptor(chunk_index);
        let source = self.physical_source_descriptor(chunk_index);
        if chunk.source_is_page_node {
            for page in 0..chunk.page_count {
                let source_page = make_child_slot_descriptor(source, PHYSICAL_LEAF_RADIX, page);
                arch::generic::convert(
                    source_page,
                    CapabilityType::Frame,
                    PAGE_BITS as Word,
                    1,
                    frame_leaf,
                    page as Word,
                )?;
            }
        } else {
            arch::generic::convert(
                source,
                CapabilityType::Frame,
                PAGE_BITS as Word,
                chunk.page_count as Word,
                frame_leaf,
                0,
            )?;
        }

        self.physical_chunks[chunk_index].frame_leaf_ready = true;
        crate::info!(
            "memory: frame leaf ready chunk={} paddr={:#x} pages={} pool={} slot={}",
            chunk_index,
            chunk.physical_base_page << PAGE_BITS,
            chunk.page_count,
            pool_index,
            pool_slot
        );
        Ok(())
    }

    fn ensure_frame_leaf_pool_capacity(&mut self) -> Result<(), CapabilityError> {
        if self.next_frame_leaf_slot < self.frame_leaf_pool_count * FRAME_LEAVES_PER_POOL {
            return Ok(());
        }
        self.allocate_frame_leaf_pool()
    }

    fn allocate_frame_leaf_pool(&mut self) -> Result<(), CapabilityError> {
        if self.frame_leaf_pool_count - 1 >= (1 << FRAME_POOL_DIRECTORY_RADIX) {
            return Err(CapabilityError::InvalidArgument);
        }
        let mut selected = None;
        for (index, chunk) in self.physical_chunks[..self.physical_chunk_count]
            .iter()
            .enumerate()
            .rev()
        {
            if chunk.is_device
                || chunk.source_is_page_node
                || chunk.frame_leaf_ready
                || chunk.used_as_frame_pool
                || chunk.page_count != PHYSICAL_LEAF_PAGES
            {
                continue;
            }
            let result = self
                .physical_allocator
                .as_mut()
                .ok_or(CapabilityError::InvalidArgument)?
                .allocate_at(
                    chunk.physical_base_page << PAGE_BITS,
                    PHYSICAL_CHUNK_SIZE,
                    false,
                );
            if result.is_ok() {
                selected = Some(index);
                break;
            }
        }
        let chunk_index = selected.ok_or(CapabilityError::InvalidArgument)?;
        let pool_slot = self.frame_leaf_pool_count - 1;
        arch::generic::convert(
            self.physical_source_descriptor(chunk_index),
            CapabilityType::Generic,
            FRAME_LEAF_POOL_RADIX as Word,
            1,
            self.frame_pool_directory,
            pool_slot as Word,
        )?;
        self.physical_chunks[chunk_index].used_as_frame_pool = true;
        self.frame_leaf_pool_count += 1;
        crate::info!(
            "memory: frame leaf pool expanded pool={} chunk={} paddr={:#x}",
            self.frame_leaf_pool_count - 1,
            chunk_index,
            self.physical_chunks[chunk_index].physical_base_page << PAGE_BITS
        );
        Ok(())
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
