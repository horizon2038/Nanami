#![no_std]
#![no_main]

use core::ptr;
use libnanami::ipc::{ServiceEvent, ServiceRequest};
use libnanami::{self, RequestError, Word};

const SLOT_SERVICE_PORT: Word = 20;
const SLOT_VFS_SERVICE: Word = 23;
const VFS_SHM_BYTES: Word = 0x40000;
const PATH_OFFSET: usize = 0;
const MANIFEST_OFFSET: usize = 512;
const MANIFEST_MAX_BYTES: usize = 3072;
const FILE_CHUNK_OFFSET: usize = 0;
const EXEC_CHILD_PCB_SLOT: Word = 40;
const EXEC_LAUNCH_MAX_ARGS: usize = 16;
const EXEC_LAUNCH_MAX_ENVS: usize = 16;
const EXEC_ARG_STACK_BYTES: usize = 0x4000;
const USER_STACK_BASE: usize = 0x0400_0000;
const USER_STACK_PAGES: usize = 64;
const USER_STACK_TOP: usize = USER_STACK_BASE + USER_STACK_PAGES * 4096;
const USER_ARG_STACK_BASE: usize = USER_STACK_TOP - EXEC_ARG_STACK_BYTES;
const REG_RSP: Word = 18;
const REGISTER_MESSAGE_BASE: Word = 3;
const STACK_VERIFY_BYTES: Word = 16;
const EXEC_LAUNCH_FLAG_DIAGNOSTICS: Word = 1 << (Word::BITS - 1);
const MAX_ENTRIES: usize = 32;
const MAX_EXEC_CLIENTS: usize = 16;
const MAX_EXEC_CHILDREN: usize = 64;
const CONNECT_RETRIES: usize = 80;
const SYSTEM_LIST_PATH: &[u8] = b"/nanami/system-list";
const SESSION_LIST_PATH: &[u8] = b"/nanami/session-list";
const FALLBACK_SYSTEM_LIST: &str = include_str!("../../../system-list");
const FALLBACK_SESSION_LIST: &str = include_str!("../../../session-list");

#[derive(Clone, Copy)]
struct SystemEntry<'a> {
    name: &'a str,
    priority: Word,
    path: &'a str,
}

#[derive(Clone, Copy)]
struct VfsClient {
    port: Word,
    shm: Word,
    shm_size: Word,
}

#[derive(Clone, Copy)]
struct ExecClient {
    active: bool,
    pid: Word,
    shm: Word,
    shm_size: Word,
}

impl ExecClient {
    const EMPTY: Self = Self {
        active: false,
        pid: 0,
        shm: 0,
        shm_size: 0,
    };
}

#[derive(Clone, Copy)]
struct ExecChild {
    active: bool,
    owner_pid: Word,
    child_pid: Word,
}

impl ExecChild {
    const EMPTY: Self = Self {
        active: false,
        owner_pid: 0,
        child_pid: 0,
    };
}

struct ExecRuntime {
    clients: [ExecClient; MAX_EXEC_CLIENTS],
    children: [ExecChild; MAX_EXEC_CHILDREN],
    vfs: Option<VfsClient>,
}

#[derive(Clone, Copy)]
struct LaunchInfo {
    priority: Word,
    diagnostics: bool,
    argc: usize,
    envc: usize,
    argv: [(usize, usize); EXEC_LAUNCH_MAX_ARGS],
    envp: [(usize, usize); EXEC_LAUNCH_MAX_ENVS],
}

impl LaunchInfo {
    const EMPTY: Self = Self {
        priority: 16,
        diagnostics: false,
        argc: 0,
        envc: 0,
        argv: [(0, 0); EXEC_LAUNCH_MAX_ARGS],
        envp: [(0, 0); EXEC_LAUNCH_MAX_ENVS],
    };
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[system-manager] panic\n");
    let _ = libnanami::request_exit();
    loop {}
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::ipc::init_ipc_tls()
        .map_err(|e| log_error("[system-manager] ipc tls failed: ", e))?;
    libnanami::print!("[system-manager] bootstrap\n");

    nanami_services::registry::register_exec_service()
        .map_err(|e| log_error("[system-manager] exec-service register failed: ", e))?;
    libnanami::print!("[system-manager] service registered: exec-service\n");

    let mut vfs_client = match connect_vfs_client() {
        Ok(client) => Some(client),
        Err(error) => {
            log_request_error("[system-manager] vfs unavailable: ", error);
            None
        }
    };

