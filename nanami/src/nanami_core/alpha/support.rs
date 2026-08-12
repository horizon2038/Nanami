use super::*;

pub(super) fn validate_process_device_slot(slot: usize) -> Result<(), CapabilityError> {
    if slot < PROCESS_DEVICE_SLOT_MIN || slot > PROCESS_DEVICE_SLOT_MAX {
        return Err(CapabilityError::InvalidArgument);
    }
    Ok(())
}

pub(super) fn validate_process_observer(
    caller_pid: usize,
    target_pid: usize,
    reaper_pid: usize,
) -> Result<(), CapabilityError> {
    if caller_pid == target_pid || reaper_pid == 0 || caller_pid == reaper_pid {
        return Ok(());
    }
    Err(CapabilityError::PermissionDenied)
}

pub(super) fn process_frame_directory_descriptor(
    process_root: CapabilityDescriptor,
) -> CapabilityDescriptor {
    make_child_slot_descriptor(process_root, PROCESS_ROOT_RADIX, PROCESS_SLOT_FRAME_NODE)
}

pub(super) fn process_frame_chunk_descriptor(
    process_root: CapabilityDescriptor,
    chunk_index: usize,
) -> CapabilityDescriptor {
    make_child_slot_descriptor(
        process_frame_directory_descriptor(process_root),
        PROCESS_FRAME_DIRECTORY_RADIX,
        chunk_index,
    )
}

pub(super) fn process_frame_descriptor(
    process_root: CapabilityDescriptor,
    global_slot: usize,
) -> CapabilityDescriptor {
    make_child_slot_descriptor(
        process_frame_chunk_descriptor(process_root, global_slot / PROCESS_FRAME_CHUNK_PAGES),
        PROCESS_FRAME_NODE_RADIX,
        global_slot % PROCESS_FRAME_CHUNK_PAGES,
    )
}

pub(super) fn fill_lazy_page(
    kind: ProcessLazyMappingKind,
    page_va: usize,
    temp_va: usize,
) -> Result<(), CapabilityError> {
    unsafe {
        ptr::write_bytes(temp_va as *mut u8, 0, PAGE_SIZE);
    }

    let ProcessLazyMappingKind::Image { image, elf } = kind else {
        return Ok(());
    };

    let page_end = page_va
        .checked_add(PAGE_SIZE)
        .ok_or(CapabilityError::InvalidArgument)?;
    let mut i = 0usize;
    while i < elf.segment_count {
        let seg = elf.segments[i];
        let seg_start = seg.virtual_address;
        let seg_mem_end = seg
            .virtual_address
            .checked_add(seg.memory_size)
            .ok_or(CapabilityError::InvalidArgument)?;
        if seg.memory_size == 0 || page_va >= seg_mem_end || page_end <= seg_start {
            i += 1;
            continue;
        }

        let file_end = seg
            .virtual_address
            .checked_add(seg.file_size)
            .ok_or(CapabilityError::InvalidArgument)?;
        let copy_start = max_usize(page_va, seg_start);
        let copy_end = min_usize(page_end, file_end);
        if copy_start < copy_end {
            let src_offset = seg
                .offset
                .checked_add(copy_start - seg.virtual_address)
                .ok_or(CapabilityError::InvalidArgument)?;
            let len = copy_end - copy_start;
            if src_offset
                .checked_add(len)
                .filter(|end| *end <= image.len())
                .is_none()
            {
                return Err(CapabilityError::InvalidArgument);
            }
            let dst = temp_va + (copy_start - page_va);
            unsafe {
                ptr::copy_nonoverlapping(image.as_ptr().add(src_offset), dst as *mut u8, len);
            }
        }

        i += 1;
    }

    if let Some(ipc_buffer_va) = elf.ipc_buffer_start {
        let tls_slot_va = ipc_buffer_va + (nun::TLS_BASE_OFFSET as usize) * nun::BYTE_BITS;
        if tls_slot_va >= page_va && tls_slot_va + core::mem::size_of::<Word>() <= page_end {
            let dst = temp_va + (tls_slot_va - page_va);
            unsafe {
                ptr::write(dst as *mut Word, ipc_buffer_va as Word);
            }
        }
    }

    Ok(())
}

pub(super) fn max_usize(a: usize, b: usize) -> usize {
    if a > b {
        a
    } else {
        b
    }
}

pub(super) fn min_usize(a: usize, b: usize) -> usize {
    if a < b {
        a
    } else {
        b
    }
}

