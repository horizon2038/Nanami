use libnanami::ipc::ServiceRequest;
use libnanami::Word;

use crate::abi::*;
use crate::loader::{load_cached_fork_linux_elf_image, map_request_error_to_status};
use crate::personality;
use crate::process::{read_register_value, write_exec_registers, REG_RSP};
use crate::state::{OsPersonality, ReplyAction, Runtime};

pub struct LaunchInfo {
    pub image_name: [u8; ALTER_IMAGE_NAME_MAX],
    pub image_name_len: usize,
    argc: usize,
    envc: usize,
    argv_offsets: [usize; ALTER_LAUNCH_MAX_ARGS],
    argv_lens: [usize; ALTER_LAUNCH_MAX_ARGS],
    env_offsets: [usize; ALTER_LAUNCH_MAX_ENVS],
    env_lens: [usize; ALTER_LAUNCH_MAX_ENVS],
}

impl LaunchInfo {
    const EMPTY: Self = Self {
        image_name: [0; ALTER_IMAGE_NAME_MAX],
        image_name_len: 0,
        argc: 0,
        envc: 0,
        argv_offsets: [0; ALTER_LAUNCH_MAX_ARGS],
        argv_lens: [0; ALTER_LAUNCH_MAX_ARGS],
        env_offsets: [0; ALTER_LAUNCH_MAX_ENVS],
        env_lens: [0; ALTER_LAUNCH_MAX_ENVS],
    };
}