    let manifest = match vfs_client.as_mut() {
        Some(client) => match read_manifest_from_vfs(client, SYSTEM_LIST_PATH) {
            Ok(manifest) => {
                libnanami::print!("[system-manager] loaded /nanami/system-list\n");
                manifest
            }
            Err(error) => {
                log_request_error("[system-manager] vfs system-list unavailable: ", error);
                ManifestBuffer::from_static(FALLBACK_SYSTEM_LIST)
            }
        },
        None => ManifestBuffer::from_static(FALLBACK_SYSTEM_LIST),
    };

    let summary = spawn_system_entries(manifest.as_str(), &mut vfs_client);
    libnanami::print!("[system-manager] spawn summary ok=");
    libnanami::print!("{}", summary.spawned);
    libnanami::print!(" failed=");
    libnanami::print!("{}", summary.failed);
    libnanami::print!("\n");

    let session_manifest = match vfs_client.as_mut() {
        Some(client) => match read_manifest_from_vfs(client, SESSION_LIST_PATH) {
            Ok(manifest) => {
                libnanami::print!("[system-manager] loaded /nanami/session-list\n");
                manifest
            }
            Err(error) => {
                log_request_error("[system-manager] vfs session-list unavailable: ", error);
                ManifestBuffer::from_static(FALLBACK_SESSION_LIST)
            }
        },
        None => ManifestBuffer::from_static(FALLBACK_SESSION_LIST),
    };

    let session_summary = spawn_system_entries(session_manifest.as_str(), &mut vfs_client);
    libnanami::print!("[system-manager] session summary ok=");
    libnanami::print!("{}", session_summary.spawned);
    libnanami::print!(" failed=");
    libnanami::print!("{}", session_summary.failed);
    libnanami::print!("\n");

    run_exec_service(ExecRuntime {
        clients: [ExecClient::EMPTY; MAX_EXEC_CLIENTS],
        children: [ExecChild::EMPTY; MAX_EXEC_CHILDREN],
        vfs: vfs_client,
    })
}

fn connect_vfs_client() -> Result<VfsClient, RequestError> {
    connect_vfs_service()?;
    let vfs_port = libnanami::ipc::process_slot_descriptor(SLOT_VFS_SERVICE);
    let (shm, shm_size) = nanami_services::vfs::vfs_attach_shared_memory(vfs_port, VFS_SHM_BYTES)?;
    if shm_size < VFS_SHM_BYTES || shm == 0 {
        return Err(RequestError::Protocol);
    }
    Ok(VfsClient {
        port: vfs_port,
        shm,
        shm_size,
    })
}

fn read_manifest_from_vfs(
    vfs: &mut VfsClient,
    path: &[u8],
) -> Result<ManifestBuffer, RequestError> {
    if path.is_empty() || path.len() > MANIFEST_OFFSET {
        return Err(RequestError::InvalidArgument);
    }
    write_shm_bytes(vfs.shm, PATH_OFFSET, path);
    let (_, size, kind) =
        nanami_services::vfs::vfs_stat(vfs.port, PATH_OFFSET as Word, path.len() as Word)?;
    if kind != nanami_services::vfs::VFS_FILE_TYPE_REGULAR {
        return Err(RequestError::InvalidArgument);
    }
    let handle = nanami_services::vfs::vfs_open(vfs.port, PATH_OFFSET as Word, path.len() as Word)?;
    let len = (size as usize).min(MANIFEST_MAX_BYTES);
    let read =
        nanami_services::vfs::vfs_read(vfs.port, handle, 0, len as Word, MANIFEST_OFFSET as Word);
    let _ = nanami_services::vfs::vfs_close(vfs.port, handle);
    let read = read? as usize;

    let mut buffer = ManifestBuffer::new();
    let count = read.min(MANIFEST_MAX_BYTES);
    let mut i = 0usize;
    while i < count {
        buffer.bytes[i] = read_shm_byte(vfs.shm, MANIFEST_OFFSET + i);
        i += 1;
    }
    buffer.len = count;
    Ok(buffer)
}

fn connect_vfs_service() -> Result<(), RequestError> {
    let mut tries = 0usize;
    loop {
        match nanami_services::registry::connect_vfs_service(SLOT_VFS_SERVICE) {
            Ok(()) => return Ok(()),
            Err(error) => {
                tries += 1;
                if tries >= CONNECT_RETRIES {
                    return Err(error);
                }
                spin_delay();
            }
        }
    }
}