pub(super) fn select_irq_notification_alias_slot(
    irq_number: usize,
    notification_slot: usize,
    interrupt_slot: usize,
) -> usize {
    let mut slot =
        PROCESS_IRQ_NOTIFICATION_ALIAS_MIN + (irq_number % PROCESS_IRQ_NOTIFICATION_ALIAS_COUNT);
    if slot == notification_slot || slot == interrupt_slot {
        slot = PROCESS_IRQ_NOTIFICATION_ALIAS_MIN
            + ((irq_number + 1) % PROCESS_IRQ_NOTIFICATION_ALIAS_COUNT);
    }
    slot
}

pub(super) fn irq_notification_identifier(irq_number: usize) -> Result<usize, CapabilityError> {
    let bits = usize::BITS as usize;
    if irq_number >= bits {
        return Err(CapabilityError::InvalidArgument);
    }
    Ok(1usize << irq_number)
}

pub(super) fn map_request_result_to_status(result: Result<(), CapabilityError>) -> (usize, usize) {
    match result {
        Ok(()) => (OS_RESPONSE_OK, 0),
        Err(CapabilityError::InvalidArgument) => (OS_RESPONSE_INVALID_ARGUMENT, 0),
        Err(CapabilityError::PermissionDenied) => (OS_RESPONSE_PERMISSION_DENIED, 0),
        Err(CapabilityError::InvalidDescriptor) => (OS_RESPONSE_INVALID_DESCRIPTOR, 0),
        Err(CapabilityError::IllegalOperation) => (OS_RESPONSE_ILLEGAL_OPERATION, 0),
        Err(CapabilityError::InvalidDepth) => (OS_RESPONSE_INVALID_ARGUMENT, 0),
        Err(CapabilityError::Fatal) => (OS_RESPONSE_FATAL, 0),
        Err(CapabilityError::DebugUnimplemented) => (OS_RESPONSE_FATAL, 0),
    }
}

pub(super) fn response_from_status_result(
    result: Result<(), CapabilityError>,
) -> (usize, usize, usize) {
    let (status, detail0) = map_request_result_to_status(result);
    (status, detail0, 0)
}

pub(super) fn response_from_detail_result(
    result: Result<usize, CapabilityError>,
) -> (usize, usize, usize) {
    match result {
        Ok(detail0) => (OS_RESPONSE_OK, detail0, 0),
        Err(error) => response_from_status_result(Err(error)),
    }
}

pub(super) fn response_from_details_result(
    result: Result<(usize, usize), CapabilityError>,
) -> (usize, usize, usize) {
    match result {
        Ok((detail0, detail1)) => (OS_RESPONSE_OK, detail0, detail1),
        Err(error) => response_from_status_result(Err(error)),
    }
}

