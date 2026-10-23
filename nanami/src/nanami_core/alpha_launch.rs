//! Allocation-free Alpha execution-environment construction shared by launchers.
//! Only the supplied Generic capabilities may back this launch. Each successful
//! conversion advances its reservation before another operation can fail.
use crate::nanami_core::{
    elf_loader,
    kernel_object::{self, KernelObjectKind},
};
use crate::nanami_utils::descriptor::{
    make_child_slot_descriptor as child, make_root_slot_descriptor as slot,
};
use nanami_uapi::alpha::{AlphaEnvironment, MappedInput, RetiredLauncher, SelfCapabilities};
use nun::{
    arch,
    capability_call::{address_space::Attribute, process_control_block::ConfigurationInfo},
    CapabilityError, CapabilityType, Word,
};

const PAGE: usize = 4096;
const ROOT_RADIX: usize = 12;
const FRAME_RADIX: usize = 14;
const BACKING_RADIX: usize = 14;
const TABLE_RADIX: usize = 10;
pub const TEMP: usize = 0x7400_0000;
const ARCHIVE: usize = 0x2000_0000;
const CONFIGURATION: usize = 0x2400_0000;
const MAX_IMAGE: usize = 64 * 1024 * 1024;
const MAX_INPUT: usize = 16 * 1024 * 1024;

pub struct Inputs<'a> {
    pub image: &'a [u8],
    pub archive: &'a [u8],
    pub configuration: &'a [u8],
    pub retire_launcher: bool,
}

/// Free slots reserved by the caller, not a fixed global child-Alpha location.
#[derive(Clone, Copy)]
pub struct LaunchSlots {
    pub root: usize,
    pub root_unit: usize,
    pub backing_unit: usize,
    pub control: usize,
}

/// A mapped page is valid only until the corresponding UNMAP. The native
/// writer accesses that mapping; host tests model it without raw host pointers.
pub trait PageWriter {
    fn zero(&mut self, address: usize);
    fn copy(&mut self, address: usize, bytes: &[u8]);
}

pub struct NativeWriter;
impl PageWriter for NativeWriter {
    fn zero(&mut self, address: usize) {
        unsafe {
            core::ptr::write_bytes(address as *mut u8, 0, PAGE);
        }
    }
    fn copy(&mut self, address: usize, bytes: &[u8]) {
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), address as *mut u8, bytes.len());
        }
    }
}

pub struct PreparedAlpha {
    pub control: Word,
    pub environment: AlphaEnvironment,
}

struct Builder<'a> {
    source: &'a mut AlphaEnvironment,
    backing: Word,
    next_backing: usize,
    tables: Word,
    next_table: usize,
    frames: Word,
    next_frame: usize,
}

fn invalid<T>() -> Result<T, CapabilityError> {
    Err(CapabilityError::InvalidArgument)
}
fn align(value: usize, bytes: usize) -> Result<usize, CapabilityError> {
    value
        .checked_add(bytes - 1)
        .map(|v| v & !(bytes - 1))
        .ok_or(CapabilityError::InvalidArgument)
}

/// Reserve a whole Generic unit before converting its contents. This makes the
/// externally recorded extent independent of padding inside a kernel object.
fn reserve(
    source: &mut AlphaEnvironment,
    radix: usize,
    node: Word,
    index: usize,
) -> Result<Word, CapabilityError> {
    let bytes = 1usize
        .checked_shl(radix as u32)
        .ok_or(CapabilityError::InvalidArgument)?;
    let mut selected = None;
    for pass in 0..2 {
        for (i, resource) in source
            .memory_resources()
            .map_err(|_| CapabilityError::InvalidArgument)?
            .iter()
            .enumerate()
        {
            if (pass == 0 && i == source.bootstrap_resource) || resource.is_device() {
                continue;
            }
            let start = align(
                resource
                    .address
                    .checked_add(resource.consumed_bytes)
                    .ok_or(CapabilityError::InvalidArgument)?,
                bytes,
            )?;
            let end = resource
                .address
                .checked_add(
                    resource
                        .size_bytes()
                        .ok_or(CapabilityError::InvalidArgument)?,
                )
                .ok_or(CapabilityError::InvalidArgument)?;
            let Some(limit) = start.checked_add(bytes).filter(|&limit| limit <= end) else {
                continue;
            };
            let remaining = end - start;
            if selected.is_none_or(|(_, _, best)| remaining < best) {
                selected = Some((i, limit, remaining));
            }
        }
        if selected.is_some() {
            break;
        }
    }
    let (i, limit, _) = selected.ok_or(CapabilityError::InvalidArgument)?;
    arch::generic::convert(
        source.resources[i].capability,
        CapabilityType::Generic,
        radix,
        1,
        node,
        index,
    )?;
    source.resources[i].consumed_bytes = limit - source.resources[i].address;
    let parent_radix = if node == source.capabilities.capability_space {
        source.capabilities.root_radix
    } else {
        BACKING_RADIX
    };
    Ok(child(node, parent_radix, index))
}