fn spawn_system_entries(manifest: &str, vfs_client: &mut Option<VfsClient>) -> SpawnSummary {
    let mut entries = [None; MAX_ENTRIES];
    let mut count = 0usize;
    for line in manifest.lines() {
        if count >= MAX_ENTRIES {
            break;
        }
        if let Some(entry) = parse_system_list_line(line) {
            entries[count] = Some(entry);
            count += 1;
        }
    }
    sort_entries_by_priority(&mut entries, count);

    let mut summary = SpawnSummary::EMPTY;
    let mut i = 0usize;
    while i < count {
        if let Some(entry) = entries[i] {
            if vfs_client.is_none() {
                *vfs_client = connect_vfs_client().ok();
            }
            let result = match vfs_client.as_mut() {
                Some(client) => spawn_rootfs_entry(entry, client),
                None => Err(RequestError::InvalidArgument),
            };
            match result {
                Ok(pid) => {
                    summary.spawned += 1;
                    libnanami::print!("[system-manager] spawned ");
                    libnanami::print!("{}", entry.name);
                    libnanami::print!(" pid=");
                    libnanami::print!("{}", pid);
                    libnanami::print!(" path=");
                    libnanami::print!("{}", entry.path);
                    libnanami::print!("\n");
                }
                Err(error) => {
                    summary.failed += 1;
                    libnanami::print!("[system-manager] spawn failed name=");
                    libnanami::print!("{}", entry.name);
                    libnanami::print!(" path=");
                    libnanami::print!("{}", entry.path);
                    libnanami::print!(" ");
                    print_request_error(error);
                    libnanami::print!("\n");
                }
            }
        }
        i += 1;
    }
    summary
}

fn spawn_rootfs_entry(entry: SystemEntry<'_>, vfs: &mut VfsClient) -> Result<Word, RequestError> {
    let path = entry.path.as_bytes();
    if path.is_empty() || path.len() >= VFS_SHM_BYTES as usize {
        return Err(RequestError::InvalidArgument);
    }
    write_shm_bytes(vfs.shm, PATH_OFFSET, path);
    spawn_vfs_path(vfs, path.len(), entry.priority)
}

fn spawn_vfs_path(
    vfs: &mut VfsClient,
    path_len: usize,
    priority: Word,
) -> Result<Word, RequestError> {
    let (_, size, kind) =
        nanami_services::vfs::vfs_stat(vfs.port, PATH_OFFSET as Word, path_len as Word)?;
    if kind != nanami_services::vfs::VFS_FILE_TYPE_REGULAR || size == 0 {
        return Err(RequestError::InvalidArgument);
    }

    let (image_base, mapped_size) = libnanami::request_heap(size)?;
    if image_base == 0 || mapped_size < size {
        return Err(RequestError::Protocol);
    }

    let handle = nanami_services::vfs::vfs_open(vfs.port, PATH_OFFSET as Word, path_len as Word)?;
    let mut offset = 0usize;
    while offset < size as usize {
        let remaining = size as usize - offset;
        let chunk = remaining.min(vfs.shm_size as usize);
        let read = nanami_services::vfs::vfs_read(
            vfs.port,
            handle,
            offset as Word,
            chunk as Word,
            FILE_CHUNK_OFFSET as Word,
        )? as usize;
        if read == 0 {
            let _ = nanami_services::vfs::vfs_close(vfs.port, handle);
            return Err(RequestError::Protocol);
        }
        unsafe {
            ptr::copy_nonoverlapping(
                (vfs.shm as usize + FILE_CHUNK_OFFSET) as *const u8,
                (image_base as usize + offset) as *mut u8,
                read,
            );
        }
        offset += read;
    }
    let _ = nanami_services::vfs::vfs_close(vfs.port, handle);

    libnanami::request_process_spawn_memory(image_base, size, priority)
}