pub fn handle_spawn_linux(runtime: &mut Runtime, request: ServiceRequest) -> ReplyAction {
    if runtime.client_shm == 0 {
        return ReplyAction::Reply(libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let Some(info) = parse_launch_info(runtime, request.arg0 as usize, request.arg1 as usize)
    else {
        return ReplyAction::Reply(libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Ok(image_name) = ::core::str::from_utf8(&info.image_name[..info.image_name_len]) else {
        return ReplyAction::Reply(libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    if request.arg2 != 0 {
        if let Err(status) = ensure_terminal_client(runtime, request.arg2) {
            return ReplyAction::Reply(status, 0, 0);
        }
        crate::cleanup_exited_processes(runtime);
    }
    let Some(pcb_slot) = runtime.next_pcb_slot() else {
        return ReplyAction::Reply(libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    };
    let personality = personality_from_flags(request.arg3);
    let diagnostics = (request.arg3 & ALTER_LAUNCH_FLAG_DIAGNOSTICS) != 0;

    if is_path_launch(runtime, &info) {
        return spawn_rootfs_linux_image(
            runtime,
            request,
            &info,
            image_name,
            pcb_slot,
            personality,
        );
    }

    match libnanami::request_process_spawn_fault_handler_suspended(image_name, pcb_slot) {
        Ok(pid) => {
            let pcb = libnanami::ipc::process_slot_descriptor(pcb_slot);
            if let Err(status) =
                prepare_initial_stack(runtime, pid, pcb, &info, personality, diagnostics)
            {
                libnanami::println!(
                    "[alter] spawn failed stage=stack image={} pid={} status={}",
                    image_name,
                    pid,
                    status
                );
                discard_spawned_process(pid);
                return ReplyAction::Reply(status, 0, 0);
            }
            if !runtime.install_managed_process(
                pid,
                request.identifier,
                pcb,
                request.arg2,
                &info.image_name[..info.image_name_len],
            ) {
                libnanami::println!(
                    "[alter] spawn failed stage=install image={} pid={}",
                    image_name,
                    pid
                );
                discard_spawned_process(pid);
                return ReplyAction::Reply(libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
            }
            if !register_linux_stack_mappings(runtime, pid) {
                libnanami::println!(
                    "[alter] spawn failed stage=stack-track image={} pid={}",
                    image_name,
                    pid
                );
                discard_spawned_process(pid);
                return ReplyAction::Reply(libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
            }
            let trace_enabled = (request.arg3 & ALTER_LAUNCH_FLAG_STRACE) != 0;
            let _ = runtime.set_trace_enabled(pid, trace_enabled);
            let _ = runtime.set_diagnostics_enabled(pid, diagnostics);
            let _ = runtime.set_personality(pid, personality);
            if a9n_abi::arch::process_control_block::resume(pcb).is_err() {
                libnanami::println!(
                    "[alter] spawn failed stage=resume image={} pid={} pcb={:#x}",
                    image_name,
                    pid,
                    pcb
                );
                runtime.remove_process(pid);
                discard_spawned_process(pid);
                return ReplyAction::Reply(libnanami::OS_RESPONSE_FATAL, 0, 0);
            }
            libnanami::println!(
                "[alter] managed process image={} pid={} pcb={:#x}",
                image_name,
                pid,
                pcb
            );
            ReplyAction::Reply(libnanami::OS_RESPONSE_OK, pid, pcb)
        }
        Err(e) => {
            libnanami::println!(
                "[alter] spawn failed stage=kernel-spawn image={} err={:?}",
                image_name,
                e
            );
            ReplyAction::Reply(map_request_error_to_status(e), 0, 0)
        }
    }
}

fn spawn_rootfs_linux_image(
    runtime: &mut Runtime,
    request: ServiceRequest,
    info: &LaunchInfo,
    image_name: &str,
    pcb_slot: Word,
    personality: OsPersonality,
) -> ReplyAction {
    let loaded = match load_cached_fork_linux_elf_image(
        runtime,
        info.argv_offsets[0] as Word,
        info.argv_lens[0] as Word,
    ) {
        Ok(loaded) => loaded,
        Err(error) => {
            return ReplyAction::Reply(crate::loader::map_load_error_to_status(error), 0, 0)
        }
    };

    match libnanami::request_process_spawn_memory_fault_handler_suspended(
        loaded.address,
        loaded.size,
        4,
        pcb_slot,
    ) {
        Ok(pid) => {
            let pcb = libnanami::ipc::process_slot_descriptor(pcb_slot);
            let diagnostics = (request.arg3 & ALTER_LAUNCH_FLAG_DIAGNOSTICS) != 0;
            if let Err(status) =
                prepare_initial_stack(runtime, pid, pcb, info, personality, diagnostics)
            {
                libnanami::println!(
                    "[alter] spawn failed stage=rootfs-stack image={} pid={} status={}",
                    image_name,
                    pid,
                    status
                );
                discard_spawned_process(pid);
                return ReplyAction::Reply(status, 0, 0);
            }
            if !runtime.install_managed_process(
                pid,
                request.identifier,
                pcb,
                request.arg2,
                &info.image_name[..info.image_name_len],
            ) {
                libnanami::println!(
                    "[alter] spawn failed stage=rootfs-install image={} pid={}",
                    image_name,
                    pid
                );
                discard_spawned_process(pid);
                return ReplyAction::Reply(libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
            }
            if !register_linux_stack_mappings(runtime, pid) {
                libnanami::println!(
                    "[alter] spawn failed stage=rootfs-stack-track image={} pid={}",
                    image_name,
                    pid
                );
                discard_spawned_process(pid);
                return ReplyAction::Reply(libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
            }
            let trace_enabled = (request.arg3 & ALTER_LAUNCH_FLAG_STRACE) != 0;
            let _ = runtime.set_trace_enabled(pid, trace_enabled);
            let _ = runtime.set_diagnostics_enabled(pid, diagnostics);
            let _ = runtime.set_personality(pid, personality);
            if a9n_abi::arch::process_control_block::resume(pcb).is_err() {
                libnanami::println!(
                    "[alter] spawn failed stage=rootfs-resume image={} pid={} pcb={:#x}",
                    image_name,
                    pid,
                    pcb
                );
                runtime.remove_process(pid);
                discard_spawned_process(pid);
                return ReplyAction::Reply(libnanami::OS_RESPONSE_FATAL, 0, 0);
            }
            libnanami::println!(
                "[alter/{}] managed rootfs process image={} pid={} pcb={:#x} entry={:#x} bytes={:#x}",
                personality::name(personality),
                image_name,
                pid,
                pcb,
                loaded.metadata.entry_point,
                loaded.size
            );
            ReplyAction::Reply(libnanami::OS_RESPONSE_OK, pid, pcb)
        }
        Err(error) => {
            libnanami::println!(
                "[alter] spawn failed stage=rootfs-kernel image={} err={:?}",
                image_name,
                error
            );
            ReplyAction::Reply(map_request_error_to_status(error), 0, 0)
        }
    }
}

fn personality_from_flags(_flags: Word) -> OsPersonality {
    crate::launch_personality_from_flags(_flags)
}

fn discard_spawned_process(pid: Word) {
    let _ = libnanami::request_process_kill(pid, 1);
    let _ = libnanami::request_process_reap(pid);
}

fn ensure_terminal_client(runtime: &mut Runtime, _terminal_id: Word) -> Result<(), Word> {
    if runtime.terminal_port != 0 && runtime.terminal_shm != 0 && runtime.terminal_shm_size != 0 {
        return Ok(());
    }

    nanami_services::registry::connect_terminal_service(SLOT_TERMINAL_SERVICE)
        .map_err(map_request_error_to_status)?;
    let terminal_port = libnanami::ipc::process_slot_descriptor(SLOT_TERMINAL_SERVICE);
    let (terminal_shm, terminal_shm_size) =
        nanami_services::terminal::terminal_attach_shared_memory(
            terminal_port,
            ALTER_DEFAULT_SHM_BYTES,
        )
        .map_err(map_request_error_to_status)?;
    runtime.terminal_port = terminal_port;
    runtime.terminal_shm = terminal_shm;
    runtime.terminal_shm_size = terminal_shm_size;
    Ok(())
}

fn parse_launch_info(runtime: &Runtime, offset: usize, len: usize) -> Option<LaunchInfo> {
    if len < 16 || offset.checked_add(len)? > runtime.client_shm_size as usize {
        return None;
    }
    let argc = read_client_word(runtime, offset)? as usize;
    let envc = read_client_word(runtime, offset + 8)? as usize;
    if argc == 0 || argc > ALTER_LAUNCH_MAX_ARGS || envc > ALTER_LAUNCH_MAX_ENVS {
        return None;
    }

    let mut info = LaunchInfo::EMPTY;
    info.argc = argc;
    info.envc = envc;
    let mut cursor = offset + 16;
    let end = offset + len;
    let mut index = 0usize;
    while index < argc {
        let (start, string_len, next) = read_nul_string(runtime, cursor, end)?;
        if string_len == 0 {
            return None;
        }
        info.argv_offsets[index] = start;
        info.argv_lens[index] = string_len;
        cursor = next;
        index += 1;
    }
    index = 0;
    while index < envc {
        let (start, string_len, next) = read_nul_string(runtime, cursor, end)?;
        if string_len == 0 {
            return None;
        }
        info.env_offsets[index] = start;
        info.env_lens[index] = string_len;
        cursor = next;
        index += 1;
    }

    let image = basename(runtime, info.argv_offsets[0], info.argv_lens[0])?;
    if image.1 == 0 || image.1 > ALTER_IMAGE_NAME_MAX {
        return None;
    }
    copy_client_bytes(runtime, image.0, image.1, &mut info.image_name);
    info.image_name_len = image.1;
    Some(info)
}

fn prepare_initial_stack(
    runtime: &mut Runtime,
    pid: Word,
    pcb: Word,
    info: &LaunchInfo,
    personality: OsPersonality,
    diagnostics: bool,
) -> Result<(), Word> {
    let elf = read_target_elf_metadata(runtime, pid);
    if elf.has_interpreter {
        libnanami::println!(
            "[alter] dynamic ELF is not supported yet pid={} interp=PT_INTERP",
            pid
        );
        return Err(libnanami::OS_RESPONSE_ILLEGAL_OPERATION);
    }
    let (stack_buffer, stack_buffer_size) =
        libnanami::request_heap(LINUX_INITIAL_STACK_BYTES as Word)
            .map_err(map_request_error_to_status)?;
    if stack_buffer_size < LINUX_INITIAL_STACK_BYTES as Word {
        return Err(libnanami::OS_RESPONSE_INVALID_ARGUMENT);
    }
    unsafe {
        ::core::ptr::write_bytes(stack_buffer as *mut u8, 0, LINUX_INITIAL_STACK_BYTES);
    }

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
        b"x86_64\0",
        &mut platform_guest,
        stack_base,
    )?;

    let aux_pairs = 18usize;
    let word_count = 1 + info.argc + 1 + info.envc + 1 + aux_pairs * 2;
    let table_bytes = word_count * ::core::mem::size_of::<Word>();
    let aligned_cursor = align_down_usize(string_cursor, 16);
    if aligned_cursor < table_bytes + 16 {
        return Err(libnanami::OS_RESPONSE_INVALID_ARGUMENT);
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
    out = write_aux(stack_buffer, out, AT_BASE, 0);
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
    if diagnostics {
        let local_argc = read_word_from_buffer(stack_buffer, sp_offset);
        let local_argv0 = read_word_from_buffer(stack_buffer, sp_offset + 8);
        libnanami::println!(
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
    libnanami::request_process_memory_write(
        pid,
        stack_base,
        stack_buffer,
        LINUX_INITIAL_STACK_BYTES as Word,
    )
    .map_err(map_request_error_to_status)?;
    if personality == OsPersonality::FreeBsd {
        let fs_base = install_freebsd_initial_tls(runtime, pid, &elf)
            .map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
        write_exec_registers(pcb, elf.entry_point, guest_sp, fs_base, 0, guest_sp, 0, 0)
            .map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
        let _ = runtime.set_fs_base(pid, fs_base);
    } else {
        write_exec_registers(pcb, elf.entry_point, guest_sp, 0, 0, 0, 0, 0)
            .map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
    }
    if diagnostics {
        let rsp = read_register_value(pcb, REG_RSP).unwrap_or(0);
        let mut stack_argc = 0;
        let mut stack_argv0 = 0;
        if libnanami::request_process_memory_read(pid, guest_sp, stack_buffer, 16).is_ok() {
            stack_argc = read_word_from_buffer(stack_buffer, 0);
            stack_argv0 = read_word_from_buffer(stack_buffer, 8);
        }
        libnanami::println!(
            "[alter/{}] stack verify pid={} rsp={:#x} argc={} argv0={:#x} personality={}",
            personality::name(personality),
            pid,
            rsp,
            stack_argc,
            stack_argv0,
            personality::name(personality)
        );
    }
    Ok(())
}

fn read_client_word(runtime: &Runtime, offset: usize) -> Option<Word> {
    if offset.checked_add(8)? > runtime.client_shm_size as usize {
        return None;
    }
    Some(unsafe {
        ::core::ptr::read_unaligned((runtime.client_shm as usize + offset) as *const Word)
    })
}

fn initial_stack_table_offset(
    aligned_cursor: usize,
    table_bytes: usize,
    personality: OsPersonality,
) -> Result<usize, Word> {
    if aligned_cursor < table_bytes + 16 {
        return Err(libnanami::OS_RESPONSE_INVALID_ARGUMENT);
    }
    let offset = align_down_usize(aligned_cursor - table_bytes, 16);
    if personality == OsPersonality::FreeBsd {
        offset
            .checked_sub(8)
            .ok_or(libnanami::OS_RESPONSE_INVALID_ARGUMENT)
    } else {
        Ok(offset)
    }
}

fn read_nul_string(runtime: &Runtime, offset: usize, end: usize) -> Option<(usize, usize, usize)> {
    let mut cursor = offset;
    while cursor < end {
        let byte =
            unsafe { ::core::ptr::read((runtime.client_shm as usize + cursor) as *const u8) };
        if byte == 0 {
            return Some((offset, cursor - offset, cursor + 1));
        }
        cursor += 1;
    }
    None
}

fn basename(runtime: &Runtime, offset: usize, len: usize) -> Option<(usize, usize)> {
    let mut start = offset;
    let mut i = 0usize;
    while i < len {
        let byte =
            unsafe { ::core::ptr::read((runtime.client_shm as usize + offset + i) as *const u8) };
        if byte == b'/' {
            start = offset + i + 1;
        }
        i += 1;
    }
    Some((start, offset + len - start))
}

fn is_path_launch(runtime: &Runtime, info: &LaunchInfo) -> bool {
    contains_slash(runtime, info.argv_offsets[0], info.argv_lens[0])
}

fn contains_slash(runtime: &Runtime, offset: usize, len: usize) -> bool {
    let mut i = 0usize;
    while i < len {
        if read_client_byte(runtime, offset + i) == b'/' {
            return true;
        }
        i += 1;
    }
    false
}

fn linux_argv0(runtime: &Runtime, info: &LaunchInfo) -> Result<(usize, usize), Word> {
    let (offset, mut len) = basename(runtime, info.argv_offsets[0], info.argv_lens[0])
        .ok_or(libnanami::OS_RESPONSE_INVALID_ARGUMENT)?;
    if len == 0 {
        return Err(libnanami::OS_RESPONSE_INVALID_ARGUMENT);
    }

    if len > 4
        && read_client_byte(runtime, offset + len - 4) == b'.'
        && read_client_byte(runtime, offset + len - 3) == b'e'
        && read_client_byte(runtime, offset + len - 2) == b'l'
        && read_client_byte(runtime, offset + len - 1) == b'f'
    {
        len -= 4;
    }
    if len == 0 {
        return Err(libnanami::OS_RESPONSE_INVALID_ARGUMENT);
    }
    Ok((offset, len))
}

fn read_client_byte(runtime: &Runtime, offset: usize) -> u8 {
    unsafe { ::core::ptr::read((runtime.client_shm as usize + offset) as *const u8) }
}

fn copy_client_bytes(runtime: &Runtime, offset: usize, len: usize, dst: &mut [u8]) {
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            (runtime.client_shm as usize + offset) as *const u8,
            dst.as_mut_ptr(),
            len,
        );
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
        .ok_or(libnanami::OS_RESPONSE_INVALID_ARGUMENT)?;
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            (runtime.client_shm as usize + offset) as *const u8,
            (stack_buffer as usize + next) as *mut u8,
            len,
        );
        ::core::ptr::write((stack_buffer as usize + next + len) as *mut u8, 0);
    }
    *guest_address = stack_base as usize + next;
    next = align_down_usize(next, 8);
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
        .ok_or(libnanami::OS_RESPONSE_INVALID_ARGUMENT)?;
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            (stack_buffer as usize + next) as *mut u8,
            bytes.len(),
        );
    }
    *guest_address = stack_base as usize + next;
    next = align_down_usize(next, 8);
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

#[derive(Clone, Copy)]
struct TargetElfMetadata {
    entry_point: Word,
    program_header_vaddr: Word,
    program_header_entry_size: Word,
    program_header_count: Word,
    has_interpreter: bool,
    tls_vaddr: Word,
    tls_file_size: Word,
    tls_memory_size: Word,
    tls_align: Word,
}

impl TargetElfMetadata {
    const EMPTY: Self = Self {
        entry_point: 0,
        program_header_vaddr: 0,
        program_header_entry_size: 0,
        program_header_count: 0,
        has_interpreter: false,
        tls_vaddr: 0,
        tls_file_size: 0,
        tls_memory_size: 0,
        tls_align: 0,
    };
}

fn read_target_elf_metadata(runtime: &mut Runtime, pid: Word) -> TargetElfMetadata {
    let candidates = [
        LINUX_DEFAULT_IMAGE_BASE,
        FREEBSD_DEFAULT_IMAGE_BASE,
        NANAMI_DEFAULT_IMAGE_BASE,
    ];
    let mut i = 0usize;
    while i < candidates.len() {
        if let Some(metadata) = read_target_elf_metadata_at(runtime, pid, candidates[i]) {
            return metadata;
        }
        i += 1;
    }
    TargetElfMetadata::EMPTY
}

fn read_target_elf_metadata_at(
    runtime: &mut Runtime,
    pid: Word,
    image_base: Word,
) -> Option<TargetElfMetadata> {
    let mut out = TargetElfMetadata {
        ..TargetElfMetadata::EMPTY
    };
    if libnanami::request_process_memory_read(
        pid,
        image_base,
        runtime.posix_shm,
        LINUX_ELF_PROBE_BYTES,
    )
    .is_err()
    {
        return None;
    }
    if read_probe_u8(runtime, 0) != 0x7f
        || read_probe_u8(runtime, 1) != b'E'
        || read_probe_u8(runtime, 2) != b'L'
        || read_probe_u8(runtime, 3) != b'F'
        || read_probe_u8(runtime, 4) != 2
        || read_probe_u8(runtime, 5) != 1
    {
        return None;
    }

    let phoff = read_probe_u64(runtime, 32) as usize;
    let phentsize = read_probe_u16(runtime, 54) as usize;
    let phnum = read_probe_u16(runtime, 56) as usize;
    if phentsize < 56 || phnum == 0 {
        return None;
    }

    let elf_type = read_probe_u16(runtime, 16);
    let load_bias = if elf_type == 3 { image_base } else { 0 };
    out.entry_point = read_probe_u64(runtime, 24) + load_bias;
    out.program_header_entry_size = phentsize as Word;
    out.program_header_count = phnum as Word;
    out.program_header_vaddr = image_base + phoff as Word;

    let mut i = 0usize;
    while i < phnum {
        let base = phoff + i * phentsize;
        if base + 56 > LINUX_ELF_PROBE_BYTES as usize {
            break;
        }
        let p_type = read_probe_u32(runtime, base);
        if p_type == PT_INTERP {
            out.has_interpreter = true;
        }
        if p_type == PT_TLS {
            out.tls_vaddr = read_probe_u64(runtime, base + 16) + load_bias;
            out.tls_file_size = read_probe_u64(runtime, base + 32);
            out.tls_memory_size = read_probe_u64(runtime, base + 40);
            out.tls_align = read_probe_u64(runtime, base + 48);
        }
        if p_type == 1 {
            let p_offset = read_probe_u64(runtime, base + 8) as usize;
            let p_vaddr = read_probe_u64(runtime, base + 16);
            let p_filesz = read_probe_u64(runtime, base + 32) as usize;
            if phoff >= p_offset && phoff < p_offset.saturating_add(p_filesz) {
                out.program_header_vaddr = p_vaddr + load_bias + (phoff - p_offset) as Word;
            }
        }
        i += 1;
    }

    Some(out)
}

fn install_freebsd_initial_tls(
    runtime: &mut Runtime,
    pid: Word,
    elf: &TargetElfMetadata,
) -> Result<Word, ()> {
    if elf.tls_memory_size == 0 {
        return Ok(0);
    }
    if elf.tls_file_size > elf.tls_memory_size {
        return Err(());
    }
    let total = align_up_word(elf.tls_memory_size + 16, 16);
    if total == 0 || total > runtime.posix_shm_size {
        return Err(());
    }
    let (tls_base, mapped) =
        libnanami::request_process_map_anonymous(pid, total).map_err(|_| ())?;
    if mapped < total {
        return Err(());
    }
    unsafe {
        ::core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, total as usize);
    }
    if elf.tls_file_size != 0 {
        libnanami::request_process_memory_read(
            pid,
            elf.tls_vaddr,
            runtime.posix_shm,
            elf.tls_file_size,
        )
        .map_err(|_| ())?;
    }
    let fs_base = tls_base + elf.tls_memory_size;
    unsafe {
        ::core::ptr::write_unaligned(
            (runtime.posix_shm + elf.tls_memory_size) as *mut Word,
            fs_base,
        );
    }
    libnanami::request_process_memory_write(pid, tls_base, runtime.posix_shm, total)
        .map_err(|_| ())?;
    libnanami::println!(
        "[alter/freebsd] tls pid={} base={:#x} fs={:#x} file={:#x} mem={:#x}",
        pid,
        tls_base,
        fs_base,
        elf.tls_file_size,
        elf.tls_memory_size
    );
    Ok(fs_base)
}

fn align_up_word(value: Word, align: Word) -> Word {
    if align == 0 {
        return value;
    }
    (value + align - 1) & !(align - 1)
}

fn read_probe_u8(runtime: &Runtime, offset: usize) -> u8 {
    unsafe { ::core::ptr::read((runtime.posix_shm as usize + offset) as *const u8) }
}

fn read_probe_u16(runtime: &Runtime, offset: usize) -> u16 {
    unsafe { ::core::ptr::read_unaligned((runtime.posix_shm as usize + offset) as *const u16) }
}

fn read_probe_u32(runtime: &Runtime, offset: usize) -> u32 {
    unsafe { ::core::ptr::read_unaligned((runtime.posix_shm as usize + offset) as *const u32) }
}

fn read_probe_u64(runtime: &Runtime, offset: usize) -> Word {
    unsafe { ::core::ptr::read_unaligned((runtime.posix_shm as usize + offset) as *const Word) }
}

fn register_linux_stack_mappings(runtime: &mut Runtime, pid: Word) -> bool {
    let stack_base = LINUX_STACK_TOP - LINUX_STACK_BYTES;
    runtime.reset_stack_mapping(
        pid,
        stack_base,
        LINUX_STACK_BYTES,
        LINUX_PROT_READ | LINUX_PROT_WRITE,
    ) && runtime.add_mapping(
        pid,
        LINUX_STACK_TOP,
        LINUX_STACK_GUARD_BYTES,
        LINUX_PROT_NONE,
    )
}

const fn align_down_usize(value: usize, align: usize) -> usize {
    value & !(align - 1)
}

const LINUX_STACK_TOP: Word = 0x4040000;
const LINUX_STACK_BYTES: Word = 0x40000;
const LINUX_STACK_GUARD_BYTES: Word = 0x3000;
const LINUX_INITIAL_STACK_BYTES: usize = 0x4000;
const LINUX_DEFAULT_IMAGE_BASE: Word = 0x400000;
const FREEBSD_DEFAULT_IMAGE_BASE: Word = 0x200000;
const NANAMI_DEFAULT_IMAGE_BASE: Word = 0x1000000;
const LINUX_ELF_PROBE_BYTES: Word = 0x1000;
const LINUX_PROT_NONE: Word = 0x0;
const LINUX_PROT_READ: Word = 0x1;
const LINUX_PROT_WRITE: Word = 0x2;
const PT_INTERP: u32 = 3;
const PT_TLS: u32 = 7;

const AT_NULL: Word = 0;
const AT_PHDR: Word = 3;
const AT_PHENT: Word = 4;
const AT_PHNUM: Word = 5;
const AT_PAGESZ: Word = 6;
const AT_BASE: Word = 7;
const AT_FLAGS: Word = 8;
const AT_ENTRY: Word = 9;
const AT_UID: Word = 11;
const AT_EUID: Word = 12;
const AT_GID: Word = 13;
const AT_EGID: Word = 14;
const AT_PLATFORM: Word = 15;
const AT_HWCAP: Word = 16;
const AT_CLKTCK: Word = 17;
const AT_SECURE: Word = 23;
const AT_RANDOM: Word = 25;
const AT_EXECFN: Word = 31;
