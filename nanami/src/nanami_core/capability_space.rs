use crate::nanami_utils::descriptor::{make_child_slot_descriptor, make_root_slot_descriptor};
use nun::{
    arch, AsCapabilityDescriptor, CapabilityDescriptor, CapabilityError, InitInfo, InitSlotOffset,
    Word,
};

const OLD_ROOT_RADIX: usize = 8;
const NEW_ROOT_RADIX: usize = 12;
const GENERIC_NODE_RADIX: usize = 7;
const NEW_ROOT_SLOT_CANDIDATES: [usize; 8] = [240, 241, 242, 243, 244, 245, 246, 247];

#[derive(Clone, Copy)]
pub struct RootCapabilitySpace {
    pub root_descriptor: CapabilityDescriptor,
    pub root_radix: usize,
    pub bootstrap_generic: CapabilityDescriptor,
}

impl RootCapabilitySpace {
    pub fn bootstrap(init_info: &InitInfo) -> Result<Self, CapabilityError> {
        crate::info!("cap-space: pick bootstrap generic");
        let bootstrap_generic_index = pick_largest_non_device_generic_index(init_info)?;
        let bootstrap_generic_old = init_info
            .get_generic_descriptor_from_index(bootstrap_generic_index as Word)
            .ok_or(CapabilityError::InvalidDescriptor)?;
        crate::info!(
            "bootstrap generic index={:>3} old_desc={:#018x}",
            bootstrap_generic_index,
            bootstrap_generic_old
        );

        let old_root = InitSlotOffset::ProcessRootNode.as_descriptor();
        crate::info!(
            "cap-space: create new root node from old root={:#018x}",
            old_root
        );
        let new_root_in_old = create_new_root_node(old_root, bootstrap_generic_old)?;
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
        })
    }
}

fn pick_largest_non_device_generic_index(init_info: &InitInfo) -> Result<usize, CapabilityError> {
    let mut best: Option<(usize, u8)> = None;
    let count = init_info.generic_list_count as usize;

    for i in 0..count {
        let g = init_info.generic_list[i];
        if g.is_device {
            continue;
        }
        match best {
            None => best = Some((i, g.size_radix)),
            Some((_, radix)) if g.size_radix > radix => best = Some((i, g.size_radix)),
            _ => {}
        }
    }

    let (idx, _) = best.ok_or(CapabilityError::InvalidArgument)?;
    Ok(idx)
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
