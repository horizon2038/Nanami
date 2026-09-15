use super::{
    align_down_usize, install_freebsd_exec_tls, log_exec_image_entry, log_exec_stack,
    map_request_error, process_diagnostics_enabled, read_current_elf_metadata,
    register_linux_stack_mappings, write_exec_registers, CurrentElfMetadata, ExecStringSnapshot,
    OsPersonality, Runtime, Word, ALTER_LAUNCH_MAX_ARGS, ALTER_LAUNCH_MAX_ENVS, AT_BASE, AT_CLKTCK,
    AT_EGID, AT_ENTRY, AT_EUID, AT_EXECFN, AT_FLAGS, AT_GID, AT_HWCAP, AT_NULL, AT_PAGESZ, AT_PHDR,
    AT_PHENT, AT_PHNUM, AT_PLATFORM, AT_RANDOM, AT_SECURE, AT_UID, EINVAL, EIO, ENOMEM, ESRCH,
    LINUX_EXEC_PATH_MAX, LINUX_EXEC_STACK_BYTES, LINUX_PAGE_SIZE, LINUX_STACK_TOP, PLATFORM,
};

pub(super) fn rewrite_linux_stack(
    runtime: &mut Runtime,
    pid: Word,
    exec_path: &[u8; LINUX_EXEC_PATH_MAX],
    exec_path_len: usize,
    snapshot: &ExecStringSnapshot,
    preferred_image_base: Word,
    loaded_elf: Option<CurrentElfMetadata>,
    interpreter: Option<&crate::common::dynamic::Interpreter>,
) -> Result<(), i32> {
    let pcb = runtime
        .managed_process(pid)
        .map(|process| process.pcb)
        .ok_or(ESRCH)?;
    let personality = runtime
        .managed_process(pid)
        .map(|process| process.personality)
        .ok_or(ESRCH)?;
    let elf = match loaded_elf {
        Some(elf) => elf,
        None => read_current_elf_metadata(runtime, pid, preferred_image_base)?,
    };
    if runtime.exec_stack_buffer == 0
        || runtime.exec_stack_buffer_size < LINUX_EXEC_STACK_BYTES as Word
    {
        let (stack_buffer, mapped) =
            libnanami::request_heap(LINUX_EXEC_STACK_BYTES as Word).map_err(map_request_error)?;
        if mapped < LINUX_EXEC_STACK_BYTES as Word {
            return Err(ENOMEM);
        }
        runtime.exec_stack_buffer = stack_buffer;
        runtime.exec_stack_buffer_size = mapped;
    }
    let stack_buffer = runtime.exec_stack_buffer;
    unsafe {
        ::core::ptr::write_bytes(stack_buffer as *mut u8, 0, LINUX_EXEC_STACK_BYTES);
    }

    let mut argc = snapshot.argc;
    let stack_base = LINUX_STACK_TOP - LINUX_EXEC_STACK_BYTES as Word;
    let mut cursor = LINUX_EXEC_STACK_BYTES;
    let mut argv_guest = [0usize; ALTER_LAUNCH_MAX_ARGS];
    let mut env_guest = [0usize; ALTER_LAUNCH_MAX_ENVS];

    let mut i = 0usize;
    while i < snapshot.envc {
        cursor = push_snapshot_string_to_stack(
            snapshot,
            stack_buffer,
            cursor,
            snapshot.env_offsets[i],
            snapshot.env_lens[i],
            &mut env_guest[i],
            stack_base,
        )?;
        i += 1;
    }

    if argc == 0 {
        cursor = push_bytes_to_stack(stack_buffer, cursor, b"", &mut argv_guest[0], stack_base)?;
        argc = 1;
    } else {
        i = 0;
        while i < argc {
            cursor = push_snapshot_string_to_stack(
                snapshot,
                stack_buffer,
                cursor,
                snapshot.argv_offsets[i],
                snapshot.argv_lens[i],
                &mut argv_guest[i],
                stack_base,
            )?;
            i += 1;
        }
    }

    let mut execfn_guest = 0usize;
    cursor = push_bytes_to_stack(
        stack_buffer,
        cursor,
        &exec_path[..exec_path_len],
        &mut execfn_guest,
        stack_base,
    )?;
    let random_bytes = [
        0x5d, 0x8f, 0x2a, 0x91, 0x48, 0x3c, 0x11, 0x67, 0x02, 0xee, 0x70, 0x31, 0xaa, 0x4b, 0xd2,
        0x09,
    ];
    let mut random_guest = 0usize;
    cursor = push_bytes_to_stack(
        stack_buffer,
        cursor,
        &random_bytes,
        &mut random_guest,
        stack_base,
    )?;
    let mut platform_guest = 0usize;
    cursor = push_bytes_to_stack(
        stack_buffer,
        cursor,
        PLATFORM,
        &mut platform_guest,
        stack_base,
    )?;

    let aux_pairs = 18usize;
    let word_count = 1 + argc + 1 + snapshot.envc + 1 + aux_pairs * 2;
    let table_bytes = word_count * ::core::mem::size_of::<Word>();
    let aligned_cursor = align_down_usize(cursor, 16);
    if aligned_cursor < table_bytes + 16 {
        return Err(ENOMEM);
    }
    let sp_offset = if personality == OsPersonality::FreeBsd {
        align_down_usize(aligned_cursor - table_bytes, 16)
            .checked_sub(8)
            .ok_or(ENOMEM)?
    } else {
        align_down_usize(aligned_cursor - table_bytes, 16)
    };
    let mut out = sp_offset;
    write_stack_word(stack_buffer, out, argc as Word);
    out += 8;
    i = 0;
    while i < argc {
        write_stack_word(stack_buffer, out, argv_guest[i] as Word);
        out += 8;
        i += 1;
    }
    write_stack_word(stack_buffer, out, 0);
    out += 8;
    i = 0;
    while i < snapshot.envc {
        write_stack_word(stack_buffer, out, env_guest[i] as Word);
        out += 8;
        i += 1;
    }
    write_stack_word(stack_buffer, out, 0);
    out += 8;

    out = write_aux(stack_buffer, out, AT_PHDR, elf.program_header_vaddr);
    out = write_aux(stack_buffer, out, AT_PHENT, elf.program_header_entry_size);
    out = write_aux(stack_buffer, out, AT_PHNUM, elf.program_header_count);
    out = write_aux(stack_buffer, out, AT_PAGESZ, LINUX_PAGE_SIZE);
    out = write_aux(
        stack_buffer,
        out,
        AT_BASE,
        interpreter.map_or(0, |image| image.load_bias),
    );
    out = write_aux(stack_buffer, out, AT_FLAGS, 0);
    out = write_aux(stack_buffer, out, AT_ENTRY, elf.entry_point);
    out = write_aux(stack_buffer, out, AT_HWCAP, 0);
    out = write_aux(stack_buffer, out, AT_CLKTCK, 100);
    out = write_aux(stack_buffer, out, AT_UID, 0);
    out = write_aux(stack_buffer, out, AT_EUID, 0);
    out = write_aux(stack_buffer, out, AT_GID, 0);
    out = write_aux(stack_buffer, out, AT_EGID, 0);
    out = write_aux(stack_buffer, out, AT_SECURE, 0);
    out = write_aux(stack_buffer, out, AT_RANDOM, random_guest as Word);
    out = write_aux(stack_buffer, out, AT_EXECFN, execfn_guest as Word);
    out = write_aux(stack_buffer, out, AT_PLATFORM, platform_guest as Word);
    let _ = write_aux(stack_buffer, out, AT_NULL, 0);

    let guest_sp = stack_base + sp_offset as Word;
    libnanami::request_process_memory_write(
        pid,
        stack_base,
        stack_buffer,
        LINUX_EXEC_STACK_BYTES as Word,
    )
    .map_err(map_request_error)?;
    if !register_linux_stack_mappings(runtime, pid) {
        return Err(ENOMEM);
    }
    if process_diagnostics_enabled(runtime, pid) {
        log_exec_image_entry(runtime, pid, elf.entry_point);
        log_exec_stack(runtime, pid, guest_sp);
    }
    if personality == OsPersonality::FreeBsd {
        let fs_base = install_freebsd_exec_tls(runtime, pid, &elf)?;
        write_exec_registers(pcb, elf.entry_point, guest_sp, fs_base, 0, guest_sp, 0, 0)
            .map_err(|_| EIO)?;
        if !runtime.set_fs_base(pid, fs_base) {
            return Err(ESRCH);
        }
    } else {
        let entry = interpreter.map_or(elf.entry_point, |image| image.entry);
        write_exec_registers(pcb, entry, guest_sp, 0, 0, 0, 0, 0).map_err(|_| EIO)?;
        if !runtime.set_fs_base(pid, 0) {
            return Err(ESRCH);
        }
    }
    Ok(())
}

