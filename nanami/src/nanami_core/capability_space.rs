use crate::nanami_core::kernel_object::{self, KernelObjectKind};
use crate::nanami_utils::descriptor::{make_child_slot_descriptor, make_root_slot_descriptor};
use nun::{
    arch, AsCapabilityDescriptor, CapabilityDescriptor, CapabilityError, InitInfo, InitSlotOffset,
    Word,
};

const OLD_ROOT_RADIX: usize = 8;
const NEW_ROOT_RADIX: usize = 12;
const GENERIC_NODE_RADIX: usize = 7;
const PHYSICAL_DIRECTORY_RADIX: usize = 10;
const FRAME_POOL_DIRECTORY_RADIX: usize = 9;
const KERNEL_OBJECT_POOL_RADIX: usize = 27;
const FRAME_LEAF_POOL_RADIX: usize = 23;
const NEW_ROOT_SLOT_CANDIDATES: [usize; 8] = [240, 241, 242, 243, 244, 245, 246, 247];

#[derive(Clone, Copy)]
pub struct RootCapabilitySpace {
    pub root_descriptor: CapabilityDescriptor,
    pub root_radix: usize,
    pub bootstrap_generic: CapabilityDescriptor,
    pub root_generic_index: usize,
    pub root_generic_consumed_bytes: usize,
    pub bootstrap_generic_index: usize,
}

impl RootCapabilitySpace {
    pub fn bootstrap(init_info: &InitInfo) -> Result<Self, CapabilityError> {
        let root_size_bits =
            kernel_object::memory_size_bits(KernelObjectKind::Node, NEW_ROOT_RADIX)
                .ok_or(CapabilityError::InvalidArgument)?;
        let (root_generic_index, root_generic_consumed_bytes) =
            pick_smallest_non_device_generic_index(init_info, root_size_bits)?;
        let root_generic_old = init_info
            .get_generic_descriptor_from_index(root_generic_index as Word)
            .ok_or(CapabilityError::InvalidDescriptor)?;
        crate::info!(
            "root generic index={:>3} old_desc={:#018x} consumed={:#x}",
            root_generic_index,
            root_generic_old,
            root_generic_consumed_bytes
        );

        crate::info!("cap-space: pick bootstrap generic");
        let bootstrap_generic_index =
            pick_kernel_backing_generic_index(init_info, root_generic_index)?;
        crate::info!("bootstrap generic index={:>3}", bootstrap_generic_index);

        let old_root = InitSlotOffset::ProcessRootNode.as_descriptor();
        crate::info!(
            "cap-space: create new root node from old root={:#018x}",
            old_root
        );
        let new_root_in_old = create_new_root_node(old_root, root_generic_old)?;
        crate::info!("new root (old addressing)={:#018x}", new_root_in_old);

        crate::info!("cap-space: copy initial slots into new root");
        copy_initial_slots_into_new_root(new_root_in_old)?;
        crate::info!("cap-space: initial slots copied");
        crate::info!("cap-space: wire recursive self slot");
        wire_recursive_self_slot(new_root_in_old)?;
        crate::info!("cap-space: recursive self slot ready");
        crate::info!("cap-space: reconfigure current process root");
        configure_current_process_root(new_root_in_old)?;
        crate::info!("cap-space: process root reconfigured");

        let new_root_recursive =
            make_root_slot_descriptor(NEW_ROOT_RADIX, InitSlotOffset::ProcessRootNode as usize);
        let bootstrap_generic = make_generic_descriptor(NEW_ROOT_RADIX, bootstrap_generic_index);
        crate::info!(
            "new root recursive descriptor={:#018x} bootstrap_generic(new)={:#018x}",
            new_root_recursive,
            bootstrap_generic
        );

        Ok(Self {
            root_descriptor: new_root_recursive,
            root_radix: NEW_ROOT_RADIX,
            bootstrap_generic,
            root_generic_index,
            root_generic_consumed_bytes,
            bootstrap_generic_index,
        })
    }
}

fn pick_smallest_non_device_generic_index(
    init_info: &InitInfo,
    minimum_size_bits: usize,
) -> Result<(usize, usize), CapabilityError> {
    let unit = 1usize
        .checked_shl(minimum_size_bits as u32)
        .ok_or(CapabilityError::InvalidArgument)?;
    let mut best: Option<(usize, u8, usize)> = None;
    let count = init_info.generic_list_count as usize;

    for i in 0..count {
        let g = init_info.generic_list[i];
        if g.is_device {
            continue;
        }
        let Some(consumed) =
            aligned_allocation_consumed(g.address as usize, g.size_radix, unit, unit)
        else {
            continue;
        };
        match best {
            None => best = Some((i, g.size_radix, consumed)),
            Some((_, radix, _)) if g.size_radix < radix => best = Some((i, g.size_radix, consumed)),
            _ => {}
        }
    }

    best.map(|(index, _, consumed)| (index, consumed))
        .ok_or(CapabilityError::InvalidArgument)
}

