use super::*;

pub(super) fn prepare_initial_stack(
    runtime: &mut Runtime,
    pid: Word,
    pcb: Word,
    info: &LaunchInfo,
    personality: OsPersonality,
    diagnostics: bool,
    loaded_metadata: Option<&ElfMetadata>,
    interpreter: Option<&crate::common::dynamic::Interpreter>,
) -> Result<Word, Word> {
    let elf = loaded_metadata
        .map(target_elf_metadata_from_loaded)
        .unwrap_or_else(|| read_target_elf_metadata(runtime, pid));
    if elf.has_interpreter && interpreter.is_none() {
        return Err(nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION);
    }
    if runtime.exec_stack_buffer_size < LINUX_INITIAL_STACK_BYTES as Word {
        let (address, size) = nanami_runtime::request_heap(LINUX_INITIAL_STACK_BYTES as Word)
            .map_err(map_request_error_to_status)?;
        if size < LINUX_INITIAL_STACK_BYTES as Word {
            let _ = nanami_runtime::request_mapping_release(address, size);
            return Err(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT);
        }
        if runtime.exec_stack_buffer != 0 {
            let _ = nanami_runtime::request_mapping_release(
                runtime.exec_stack_buffer,
                runtime.exec_stack_buffer_size,
            );
        }
        runtime.exec_stack_buffer = address;
        runtime.exec_stack_buffer_size = size;
    }
    let stack_buffer = runtime.exec_stack_buffer;

    let stack_base = LINUX_STACK_TOP - LINUX_INITIAL_STACK_BYTES as Word;
    let mut string_cursor = LINUX_INITIAL_STACK_BYTES;
    let mut argv_guest = [0usize; ALTER_LAUNCH_MAX_ARGS];
    let mut env_guest = [0usize; ALTER_LAUNCH_MAX_ENVS];

    let mut i = 0usize;
    while i < info.envc {
        string_cursor = push_client_string(
            runtime,
            stack_buffer,
            string_cursor,
            info.env_offsets[i],
            info.env_lens[i],
            &mut env_guest[i],
            stack_base,
        )?;
        i += 1;
    }
    i = 0;
    while i < info.argc {
        let (offset, len) = if i == 0 {
            linux_argv0(runtime, info)?
        } else {
            (info.argv_offsets[i], info.argv_lens[i])
        };
        string_cursor = push_client_string(
            runtime,
            stack_buffer,
            string_cursor,
            offset,
            len,
            &mut argv_guest[i],
            stack_base,
        )?;
        i += 1;
    }
    let execfn_guest = argv_guest[0];

    let random_bytes = [
        0x91, 0x2a, 0x5d, 0x7c, 0x10, 0x42, 0xb3, 0xc4, 0x09, 0x88, 0x31, 0x65, 0xaa, 0xfe, 0x28,
        0x53,
    ];
    let mut random_guest = 0usize;
    string_cursor = push_raw_bytes(
        stack_buffer,
        string_cursor,
        &random_bytes,
        &mut random_guest,
        stack_base,
    )?;
    let mut platform_guest = 0usize;
    string_cursor = push_raw_bytes(
        stack_buffer,
        string_cursor,
        PLATFORM_NUL,
        &mut platform_guest,
        stack_base,
    )?;

    let aux_pairs = 18usize;
    let word_count = 1 + info.argc + 1 + info.envc + 1 + aux_pairs * 2;
    let table_bytes = word_count * ::core::mem::size_of::<Word>();
    let aligned_cursor = align_down_usize(string_cursor, 16);
    if aligned_cursor < table_bytes + 16 {
        return Err(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT);
    }
    let sp_offset = initial_stack_table_offset(aligned_cursor, table_bytes, personality)?;
    let mut out = sp_offset;
    write_stack_word(stack_buffer, out, info.argc as Word);
    out += 8;
    i = 0;
    while i < info.argc {
        write_stack_word(stack_buffer, out, argv_guest[i] as Word);
        out += 8;
        i += 1;
    }
    write_stack_word(stack_buffer, out, 0);
    out += 8;
    i = 0;
    while i < info.envc {
        write_stack_word(stack_buffer, out, env_guest[i] as Word);
        out += 8;
        i += 1;
    }
    write_stack_word(stack_buffer, out, 0);
    out += 8;

    out = write_aux(stack_buffer, out, AT_PHDR, elf.program_header_vaddr);
    out = write_aux(stack_buffer, out, AT_PHENT, elf.program_header_entry_size);
    out = write_aux(stack_buffer, out, AT_PHNUM, elf.program_header_count);
    out = write_aux(stack_buffer, out, AT_PAGESZ, 4096);
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
    out = write_aux(stack_buffer, out, AT_NULL, 0);
    // Payload/table bytes are initialized by their writers; only the gap
    // between table and aligned strings still needs zeroing before publication.
    unsafe {
        ::core::ptr::write_bytes((stack_buffer + out) as *mut u8, 0, string_cursor - out);
    }

    let guest_sp = stack_base + sp_offset as Word;
    if diagnostics {
        let local_argc = read_word_from_buffer(stack_buffer, sp_offset);
        let local_argv0 = read_word_from_buffer(stack_buffer, sp_offset + 8);
        nanami_runtime::println!(
            "[alter/{}] stack pid={} sp={:#x} argc={} argv0={:#x} local_argc={} local_argv0={:#x} phdr={:#x} phnum={}",
            personality::name(personality),
            pid,
            guest_sp,
            info.argc,
            argv_guest[0],
            local_argc,
            local_argv0,
            elf.program_header_vaddr,
            elf.program_header_count
        );
    }
    nanami_runtime::request_process_memory_write(
        pid,
        guest_sp,
        stack_buffer + sp_offset as Word,
        (LINUX_INITIAL_STACK_BYTES - sp_offset) as Word,
    )
    .map_err(map_request_error_to_status)?;
    if personality == OsPersonality::FreeBsd {
        let fs_base = install_freebsd_initial_tls(runtime, pid, &elf)
            .map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
        write_exec_registers(pcb, elf.entry_point, guest_sp, fs_base, 0, guest_sp, 0, 0)
            .map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
        let _ = runtime.set_fs_base(pid, fs_base);
    } else {
        let entry = interpreter.map_or(elf.entry_point, |image| image.entry);
        write_exec_registers(pcb, entry, guest_sp, 0, 0, 0, 0, 0)
            .map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
    }
    if diagnostics {
        let pc = read_register_value(pcb, REG_PC).unwrap_or(0);
        let rsp = read_register_value(pcb, REG_RSP).unwrap_or(0);
        let mut stack_argc = 0;
        let mut stack_argv0 = 0;
        if nanami_runtime::request_process_memory_read(pid, guest_sp, stack_buffer, 16).is_ok() {
            stack_argc = read_word_from_buffer(stack_buffer, 0);
            stack_argv0 = read_word_from_buffer(stack_buffer, 8);
        }
        nanami_runtime::println!(
            "[alter/{}] stack verify pid={} pc={:#x} rsp={:#x} argc={} argv0={:#x} personality={}",
            personality::name(personality),
            pid,
            pc,
            rsp,
            stack_argc,
            stack_argv0,
            personality::name(personality)
        );
    }
    Ok(guest_sp)
}