fn spawn_vfs_path_with_launch(
    vfs: &mut VfsClient,
    path_len: usize,
    client_shm: Word,
    launch: LaunchInfo,
) -> Result<Word, RequestError> {
    let (_, size, kind) =
        nanami_services::vfs::vfs_stat(vfs.port, PATH_OFFSET as Word, path_len as Word)?;
    if kind != nanami_services::vfs::VFS_FILE_TYPE_REGULAR || size == 0 {
        return Err(RequestError::InvalidArgument);
    }

    let (image_base, mapped_size) = libnanami::request_heap(size)?;
    if image_base == 0 || mapped_size < size {
        return Err(RequestError::Protocol);
    }

    let handle = nanami_services::vfs::vfs_open(vfs.port, PATH_OFFSET as Word, path_len as Word)?;
    let mut offset = 0usize;
    while offset < size as usize {
        let remaining = size as usize - offset;
        let chunk = remaining.min(vfs.shm_size as usize);
        let read = nanami_services::vfs::vfs_read(
            vfs.port,
            handle,
            offset as Word,
            chunk as Word,
            FILE_CHUNK_OFFSET as Word,
        )? as usize;
        if read == 0 {
            let _ = nanami_services::vfs::vfs_close(vfs.port, handle);
            return Err(RequestError::Protocol);
        }
        unsafe {
            ptr::copy_nonoverlapping(
                (vfs.shm as usize + FILE_CHUNK_OFFSET) as *const u8,
                (image_base as usize + offset) as *mut u8,
                read,
            );
        }
        offset += read;
    }
    let _ = nanami_services::vfs::vfs_close(vfs.port, handle);

    let pid = libnanami::request_process_spawn_memory_suspended(
        image_base,
        size,
        launch.priority,
        EXEC_CHILD_PCB_SLOT,
    )?;
    install_initial_stack(pid, client_shm, launch)?;
    let pcb = libnanami::ipc::process_slot_descriptor(EXEC_CHILD_PCB_SLOT);
    a9n_abi::arch::process_control_block::resume(pcb).map_err(|_| RequestError::Transport)?;
    Ok(pid)
}

fn parse_exec_launch_block(
    client: &ExecClient,
    offset: usize,
    len: usize,
) -> Result<LaunchInfo, RequestError> {
    if len < 24 || offset + len > client.shm_size as usize {
        return Err(RequestError::InvalidArgument);
    }
    let end = offset + len;
    let mut launch = LaunchInfo::EMPTY;
    let packed_priority = read_shm_word(client.shm, offset);
    launch.priority = packed_priority & !EXEC_LAUNCH_FLAG_DIAGNOSTICS;
    launch.diagnostics = (packed_priority & EXEC_LAUNCH_FLAG_DIAGNOSTICS) != 0;
    launch.argc = read_shm_word(client.shm, offset + 8) as usize;
    launch.envc = read_shm_word(client.shm, offset + 16) as usize;
    if launch.diagnostics {
        libnanami::println!(
            "[system-manager] launch parse owner={} offset={:#x} len={:#x} priority={} argc={} envc={}",
            client.pid,
            offset,
            len,
            launch.priority,
            launch.argc,
            launch.envc
        );
    }
    if launch.argc == 0 {
        return Err(RequestError::InvalidArgument);
    }
    if launch.argc > EXEC_LAUNCH_MAX_ARGS || launch.envc > EXEC_LAUNCH_MAX_ENVS {
        return Err(RequestError::InvalidArgument);
    }
    if launch.priority == 0 {
        launch.priority = 16;
    }

    let mut cursor = offset + 24;
    let mut i = 0usize;
    while i < launch.argc {
        let start = cursor;
        cursor = find_nul(client.shm, cursor, end)?;
        launch.argv[i] = (start, cursor - start);
        cursor += 1;
        i += 1;
    }
    i = 0;
    while i < launch.envc {
        let start = cursor;
        cursor = find_nul(client.shm, cursor, end)?;
        launch.envp[i] = (start, cursor - start);
        cursor += 1;
        i += 1;
    }
    Ok(launch)
}