pub(super) fn io_port_mint(
    root_io_port: CapabilityDescriptor,
    range_min: Word,
    range_max: Word,
    destination_node: CapabilityDescriptor,
    destination_index: Word,
) -> Result<(), CapabilityError> {
    let mut a0 = root_io_port;
    let mut a1 = nun::capability_call::io_port::OperationType::Mint as Word;
    let a2 = range_min;
    let a3 = range_max;
    let a4 = destination_node as Word;
    let a5 = destination_index;

    unsafe {
        asm!(
            "syscall",
            in("rax") KernelCallType::CapabilityCall as Sword,
            inout("rdi") a0 => a0,
            inout("rsi") a1 => a1,
            in("rdx") a2,
            in("r8") a3,
            in("r9") a4,
            in("r10") a5,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }

    convert_capability_result(a0, a1)
}

pub(super) extern "C" fn run_on_relocated_stack(alpha_ptr: *mut Alpha) -> ! {
    let alpha = unsafe { &mut *alpha_ptr };
    info!("[stack] switched to runtime stack");
    alpha.run_event_loop();
}

pub(super) unsafe fn jump_to_relocated_stack(alpha_ptr: *mut Alpha, new_sp: usize) -> ! {
    asm!(
        "mov rdi, {alpha}",
        "mov rsp, {stack}",
        // We enter by `jmp` (not `call`), so synthesize a call frame to satisfy SysV ABI.
        // On function entry, rsp must be 8 mod 16.
        "and rsp, -16",
        "sub rsp, 8",
        "mov rbp, rsp",
        "jmp {entry}",
        alpha = in(reg) alpha_ptr,
        stack = in(reg) new_sp,
        entry = in(reg) run_on_relocated_stack as extern "C" fn(*mut Alpha) -> !,
        options(noreturn)
    )
}

#[inline(always)]
pub(super) fn align_down(value: usize, align: usize) -> usize {
    value & !(align - 1)
}

#[inline(always)]
pub(super) fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

pub(super) fn align_up_checked(value: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    value.checked_add(align - 1).map(|v| v & !(align - 1))
}

pub(super) fn create_alpha_os_port(
    root_descriptor: CapabilityDescriptor,
    _root_radix: usize,
    bootstrap_generic: CapabilityDescriptor,
) -> Result<CapabilityDescriptor, CapabilityError> {
    info!(
        "convert IpcPort into root slot {:>3}",
        ORIGINAL_OS_PORT_SLOT
    );

    arch::generic::convert(
        bootstrap_generic,
        nun::CapabilityType::IpcPort,
        0,
        1,
        root_descriptor,
        ORIGINAL_OS_PORT_SLOT,
    )?;

    let descriptor =
        crate::nanami_utils::descriptor::make_root_slot_descriptor(12, ORIGINAL_OS_PORT_SLOT);
    let _ = arch::ipc_port::identify(descriptor, 0);
    info!(
        "slot {:>3} descriptor={:#018x}",
        ORIGINAL_OS_PORT_SLOT, descriptor
    );
    Ok(descriptor)
}

pub(super) fn make_generic_descriptor(
    root_radix: usize,
    generic_index: usize,
) -> CapabilityDescriptor {
    let generic_node = make_root_slot_descriptor(root_radix, InitSlotOffset::GenericNode as usize);
    make_child_slot_descriptor(generic_node, GENERIC_NODE_RADIX, generic_index)
}

pub(super) fn extract_initial_framebuffer_information(
    init_info: &InitInfo,
) -> Option<InitialFramebufferInformation> {
    let mut raw = [0usize; 13];
    raw.copy_from_slice(&init_info.arch_info[1..14]);
    let fb = FramebufferInfo::deserialize(&raw);
    if fb.address == 0 || fb.width == 0 || fb.height == 0 || fb.bits_per_pixel == 0 {
        return None;
    }

    let bytes_per_pixel = (fb.bits_per_pixel as usize).saturating_div(8);
    if bytes_per_pixel == 0 {
        return None;
    }

    let stride_raw = fb.stride as usize;
    let stride_bytes = if stride_raw >= fb.width as usize * bytes_per_pixel {
        stride_raw
    } else {
        stride_raw.saturating_mul(bytes_per_pixel)
    };
    let size_bytes = stride_bytes.saturating_mul(fb.height as usize);
    if size_bytes == 0 {
        return None;
    }

    Some(InitialFramebufferInformation {
        display_id: 0,
        address: fb.address,
        size_bytes,
        width: fb.width as usize,
        height: fb.height as usize,
        stride: fb.stride as usize,
        bits_per_pixel: fb.bits_per_pixel as usize,
        red_position: fb.red.position as usize,
        red_size: fb.red.size as usize,
        green_position: fb.green.position as usize,
        green_size: fb.green.size as usize,
        blue_position: fb.blue.position as usize,
        blue_size: fb.blue.size as usize,
    })
}

pub(super) fn pack_framebuffer_color_information(
    red_position: usize,
    red_size: usize,
    green_position: usize,
    green_size: usize,
    blue_position: usize,
    blue_size: usize,
) -> usize {
    (red_position & 0x1f)
        | ((red_size & 0x1f) << 5)
        | ((green_position & 0x1f) << 10)
        | ((green_size & 0x1f) << 15)
        | ((blue_position & 0x1f) << 20)
        | ((blue_size & 0x1f) << 25)
}

pub(super) fn process_priority_for_image(image_name: &str) -> Word {
    match strip_elf_suffix(basename(image_name)) {
        // Timer must preempt clients promptly; animation and network timeouts depend on it.
        "timer-server" => PROCESS_PRIORITY_TIMER_SERVER,
        // Input pipeline must stay above the compositor and every input consumer.
        "input-server" | "ps2-server" => PROCESS_PRIORITY_INPUT_SERVER,
        // GUI servers are above GUI clients, but below timer/input IRQ-facing services.
        "fb-server" | "honoka" => PROCESS_PRIORITY_GUI_SERVER,
        // Background servers stay above clients, but below the GUI critical path.
        "block-device-server" | "virtio-blk-server" => PROCESS_PRIORITY_BACKGROUND_SERVER + 2,
        "virtio-net" => PROCESS_PRIORITY_BACKGROUND_SERVER + 2,
        "ext2-server" => PROCESS_PRIORITY_BACKGROUND_SERVER + 1,
        "net-server" => PROCESS_PRIORITY_BACKGROUND_SERVER + 1,
        "rtc-server" => PROCESS_PRIORITY_BACKGROUND_SERVER + 1,
        "http-server" => PROCESS_PRIORITY_BACKGROUND_SERVER,
        "honoka-client" | "eg-test" | "image-viewer" | "performance-monitor" => {
            PROCESS_PRIORITY_INTERACTIVE_CLIENT
        }
        "shell" => PROCESS_PRIORITY_CLIENT,
        "cpp-hello" | "rust-hello" => PROCESS_PRIORITY_BACKGROUND_CLIENT,
        _ => PROCESS_PRIORITY_LOW,
    }
}

pub(super) fn basename(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some((_, name)) => name,
        None => path,
    }
}