impl Builder<'_> {
    fn object(
        &mut self,
        kind: CapabilityType,
        size: KernelObjectKind,
        bits: usize,
        node: Word,
        radix: usize,
        index: usize,
    ) -> Result<Word, CapabilityError> {
        if self.next_backing >= 1 << BACKING_RADIX {
            return invalid();
        }
        let unit_bits =
            kernel_object::memory_size_bits(size, bits).ok_or(CapabilityError::InvalidArgument)?;
        // A whole 16-KiB unit safely accommodates the supported PCB layouts.
        let unit_bits = if matches!(kind, CapabilityType::ProcessControlBlock) {
            unit_bits.max(14)
        } else {
            unit_bits
        };
        let unit = reserve(self.source, unit_bits, self.backing, self.next_backing)?;
        self.next_backing += 1;
        arch::generic::convert(unit, kind, bits, 1, node, index)?;
        Ok(child(node, radix, index))
    }
    fn ensure_tables(
        &mut self,
        address_space: Word,
        address: usize,
    ) -> Result<(), CapabilityError> {
        loop {
            let depth = arch::address_space::get_unset_depth(address_space, address, 12)?;
            if depth == 0 {
                return Ok(());
            }
            if depth > 3 || self.next_table >= 1 << TABLE_RADIX {
                return invalid();
            }
            let index = self.next_table;
            self.next_table += 1;
            let table = self.object(
                CapabilityType::PageTable,
                KernelObjectKind::PageTable,
                depth,
                self.tables,
                TABLE_RADIX,
                index,
            )?;
            arch::address_space::map(address_space, table, address, Attribute::ALL)?;
        }
    }
    fn mapped_page(
        &mut self,
        address_space: Word,
        address: usize,
        fill: impl FnOnce(&mut dyn PageWriter),
        writer: &mut impl PageWriter,
    ) -> Result<Word, CapabilityError> {
        if self.next_frame >= 1 << FRAME_RADIX {
            return invalid();
        }
        let index = self.next_frame;
        self.next_frame += 1;
        let frame = self.object(
            CapabilityType::Frame,
            KernelObjectKind::Frame,
            12,
            self.frames,
            FRAME_RADIX,
            index,
        )?;
        let own_as = self.source.capabilities.address_space;
        self.ensure_tables(own_as, TEMP)?;
        arch::address_space::map(own_as, frame, TEMP, Attribute::READ | Attribute::WRITE)?;
        writer.zero(TEMP);
        fill(writer);
        arch::address_space::unmap(own_as, frame, TEMP)?;
        self.ensure_tables(address_space, address)?;
        arch::address_space::map(address_space, frame, address, Attribute::ALL)?;
        Ok(frame)
    }
    fn input(
        &mut self,
        address_space: Word,
        address: usize,
        data: &[u8],
        writer: &mut impl PageWriter,
    ) -> Result<MappedInput, CapabilityError> {
        if data.is_empty() {
            return Ok(MappedInput::default());
        }
        for (page, bytes) in data.chunks(PAGE).enumerate() {
            self.mapped_page(
                address_space,
                address + page * PAGE,
                |writer| writer.copy(TEMP, bytes),
                writer,
            )?;
        }
        Ok(MappedInput {
            address,
            bytes: data.len(),
        })
    }
}