fn install_initial_stack(
    pid: Word,
    client_shm: Word,
    launch: LaunchInfo,
) -> Result<(), RequestError> {
    let mut stack = [0u8; EXEC_ARG_STACK_BYTES];
    let mut sp = EXEC_ARG_STACK_BYTES;
    let mut argv_guest = [0usize; EXEC_LAUNCH_MAX_ARGS];
    let mut envp_guest = [0usize; EXEC_LAUNCH_MAX_ENVS];

    let mut i = 0usize;
    while i < launch.argc {
        argv_guest[i] = copy_launch_string_to_stack(
            &mut stack,
            &mut sp,
            client_shm,
            launch.argv[i].0,
            launch.argv[i].1,
        )?;
        i += 1;
    }
    i = 0;
    while i < launch.envc {
        envp_guest[i] = copy_launch_string_to_stack(
            &mut stack,
            &mut sp,
            client_shm,
            launch.envp[i].0,
            launch.envp[i].1,
        )?;
        i += 1;
    }

    let word_count = 1 + launch.argc + 1 + launch.envc + 1;
    let word_bytes = word_count * core::mem::size_of::<Word>();
    if sp < word_bytes {
        return Err(RequestError::InvalidArgument);
    }
    // Process entry runs a `call` into Rust; keep the entry RSP 16-byte aligned.
    sp = (sp - word_bytes) & !15;
    let mut out = sp;
    write_stack_word(&mut stack, &mut out, launch.argc as Word);
    i = 0;
    while i < launch.argc {
        write_stack_word(&mut stack, &mut out, argv_guest[i] as Word);
        i += 1;
    }
    write_stack_word(&mut stack, &mut out, 0);
    i = 0;
    while i < launch.envc {
        write_stack_word(&mut stack, &mut out, envp_guest[i] as Word);
        i += 1;
    }
    write_stack_word(&mut stack, &mut out, 0);

    let local = stack.as_ptr() as Word;
    let guest_sp = (USER_ARG_STACK_BASE + sp) as Word;
    let stack_bytes = EXEC_ARG_STACK_BYTES - sp;
    if launch.diagnostics {
        let local_argc = read_word_from_buffer(&stack, sp);
        let local_argv0 = read_word_from_buffer(&stack, sp + core::mem::size_of::<Word>());
        libnanami::println!(
            "[system-manager] argv local pid={} sp={:#x} argc={} argv0={:#x} bytes={:#x}",
            pid,
            guest_sp,
            local_argc,
            local_argv0,
            stack_bytes
        );
    }
    libnanami::request_process_memory_write(
        pid,
        guest_sp,
        local + sp as Word,
        stack_bytes as Word,
    )?;
    let pcb = libnanami::ipc::process_slot_descriptor(EXEC_CHILD_PCB_SLOT);
    write_register_value(pcb, REG_RSP, guest_sp)?;
    if launch.diagnostics {
        let rsp = read_register_value(pcb, REG_RSP).unwrap_or(0);
        let mut stack_argc = 0;
        let mut stack_argv0 = 0;
        if libnanami::request_process_memory_read(pid, guest_sp, local, STACK_VERIFY_BYTES).is_ok()
        {
            stack_argc = read_word_from_buffer(&stack, 0);
            stack_argv0 = read_word_from_buffer(&stack, core::mem::size_of::<Word>());
        }
        libnanami::println!(
            "[system-manager] argv stack pid={} rsp={:#x} argc={} argv0={:#x}",
            pid,
            rsp,
            stack_argc,
            stack_argv0
        );
    }
    Ok(())
}

fn copy_launch_string_to_stack(
    stack: &mut [u8; EXEC_ARG_STACK_BYTES],
    sp: &mut usize,
    source_base: Word,
    source_offset: usize,
    len: usize,
) -> Result<usize, RequestError> {
    if len == 0 || *sp < len + 1 {
        return Err(RequestError::InvalidArgument);
    }
    *sp -= len + 1;
    let mut i = 0usize;
    while i < len {
        stack[*sp + i] = read_shm_byte(source_base, source_offset + i);
        i += 1;
    }
    stack[*sp + len] = 0;
    Ok(USER_ARG_STACK_BASE + *sp)
}

fn write_stack_word(stack: &mut [u8; EXEC_ARG_STACK_BYTES], offset: &mut usize, value: Word) {
    let bytes = value.to_ne_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        stack[*offset + i] = bytes[i];
        i += 1;
    }
    *offset += core::mem::size_of::<Word>();
}

fn write_register_value(pcb: Word, register_index: Word, value: Word) -> Result<(), RequestError> {
    let count = register_index
        .checked_add(1)
        .ok_or(RequestError::InvalidArgument)?;
    a9n_abi::arch::process_control_block::read_register(pcb, count)
        .map_err(|_| RequestError::Transport)?;
    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    ipc.configure_message((REGISTER_MESSAGE_BASE + register_index) as usize, value);
    a9n_abi::arch::process_control_block::write_register(pcb, count)
        .map_err(|_| RequestError::Transport)
}

fn read_register_value(pcb: Word, register_index: Word) -> Result<Word, RequestError> {
    let count = register_index
        .checked_add(1)
        .ok_or(RequestError::InvalidArgument)?;
    a9n_abi::arch::process_control_block::read_register(pcb, count)
        .map_err(|_| RequestError::Transport)?;
    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    Ok(ipc.get_message((REGISTER_MESSAGE_BASE + register_index) as usize))
}

fn read_word_from_buffer(buffer: &[u8], offset: usize) -> Word {
    let mut bytes = [0u8; core::mem::size_of::<Word>()];
    let mut i = 0usize;
    while i < bytes.len() && offset + i < buffer.len() {
        bytes[i] = buffer[offset + i];
        i += 1;
    }
    Word::from_ne_bytes(bytes)
}

