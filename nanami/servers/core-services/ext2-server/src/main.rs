#![no_std]
#![no_main]

use core::cmp::min;
use core::ptr;
use libnanami::ipc::{ServiceEvent, ServiceRequest};
use libnanami::{self, RequestError, Word};

const SLOT_SERVICE_PORT: Word = 20;
const SLOT_BLOCK_DEVICE: Word = 23;
const SLOT_TIMER_SERVICE: Word = 24;
const BLOCK_SHM_BYTES: Word = 0x4000;
const BLOCK_BUFFER_OFFSET: Word = 0;
const BLOCK_CONNECT_RETRIES: usize = 64;
const BLOCK_CONNECT_RETRY_MS: Word = 100;
const MAX_SESSIONS: usize = 16;
const MAX_DELEGATED_SESSIONS: usize = 16;
const MAX_HANDLES: usize = 64;
const EXT2_MAGIC: u16 = 0xef53;
const EXT2_ROOT_INODE: u32 = 2;
const EXT2_GOOD_OLD_FIRST_INO: usize = 11;
const EXT2_S_IFDIR: u16 = 0x4000;
const EXT2_S_IFREG: u16 = 0x8000;
const EXT2_NAME_LEN_MAX: usize = 255;
const EXT2_MAX_BLOCK_GROUPS: usize = 32;
const EXT2_MAX_DIRECT_BLOCKS: usize = 12;
const EXT2_SINGLE_INDIRECT_INDEX: usize = 12;
const EXT2_DOUBLE_INDIRECT_INDEX: usize = 13;
const EXT2_INODE_BLOCK_POINTERS: usize = 15;
const EXT2_BLOCK_CACHE_ENTRIES: usize = 8;
const EXT2_BLOCK_CACHE_BYTES: usize = 4096;
const EXT2_INODE_CACHE_ENTRIES: usize = 64;
const EXT2_DENTRY_CACHE_ENTRIES: usize = 128;
const EXT2_FT_REG_FILE: u8 = 1;
const EXT2_FT_DIR: u8 = 2;

#[derive(Clone, Copy)]
struct ClientSession {
    active: bool,
    pid: Word,
    shm_local: Word,
    shm_size: Word,
}

impl ClientSession {
    const EMPTY: Self = Self {
        active: false,
        pid: 0,
        shm_local: 0,
        shm_size: 0,
    };
}

#[derive(Clone, Copy)]
struct DelegatedSession {
    active: bool,
    owner_pid: Word,
    peer_pid: Word,
    shm_local: Word,
    shm_size: Word,
    peer_vaddr: Word,
}

impl DelegatedSession {
    const EMPTY: Self = Self {
        active: false,
        owner_pid: 0,
        peer_pid: 0,
        shm_local: 0,
        shm_size: 0,
        peer_vaddr: 0,
    };
}

#[derive(Clone, Copy)]
struct FileHandle {
    active: bool,
    owner_pid: Word,
    inode: u32,
    size: u32,
    mode: u16,
}

impl FileHandle {
    const EMPTY: Self = Self {
        active: false,
        owner_pid: 0,
        inode: 0,
        size: 0,
        mode: 0,
    };
}

#[derive(Clone, Copy)]
struct Ext2Superblock {
    inodes_count: u32,
    blocks_per_group: u32,
    inodes_per_group: u32,
    inode_size: usize,
}

#[derive(Clone, Copy)]
struct Ext2GroupDescriptor {
    block_bitmap_block: u32,
    inode_bitmap_block: u32,
    inode_table_block: u32,
    block_count: u32,
}

const EXT2_GROUP_EMPTY: Ext2GroupDescriptor = Ext2GroupDescriptor {
    block_bitmap_block: 0,
    inode_bitmap_block: 0,
    inode_table_block: 0,
    block_count: 0,
};

#[derive(Clone, Copy)]
struct Ext2Inode {
    mode: u16,
    size: u32,
    block: [u32; EXT2_INODE_BLOCK_POINTERS],
}

#[derive(Clone, Copy)]
struct Ext2DirectoryEntry {
    inode: u32,
    record_len: usize,
    name_len: usize,
    file_type: u8,
    name_ptr: *const u8,
}

#[derive(Clone, Copy)]
struct CachedBlock {
    valid: bool,
    block: usize,
    data: [u8; EXT2_BLOCK_CACHE_BYTES],
}

#[derive(Clone, Copy)]
struct CachedInode {
    valid: bool,
    inode_no: u32,
    inode: Ext2Inode,
}

impl CachedInode {
    const EMPTY: Self = Self {
        valid: false,
        inode_no: 0,
        inode: Ext2Inode {
            mode: 0,
            size: 0,
            block: [0; EXT2_INODE_BLOCK_POINTERS],
        },
    };
}

#[derive(Clone, Copy)]
struct CachedDentry {
    valid: bool,
    parent_inode: u32,
    inode: u32,
    name_len: u16,
    name: [u8; EXT2_NAME_LEN_MAX],
}

impl CachedDentry {
    const EMPTY: Self = Self {
        valid: false,
        parent_inode: 0,
        inode: 0,
        name_len: 0,
        name: [0; EXT2_NAME_LEN_MAX],
    };
}

impl CachedBlock {
    const EMPTY: Self = Self {
        valid: false,
        block: 0,
        data: [0; EXT2_BLOCK_CACHE_BYTES],
    };
}

struct Ext2Runtime {
    block_port: Word,
    block_shm: Word,
    block_shm_size: Word,
    block_size: usize,
    block_count: usize,
    superblock: Ext2Superblock,
    groups: [Ext2GroupDescriptor; EXT2_MAX_BLOCK_GROUPS],
    group_count: usize,
    sessions: [ClientSession; MAX_SESSIONS],
    delegated_sessions: [DelegatedSession; MAX_DELEGATED_SESSIONS],
    handles: [FileHandle; MAX_HANDLES],
    block_cache: [CachedBlock; EXT2_BLOCK_CACHE_ENTRIES],
    block_cache_next: usize,
    inode_cache: [CachedInode; EXT2_INODE_CACHE_ENTRIES],
    inode_cache_next: usize,
    dentry_cache: [CachedDentry; EXT2_DENTRY_CACHE_ENTRIES],
    dentry_cache_next: usize,
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[ext2-server] panic\n");
    loop {}
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::print!("[ext2-server] bootstrap\n");
    let block_port = connect_block_device()
        .map_err(|e| log_error("[ext2-server] block-device unavailable: ", e))?;
    let (block_shm, block_shm_size) =
        nanami_services::block::block_device_attach_shared_memory(block_port, BLOCK_SHM_BYTES)
            .map_err(|e| log_error("[ext2-server] block shm attach failed: ", e))?;
    let (block_size, block_count) = nanami_services::block::block_device_info(block_port)
        .map_err(|e| log_error("[ext2-server] block info failed: ", e))?;

    let mut runtime = Ext2Runtime {
        block_port,
        block_shm,
        block_shm_size,
        block_size: block_size as usize,
        block_count: block_count as usize,
        superblock: Ext2Superblock {
            inodes_count: 0,
            blocks_per_group: 0,
            inodes_per_group: 0,
            inode_size: 0,
        },
        groups: [EXT2_GROUP_EMPTY; EXT2_MAX_BLOCK_GROUPS],
        group_count: 0,
        sessions: [ClientSession::EMPTY; MAX_SESSIONS],
        delegated_sessions: [DelegatedSession::EMPTY; MAX_DELEGATED_SESSIONS],
        handles: [FileHandle::EMPTY; MAX_HANDLES],
        block_cache: [CachedBlock::EMPTY; EXT2_BLOCK_CACHE_ENTRIES],
        block_cache_next: 0,
        inode_cache: [CachedInode::EMPTY; EXT2_INODE_CACHE_ENTRIES],
        inode_cache_next: 0,
        dentry_cache: [CachedDentry::EMPTY; EXT2_DENTRY_CACHE_ENTRIES],
        dentry_cache_next: 0,
    };
    mount_ext2(&mut runtime).map_err(|e| log_error("[ext2-server] mount failed: ", e))?;

    nanami_services::registry::register_vfs_service()
        .map_err(|e| log_error("[ext2-server] register failed: ", e))?;
    libnanami::print!("[ext2-server] service registered: vfs-service\n");

    let service_port = libnanami::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let mut pending = (libnanami::OS_RESPONSE_OK, 0, 0);
    let mut has_reply = false;
    loop {
        let event = if has_reply {
            has_reply = false;
            match libnanami::ipc::service_reply_receive_event(
                service_port,
                pending.0,
                pending.1,
                pending.2,
            ) {
                Ok(event) => event,
                Err(e) => {
                    log_request_error("[ext2-server] reply_receive failed: ", e);
                    continue;
                }
            }
        } else {
            match libnanami::ipc::service_receive_event(service_port) {
                Ok(event) => event,
                Err(e) => return Err(log_error("[ext2-server] receive failed: ", e)),
            }
        };

        match event {
            ServiceEvent::Request(request) => {
                pending = handle_request(request, &mut runtime);
                has_reply = true;
            }
            ServiceEvent::Notification { .. } => {}
            ServiceEvent::Fault {
                identifier, reason, ..
            } => {
                libnanami::print!("[ext2-server] fault id={}", identifier);
                libnanami::print!(" reason={:#x}\n", reason);
            }
        }
    }
}

fn connect_block_device() -> Result<Word, RequestError> {
    let timer_port = match nanami_services::registry::connect_timer_service(SLOT_TIMER_SERVICE) {
        Ok(()) => Some(libnanami::ipc::process_slot_descriptor(SLOT_TIMER_SERVICE)),
        Err(e) => {
            log_request_error("[ext2-server] timer connect failed: ", e);
            None
        }
    };
    let mut tries = 0usize;
    loop {
        match nanami_services::registry::connect_block_device_with_pid(SLOT_BLOCK_DEVICE) {
            Ok(_) => return Ok(libnanami::ipc::process_slot_descriptor(SLOT_BLOCK_DEVICE)),
            Err(e) => {
                if tries == 0 || tries + 1 == BLOCK_CONNECT_RETRIES {
                    log_request_error("[ext2-server] waiting block-device: ", e);
                }
                tries += 1;
                if tries >= BLOCK_CONNECT_RETRIES {
                    return Err(e);
                }
                if let Some(timer) = timer_port {
                    let _ = nanami_services::timer::timer_service_sleep_milliseconds(
                        timer,
                        BLOCK_CONNECT_RETRY_MS,
                    );
                } else {
                    libnanami::yield_now();
                }
            }
        }
    }
}