pub(super) fn decode_service_name_24(
    arg1: Word,
    arg2: Word,
    arg3: Word,
) -> Option<([u8; 24], usize)> {
    let mut raw = [0u8; 24];
    raw[0..8].copy_from_slice(&arg1.to_le_bytes());
    raw[8..16].copy_from_slice(&arg2.to_le_bytes());
    raw[16..24].copy_from_slice(&arg3.to_le_bytes());

    let mut len = 0usize;
    while len < raw.len() && raw[len] != 0 {
        len += 1;
    }
    if len == 0 {
        return None;
    }
    Some((raw, len))
}

pub(super) struct ControlText {
    raw: [u8; 16],
    len: usize,
}

pub(super) fn decode_control_text(arg0: Word, arg1: Word) -> ControlText {
    let mut raw = [0u8; 16];
    raw[0..8].copy_from_slice(&arg0.to_le_bytes());
    raw[8..16].copy_from_slice(&arg1.to_le_bytes());
    let mut len = 0usize;
    while len < raw.len() && raw[len] != 0 {
        len += 1;
    }
    ControlText { raw, len }
}

pub(super) fn bytes_equal(text: &ControlText, expected: &[u8]) -> bool {
    text.len == expected.len() && &text.raw[..text.len] == expected
}

pub(super) fn initramfs_entry_data(requested_name: &str) -> Option<&'static [u8]> {
    let mut found = None;
    let _ = cpio::for_each_newc_entry(INITRAMFS_IMAGE, |entry| {
        if found.is_none() && initramfs_path_matches(entry.name, requested_name) {
            found = Some(entry.data);
        }
        Ok(())
    });
    found
}

pub(super) fn parse_boot_list_line(line: &str) -> Option<BootListEntry<'_>> {
    let line = line.split('#').next().unwrap_or("");
    let mut tokens = line.split_whitespace();
    let name = tokens.next()?;
    let priority = parse_decimal_word(tokens.next()?)?;
    let image_path = tokens.next()?;
    Some(BootListEntry {
        _name: name,
        priority,
        image_path,
    })
}

pub(super) fn parse_decimal_word(text: &str) -> Option<Word> {
    if text.is_empty() {
        return None;
    }
    let mut value = 0usize;
    for byte in text.bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?;
        value = value.checked_add((byte - b'0') as usize)?;
    }
    Some(value as Word)
}

pub(super) fn initramfs_image_name_matches(entry_name: &str, requested_name: &str) -> bool {
    if initramfs_path_matches(entry_name, requested_name) {
        return true;
    }
    let entry_base = path_basename(entry_name);
    let requested_base = path_basename(requested_name);
    if initramfs_path_matches(entry_base, requested_name)
        || initramfs_path_matches(entry_name, requested_base)
        || entry_base == requested_base
    {
        return true;
    }

    let entry_stem = strip_elf_suffix(entry_base);
    let requested_stem = strip_elf_suffix(requested_base);
    entry_stem == strip_elf_suffix(requested_name)
        || strip_elf_suffix(entry_name) == requested_stem
        || entry_stem == requested_stem
}

pub(super) fn initramfs_path_matches(entry_name: &str, requested_name: &str) -> bool {
    entry_name == requested_name
        || entry_name.strip_prefix("./") == Some(requested_name)
        || requested_name.strip_prefix("./") == Some(entry_name)
}

pub(super) fn initramfs_image_is_explicit_only(name: &str) -> bool {
    path_basename(name).starts_with('_')
}

pub(super) fn initramfs_image_is_auto_spawn_candidate(name: &str) -> bool {
    name.starts_with("./bin/") || name.starts_with("bin/")
}

pub(super) fn strip_elf_suffix(name: &str) -> &str {
    name.strip_suffix(".elf").unwrap_or(name)
}

pub(super) fn path_basename(path: &str) -> &str {
    let bytes = path.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'/' {
            start = i + 1;
        }
        i += 1;
    }
    &path[start..]
}