/// Construct a suspended Alpha. On failure, all successful reservations remain
/// accounted/quarantined; never retry by restoring old consumed watermarks.
/// MOVE is used for Generic authority because A9N deliberately forbids COPY.
pub fn prepare(
    source: &mut AlphaEnvironment,
    slots: LaunchSlots,
    input: Inputs<'_>,
    writer: &mut impl PageWriter,
) -> Result<PreparedAlpha, CapabilityError> {
    source
        .validate()
        .map_err(|_| CapabilityError::InvalidArgument)?;
    let slot_indices = [
        slots.root,
        slots.root_unit,
        slots.backing_unit,
        slots.control,
    ];
    for (i, &index) in slot_indices.iter().enumerate() {
        if index < 128 || index >= 1 << ROOT_RADIX || slot_indices[..i].contains(&index) {
            return invalid();
        }
    }
    if source.capabilities.root_radix != ROOT_RADIX
        || input.archive.len() > MAX_INPUT
        || input.configuration.len() > 65536
    {
        return invalid();
    }
    let image = elf_loader::parse_elf64(input.image)?;
    let symbol = |name: &[u8]| {
        elf_loader::symbol_value(input.image, name)?.ok_or(CapabilityError::InvalidArgument)
    };
    let environment_va = symbol(b"__alpha_environment")?;
    let environment_end = symbol(b"__alpha_environment_end")?;
    let ipc_va = image
        .ipc_buffer_start
        .ok_or(CapabilityError::InvalidArgument)?;
    let stack = symbol(b"__alpha_stack_end")?;
    let mut begin = usize::MAX;
    let mut end = 0;
    for segment in &image.segments[..image.segment_count] {
        if segment.memory_size == 0 {
            continue;
        }
        begin = begin.min(segment.virtual_address & !(PAGE - 1));
        end = end.max(align(
            segment
                .virtual_address
                .checked_add(segment.memory_size)
                .ok_or(CapabilityError::InvalidArgument)?,
            PAGE,
        )?);
    }
    if begin == 0
        || begin == usize::MAX
        || end <= begin
        || end > MAX_IMAGE
        || image.entry_point < begin
        || image.entry_point >= end
        || environment_va & (PAGE - 1) != 0
        || ipc_va & (PAGE - 1) != 0
        || environment_va < begin
        || environment_end > end
        || environment_end
            .checked_sub(environment_va)
            .is_none_or(|bytes| bytes < core::mem::size_of::<AlphaEnvironment>())
        || ipc_va < begin
        || ipc_va.checked_add(PAGE).is_none_or(|limit| limit > end)
        || stack > end
        || stack < begin + 32
        || (begin..end).contains(&TEMP)
    {
        return invalid();
    }

    let own_root = source.capabilities.capability_space;
    let root_unit = reserve(
        source,
        kernel_object::node_memory_size_bits(ROOT_RADIX),
        own_root,
        slots.root_unit,
    )?;
    arch::generic::convert(
        root_unit,
        CapabilityType::Node,
        ROOT_RADIX,
        1,
        own_root,
        slots.root,
    )?;
    let root = slot(ROOT_RADIX, slots.root);
    arch::node::copy(root, 3, root)?;
    let backing_unit = reserve(
        source,
        kernel_object::node_memory_size_bits(BACKING_RADIX),
        own_root,
        slots.backing_unit,
    )?;
    arch::generic::convert(
        backing_unit,
        CapabilityType::Node,
        BACKING_RADIX,
        1,
        root,
        11,
    )?;
    let backing = child(root, ROOT_RADIX, 11);
    arch::node::movec(backing, 0, root_unit)?;
    arch::node::movec(backing, 1, backing_unit)?;
    let mut builder = Builder {
        source,
        backing,
        next_backing: 2,
        tables: 0,
        next_table: 0,
        frames: 0,
        next_frame: 0,
    };
    let pcb = builder.object(
        CapabilityType::ProcessControlBlock,
        KernelObjectKind::ProcessControlBlock,
        14,
        root,
        ROOT_RADIX,
        1,
    )?;
    let address_space = builder.object(
        CapabilityType::AddressSpace,
        KernelObjectKind::AddressSpace,
        0,
        root,
        ROOT_RADIX,
        2,
    )?;
    builder.tables = builder.object(
        CapabilityType::Node,
        KernelObjectKind::Node,
        TABLE_RADIX,
        root,
        ROOT_RADIX,
        4,
    )?;
    builder.frames = builder.object(
        CapabilityType::Node,
        KernelObjectKind::Node,
        FRAME_RADIX,
        root,
        ROOT_RADIX,
        5,
    )?;
    let resources = builder.object(
        CapabilityType::Node,
        KernelObjectKind::Node,
        7,
        root,
        ROOT_RADIX,
        7,
    )?;
    let imports = if builder.source.import_count != 0 {
        builder.object(
            CapabilityType::Node,
            KernelObjectKind::Node,
            4,
            root,
            ROOT_RADIX,
            10,
        )?
    } else {
        0
    };
    let mut ipc_frame = 0;
    for va in (begin..end).step_by(PAGE) {
        let frame = builder.mapped_page(
            address_space,
            va,
            |writer| {
                for segment in &image.segments[..image.segment_count] {
                    let start = va.max(segment.virtual_address);
                    let limit = (va + PAGE).min(segment.virtual_address + segment.file_size);
                    if start < limit {
                        let offset = segment.offset + start - segment.virtual_address;
                        writer.copy(
                            TEMP + start - va,
                            &input.image[offset..offset + limit - start],
                        );
                    }
                }
                if va == ipc_va {
                    let offset = nun::TLS_BASE_OFFSET as usize * nun::BYTE_BITS;
                    writer.copy(TEMP + offset, &ipc_va.to_ne_bytes());
                }
            },
            writer,
        )?;
        if va == ipc_va {
            ipc_frame = frame;
        }
    }
    arch::node::copy(root, 6, ipc_frame)?;
    let archive = builder.input(address_space, ARCHIVE, input.archive, writer)?;
    let configuration = builder.input(address_space, CONFIGURATION, input.configuration, writer)?;
    let mut target = *builder.source;
    target.capabilities = SelfCapabilities {
        capability_space: slot(ROOT_RADIX, 3),
        root_radix: ROOT_RADIX,
        process_control_block: slot(ROOT_RADIX, 1),
        address_space: slot(ROOT_RADIX, 2),
        page_tables: slot(ROOT_RADIX, 4),
        frames: slot(ROOT_RADIX, 5),
        ipc_buffer: slot(ROOT_RADIX, 6),
        interrupt_region: 0,
        io_ports: 0,
    };
    target.parent_control = 0;
    target.boot_archive = archive;
    target.boot_configuration = configuration;
    target.retired_launcher = RetiredLauncher::default();
    if input.retire_launcher {
        let original = builder.source.capabilities;
        for (index, capability) in [
            original.process_control_block,
            original.capability_space,
            original.address_space,
            original.frames,
            original.page_tables,
            original.ipc_buffer,
        ]
        .into_iter()
        .enumerate()
        {
            if capability != 0 {
                arch::node::copy(root, 16 + index, capability)?;
            }
        }
        target.retired_launcher = RetiredLauncher {
            process: slot(ROOT_RADIX, 16),
            capability_space: slot(ROOT_RADIX, 17),
            address_space: slot(ROOT_RADIX, 18),
            frames: slot(ROOT_RADIX, 19),
            page_tables: slot(ROOT_RADIX, 20),
            ipc_buffer: slot(ROOT_RADIX, 21),
        };
    }
    for (original, destination) in [
        (builder.source.capabilities.interrupt_region, 8),
        (builder.source.capabilities.io_ports, 9),
    ] {
        if original == 0 {
            continue;
        }
        arch::node::movec(root, destination, original)?;
        if destination == 8 {
            target.capabilities.interrupt_region = slot(ROOT_RADIX, destination);
        } else {
            target.capabilities.io_ports = slot(ROOT_RADIX, destination);
        }
    }
    for (i, resource) in target.resources[..target.resource_count]
        .iter_mut()
        .enumerate()
    {
        arch::node::movec(resources, i, resource.capability)?;
        resource.capability = child(slot(ROOT_RADIX, 7), 7, i);
    }
    for (i, import) in target.imported_services[..target.import_count]
        .iter_mut()
        .enumerate()
    {
        // No receiver or identity-changing authority crosses the domain boundary.
        arch::node::mint(imports, i, import.endpoint, 0b110)?;
        import.endpoint = child(slot(ROOT_RADIX, 10), 4, i);
    }
    target
        .validate()
        .map_err(|_| CapabilityError::InvalidArgument)?;
    // Alias only already-owned header frames. No allocation can change the final
    // reservation watermarks after the resources have moved.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&target as *const AlphaEnvironment).cast::<u8>(),
            core::mem::size_of::<AlphaEnvironment>(),
        )
    };
    for (page, data) in bytes.chunks(PAGE).enumerate() {
        let index = (environment_va - begin) / PAGE + page;
        let frame = child(builder.frames, FRAME_RADIX, index);
        arch::address_space::map(
            builder.source.capabilities.address_space,
            frame,
            TEMP,
            Attribute::READ | Attribute::WRITE,
        )?;
        writer.copy(TEMP, data);
        arch::address_space::unmap(builder.source.capabilities.address_space, frame, TEMP)?;
    }
    let tls = if cfg!(target_arch = "x86_64") {
        ipc_va + nun::TLS_BASE_OFFSET as usize * nun::BYTE_BITS
    } else {
        ipc_va
    };
    let config =
        ConfigurationInfo::new(true, true, true, false, false, true, true, true, true, true);
    arch::process_control_block::configure(
        pcb,
        config,
        address_space,
        root,
        ipc_frame,
        0,
        0,
        image.entry_point,
        stack - 32,
        tls,
        31,
        0,
    )?;
    arch::node::copy(own_root, slots.control, pcb)?;
    Ok(PreparedAlpha {
        control: slot(ROOT_RADIX, slots.control),
        environment: target,
    })
}