fn find_nul(base: Word, mut cursor: usize, end: usize) -> Result<usize, RequestError> {
    while cursor < end {
        if read_shm_byte(base, cursor) == 0 {
            return Ok(cursor);
        }
        cursor += 1;
    }
    Err(RequestError::InvalidArgument)
}

fn run_exec_service(mut runtime: ExecRuntime) -> libnanami::NanamiResult {
    let service_port = libnanami::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let mut pending = ExecReply::Drop;

    loop {
        let event = match pending {
            ExecReply::Send(status, detail0, detail1) => {
                pending = ExecReply::Drop;
                libnanami::ipc::service_reply_receive_event(service_port, status, detail0, detail1)
            }
            ExecReply::Drop => libnanami::ipc::service_receive_event(service_port),
        };
        let event = match event {
            Ok(event) => event,
            Err(error) => return Err(log_error("[system-manager] exec ipc failed: ", error).into()),
        };

        pending = match event {
            ServiceEvent::Request(request) => handle_exec_request(&mut runtime, request),
            ServiceEvent::Notification { .. } | ServiceEvent::Fault { .. } => ExecReply::Drop,
        };
    }
}

#[derive(Clone, Copy)]
enum ExecReply {
    Send(Word, Word, Word),
    Drop,
}

fn handle_exec_request(runtime: &mut ExecRuntime, request: ServiceRequest) -> ExecReply {
    let (status, detail0, detail1) = match request.code {
        nanami_services::exec::EXEC_REQUEST_CONTROL => handle_exec_control(runtime, request),
        nanami_services::exec::EXEC_REQUEST_SPAWN_PATH => handle_exec_spawn_path(runtime, request),
        nanami_services::exec::EXEC_REQUEST_SPAWN_PATH_ARGUMENTS => {
            handle_exec_spawn_path_arguments(runtime, request)
        }
        nanami_services::exec::EXEC_REQUEST_PROCESS_STATUS => {
            handle_exec_process_status(runtime, request)
        }
        nanami_services::exec::EXEC_REQUEST_PROCESS_REAP => {
            handle_exec_process_reap(runtime, request)
        }
        nanami_services::exec::EXEC_REQUEST_PROCESS_KILL => {
            handle_exec_process_kill(runtime, request)
        }
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    };
    ExecReply::Send(status, detail0, detail1)
}