fn initial_stack_table_offset(
    aligned_cursor: usize,
    table_bytes: usize,
    personality: OsPersonality,
) -> Result<usize, Word> {
    if aligned_cursor < table_bytes + 16 {
        return Err(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT);
    }
    let offset = align_down_usize(aligned_cursor - table_bytes, 16);
    if personality == OsPersonality::FreeBsd {
        offset
            .checked_sub(8)
            .ok_or(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT)
    } else {
        Ok(offset)
    }
}

fn push_client_string(
    runtime: &Runtime,
    stack_buffer: Word,
    cursor: usize,
    offset: usize,
    len: usize,
    guest_address: &mut usize,
    stack_base: Word,
) -> Result<usize, Word> {
    let mut next = cursor
        .checked_sub(len + 1)
        .ok_or(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT)?;
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            (runtime.client_shm as usize + offset) as *const u8,
            (stack_buffer as usize + next) as *mut u8,
            len,
        );
        ::core::ptr::write((stack_buffer as usize + next + len) as *mut u8, 0);
    }
    *guest_address = stack_base as usize + next;
    let aligned = align_down_usize(next, 8);
    unsafe {
        ::core::ptr::write_bytes((stack_buffer + aligned) as *mut u8, 0, next - aligned);
    }
    next = aligned;
    Ok(next)
}

fn push_raw_bytes(
    stack_buffer: Word,
    cursor: usize,
    bytes: &[u8],
    guest_address: &mut usize,
    stack_base: Word,
) -> Result<usize, Word> {
    let mut next = cursor
        .checked_sub(bytes.len())
        .ok_or(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT)?;
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            (stack_buffer as usize + next) as *mut u8,
            bytes.len(),
        );
    }
    *guest_address = stack_base as usize + next;
    let aligned = align_down_usize(next, 8);
    unsafe {
        ::core::ptr::write_bytes((stack_buffer + aligned) as *mut u8, 0, next - aligned);
    }
    next = aligned;
    Ok(next)
}

fn write_stack_word(stack_buffer: Word, offset: usize, value: Word) {
    unsafe {
        ::core::ptr::write_unaligned((stack_buffer as usize + offset) as *mut Word, value);
    }
}

fn write_aux(stack_buffer: Word, offset: usize, key: Word, value: Word) -> usize {
    write_stack_word(stack_buffer, offset, key);
    write_stack_word(stack_buffer, offset + 8, value);
    offset + 16
}

fn read_word_from_buffer(buffer: Word, offset: usize) -> Word {
    unsafe { ::core::ptr::read_unaligned((buffer as usize + offset) as *const Word) }
}