fn pick_kernel_backing_generic_index(
    init_info: &InitInfo,
    root_generic_index: usize,
) -> Result<usize, CapabilityError> {
    let alignment = 1usize << KERNEL_OBJECT_POOL_RADIX;
    let required = (1usize << KERNEL_OBJECT_POOL_RADIX)
        .checked_add(1usize << FRAME_LEAF_POOL_RADIX)
        .and_then(|bytes| {
            bytes.checked_add(
                2usize << kernel_object::node_memory_size_bits(PHYSICAL_DIRECTORY_RADIX),
            )
        })
        .and_then(|bytes| {
            bytes.checked_add(
                1usize << kernel_object::node_memory_size_bits(FRAME_POOL_DIRECTORY_RADIX),
            )
        })
        .ok_or(CapabilityError::InvalidArgument)?;
    let mut best: Option<(usize, usize)> = None;
    let count = init_info.generic_list_count as usize;

    for i in 0..count {
        let g = init_info.generic_list[i];
        if g.is_device || i == root_generic_index {
            continue;
        }
        let Some(consumed) =
            aligned_allocation_consumed(g.address as usize, g.size_radix, alignment, required)
        else {
            continue;
        };
        match best {
            None => best = Some((i, consumed)),
            Some((_, best_consumed)) if consumed < best_consumed => best = Some((i, consumed)),
            _ => {}
        }
    }

    let (idx, _) = best.ok_or(CapabilityError::InvalidArgument)?;
    Ok(idx)
}

fn aligned_allocation_consumed(
    base: usize,
    size_radix: u8,
    alignment: usize,
    allocation_size: usize,
) -> Option<usize> {
    let region_size = 1usize.checked_shl(size_radix as u32)?;
    let region_end = base.checked_add(region_size)?;
    let allocation_base = base.checked_add(alignment.checked_sub(1)?)? & !(alignment - 1);
    let allocation_end = allocation_base.checked_add(allocation_size)?;
    (allocation_end <= region_end).then_some(allocation_end - base)
}

fn make_generic_descriptor(root_radix: usize, generic_index: usize) -> CapabilityDescriptor {
    let generic_node = make_root_slot_descriptor(root_radix, InitSlotOffset::GenericNode as usize);
    make_child_slot_descriptor(generic_node, GENERIC_NODE_RADIX, generic_index)
}

fn create_new_root_node(
    old_root: CapabilityDescriptor,
    generic: CapabilityDescriptor,
) -> Result<CapabilityDescriptor, CapabilityError> {
    for slot in NEW_ROOT_SLOT_CANDIDATES {
        crate::info!("cap-space: convert root node into slot {:>5}", slot);
        match arch::generic::convert(
            generic,
            nun::CapabilityType::Node,
            NEW_ROOT_RADIX as Word,
            1,
            old_root,
            slot,
        ) {
            Ok(()) => {
                crate::info!("cap-space: new root node slot={:>5}", slot);
                return Ok(make_root_slot_descriptor(OLD_ROOT_RADIX, slot));
            }
            Err(CapabilityError::InvalidArgument) => continue,
            Err(e) => return Err(e),
        }
    }

    Err(CapabilityError::InvalidArgument)
}

fn copy_initial_slots_into_new_root(
    new_root_in_old: CapabilityDescriptor,
) -> Result<(), CapabilityError> {
    let copies = [
        (
            InitSlotOffset::ProcessControlBlock as usize,
            InitSlotOffset::ProcessControlBlock.as_descriptor(),
        ),
        (
            InitSlotOffset::ProcessAddressSpace as usize,
            InitSlotOffset::ProcessAddressSpace.as_descriptor(),
        ),
        (
            InitSlotOffset::ProcessPageTableNode as usize,
            InitSlotOffset::ProcessPageTableNode.as_descriptor(),
        ),
        (
            InitSlotOffset::ProcessFrameNode as usize,
            InitSlotOffset::ProcessFrameNode.as_descriptor(),
        ),
        (
            InitSlotOffset::ProcessIpcBufferFrame as usize,
            InitSlotOffset::ProcessIpcBufferFrame.as_descriptor(),
        ),
        (
            InitSlotOffset::GenericNode as usize,
            InitSlotOffset::GenericNode.as_descriptor(),
        ),
        (
            InitSlotOffset::InterruptRegion as usize,
            InitSlotOffset::InterruptRegion.as_descriptor(),
        ),
        (
            InitSlotOffset::IoPort as usize,
            InitSlotOffset::IoPort.as_descriptor(),
        ),
    ];

    for (destination_slot, source) in copies {
        crate::info!("slot {:>3} source={:#018x}", destination_slot, source);
        match arch::node::copy(new_root_in_old, destination_slot, source) {
            Ok(()) => {}
            Err(CapabilityError::PermissionDenied) => {
                crate::info!("slot {:>3} copy denied, fallback to move", destination_slot);
                arch::node::movec(new_root_in_old, destination_slot, source)?;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn wire_recursive_self_slot(new_root_in_old: CapabilityDescriptor) -> Result<(), CapabilityError> {
    crate::info!(
        "copy self-root source={:#018x} dst_slot={:>3}",
        new_root_in_old,
        InitSlotOffset::ProcessRootNode as usize
    );
    arch::node::copy(
        new_root_in_old,
        InitSlotOffset::ProcessRootNode as usize,
        new_root_in_old,
    )
}

fn configure_current_process_root(
    new_root_in_old: CapabilityDescriptor,
) -> Result<(), CapabilityError> {
    let pcb = InitSlotOffset::ProcessControlBlock.as_descriptor();
    crate::info!("pcb={:#018x} new_root={:#018x}", pcb, new_root_in_old);
    let config = nun::capability_call::process_control_block::ConfigurationInfo::new(
        false, true, false, false, false, false, false, false, false, false,
    );

    arch::process_control_block::configure(pcb, config, 0, new_root_in_old, 0, 0, 0, 0, 0, 0, 0, 0)
}