fn handle_exec_control(runtime: &mut ExecRuntime, request: ServiceRequest) -> (Word, Word, Word) {
    match request.arg0 {
        nanami_services::exec::EXEC_CONTROL_ATTACH_SHARED_MEMORY => {
            let size = if request.arg1 == 0 {
                nanami_services::exec::EXEC_DEFAULT_SHM_BYTES
            } else {
                request.arg1
            };
            match libnanami::request_shared_memory(request.identifier, size) {
                Ok((local, peer)) => match exec_client_for_pid(runtime, request.identifier) {
                    Some(index) => {
                        runtime.clients[index] = ExecClient {
                            active: true,
                            pid: request.identifier,
                            shm: local,
                            shm_size: size,
                        };
                        (libnanami::OS_RESPONSE_OK, peer, size)
                    }
                    None => (libnanami::OS_RESPONSE_FATAL, 0, 0),
                },
                Err(error) => (map_request_error_to_status(error), 0, 0),
            }
        }
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_exec_spawn_path(
    runtime: &mut ExecRuntime,
    request: ServiceRequest,
) -> (Word, Word, Word) {
    let Some(index) = find_exec_client(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let client = runtime.clients[index];
    let path_offset = request.arg0 as usize;
    let path_len = request.arg1 as usize;
    if path_len == 0
        || path_offset >= client.shm_size as usize
        || path_len > client.shm_size as usize - path_offset
    {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }

    if runtime.vfs.is_none() {
        runtime.vfs = connect_vfs_client().ok();
    }
    let Some(vfs) = runtime.vfs.as_mut() else {
        return (libnanami::OS_RESPONSE_FATAL, 0, 0);
    };
    if path_len >= vfs.shm_size as usize {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }

    unsafe {
        ptr::copy_nonoverlapping(
            (client.shm as usize + path_offset) as *const u8,
            (vfs.shm as usize + PATH_OFFSET) as *mut u8,
            path_len,
        );
    }

    match spawn_vfs_path(vfs, path_len, request.arg2) {
        Ok(pid) => {
            register_exec_child(runtime, request.identifier, pid);
            (libnanami::OS_RESPONSE_OK, pid, 0)
        }
        Err(error) => (map_request_error_to_status(error), 0, 0),
    }
}

fn handle_exec_spawn_path_arguments(
    runtime: &mut ExecRuntime,
    request: ServiceRequest,
) -> (Word, Word, Word) {
    let Some(index) = find_exec_client(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let client = runtime.clients[index];
    let path_offset = request.arg0 as usize;
    let path_len = request.arg1 as usize;
    let launch_offset = request.arg2 as usize;
    let launch_len = request.arg3 as usize;
    if path_len == 0
        || launch_len == 0
        || path_offset >= client.shm_size as usize
        || launch_offset >= client.shm_size as usize
        || path_len > client.shm_size as usize - path_offset
        || launch_len > client.shm_size as usize - launch_offset
    {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }

    let launch = match parse_exec_launch_block(&client, launch_offset, launch_len) {
        Ok(launch) => launch,
        Err(error) => return (map_request_error_to_status(error), 0, 0),
    };

    if runtime.vfs.is_none() {
        runtime.vfs = connect_vfs_client().ok();
    }
    let Some(vfs) = runtime.vfs.as_mut() else {
        return (libnanami::OS_RESPONSE_FATAL, 0, 0);
    };
    if path_len >= vfs.shm_size as usize {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }

    unsafe {
        ptr::copy_nonoverlapping(
            (client.shm as usize + path_offset) as *const u8,
            (vfs.shm as usize + PATH_OFFSET) as *mut u8,
            path_len,
        );
    }

    match spawn_vfs_path_with_launch(vfs, path_len, client.shm, launch) {
        Ok(pid) => {
            register_exec_child(runtime, request.identifier, pid);
            (libnanami::OS_RESPONSE_OK, pid, 0)
        }
        Err(error) => (map_request_error_to_status(error), 0, 0),
    }
}

fn handle_exec_process_status(
    runtime: &mut ExecRuntime,
    request: ServiceRequest,
) -> (Word, Word, Word) {
    let pid = request.arg0;
    if find_exec_child(runtime, request.identifier, pid).is_none() {
        return (libnanami::OS_RESPONSE_OK, 1, 0);
    }
    match libnanami::request_process_status(pid) {
        Ok((exited, exit_status)) => (libnanami::OS_RESPONSE_OK, exited as Word, exit_status),
        Err(error) => (map_request_error_to_status(error), 0, 0),
    }
}

fn handle_exec_process_reap(
    runtime: &mut ExecRuntime,
    request: ServiceRequest,
) -> (Word, Word, Word) {
    let pid = request.arg0;
    let Some(index) = find_exec_child(runtime, request.identifier, pid) else {
        return (libnanami::OS_RESPONSE_OK, 0, 0);
    };
    match libnanami::request_process_reap(pid) {
        Ok(()) => {
            runtime.children[index] = ExecChild::EMPTY;
            (libnanami::OS_RESPONSE_OK, 0, 0)
        }
        Err(_) => {
            runtime.children[index] = ExecChild::EMPTY;
            (libnanami::OS_RESPONSE_OK, 0, 0)
        }
    }
}

fn handle_exec_process_kill(
    runtime: &mut ExecRuntime,
    request: ServiceRequest,
) -> (Word, Word, Word) {
    let pid = request.arg0;
    if find_exec_child(runtime, request.identifier, pid).is_none() {
        return (libnanami::OS_RESPONSE_OK, 0, 0);
    }
    match libnanami::request_process_kill(pid, request.arg1) {
        Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
        Err(_) => (libnanami::OS_RESPONSE_OK, 0, 0),
    }
}

fn find_exec_client(runtime: &ExecRuntime, pid: Word) -> Option<usize> {
    let mut i = 0usize;
    while i < MAX_EXEC_CLIENTS {
        let client = runtime.clients[i];
        if client.active && client.pid == pid {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn exec_client_for_pid(runtime: &ExecRuntime, pid: Word) -> Option<usize> {
    if let Some(index) = find_exec_client(runtime, pid) {
        return Some(index);
    }
    let mut i = 0usize;
    while i < MAX_EXEC_CLIENTS {
        if !runtime.clients[i].active {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn register_exec_child(runtime: &mut ExecRuntime, owner_pid: Word, child_pid: Word) {
    let mut i = 0usize;
    while i < MAX_EXEC_CHILDREN {
        if !runtime.children[i].active {
            runtime.children[i] = ExecChild {
                active: true,
                owner_pid,
                child_pid,
            };
            return;
        }
        i += 1;
    }
}

fn find_exec_child(runtime: &ExecRuntime, owner_pid: Word, child_pid: Word) -> Option<usize> {
    let mut i = 0usize;
    while i < MAX_EXEC_CHILDREN {
        let child = runtime.children[i];
        if child.active && child.owner_pid == owner_pid && child.child_pid == child_pid {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn parse_system_list_line(line: &str) -> Option<SystemEntry<'_>> {
    let line = line.split('#').next().unwrap_or("");
    let mut tokens = line.split_whitespace();
    let name = tokens.next()?;
    let priority = parse_decimal_word(tokens.next()?)?;
    let path = tokens.next()?;
    Some(SystemEntry {
        name,
        priority,
        path,
    })
}

fn sort_entries_by_priority(entries: &mut [Option<SystemEntry<'_>>; MAX_ENTRIES], count: usize) {
    let mut i = 1usize;
    while i < count {
        let current = entries[i];
        let Some(current_entry) = current else {
            i += 1;
            continue;
        };
        let mut j = i;
        while j > 0 {
            let previous_priority = entries[j - 1].map(|entry| entry.priority).unwrap_or(0);
            if previous_priority >= current_entry.priority {
                break;
            }
            entries[j] = entries[j - 1];
            j -= 1;
        }
        entries[j] = current;
        i += 1;
    }
}

fn parse_decimal_word(text: &str) -> Option<Word> {
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

struct ManifestBuffer {
    bytes: [u8; MANIFEST_MAX_BYTES],
    len: usize,
    static_text: Option<&'static str>,
}

impl ManifestBuffer {
    const fn new() -> Self {
        Self {
            bytes: [0; MANIFEST_MAX_BYTES],
            len: 0,
            static_text: None,
        }
    }

    const fn from_static(text: &'static str) -> Self {
        Self {
            bytes: [0; MANIFEST_MAX_BYTES],
            len: 0,
            static_text: Some(text),
        }
    }

    fn as_str(&self) -> &str {
        if let Some(text) = self.static_text {
            return text;
        }
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

struct SpawnSummary {
    spawned: usize,
    failed: usize,
}

impl SpawnSummary {
    const EMPTY: Self = Self {
        spawned: 0,
        failed: 0,
    };
}

fn write_shm_bytes(base: Word, offset: usize, bytes: &[u8]) {
    let mut i = 0usize;
    while i < bytes.len() {
        unsafe {
            // The VFS service owns protocol bounds; caller only writes within the negotiated shm page.
            core::ptr::write_volatile((base as usize + offset + i) as *mut u8, bytes[i]);
        }
        i += 1;
    }
}

fn read_shm_byte(base: Word, offset: usize) -> u8 {
    unsafe {
        // The offset is limited by MANIFEST_MAX_BYTES and VFS_SHM_BYTES before this read.
        core::ptr::read_volatile((base as usize + offset) as *const u8)
    }
}

fn read_shm_word(base: Word, offset: usize) -> Word {
    let mut bytes = [0u8; core::mem::size_of::<Word>()];
    let mut i = 0usize;
    while i < bytes.len() {
        unsafe {
            bytes[i] = core::ptr::read_volatile((base as usize + offset + i) as *const u8);
        }
        i += 1;
    }
    Word::from_ne_bytes(bytes)
}

fn spin_delay() {
    libnanami::yield_now();
}

fn log_error(prefix: &str, error: RequestError) -> RequestError {
    libnanami::print!("{}", prefix);
    print_request_error(error);
    libnanami::print!("\n");
    error
}

fn log_request_error(prefix: &str, error: RequestError) {
    libnanami::print!("{}", prefix);
    print_request_error(error);
    libnanami::print!("\n");
}

fn map_request_error_to_status(error: RequestError) -> Word {
    match error {
        RequestError::InvalidArgument => libnanami::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Transport | RequestError::Protocol => libnanami::OS_RESPONSE_FATAL,
        RequestError::Status(status) => status,
    }
}

fn print_request_error(error: RequestError) {
    match error {
        RequestError::InvalidArgument => libnanami::print!("invalid-argument"),
        RequestError::Unsupported => libnanami::print!("unsupported"),
        RequestError::Transport => libnanami::print!("transport"),
        RequestError::Protocol => libnanami::print!("protocol"),
        RequestError::Status(status) => {
            libnanami::print!("status=");
            libnanami::print!("{:#x}", status);
        }
    }
}

libnanami::nanami_entry!(nanami_main);