pub(super) fn push_snapshot_string_to_stack(
    snapshot: &ExecStringSnapshot,
    stack_buffer: Word,
    cursor: usize,
    snapshot_offset: usize,
    len: usize,
    guest_address: &mut usize,
    stack_base: Word,
) -> Result<usize, i32> {
    if snapshot_offset
        .checked_add(len)
        .filter(|end| *end <= snapshot.used)
        .is_none()
    {
        return Err(EINVAL);
    }
    let bytes = unsafe {
        ::core::slice::from_raw_parts(
            (snapshot.buffer as usize + snapshot_offset) as *const u8,
            len,
        )
    };
    push_bytes_to_stack(stack_buffer, cursor, bytes, guest_address, stack_base)
}

pub(super) fn push_bytes_to_stack(
    stack_buffer: Word,
    cursor: usize,
    bytes: &[u8],
    guest_address: &mut usize,
    stack_base: Word,
) -> Result<usize, i32> {
    let mut next = cursor.checked_sub(bytes.len() + 1).ok_or(ENOMEM)?;
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            (stack_buffer as usize + next) as *mut u8,
            bytes.len(),
        );
        ::core::ptr::write((stack_buffer as usize + next + bytes.len()) as *mut u8, 0);
    }
    *guest_address = stack_base as usize + next;
    next = align_down_usize(next, 8);
    Ok(next)
}

pub(super) fn write_stack_word(stack_buffer: Word, offset: usize, value: Word) {
    unsafe {
        ::core::ptr::write_unaligned((stack_buffer as usize + offset) as *mut Word, value);
    }
}

pub(super) fn write_aux(stack_buffer: Word, offset: usize, key: Word, value: Word) -> usize {
    write_stack_word(stack_buffer, offset, key);
    write_stack_word(stack_buffer, offset + 8, value);
    offset + 16
}
