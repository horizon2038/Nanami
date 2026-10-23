//! The only adapter from A9N init metadata/slot layout to Alpha resource input.
//! No ordinary OS allocator, registry, service routing or child-domain manager
//! is initialized here. The shared launcher constructs a suspended Alpha.
mod capability_space;

use crate::nanami_utils::descriptor::{make_child_slot_descriptor, make_root_slot_descriptor};
use capability_space::PreparedRoot;
use nanami_uapi::alpha::{
    AlphaEnvironment, FramebufferInformation, KernelVersion, MemoryResource, PlatformInformation,
    SelfCapabilities, RESOURCE_DEVICE,
};
use nun::{CapabilityError, FramebufferInfo, InitInfo, InitSlotOffset};

pub fn boot(init: &InitInfo) -> ! {
    let mut environment = prepare(init).expect("boot resource conversion failed");
    let archive = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/servers/initramfs.cpio"
    ));
    let mut configuration = None;
    crate::nanami_core::cpio::for_each_newc_entry(archive, |entry| {
        if entry.name == "./nanami/boot-list" {
            configuration = Some(entry.data);
        }
        Ok(())
    })
    .expect("invalid boot archive");
    crate::force_info!("[boot] Alpha Booter preparing independent Alpha");
    let prepared = crate::nanami_core::alpha_launch::prepare(
        &mut environment,
        crate::nanami_core::alpha_launch::LaunchSlots {
            root: 1310,
            root_unit: 1311,
            backing_unit: 1312,
            control: 1313,
        },
        crate::nanami_core::alpha_launch::Inputs {
            image: include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/alpha.elf")),
            archive,
            configuration: configuration.expect("missing external boot configuration"),
            retire_launcher: true,
        },
        &mut crate::nanami_core::alpha_launch::NativeWriter,
    )
    .expect("First Alpha construction failed");
    crate::force_info!(
        "[boot] handoff ready resources={} control={:#x}",
        prepared.environment.resource_count,
        prepared.control
    );
    nun::arch::process_control_block::resume(prepared.control).expect("First Alpha resume failed");
    nun::arch::process_control_block::suspend(environment.capabilities.process_control_block)
        .expect("Booter suspension failed");
    panic!("retired Booter resumed unexpectedly");
}

pub fn prepare(init: &InitInfo) -> Result<AlphaEnvironment, CapabilityError> {
    let root = PreparedRoot::bootstrap(init)?;
    let descriptor =
        |slot: InitSlotOffset| make_root_slot_descriptor(root.root_radix, slot as usize);
    let mut environment = AlphaEnvironment {
        capabilities: SelfCapabilities {
            capability_space: root.root_descriptor,
            root_radix: root.root_radix,
            address_space: descriptor(InitSlotOffset::ProcessAddressSpace),
            process_control_block: descriptor(InitSlotOffset::ProcessControlBlock),
            page_tables: descriptor(InitSlotOffset::ProcessPageTableNode),
            frames: descriptor(InitSlotOffset::ProcessFrameNode),
            ipc_buffer: descriptor(InitSlotOffset::ProcessIpcBufferFrame),
            interrupt_region: descriptor(InitSlotOffset::InterruptRegion),
            io_ports: descriptor(InitSlotOffset::IoPort),
        },
        platform: PlatformInformation {
            architecture: init.architecture_name,
            platform: init.platform_name,
            core_count: init.core_count,
            rsdp_address: init.arch_info[0],
            kernel: KernelVersion {
                major: init.kernel_major_version,
                minor: init.kernel_minor_version,
                patch: init.kernel_patch_version,
                pre_release: init.kernel_pre_release,
                build_metadata: init.kernel_build_metadata,
            },
        },
        framebuffer: framebuffer(init).unwrap_or_default(),
        resource_count: init.generic_list_count,
        bootstrap_resource: root.bootstrap_generic_index,
        ..AlphaEnvironment::default()
    };
    let generics = descriptor(InitSlotOffset::GenericNode);
    for (index, generic) in init.generic_list[..init.generic_list_count]
        .iter()
        .enumerate()
    {
        let flags = if generic.is_device {
            RESOURCE_DEVICE
        } else {
            0
        };
        let consumed_bytes = if index == root.root_generic_index {
            root.root_generic_consumed_bytes
        } else {
            0
        };
        environment.resources[index] = MemoryResource {
            capability: make_child_slot_descriptor(generics, 7, index),
            address: generic.address,
            size_radix: generic.size_radix as usize,
            flags,
            consumed_bytes,
        };
    }
    environment
        .validate()
        .map_err(|_| CapabilityError::InvalidArgument)?;
    Ok(environment)
}

fn framebuffer(init: &InitInfo) -> Option<FramebufferInformation> {
    let mut raw = [0usize; 13];
    raw.copy_from_slice(&init.arch_info[1..14]);
    let fb = FramebufferInfo::deserialize(&raw);
    if fb.address == 0 || fb.width == 0 || fb.height == 0 || fb.bits_per_pixel == 0 {
        return None;
    }
    let bytes_per_pixel = (fb.bits_per_pixel as usize).checked_div(8)?;
    if bytes_per_pixel == 0 {
        return None;
    }
    let row_bytes = (fb.width as usize).checked_mul(bytes_per_pixel)?;
    let stride = fb.stride as usize;
    let stride_bytes = if stride >= row_bytes {
        stride
    } else {
        stride.checked_mul(bytes_per_pixel)?
    };
    let size_bytes = stride_bytes.checked_mul(fb.height as usize)?;
    if size_bytes == 0 || fb.address.checked_add(size_bytes).is_none() {
        return None;
    }
    Some(FramebufferInformation {
        display_id: 0,
        address: fb.address,
        size_bytes,
        width: fb.width as usize,
        height: fb.height as usize,
        stride,
        bits_per_pixel: fb.bits_per_pixel as usize,
        red_position: fb.red.position as usize,
        red_size: fb.red.size as usize,
        green_position: fb.green.position as usize,
        green_size: fb.green.size as usize,
        blue_position: fb.blue.position as usize,
        blue_size: fb.blue.size as usize,
    })
}