fn handle_request(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    match request.code {
        nanami_services::vfs::VFS_REQUEST_CONTROL => handle_control(request, runtime),
        nanami_services::vfs::VFS_REQUEST_OPEN => handle_open(request, runtime),
        nanami_services::vfs::VFS_REQUEST_OPEN_COMPOUND => handle_open_compound(request, runtime),
        nanami_services::vfs::VFS_REQUEST_READ => handle_read(request, runtime),
        nanami_services::vfs::VFS_REQUEST_READ_DELEGATED => handle_read_delegated(request, runtime),
        nanami_services::vfs::VFS_REQUEST_STAT => handle_stat(request, runtime),
        nanami_services::vfs::VFS_REQUEST_CLOSE => handle_close(request, runtime),
        nanami_services::vfs::VFS_REQUEST_READ_DIR => handle_read_dir(request, runtime),
        nanami_services::vfs::VFS_REQUEST_FSTAT => handle_fstat(request, runtime),
        nanami_services::vfs::VFS_REQUEST_CREATE => handle_create(request, runtime, false),
        nanami_services::vfs::VFS_REQUEST_MKDIR => handle_create(request, runtime, true),
        nanami_services::vfs::VFS_REQUEST_WRITE => handle_write_file(request, runtime),
        nanami_services::vfs::VFS_REQUEST_REMOVE => handle_remove(request, runtime),
        nanami_services::vfs::VFS_REQUEST_RENAME => handle_rename(request, runtime),
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_control(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    match request.arg0 {
        nanami_services::vfs::VFS_CONTROL_ATTACH_SHARED_MEMORY => {
            let size = if request.arg1 == 0 {
                nanami_services::vfs::VFS_DEFAULT_SHM_BYTES
            } else {
                request.arg1
            };
            match libnanami::request_shared_memory(request.identifier, size) {
                Ok((local, peer)) => match session_for_pid(runtime, request.identifier) {
                    Some(index) => {
                        runtime.sessions[index] = ClientSession {
                            active: true,
                            pid: request.identifier,
                            shm_local: local,
                            shm_size: size,
                        };
                        (libnanami::OS_RESPONSE_OK, peer, size)
                    }
                    None => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
                },
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_services::vfs::VFS_CONTROL_ATTACH_DELEGATED_SHARED_MEMORY => {
            if request.arg1 == 0 || find_session(runtime, request.identifier).is_none() {
                return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            let size = if request.arg2 == 0 {
                nanami_services::vfs::VFS_DEFAULT_SHM_BYTES
            } else {
                request.arg2
            };
            if size > nanami_services::vfs::VFS_DELEGATE_VALUE_MASK {
                return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            if let Some(index) = find_delegated_session(runtime, request.identifier, request.arg1) {
                let session = runtime.delegated_sessions[index];
                return delegated_attach_response(index, session.peer_vaddr, session.shm_size);
            }
            let Some(index) = free_delegated_session(runtime) else {
                return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            };
            match libnanami::request_shared_memory(request.arg1, size) {
                Ok((local, peer)) => {
                    runtime.delegated_sessions[index] = DelegatedSession {
                        active: true,
                        owner_pid: request.identifier,
                        peer_pid: request.arg1,
                        shm_local: local,
                        shm_size: size,
                        peer_vaddr: peer,
                    };
                    delegated_attach_response(index, peer, size)
                }
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn delegated_attach_response(index: usize, peer_vaddr: Word, size: Word) -> (Word, Word, Word) {
    let delegate_id = index as Word + 1;
    let packed = (delegate_id << nanami_services::vfs::VFS_DELEGATE_ID_SHIFT)
        | (size & nanami_services::vfs::VFS_DELEGATE_VALUE_MASK);
    (libnanami::OS_RESPONSE_OK, peer_vaddr, packed)
}

fn handle_open(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let Some(session) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some(inode_no) = lookup_path(
        runtime,
        session,
        request.arg0 as usize,
        request.arg1 as usize,
    ) else {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    let Ok(inode) = read_inode(runtime, inode_no) else {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    match alloc_handle(runtime, request.identifier, inode_no, inode) {
        Some(handle) => (libnanami::OS_RESPONSE_OK, handle as Word, 0),
        None => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_open_compound(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let flags = request.arg2;
    let supported_flags = nanami_services::vfs::VFS_OPEN_CREATE
        | nanami_services::vfs::VFS_OPEN_TRUNCATE
        | nanami_services::vfs::VFS_OPEN_DIRECTORY;
    if (flags & !supported_flags) != 0 || !has_available_handle(runtime) {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let Some(session) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let path_offset = request.arg0 as usize;
    let path_len = request.arg1 as usize;
    let inode_no = match lookup_path(runtime, session, path_offset, path_len) {
        Some(inode_no) => inode_no,
        None if (flags & nanami_services::vfs::VFS_OPEN_CREATE) != 0 => {
            if (flags & nanami_services::vfs::VFS_OPEN_DIRECTORY) != 0 {
                return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
            }
            let Some((parent_inode, name_ptr, name_len)) =
                lookup_parent_path(runtime, session, path_offset, path_len)
            else {
                return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
            };
            match create_node(runtime, parent_inode, name_ptr, name_len, false) {
                Ok(inode_no) => inode_no,
                Err(status) => return (status, 0, 0),
            }
        }
        None => return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0),
    };
    let Ok(mut inode) = read_inode(runtime, inode_no) else {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    let kind = file_kind(inode.mode);
    if (flags & nanami_services::vfs::VFS_OPEN_DIRECTORY) != 0
        && kind != nanami_services::vfs::VFS_FILE_TYPE_DIRECTORY
    {
        return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    }
    if (flags & nanami_services::vfs::VFS_OPEN_TRUNCATE) != 0 {
        if kind == nanami_services::vfs::VFS_FILE_TYPE_DIRECTORY {
            return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
        }
        if let Err(status) = truncate_inode(runtime, inode_no, &mut inode) {
            return (status, 0, 0);
        }
    }
    match alloc_handle(runtime, request.identifier, inode_no, inode) {
        Some(handle) => {
            let opened = ((inode_no as Word) << nanami_services::vfs::VFS_OPEN_INODE_SHIFT)
                | (handle as Word & nanami_services::vfs::VFS_OPEN_HANDLE_MASK);
            (
                libnanami::OS_RESPONSE_OK,
                opened,
                pack_stat(inode.size as Word, kind),
            )
        }
        None => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_stat(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let Some(session) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some(inode_no) = lookup_path(
        runtime,
        session,
        request.arg0 as usize,
        request.arg1 as usize,
    ) else {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    let Ok(inode) = read_inode(runtime, inode_no) else {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    (
        libnanami::OS_RESPONSE_OK,
        inode_no as Word,
        pack_stat(inode.size as Word, file_kind(inode.mode)),
    )
}

fn handle_fstat(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let handle = request.arg0 as usize;
    if handle >= runtime.handles.len()
        || !runtime.handles[handle].active
        || runtime.handles[handle].owner_pid != request.identifier
    {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let file = runtime.handles[handle];
    (
        libnanami::OS_RESPONSE_OK,
        file.inode as Word,
        pack_stat(file.size as Word, file_kind(file.mode)),
    )
}

fn handle_read(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let Some(session) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    handle_read_into(request, runtime, session, request.arg3 as usize)
}

fn handle_read_delegated(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let delegate_id = (request.arg3 >> nanami_services::vfs::VFS_DELEGATE_ID_SHIFT) as usize;
    if delegate_id == 0 {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let index = delegate_id - 1;
    if index >= runtime.delegated_sessions.len() {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let delegated = runtime.delegated_sessions[index];
    if !delegated.active || delegated.owner_pid != request.identifier {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let session = ClientSession {
        active: true,
        pid: delegated.peer_pid,
        shm_local: delegated.shm_local,
        shm_size: delegated.shm_size,
    };
    let out_offset = (request.arg3 & nanami_services::vfs::VFS_DELEGATE_VALUE_MASK) as usize;
    handle_read_into(request, runtime, session, out_offset)
}

fn handle_read_into(
    request: ServiceRequest,
    runtime: &mut Ext2Runtime,
    session: ClientSession,
    out_offset: usize,
) -> (Word, Word, Word) {
    let handle = request.arg0 as usize;
    if handle >= runtime.handles.len()
        || !runtime.handles[handle].active
        || runtime.handles[handle].owner_pid != request.identifier
    {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let file = runtime.handles[handle];
    if (file.mode & EXT2_S_IFREG) != EXT2_S_IFREG {
        return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    }
    if request.arg1 as usize >= file.size as usize || request.arg2 == 0 {
        return (libnanami::OS_RESPONSE_OK, 0, 0);
    }
    let Ok(inode) = read_inode(runtime, file.inode) else {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    match read_file(
        runtime,
        session,
        inode,
        request.arg1 as usize,
        request.arg2 as usize,
        out_offset,
    ) {
        Ok(bytes) => (libnanami::OS_RESPONSE_OK, bytes as Word, 0),
        Err(status) => (status, 0, 0),
    }
}

fn handle_close(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let handle = request.arg0 as usize;
    if handle < runtime.handles.len()
        && runtime.handles[handle].active
        && runtime.handles[handle].owner_pid == request.identifier
    {
        runtime.handles[handle] = FileHandle::EMPTY;
        return (libnanami::OS_RESPONSE_OK, 0, 0);
    }
    (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0)
}

fn handle_read_dir(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let handle = request.arg0 as usize;
    if handle >= runtime.handles.len()
        || !runtime.handles[handle].active
        || runtime.handles[handle].owner_pid != request.identifier
    {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let Some(session) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let file = runtime.handles[handle];
    if (file.mode & EXT2_S_IFDIR) != EXT2_S_IFDIR {
        return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    }
    let Ok(inode) = read_inode(runtime, file.inode) else {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    match read_directory(
        runtime,
        session,
        inode,
        request.arg1 as usize,
        request.arg2 as usize,
        request.arg3 as usize,
    ) {
        Ok((entries, next_index)) => (
            libnanami::OS_RESPONSE_OK,
            entries as Word,
            next_index as Word,
        ),
        Err(status) => (status, 0, 0),
    }
}

fn handle_write_file(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let handle = request.arg0 as usize;
    if handle >= runtime.handles.len()
        || !runtime.handles[handle].active
        || runtime.handles[handle].owner_pid != request.identifier
    {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let Some(session) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let file = runtime.handles[handle];
    if (file.mode & EXT2_S_IFREG) != EXT2_S_IFREG {
        return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    }
    let input_offset = request.arg3 as usize;
    let len = request.arg2 as usize;
    if input_offset + len > session.shm_size as usize {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let Ok(mut inode) = read_inode(runtime, file.inode) else {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    match write_file(
        runtime,
        session,
        &mut inode,
        file.inode,
        request.arg1 as usize,
        len,
        input_offset,
    ) {
        Ok(bytes) => {
            runtime.handles[handle].size = inode.size;
            (libnanami::OS_RESPONSE_OK, bytes as Word, 0)
        }
        Err(status) => (status, 0, 0),
    }
}

fn handle_create(
    request: ServiceRequest,
    runtime: &mut Ext2Runtime,
    is_directory: bool,
) -> (Word, Word, Word) {
    let Some(session) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((parent_inode, name_ptr, name_len)) = lookup_parent_path(
        runtime,
        session,
        request.arg0 as usize,
        request.arg1 as usize,
    ) else {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    if unsafe { lookup_in_directory(runtime, parent_inode, name_ptr, name_len).is_some() } {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    match create_node(runtime, parent_inode, name_ptr, name_len, is_directory) {
        Ok(inode) => (libnanami::OS_RESPONSE_OK, inode as Word, 0),
        Err(status) => (status, 0, 0),
    }
}

fn handle_remove(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let Some(session) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((parent_inode, name_ptr, name_len)) = lookup_parent_path(
        runtime,
        session,
        request.arg0 as usize,
        request.arg1 as usize,
    ) else {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    match remove_node(runtime, parent_inode, name_ptr, name_len) {
        Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
        Err(status) => (status, 0, 0),
    }
}

fn handle_rename(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let Some(session) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((old_parent, old_name, old_name_len)) = lookup_parent_path(
        runtime,
        session,
        request.arg0 as usize,
        request.arg1 as usize,
    ) else {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    let Some((new_parent, new_name, new_name_len)) = lookup_parent_path(
        runtime,
        session,
        request.arg2 as usize,
        request.arg3 as usize,
    ) else {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    if old_parent == new_parent
        && old_name_len == new_name_len
        && unsafe { bytes_eq(old_name, new_name, old_name_len) }
    {
        return (libnanami::OS_RESPONSE_OK, 0, 0);
    }
    if unsafe { lookup_in_directory(runtime, new_parent, new_name, new_name_len).is_some() } {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    match rename_node(
        runtime,
        old_parent,
        old_name,
        old_name_len,
        new_parent,
        new_name,
        new_name_len,
    ) {
        Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
        Err(status) => (status, 0, 0),
    }
}

fn mount_ext2(runtime: &mut Ext2Runtime) -> Result<(), RequestError> {
    read_block(runtime, 1)?;
    let p = runtime.block_shm as usize;
    let magic = r16(p + 56);
    if magic != EXT2_MAGIC {
        return Err(RequestError::Protocol);
    }
    let log_block_size = r32(p + 24) as usize;
    let block_size = 1024usize << log_block_size;
    if block_size != runtime.block_size {
        return Err(RequestError::Unsupported);
    }
    let inodes_count = r32(p + 0);
    let blocks_count = r32(p + 4);
    let blocks_per_group = r32(p + 32);
    let inodes_per_group = r32(p + 40);
    if blocks_per_group == 0 || inodes_per_group == 0 {
        return Err(RequestError::Protocol);
    }
    let group_count =
        ((blocks_count as usize) + (blocks_per_group as usize) - 1) / (blocks_per_group as usize);
    if group_count == 0 || group_count > EXT2_MAX_BLOCK_GROUPS {
        return Err(RequestError::Unsupported);
    }
    runtime.superblock = Ext2Superblock {
        inodes_count,
        blocks_per_group,
        inodes_per_group,
        inode_size: r16(p + 88) as usize,
    };
    if runtime.superblock.inode_size == 0 {
        runtime.superblock.inode_size = 128;
    }
    let gd_block = if block_size == 1024 { 2 } else { 1 };
    read_block(runtime, gd_block)?;
    let gp = runtime.block_shm as usize;
    runtime.group_count = group_count;
    let mut group = 0usize;
    while group < group_count {
        let off = gp + group * 32;
        let start = group * blocks_per_group as usize;
        let remaining = runtime.block_count.saturating_sub(start);
        let count = min(remaining, blocks_per_group as usize);
        runtime.groups[group] = Ext2GroupDescriptor {
            block_bitmap_block: r32(off + 0),
            inode_bitmap_block: r32(off + 4),
            inode_table_block: r32(off + 8),
            block_count: count as u32,
        };
        group += 1;
    }
    libnanami::print!("[ext2-server] mounted ext2 block-size={}", block_size);
    libnanami::print!(
        " groups={} inode-table={}\n",
        runtime.group_count,
        runtime.groups[0].inode_table_block as usize
    );
    Ok(())
}

fn read_block(runtime: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    if load_cached_block(runtime, block) {
        return Ok(());
    }
    read_blocks_uncached(runtime, block, 1)?;
    store_cached_block(runtime, block);
    Ok(())
}

fn read_blocks(runtime: &mut Ext2Runtime, block: usize, count: usize) -> Result<(), RequestError> {
    if count == 1 {
        return read_block(runtime, block);
    }
    read_blocks_uncached(runtime, block, count)
}

fn read_blocks_uncached(
    runtime: &mut Ext2Runtime,
    block: usize,
    count: usize,
) -> Result<(), RequestError> {
    let bytes = count
        .checked_mul(runtime.block_size)
        .ok_or(RequestError::InvalidArgument)?;
    let end = block
        .checked_add(count)
        .ok_or(RequestError::InvalidArgument)?;
    if count == 0
        || block >= runtime.block_count
        || end > runtime.block_count
        || bytes as Word > runtime.block_shm_size
    {
        return Err(RequestError::InvalidArgument);
    }
    let read = nanami_services::block::block_device_read(
        runtime.block_port,
        block as Word,
        count as Word,
        BLOCK_BUFFER_OFFSET,
    )?;
    if read as usize != bytes {
        return Err(RequestError::Protocol);
    }
    Ok(())
}

fn write_block(runtime: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    if block >= runtime.block_count || runtime.block_size as Word > runtime.block_shm_size {
        return Err(RequestError::InvalidArgument);
    }
    let bytes = nanami_services::block::block_device_write(
        runtime.block_port,
        block as Word,
        1,
        BLOCK_BUFFER_OFFSET,
    )?;
    if bytes as usize != runtime.block_size {
        return Err(RequestError::Protocol);
    }
    store_cached_block(runtime, block);
    Ok(())
}

fn load_cached_block(runtime: &mut Ext2Runtime, block: usize) -> bool {
    if runtime.block_size > EXT2_BLOCK_CACHE_BYTES {
        return false;
    }
    let mut i = 0usize;
    while i < runtime.block_cache.len() {
        let entry = &runtime.block_cache[i];
        if entry.valid && entry.block == block {
            unsafe {
                ptr::copy_nonoverlapping(
                    entry.data.as_ptr(),
                    runtime.block_shm as *mut u8,
                    runtime.block_size,
                );
            }
            return true;
        }
        i += 1;
    }
    false
}

fn store_cached_block(runtime: &mut Ext2Runtime, block: usize) {
    if runtime.block_size > EXT2_BLOCK_CACHE_BYTES {
        return;
    }
    let mut index = runtime.block_cache_next;
    let mut i = 0usize;
    while i < runtime.block_cache.len() {
        if runtime.block_cache[i].valid && runtime.block_cache[i].block == block {
            index = i;
            break;
        }
        i += 1;
    }
    if i == runtime.block_cache.len() {
        runtime.block_cache_next = (runtime.block_cache_next + 1) % runtime.block_cache.len();
    }
    let entry = &mut runtime.block_cache[index];
    entry.valid = true;
    entry.block = block;
    unsafe {
        ptr::copy_nonoverlapping(
            runtime.block_shm as *const u8,
            entry.data.as_mut_ptr(),
            runtime.block_size,
        );
    }
}

fn load_cached_inode(runtime: &Ext2Runtime, inode_no: u32) -> Option<Ext2Inode> {
    runtime
        .inode_cache
        .iter()
        .find(|entry| entry.valid && entry.inode_no == inode_no)
        .map(|entry| entry.inode)
}

fn store_cached_inode(runtime: &mut Ext2Runtime, inode_no: u32, inode: Ext2Inode) {
    let index = runtime
        .inode_cache
        .iter()
        .position(|entry| entry.valid && entry.inode_no == inode_no)
        .unwrap_or_else(|| {
            let index = runtime.inode_cache_next;
            runtime.inode_cache_next = (runtime.inode_cache_next + 1) % runtime.inode_cache.len();
            index
        });
    runtime.inode_cache[index] = CachedInode {
        valid: true,
        inode_no,
        inode,
    };
}

fn invalidate_cached_inode(runtime: &mut Ext2Runtime, inode_no: u32) {
    for entry in runtime.inode_cache.iter_mut() {
        if entry.valid && entry.inode_no == inode_no {
            entry.valid = false;
        }
    }
}

unsafe fn load_cached_dentry(
    runtime: &Ext2Runtime,
    parent_inode: u32,
    name: *const u8,
    name_len: usize,
) -> Option<Option<u32>> {
    if name_len == 0 || name_len > EXT2_NAME_LEN_MAX {
        return None;
    }
    runtime
        .dentry_cache
        .iter()
        .find(|entry| {
            entry.valid
                && entry.parent_inode == parent_inode
                && entry.name_len as usize == name_len
                && bytes_eq(entry.name.as_ptr(), name, name_len)
        })
        .map(|entry| (entry.inode != 0).then_some(entry.inode))
}

unsafe fn store_cached_dentry(
    runtime: &mut Ext2Runtime,
    parent_inode: u32,
    name: *const u8,
    name_len: usize,
    inode: Option<u32>,
) {
    if name_len == 0 || name_len > EXT2_NAME_LEN_MAX {
        return;
    }
    let index = runtime
        .dentry_cache
        .iter()
        .position(|entry| {
            entry.valid
                && entry.parent_inode == parent_inode
                && entry.name_len as usize == name_len
                && bytes_eq(entry.name.as_ptr(), name, name_len)
        })
        .unwrap_or_else(|| {
            let index = runtime.dentry_cache_next;
            runtime.dentry_cache_next =
                (runtime.dentry_cache_next + 1) % runtime.dentry_cache.len();
            index
        });
    let entry = &mut runtime.dentry_cache[index];
    entry.valid = true;
    entry.parent_inode = parent_inode;
    entry.inode = inode.unwrap_or(0);
    entry.name_len = name_len as u16;
    ptr::copy_nonoverlapping(name, entry.name.as_mut_ptr(), name_len);
}

unsafe fn invalidate_cached_dentry(
    runtime: &mut Ext2Runtime,
    parent_inode: u32,
    name: *const u8,
    name_len: usize,
) {
    for entry in runtime.dentry_cache.iter_mut() {
        if entry.valid
            && entry.parent_inode == parent_inode
            && entry.name_len as usize == name_len
            && bytes_eq(entry.name.as_ptr(), name, name_len)
        {
            entry.valid = false;
        }
    }
}

fn invalidate_cached_dentries_for_inode(runtime: &mut Ext2Runtime, inode_no: u32) {
    for entry in runtime.dentry_cache.iter_mut() {
        if entry.valid && (entry.parent_inode == inode_no || entry.inode == inode_no) {
            entry.valid = false;
        }
    }
}

fn update_cached_handle_metadata(runtime: &mut Ext2Runtime, inode_no: u32, inode: Ext2Inode) {
    for handle in runtime.handles.iter_mut() {
        if handle.active && handle.inode == inode_no {
            handle.size = inode.size;
            handle.mode = inode.mode;
        }
    }
}

fn read_inode(runtime: &mut Ext2Runtime, inode_no: u32) -> Result<Ext2Inode, RequestError> {
    if inode_no == 0 || runtime.superblock.inodes_per_group == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let index = inode_no as usize - 1;
    if index >= runtime.superblock.inodes_count as usize {
        return Err(RequestError::InvalidArgument);
    }
    if let Some(inode) = load_cached_inode(runtime, inode_no) {
        return Ok(inode);
    }
    let group_index = index / runtime.superblock.inodes_per_group as usize;
    if group_index >= runtime.group_count {
        return Err(RequestError::InvalidArgument);
    }
    let index_in_group = index % runtime.superblock.inodes_per_group as usize;
    let inode_offset = index_in_group * runtime.superblock.inode_size;
    let block =
        runtime.groups[group_index].inode_table_block as usize + inode_offset / runtime.block_size;
    let in_block = inode_offset % runtime.block_size;
    read_block(runtime, block)?;
    let p = runtime.block_shm as usize + in_block;
    let mut blocks = [0u32; EXT2_INODE_BLOCK_POINTERS];
    let mut i = 0usize;
    while i < EXT2_INODE_BLOCK_POINTERS {
        blocks[i] = r32(p + 40 + i * 4);
        i += 1;
    }
    let inode = Ext2Inode {
        mode: r16(p + 0),
        size: r32(p + 4),
        block: blocks,
    };
    store_cached_inode(runtime, inode_no, inode);
    Ok(inode)
}

fn write_inode(
    runtime: &mut Ext2Runtime,
    inode_no: u32,
    inode: Ext2Inode,
) -> Result<(), RequestError> {
    if inode_no == 0 || runtime.superblock.inodes_per_group == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let sectors = data_sectors(runtime, inode)?;
    let index = inode_no as usize - 1;
    if index >= runtime.superblock.inodes_count as usize {
        return Err(RequestError::InvalidArgument);
    }
    let group_index = index / runtime.superblock.inodes_per_group as usize;
    if group_index >= runtime.group_count {
        return Err(RequestError::InvalidArgument);
    }
    let index_in_group = index % runtime.superblock.inodes_per_group as usize;
    let inode_offset = index_in_group * runtime.superblock.inode_size;
    let block =
        runtime.groups[group_index].inode_table_block as usize + inode_offset / runtime.block_size;
    let in_block = inode_offset % runtime.block_size;
    read_block(runtime, block)?;
    let p = runtime.block_shm as usize + in_block;
    w16_mem(p + 0, inode.mode);
    w32_mem(p + 4, inode.size);
    w32_mem(p + 24, sectors);
    let mut i = 0usize;
    while i < EXT2_INODE_BLOCK_POINTERS {
        w32_mem(p + 40 + i * 4, inode.block[i]);
        i += 1;
    }
    write_block(runtime, block)?;
    invalidate_cached_inode(runtime, inode_no);
    store_cached_inode(runtime, inode_no, inode);
    update_cached_handle_metadata(runtime, inode_no, inode);
    Ok(())
}

fn indirect_entries_per_block(runtime: &Ext2Runtime) -> usize {
    runtime.block_size / 4
}

fn max_file_blocks(runtime: &Ext2Runtime) -> usize {
    let entries = indirect_entries_per_block(runtime);
    EXT2_MAX_DIRECT_BLOCKS + entries + entries * entries
}

fn get_data_block(
    runtime: &mut Ext2Runtime,
    inode: Ext2Inode,
    logical_block: usize,
) -> Result<u32, RequestError> {
    if logical_block < EXT2_MAX_DIRECT_BLOCKS {
        return Ok(inode.block[logical_block]);
    }
    let indirect_index = logical_block - EXT2_MAX_DIRECT_BLOCKS;
    let entries = indirect_entries_per_block(runtime);
    if indirect_index < entries {
        let indirect_block = inode.block[EXT2_SINGLE_INDIRECT_INDEX];
        if indirect_block == 0 {
            return Ok(0);
        }
        read_block(runtime, indirect_block as usize)?;
        return Ok(r32(runtime.block_shm as usize + indirect_index * 4));
    }

    let double_index = indirect_index - entries;
    if double_index >= entries * entries {
        return Err(RequestError::Unsupported);
    }
    let double_block = inode.block[EXT2_DOUBLE_INDIRECT_INDEX];
    if double_block == 0 {
        return Ok(0);
    }
    let first_index = double_index / entries;
    let second_index = double_index % entries;
    read_block(runtime, double_block as usize)?;
    let indirect_block = r32(runtime.block_shm as usize + first_index * 4);
    if indirect_block == 0 {
        return Ok(0);
    }
    read_block(runtime, indirect_block as usize)?;
    Ok(r32(runtime.block_shm as usize + second_index * 4))
}

fn ensure_data_block(
    runtime: &mut Ext2Runtime,
    inode: &mut Ext2Inode,
    logical_block: usize,
) -> Result<u32, RequestError> {
    if logical_block < EXT2_MAX_DIRECT_BLOCKS {
        if inode.block[logical_block] == 0 {
            inode.block[logical_block] = alloc_block(runtime)? as u32;
            zero_block(runtime, inode.block[logical_block] as usize)?;
        }
        return Ok(inode.block[logical_block]);
    }
    let indirect_index = logical_block - EXT2_MAX_DIRECT_BLOCKS;
    let entries = indirect_entries_per_block(runtime);
    if indirect_index < entries {
        if inode.block[EXT2_SINGLE_INDIRECT_INDEX] == 0 {
            inode.block[EXT2_SINGLE_INDIRECT_INDEX] = alloc_block(runtime)? as u32;
            zero_block(runtime, inode.block[EXT2_SINGLE_INDIRECT_INDEX] as usize)?;
        }
        let indirect_block = inode.block[EXT2_SINGLE_INDIRECT_INDEX] as usize;
        read_block(runtime, indirect_block)?;
        let entry_addr = runtime.block_shm as usize + indirect_index * 4;
        let mut block = r32(entry_addr);
        if block == 0 {
            block = alloc_block(runtime)? as u32;
            w32_mem(entry_addr, block);
            write_block(runtime, indirect_block)?;
            zero_block(runtime, block as usize)?;
        }
        return Ok(block);
    }

    let double_index = indirect_index - entries;
    if double_index >= entries * entries {
        return Err(RequestError::Unsupported);
    }
    if inode.block[EXT2_DOUBLE_INDIRECT_INDEX] == 0 {
        inode.block[EXT2_DOUBLE_INDIRECT_INDEX] = alloc_block(runtime)? as u32;
        zero_block(runtime, inode.block[EXT2_DOUBLE_INDIRECT_INDEX] as usize)?;
    }
    let double_block = inode.block[EXT2_DOUBLE_INDIRECT_INDEX] as usize;
    let first_index = double_index / entries;
    let second_index = double_index % entries;

    read_block(runtime, double_block)?;
    let first_entry_addr = runtime.block_shm as usize + first_index * 4;
    let mut indirect_block = r32(first_entry_addr);
    if indirect_block == 0 {
        indirect_block = alloc_block(runtime)? as u32;
        w32_mem(first_entry_addr, indirect_block);
        write_block(runtime, double_block)?;
        zero_block(runtime, indirect_block as usize)?;
    }

    let indirect_block = indirect_block as usize;
    read_block(runtime, indirect_block)?;
    let entry_addr = runtime.block_shm as usize + second_index * 4;
    let mut block = r32(entry_addr);
    if block == 0 {
        block = alloc_block(runtime)? as u32;
        w32_mem(entry_addr, block);
        write_block(runtime, indirect_block)?;
        zero_block(runtime, block as usize)?;
    }
    Ok(block)
}

fn zero_block(runtime: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    read_block(runtime, block)?;
    unsafe {
        ptr::write_bytes(runtime.block_shm as *mut u8, 0, runtime.block_size);
    }
    write_block(runtime, block)
}

fn lookup_path(
    runtime: &mut Ext2Runtime,
    session: ClientSession,
    path_offset: usize,
    path_len: usize,
) -> Option<u32> {
    if path_len == 0 || path_offset + path_len > session.shm_size as usize {
        return None;
    }
    let path = (session.shm_local as usize + path_offset) as *const u8;
    unsafe {
        if *path != b'/' {
            return None;
        }
        if path_len == 1 {
            return Some(EXT2_ROOT_INODE);
        }
        let mut current = EXT2_ROOT_INODE;
        let mut component_start = 1usize;
        while component_start < path_len {
            while component_start < path_len && *path.add(component_start) == b'/' {
                component_start += 1;
            }
            if component_start >= path_len {
                break;
            }
            let mut component_end = component_start;
            while component_end < path_len && *path.add(component_end) != b'/' {
                component_end += 1;
            }
            let component_len = component_end - component_start;
            if component_len == 0 || component_len > EXT2_NAME_LEN_MAX {
                return None;
            }
            current =
                lookup_in_directory(runtime, current, path.add(component_start), component_len)?;
            component_start = component_end;
        }
        Some(current)
    }
}

fn lookup_parent_path(
    runtime: &mut Ext2Runtime,
    session: ClientSession,
    path_offset: usize,
    path_len: usize,
) -> Option<(u32, *const u8, usize)> {
    if path_len <= 1 || path_offset + path_len > session.shm_size as usize {
        return None;
    }
    let path = (session.shm_local as usize + path_offset) as *const u8;
    unsafe {
        if *path != b'/' {
            return None;
        }
        let mut end = path_len;
        while end > 1 && *path.add(end - 1) == b'/' {
            end -= 1;
        }
        let mut last_slash = end - 1;
        while last_slash > 0 && *path.add(last_slash) != b'/' {
            last_slash -= 1;
        }
        let name_start = last_slash + 1;
        let name_len = end - name_start;
        if name_len == 0 || name_len > EXT2_NAME_LEN_MAX {
            return None;
        }
        let parent_inode = if last_slash == 0 {
            EXT2_ROOT_INODE
        } else {
            lookup_path(runtime, session, path_offset, last_slash)?
        };
        Some((parent_inode, path.add(name_start), name_len))
    }
}

unsafe fn lookup_in_directory(
    runtime: &mut Ext2Runtime,
    dir_inode: u32,
    name: *const u8,
    name_len: usize,
) -> Option<u32> {
    if let Some(cached) = load_cached_dentry(runtime, dir_inode, name, name_len) {
        return cached;
    }
    let inode = read_inode(runtime, dir_inode).ok()?;
    if (inode.mode & EXT2_S_IFDIR) != EXT2_S_IFDIR {
        return None;
    }
    let mut logical_block = 0usize;
    while logical_block < max_file_blocks(runtime) {
        let block = get_data_block(runtime, inode, logical_block).ok()?;
        if block == 0 {
            break;
        }
        read_block(runtime, block as usize).ok()?;
        let limit = min(
            runtime.block_size,
            inode
                .size
                .saturating_sub((logical_block * runtime.block_size) as u32) as usize,
        );
        let mut offset = 0usize;
        while offset < limit {
            let entry = parse_directory_entry(runtime.block_shm as usize + offset, limit - offset)?;
            if entry.inode != 0
                && entry.name_len == name_len
                && bytes_eq(entry.name_ptr, name, name_len)
            {
                store_cached_dentry(runtime, dir_inode, name, name_len, Some(entry.inode));
                return Some(entry.inode);
            }
            offset += entry.record_len;
        }
        logical_block += 1;
    }
    store_cached_dentry(runtime, dir_inode, name, name_len, None);
    None
}

fn read_directory(
    runtime: &mut Ext2Runtime,
    session: ClientSession,
    inode: Ext2Inode,
    start_index: usize,
    max_entries: usize,
    out_offset: usize,
) -> Result<(usize, usize), Word> {
    let record_bytes = nanami_services::vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES;
    let max_by_buffer = if out_offset <= session.shm_size as usize {
        (session.shm_size as usize - out_offset) / record_bytes
    } else {
        0
    };
    let limit_entries = min(max_entries, max_by_buffer);
    if limit_entries == 0 {
        return Ok((0, 0));
    }
    let mut seen = 0usize;
    let mut written = 0usize;
    let mut logical_block = 0usize;
    while logical_block < max_file_blocks(runtime) && written < limit_entries {
        let block = get_data_block(runtime, inode, logical_block)
            .map_err(|_| libnanami::OS_RESPONSE_INVALID_DESCRIPTOR)?;
        if block == 0 {
            break;
        }
        if read_block(runtime, block as usize).is_err() {
            return Err(libnanami::OS_RESPONSE_INVALID_DESCRIPTOR);
        }
        let block_file_offset = logical_block * runtime.block_size;
        if block_file_offset >= inode.size as usize {
            break;
        }
        let block_limit = min(runtime.block_size, inode.size as usize - block_file_offset);
        let mut offset = 0usize;
        while offset < block_limit && written < limit_entries {
            let Some(entry) =
                parse_directory_entry(runtime.block_shm as usize + offset, block_limit - offset)
            else {
                return Err(libnanami::OS_RESPONSE_INVALID_DESCRIPTOR);
            };
            if entry.inode != 0 {
                if seen >= start_index {
                    unsafe {
                        write_vfs_dirent(
                            (session.shm_local as usize + out_offset + written * record_bytes)
                                as *mut u8,
                            entry,
                        );
                    }
                    written += 1;
                }
                seen += 1;
            }
            offset += entry.record_len;
        }
        logical_block += 1;
    }
    Ok((written, seen))
}

fn parse_directory_entry(addr: usize, available: usize) -> Option<Ext2DirectoryEntry> {
    if available < 8 {
        return None;
    }
    let record_len = r16(addr + 4) as usize;
    let name_len = unsafe { *((addr + 6) as *const u8) as usize };
    let file_type = unsafe { *((addr + 7) as *const u8) };
    if record_len < 8
        || record_len > available
        || name_len > EXT2_NAME_LEN_MAX
        || 8 + name_len > record_len
    {
        return None;
    }
    Some(Ext2DirectoryEntry {
        inode: r32(addr + 0),
        record_len,
        name_len,
        file_type,
        name_ptr: (addr + 8) as *const u8,
    })
}

unsafe fn write_vfs_dirent(dst: *mut u8, entry: Ext2DirectoryEntry) {
    write_word(
        dst.add(nanami_services::vfs::VFS_DIRECTORY_ENTRY_INODE_OFFSET),
        entry.inode as Word,
    );
    write_word(
        dst.add(nanami_services::vfs::VFS_DIRECTORY_ENTRY_TYPE_OFFSET),
        ext2_dirent_file_type_to_vfs(entry.file_type),
    );
    write_word(
        dst.add(nanami_services::vfs::VFS_DIRECTORY_ENTRY_NAME_LEN_OFFSET),
        entry.name_len as Word,
    );
    write_word(
        dst.add(nanami_services::vfs::VFS_DIRECTORY_ENTRY_RECORD_LEN_OFFSET),
        nanami_services::vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES as Word,
    );
    ptr::write_bytes(
        dst.add(nanami_services::vfs::VFS_DIRECTORY_ENTRY_NAME_OFFSET),
        0,
        nanami_services::vfs::VFS_DIRECTORY_ENTRY_NAME_BYTES,
    );
    ptr::copy_nonoverlapping(
        entry.name_ptr,
        dst.add(nanami_services::vfs::VFS_DIRECTORY_ENTRY_NAME_OFFSET),
        min(
            entry.name_len,
            nanami_services::vfs::VFS_DIRECTORY_ENTRY_NAME_BYTES,
        ),
    );
}

fn read_file(
    runtime: &mut Ext2Runtime,
    session: ClientSession,
    inode: Ext2Inode,
    file_offset: usize,
    len: usize,
    out_offset: usize,
) -> Result<usize, Word> {
    if out_offset
        .checked_add(len)
        .filter(|end| *end <= session.shm_size as usize)
        .is_none()
    {
        return Err(libnanami::OS_RESPONSE_INVALID_ARGUMENT);
    }
    let file_size = inode.size as usize;
    if file_offset >= file_size || len == 0 {
        return Ok(0);
    }
    let mut remaining = min(len, file_size - file_offset);
    let mut copied = 0usize;
    while remaining > 0 {
        let logical_block = (file_offset + copied) / runtime.block_size;
        let Ok(physical_block) = get_data_block(runtime, inode, logical_block) else {
            return Err(libnanami::OS_RESPONSE_ILLEGAL_OPERATION);
        };
        if physical_block == 0 {
            break;
        }
        let block_offset = (file_offset + copied) % runtime.block_size;
        let block_capacity = runtime.block_shm_size as usize / runtime.block_size;
        if block_capacity == 0 {
            return Err(libnanami::OS_RESPONSE_INVALID_ARGUMENT);
        }
        let needed_blocks =
            (block_offset + remaining + runtime.block_size - 1) / runtime.block_size;
        let max_blocks = min(needed_blocks, block_capacity);
        let mut run_blocks = 1usize;
        while run_blocks < max_blocks {
            let Ok(next_block) = get_data_block(runtime, inode, logical_block + run_blocks) else {
                break;
            };
            if next_block == 0 || next_block as usize != physical_block as usize + run_blocks {
                break;
            }
            run_blocks += 1;
        }
        if read_blocks(runtime, physical_block as usize, run_blocks).is_err() {
            return Err(libnanami::OS_RESPONSE_INVALID_DESCRIPTOR);
        }
        let n = min(remaining, run_blocks * runtime.block_size - block_offset);
        unsafe {
            ptr::copy_nonoverlapping(
                (runtime.block_shm as usize + block_offset) as *const u8,
                (session.shm_local as usize + out_offset + copied) as *mut u8,
                n,
            );
        }
        copied += n;
        remaining -= n;
    }
    Ok(copied)
}

fn write_file(
    runtime: &mut Ext2Runtime,
    session: ClientSession,
    inode: &mut Ext2Inode,
    inode_no: u32,
    file_offset: usize,
    len: usize,
    input_offset: usize,
) -> Result<usize, Word> {
    if file_offset.checked_add(len).is_none() {
        return Err(libnanami::OS_RESPONSE_INVALID_ARGUMENT);
    }
    let end = file_offset + len;
    if end > runtime.block_size * max_file_blocks(runtime) {
        return Err(libnanami::OS_RESPONSE_ILLEGAL_OPERATION);
    }
    let mut copied = 0usize;
    while copied < len {
        let logical_block = (file_offset + copied) / runtime.block_size;
        let block_offset = (file_offset + copied) % runtime.block_size;
        let n = min(len - copied, runtime.block_size - block_offset);
        let physical_block = ensure_data_block(runtime, inode, logical_block)
            .map_err(|_| libnanami::OS_RESPONSE_FATAL)? as usize;
        read_block(runtime, physical_block)
            .map_err(|_| libnanami::OS_RESPONSE_INVALID_DESCRIPTOR)?;
        unsafe {
            ptr::copy_nonoverlapping(
                (session.shm_local as usize + input_offset + copied) as *const u8,
                (runtime.block_shm as usize + block_offset) as *mut u8,
                n,
            );
        }
        write_block(runtime, physical_block).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
        copied += n;
    }
    if end > inode.size as usize {
        inode.size = end as u32;
    }
    write_inode(runtime, inode_no, *inode).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
    Ok(copied)
}

fn create_node(
    runtime: &mut Ext2Runtime,
    parent_inode: u32,
    name: *const u8,
    name_len: usize,
    is_directory: bool,
) -> Result<u32, Word> {
    let inode_no = alloc_inode(runtime).map_err(|_| libnanami::OS_RESPONSE_FATAL)? as u32;
    let mut inode = Ext2Inode {
        mode: if is_directory { 0x41ed } else { 0x81a4 },
        size: if is_directory {
            runtime.block_size as u32
        } else {
            0
        },
        block: [0; EXT2_INODE_BLOCK_POINTERS],
    };
    if is_directory {
        let block = alloc_block(runtime).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
        inode.block[0] = block as u32;
        read_block(runtime, block).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
        unsafe {
            ptr::write_bytes(runtime.block_shm as *mut u8, 0, runtime.block_size);
        }
        write_ext2_directory_entry(
            runtime.block_shm as usize,
            inode_no,
            12,
            1,
            EXT2_FT_DIR,
            b".",
        );
        write_ext2_directory_entry(
            runtime.block_shm as usize + 12,
            parent_inode,
            (runtime.block_size - 12) as u16,
            2,
            EXT2_FT_DIR,
            b"..",
        );
        write_block(runtime, block).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
    }
    write_inode(runtime, inode_no, inode).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
    insert_directory_entry(
        runtime,
        parent_inode,
        inode_no,
        name,
        name_len,
        if is_directory {
            EXT2_FT_DIR
        } else {
            EXT2_FT_REG_FILE
        },
    )?;
    Ok(inode_no)
}

fn remove_node(
    runtime: &mut Ext2Runtime,
    parent_inode: u32,
    name: *const u8,
    name_len: usize,
) -> Result<(), Word> {
    let (inode_no, block, offset) = find_directory_entry(runtime, parent_inode, name, name_len)
        .ok_or(libnanami::OS_RESPONSE_INVALID_DESCRIPTOR)?;
    let inode =
        read_inode(runtime, inode_no).map_err(|_| libnanami::OS_RESPONSE_INVALID_DESCRIPTOR)?;
    if (inode.mode & EXT2_S_IFDIR) == EXT2_S_IFDIR && !is_directory_empty(runtime, inode)? {
        return Err(libnanami::OS_RESPONSE_ILLEGAL_OPERATION);
    }
    read_block(runtime, block).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
    w32_mem(runtime.block_shm as usize + offset, 0);
    write_block(runtime, block).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
    unsafe {
        invalidate_cached_dentry(runtime, parent_inode, name, name_len);
        store_cached_dentry(runtime, parent_inode, name, name_len, None);
    }
    free_inode_blocks(runtime, inode).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
    free_inode(runtime, inode_no as usize).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
    invalidate_cached_inode(runtime, inode_no);
    invalidate_cached_dentries_for_inode(runtime, inode_no);
    Ok(())
}

fn rename_node(
    runtime: &mut Ext2Runtime,
    old_parent: u32,
    old_name: *const u8,
    old_name_len: usize,
    new_parent: u32,
    new_name: *const u8,
    new_name_len: usize,
) -> Result<(), Word> {
    let (inode_no, old_block, old_offset) =
        find_directory_entry(runtime, old_parent, old_name, old_name_len)
            .ok_or(libnanami::OS_RESPONSE_INVALID_DESCRIPTOR)?;
    let inode =
        read_inode(runtime, inode_no).map_err(|_| libnanami::OS_RESPONSE_INVALID_DESCRIPTOR)?;
    let file_type = if (inode.mode & EXT2_S_IFDIR) == EXT2_S_IFDIR {
        EXT2_FT_DIR
    } else {
        EXT2_FT_REG_FILE
    };
    if file_type == EXT2_FT_DIR && is_directory_ancestor(runtime, inode_no, new_parent)? {
        return Err(libnanami::OS_RESPONSE_ILLEGAL_OPERATION);
    }
    insert_directory_entry(
        runtime,
        new_parent,
        inode_no,
        new_name,
        new_name_len,
        file_type,
    )?;
    read_block(runtime, old_block).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
    w32_mem(runtime.block_shm as usize + old_offset, 0);
    write_block(runtime, old_block).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
    unsafe {
        invalidate_cached_dentry(runtime, old_parent, old_name, old_name_len);
        store_cached_dentry(runtime, old_parent, old_name, old_name_len, None);
    }
    if file_type == EXT2_FT_DIR && old_parent != new_parent {
        update_parent_directory_entry(runtime, inode_no, new_parent)?;
    }
    Ok(())
}

fn session_for_pid(runtime: &mut Ext2Runtime, pid: Word) -> Option<usize> {
    let mut free = None;
    let mut i = 0usize;
    while i < runtime.sessions.len() {
        if runtime.sessions[i].active && runtime.sessions[i].pid == pid {
            return Some(i);
        }
        if !runtime.sessions[i].active && free.is_none() {
            free = Some(i);
        }
        i += 1;
    }
    free
}

fn find_delegated_session(runtime: &Ext2Runtime, owner_pid: Word, peer_pid: Word) -> Option<usize> {
    runtime.delegated_sessions.iter().position(|session| {
        session.active && session.owner_pid == owner_pid && session.peer_pid == peer_pid
    })
}

fn free_delegated_session(runtime: &Ext2Runtime) -> Option<usize> {
    runtime
        .delegated_sessions
        .iter()
        .position(|session| !session.active)
}

fn insert_directory_entry(
    runtime: &mut Ext2Runtime,
    dir_inode_no: u32,
    child_inode: u32,
    name: *const u8,
    name_len: usize,
    file_type: u8,
) -> Result<(), Word> {
    let mut dir_inode =
        read_inode(runtime, dir_inode_no).map_err(|_| libnanami::OS_RESPONSE_INVALID_DESCRIPTOR)?;
    if (dir_inode.mode & EXT2_S_IFDIR) != EXT2_S_IFDIR || dir_inode.block[0] == 0 {
        return Err(libnanami::OS_RESPONSE_INVALID_DESCRIPTOR);
    }
    let needed = align4(8 + name_len);
    let mut logical_block = 0usize;
    while logical_block < max_file_blocks(runtime) {
        let mut block = get_data_block(runtime, dir_inode, logical_block)
            .map_err(|_| libnanami::OS_RESPONSE_INVALID_DESCRIPTOR)?
            as usize;
        if block == 0 {
            block = ensure_data_block(runtime, &mut dir_inode, logical_block)
                .map_err(|_| libnanami::OS_RESPONSE_FATAL)? as usize;
            dir_inode.size = ((logical_block + 1) * runtime.block_size) as u32;
            write_inode(runtime, dir_inode_no, dir_inode)
                .map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
            read_block(runtime, block).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
            unsafe {
                write_ext2_directory_entry_from_ptr(
                    runtime.block_shm as usize,
                    child_inode,
                    runtime.block_size as u16,
                    name_len as u8,
                    file_type,
                    name,
                );
            }
            write_block(runtime, block).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
            unsafe {
                invalidate_cached_dentry(runtime, dir_inode_no, name, name_len);
                store_cached_dentry(runtime, dir_inode_no, name, name_len, Some(child_inode));
            }
            return Ok(());
        }
        read_block(runtime, block).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
        let mut offset = 0usize;
        while offset < runtime.block_size {
            let Some(entry) = parse_directory_entry(
                runtime.block_shm as usize + offset,
                runtime.block_size - offset,
            ) else {
                return Err(libnanami::OS_RESPONSE_INVALID_DESCRIPTOR);
            };
            let actual = if entry.inode == 0 {
                8
            } else {
                align4(8 + entry.name_len)
            };
            if entry.record_len >= actual + needed {
                let new_offset = if entry.inode == 0 {
                    offset
                } else {
                    offset + actual
                };
                let new_len = if entry.inode == 0 {
                    entry.record_len
                } else {
                    entry.record_len - actual
                };
                if entry.inode != 0 {
                    w16_mem(runtime.block_shm as usize + offset + 4, actual as u16);
                }
                unsafe {
                    write_ext2_directory_entry_from_ptr(
                        runtime.block_shm as usize + new_offset,
                        child_inode,
                        new_len as u16,
                        name_len as u8,
                        file_type,
                        name,
                    );
                }
                write_block(runtime, block).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
                unsafe {
                    invalidate_cached_dentry(runtime, dir_inode_no, name, name_len);
                    store_cached_dentry(runtime, dir_inode_no, name, name_len, Some(child_inode));
                }
                return Ok(());
            }
            offset += entry.record_len;
        }
        logical_block += 1;
    }
    Err(libnanami::OS_RESPONSE_ILLEGAL_OPERATION)
}

fn find_directory_entry(
    runtime: &mut Ext2Runtime,
    dir_inode_no: u32,
    name: *const u8,
    name_len: usize,
) -> Option<(u32, usize, usize)> {
    let inode = read_inode(runtime, dir_inode_no).ok()?;
    if (inode.mode & EXT2_S_IFDIR) != EXT2_S_IFDIR {
        return None;
    }
    let mut logical_block = 0usize;
    while logical_block < max_file_blocks(runtime) {
        let block = get_data_block(runtime, inode, logical_block).ok()? as usize;
        if block == 0 {
            break;
        }
        read_block(runtime, block).ok()?;
        let mut offset = 0usize;
        while offset < runtime.block_size {
            let entry = parse_directory_entry(
                runtime.block_shm as usize + offset,
                runtime.block_size - offset,
            )?;
            if entry.inode != 0
                && entry.name_len == name_len
                && unsafe { bytes_eq(entry.name_ptr, name, name_len) }
            {
                return Some((entry.inode, block, offset));
            }
            offset += entry.record_len;
        }
        logical_block += 1;
    }
    None
}

fn is_directory_empty(runtime: &mut Ext2Runtime, inode: Ext2Inode) -> Result<bool, Word> {
    let mut logical_block = 0usize;
    while logical_block < max_file_blocks(runtime) {
        let block = get_data_block(runtime, inode, logical_block)
            .map_err(|_| libnanami::OS_RESPONSE_FATAL)? as usize;
        if block == 0 {
            break;
        }
        read_block(runtime, block).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
        let mut offset = 0usize;
        while offset < runtime.block_size {
            let Some(entry) = parse_directory_entry(
                runtime.block_shm as usize + offset,
                runtime.block_size - offset,
            ) else {
                return Err(libnanami::OS_RESPONSE_INVALID_DESCRIPTOR);
            };
            if entry.inode != 0
                && !(entry.name_len == 1 && unsafe { *entry.name_ptr == b'.' })
                && !(entry.name_len == 2
                    && unsafe { *entry.name_ptr == b'.' && *entry.name_ptr.add(1) == b'.' })
            {
                return Ok(false);
            }
            offset += entry.record_len;
        }
        logical_block += 1;
    }
    Ok(true)
}

fn update_parent_directory_entry(
    runtime: &mut Ext2Runtime,
    directory_inode_no: u32,
    new_parent_inode_no: u32,
) -> Result<(), Word> {
    let inode = read_inode(runtime, directory_inode_no)
        .map_err(|_| libnanami::OS_RESPONSE_INVALID_DESCRIPTOR)?;
    if (inode.mode & EXT2_S_IFDIR) != EXT2_S_IFDIR {
        return Err(libnanami::OS_RESPONSE_INVALID_DESCRIPTOR);
    }
    let name = b"..";
    let mut logical_block = 0usize;
    while logical_block < max_file_blocks(runtime) {
        let block = get_data_block(runtime, inode, logical_block)
            .map_err(|_| libnanami::OS_RESPONSE_FATAL)? as usize;
        if block == 0 {
            break;
        }
        read_block(runtime, block).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
        let mut offset = 0usize;
        while offset < runtime.block_size {
            let Some(entry) = parse_directory_entry(
                runtime.block_shm as usize + offset,
                runtime.block_size - offset,
            ) else {
                return Err(libnanami::OS_RESPONSE_INVALID_DESCRIPTOR);
            };
            if entry.inode != 0
                && entry.name_len == name.len()
                && unsafe { bytes_eq(entry.name_ptr, name.as_ptr(), name.len()) }
            {
                w32_mem(runtime.block_shm as usize + offset, new_parent_inode_no);
                write_block(runtime, block).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
                unsafe {
                    invalidate_cached_dentry(
                        runtime,
                        directory_inode_no,
                        name.as_ptr(),
                        name.len(),
                    );
                    store_cached_dentry(
                        runtime,
                        directory_inode_no,
                        name.as_ptr(),
                        name.len(),
                        Some(new_parent_inode_no),
                    );
                }
                return Ok(());
            }
            offset += entry.record_len;
        }
        logical_block += 1;
    }
    Err(libnanami::OS_RESPONSE_INVALID_DESCRIPTOR)
}

fn is_directory_ancestor(
    runtime: &mut Ext2Runtime,
    ancestor_inode_no: u32,
    mut inode_no: u32,
) -> Result<bool, Word> {
    let mut depth = 0usize;
    while depth < runtime.superblock.inodes_per_group as usize {
        if inode_no == ancestor_inode_no {
            return Ok(true);
        }
        if inode_no == EXT2_ROOT_INODE {
            return Ok(false);
        }
        inode_no = parent_directory_inode(runtime, inode_no)?;
        depth += 1;
    }
    Err(libnanami::OS_RESPONSE_INVALID_DESCRIPTOR)
}

fn parent_directory_inode(runtime: &mut Ext2Runtime, inode_no: u32) -> Result<u32, Word> {
    let inode =
        read_inode(runtime, inode_no).map_err(|_| libnanami::OS_RESPONSE_INVALID_DESCRIPTOR)?;
    if (inode.mode & EXT2_S_IFDIR) != EXT2_S_IFDIR {
        return Err(libnanami::OS_RESPONSE_INVALID_DESCRIPTOR);
    }
    let name = b"..";
    let mut logical_block = 0usize;
    while logical_block < max_file_blocks(runtime) {
        let block = get_data_block(runtime, inode, logical_block)
            .map_err(|_| libnanami::OS_RESPONSE_FATAL)? as usize;
        if block == 0 {
            break;
        }
        read_block(runtime, block).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
        let mut offset = 0usize;
        while offset < runtime.block_size {
            let Some(entry) = parse_directory_entry(
                runtime.block_shm as usize + offset,
                runtime.block_size - offset,
            ) else {
                return Err(libnanami::OS_RESPONSE_INVALID_DESCRIPTOR);
            };
            if entry.inode != 0
                && entry.name_len == name.len()
                && unsafe { bytes_eq(entry.name_ptr, name.as_ptr(), name.len()) }
            {
                return Ok(entry.inode);
            }
            offset += entry.record_len;
        }
        logical_block += 1;
    }
    Err(libnanami::OS_RESPONSE_INVALID_DESCRIPTOR)
}

fn alloc_inode(runtime: &mut Ext2Runtime) -> Result<usize, RequestError> {
    let inodes_per_group = runtime.superblock.inodes_per_group as usize;
    let mut group = 0usize;
    while group < runtime.group_count {
        let start_bit = if group == 0 {
            EXT2_GOOD_OLD_FIRST_INO
        } else {
            0
        };
        let remaining =
            (runtime.superblock.inodes_count as usize).saturating_sub(group * inodes_per_group);
        let limit = min(inodes_per_group, remaining);
        if let Some(bit) = alloc_bitmap_bit(
            runtime,
            runtime.groups[group].inode_bitmap_block as usize,
            start_bit,
            limit,
        )? {
            adjust_free_count(runtime, group, true, -1)?;
            return Ok(group * inodes_per_group + bit + 1);
        }
        group += 1;
    }
    Err(RequestError::Unsupported)
}

fn free_inode(runtime: &mut Ext2Runtime, inode: usize) -> Result<(), RequestError> {
    if inode == 0 || inode > runtime.superblock.inodes_count as usize {
        return Err(RequestError::InvalidArgument);
    }
    let group = (inode - 1) / runtime.superblock.inodes_per_group as usize;
    let bit = (inode - 1) % runtime.superblock.inodes_per_group as usize;
    clear_bitmap_bit(
        runtime,
        runtime.groups[group].inode_bitmap_block as usize,
        bit,
    )?;
    invalidate_cached_inode(runtime, inode as u32);
    invalidate_cached_dentries_for_inode(runtime, inode as u32);
    adjust_free_count(runtime, group, true, 1)
}

fn alloc_block(runtime: &mut Ext2Runtime) -> Result<usize, RequestError> {
    let blocks_per_group = runtime.superblock.blocks_per_group as usize;
    let mut group = 0usize;
    while group < runtime.group_count {
        let limit = runtime.groups[group].block_count as usize;
        if let Some(bit) = alloc_bitmap_bit(
            runtime,
            runtime.groups[group].block_bitmap_block as usize,
            0,
            limit,
        )? {
            adjust_free_count(runtime, group, false, -1)?;
            return Ok(group * blocks_per_group + bit);
        }
        group += 1;
    }
    Err(RequestError::Unsupported)
}

fn free_block(runtime: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    if block >= runtime.block_count {
        return Err(RequestError::InvalidArgument);
    }
    let blocks_per_group = runtime.superblock.blocks_per_group as usize;
    let group = block / blocks_per_group;
    if group >= runtime.group_count {
        return Err(RequestError::InvalidArgument);
    }
    let bit = block - group * blocks_per_group;
    clear_bitmap_bit(
        runtime,
        runtime.groups[group].block_bitmap_block as usize,
        bit,
    )?;
    adjust_free_count(runtime, group, false, 1)
}

fn free_inode_blocks(runtime: &mut Ext2Runtime, inode: Ext2Inode) -> Result<(), RequestError> {
    let mut i = 0usize;
    while i < EXT2_MAX_DIRECT_BLOCKS {
        if inode.block[i] != 0 {
            free_block(runtime, inode.block[i] as usize)?;
        }
        i += 1;
    }
    if inode.block[EXT2_SINGLE_INDIRECT_INDEX] != 0 {
        let indirect_block = inode.block[EXT2_SINGLE_INDIRECT_INDEX] as usize;
        let mut entry = 0usize;
        while entry < indirect_entries_per_block(runtime) {
            read_block(runtime, indirect_block)?;
            let block = r32(runtime.block_shm as usize + entry * 4);
            if block != 0 {
                free_block(runtime, block as usize)?;
            }
            entry += 1;
        }
        free_block(runtime, indirect_block)?;
    }
    if inode.block[EXT2_DOUBLE_INDIRECT_INDEX] != 0 {
        let double_block = inode.block[EXT2_DOUBLE_INDIRECT_INDEX] as usize;
        let entries = indirect_entries_per_block(runtime);
        let mut first = 0usize;
        while first < entries {
            read_block(runtime, double_block)?;
            let indirect_block = r32(runtime.block_shm as usize + first * 4);
            if indirect_block != 0 {
                let indirect_block = indirect_block as usize;
                let mut second = 0usize;
                while second < entries {
                    read_block(runtime, indirect_block)?;
                    let block = r32(runtime.block_shm as usize + second * 4);
                    if block != 0 {
                        free_block(runtime, block as usize)?;
                    }
                    second += 1;
                }
                free_block(runtime, indirect_block)?;
            }
            first += 1;
        }
        free_block(runtime, double_block)?;
    }
    Ok(())
}

fn truncate_inode(
    runtime: &mut Ext2Runtime,
    inode_no: u32,
    inode: &mut Ext2Inode,
) -> Result<(), Word> {
    free_inode_blocks(runtime, *inode).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
    inode.size = 0;
    inode.block = [0; EXT2_INODE_BLOCK_POINTERS];
    write_inode(runtime, inode_no, *inode).map_err(|_| libnanami::OS_RESPONSE_FATAL)?;
    let mut i = 1usize;
    while i < runtime.handles.len() {
        if runtime.handles[i].active && runtime.handles[i].inode == inode_no {
            runtime.handles[i].size = 0;
        }
        i += 1;
    }
    Ok(())
}

fn alloc_bitmap_bit(
    runtime: &mut Ext2Runtime,
    bitmap_block: usize,
    start_bit: usize,
    limit_bits: usize,
) -> Result<Option<usize>, RequestError> {
    read_block(runtime, bitmap_block)?;
    let base = runtime.block_shm as usize;
    let mut bit = start_bit;
    while bit < limit_bits {
        let byte = unsafe { *((base + bit / 8) as *const u8) };
        let mask = 1u8 << (bit % 8);
        if byte & mask == 0 {
            unsafe {
                *((base + bit / 8) as *mut u8) = byte | mask;
            }
            write_block(runtime, bitmap_block)?;
            return Ok(Some(bit));
        }
        bit += 1;
    }
    Ok(None)
}

fn clear_bitmap_bit(
    runtime: &mut Ext2Runtime,
    bitmap_block: usize,
    bit: usize,
) -> Result<(), RequestError> {
    read_block(runtime, bitmap_block)?;
    let addr = runtime.block_shm as usize + bit / 8;
    let byte = unsafe { *(addr as *const u8) };
    unsafe {
        *(addr as *mut u8) = byte & !(1u8 << (bit % 8));
    }
    write_block(runtime, bitmap_block)
}

fn adjust_free_count(
    runtime: &mut Ext2Runtime,
    group: usize,
    is_inode: bool,
    delta: i32,
) -> Result<(), RequestError> {
    let (super_offset, group_offset) = if is_inode {
        (16usize, 14usize)
    } else {
        (12usize, 12usize)
    };
    read_block(runtime, 1)?;
    let super_addr = runtime.block_shm as usize + super_offset;
    w32_mem(super_addr, add_signed_u32(r32(super_addr), delta));
    write_block(runtime, 1)?;

    let gd_block = if runtime.block_size == 1024 { 2 } else { 1 };
    read_block(runtime, gd_block)?;
    let group_addr = runtime.block_shm as usize + group * 32 + group_offset;
    w16_mem(group_addr, add_signed_u16(r16(group_addr), delta));
    write_block(runtime, gd_block)
}

fn find_session(runtime: &Ext2Runtime, pid: Word) -> Option<ClientSession> {
    let mut i = 0usize;
    while i < runtime.sessions.len() {
        let s = runtime.sessions[i];
        if s.active && s.pid == pid {
            return Some(s);
        }
        i += 1;
    }
    None
}

fn alloc_handle(
    runtime: &mut Ext2Runtime,
    owner_pid: Word,
    inode_no: u32,
    inode: Ext2Inode,
) -> Option<usize> {
    let mut i = 1usize;
    while i < runtime.handles.len() {
        if !runtime.handles[i].active {
            runtime.handles[i] = FileHandle {
                active: true,
                owner_pid,
                inode: inode_no,
                size: inode.size,
                mode: inode.mode,
            };
            return Some(i);
        }
        i += 1;
    }
    None
}

fn has_available_handle(runtime: &Ext2Runtime) -> bool {
    let mut i = 1usize;
    while i < runtime.handles.len() {
        if !runtime.handles[i].active {
            return true;
        }
        i += 1;
    }
    false
}

fn file_kind(mode: u16) -> Word {
    if (mode & EXT2_S_IFDIR) == EXT2_S_IFDIR {
        nanami_services::vfs::VFS_FILE_TYPE_DIRECTORY
    } else if (mode & EXT2_S_IFREG) == EXT2_S_IFREG {
        nanami_services::vfs::VFS_FILE_TYPE_REGULAR
    } else {
        nanami_services::vfs::VFS_FILE_TYPE_UNKNOWN
    }
}

fn ext2_dirent_file_type_to_vfs(file_type: u8) -> Word {
    match file_type {
        1 => nanami_services::vfs::VFS_FILE_TYPE_REGULAR,
        2 => nanami_services::vfs::VFS_FILE_TYPE_DIRECTORY,
        _ => nanami_services::vfs::VFS_FILE_TYPE_UNKNOWN,
    }
}

fn pack_stat(size: Word, kind: Word) -> Word {
    (size & nanami_services::vfs::VFS_STAT_SIZE_MASK)
        | (kind << nanami_services::vfs::VFS_STAT_TYPE_SHIFT)
}

fn align4(value: usize) -> usize {
    (value + 3) & !3
}

fn add_signed_u32(value: u32, delta: i32) -> u32 {
    if delta >= 0 {
        value.saturating_add(delta as u32)
    } else {
        value.saturating_sub((-delta) as u32)
    }
}

fn add_signed_u16(value: u16, delta: i32) -> u16 {
    if delta >= 0 {
        value.saturating_add(delta as u16)
    } else {
        value.saturating_sub((-delta) as u16)
    }
}

fn data_sectors(runtime: &mut Ext2Runtime, inode: Ext2Inode) -> Result<u32, RequestError> {
    let mut blocks = 0u32;
    let mut i = 0usize;
    while i < EXT2_MAX_DIRECT_BLOCKS {
        if inode.block[i] != 0 {
            blocks += 1;
        }
        i += 1;
    }
    if inode.block[EXT2_SINGLE_INDIRECT_INDEX] != 0 {
        blocks += 1; // the indirect block itself
        let indirect_block = inode.block[EXT2_SINGLE_INDIRECT_INDEX] as usize;
        read_block(runtime, indirect_block)?;
        let mut entry = 0usize;
        while entry < indirect_entries_per_block(runtime) {
            if r32(runtime.block_shm as usize + entry * 4) != 0 {
                blocks += 1;
            }
            entry += 1;
        }
    }
    if inode.block[EXT2_DOUBLE_INDIRECT_INDEX] != 0 {
        blocks += 1; // the double indirect block itself
        let double_block = inode.block[EXT2_DOUBLE_INDIRECT_INDEX] as usize;
        let entries = indirect_entries_per_block(runtime);
        let mut first = 0usize;
        while first < entries {
            read_block(runtime, double_block)?;
            let indirect_block = r32(runtime.block_shm as usize + first * 4);
            if indirect_block != 0 {
                blocks += 1; // second-level indirect block
                let indirect_block = indirect_block as usize;
                let mut second = 0usize;
                while second < entries {
                    read_block(runtime, indirect_block)?;
                    if r32(runtime.block_shm as usize + second * 4) != 0 {
                        blocks += 1;
                    }
                    second += 1;
                }
            }
            first += 1;
        }
    }
    Ok(blocks * ((runtime.block_size / 512) as u32))
}

unsafe fn bytes_eq(a: *const u8, b: *const u8, len: usize) -> bool {
    let mut i = 0usize;
    while i < len {
        if *a.add(i) != *b.add(i) {
            return false;
        }
        i += 1;
    }
    true
}

unsafe fn write_word(dst: *mut u8, value: Word) {
    let mut i = 0usize;
    while i < core::mem::size_of::<Word>() {
        *dst.add(i) = (value >> (i * 8)) as u8;
        i += 1;
    }
}

fn write_ext2_directory_entry(
    offset: usize,
    inode: u32,
    record_len: u16,
    name_len: u8,
    file_type: u8,
    name: &[u8],
) {
    w32_mem(offset + 0, inode);
    w16_mem(offset + 4, record_len);
    unsafe {
        *((offset + 6) as *mut u8) = name_len;
        *((offset + 7) as *mut u8) = file_type;
        ptr::copy_nonoverlapping(name.as_ptr(), (offset + 8) as *mut u8, name_len as usize);
    }
}

unsafe fn write_ext2_directory_entry_from_ptr(
    offset: usize,
    inode: u32,
    record_len: u16,
    name_len: u8,
    file_type: u8,
    name: *const u8,
) {
    w32_mem(offset + 0, inode);
    w16_mem(offset + 4, record_len);
    *((offset + 6) as *mut u8) = name_len;
    *((offset + 7) as *mut u8) = file_type;
    ptr::copy_nonoverlapping(name, (offset + 8) as *mut u8, name_len as usize);
}

fn r16(addr: usize) -> u16 {
    unsafe { (*(addr as *const u8) as u16) | ((*(addr as *const u8).add(1) as u16) << 8) }
}

fn r32(addr: usize) -> u32 {
    unsafe {
        (*(addr as *const u8) as u32)
            | ((*(addr as *const u8).add(1) as u32) << 8)
            | ((*(addr as *const u8).add(2) as u32) << 16)
            | ((*(addr as *const u8).add(3) as u32) << 24)
    }
}

fn w16_mem(addr: usize, value: u16) {
    unsafe {
        *(addr as *mut u8) = value as u8;
        *((addr + 1) as *mut u8) = (value >> 8) as u8;
    }
}

fn w32_mem(addr: usize, value: u32) {
    unsafe {
        *(addr as *mut u8) = value as u8;
        *((addr + 1) as *mut u8) = (value >> 8) as u8;
        *((addr + 2) as *mut u8) = (value >> 16) as u8;
        *((addr + 3) as *mut u8) = (value >> 24) as u8;
    }
}

fn log_error(prefix: &str, e: RequestError) -> libnanami::NanamiError {
    log_request_error(prefix, e);
    e.into()
}

fn log_request_error(prefix: &str, e: RequestError) {
    libnanami::print!("{}{}\n", prefix, e);
}

fn map_request_error_to_status(e: RequestError) -> Word {
    match e {
        RequestError::InvalidArgument => libnanami::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Transport | RequestError::Protocol => libnanami::OS_RESPONSE_FATAL,
        RequestError::Status(status) => status,
    }
}

libnanami::nanami_entry!(nanami_main);
