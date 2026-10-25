#![no_std]
#![no_main]

extern crate alloc;
mod block_cache;
mod writeback;

mod data_io;
use data_io::*;

mod block_io;
use block_io::*;

mod block_allocation;
use block_allocation::*;

mod file_io;
use file_io::*;
mod directory;
use directory::{parse_directory_entry, read_directory};
mod truncate;

use core::cmp::min;
use core::ptr;
use nanami_runtime::ipc::{ServiceEvent, ServiceRequest};
use nanami_runtime::{self, RequestError, Word};

mod block_connection;
use block_connection::connect_block_device;

const SLOT_SERVICE_PORT: Word = 20;
const SLOT_BLOCK_DEVICE: Word = 23;
const SLOT_TIMER_SERVICE: Word = 24;
const BLOCK_SHM_BYTES: Word = nanami_client::block::BLOCK_DEVICE_DEFAULT_SHM_BYTES;
const BLOCK_READ_RETRY_LIMIT: usize = 4;
const MAX_SESSIONS: usize = 16;
const MAX_DELEGATED_SESSIONS: usize = 16;
const MAX_HANDLES: usize = 64;
const EXT2_MAGIC: u16 = 0xef53;
const EXT2_ROOT_INODE: u32 = 2;
const EXT2_GOOD_OLD_FIRST_INO: usize = 11;
const EXT2_S_IFDIR: u16 = 0x4000;
const EXT2_S_IFREG: u16 = 0x8000;
const EXT2_S_IFMT: u16 = 0xf000;
const EXT2_S_IFSOCK: u16 = 0xc000;
const EXT2_NAME_LEN_MAX: usize = 255;
const EXT2_MAX_BLOCK_GROUPS: usize = 32;
const EXT2_MAX_DIRECT_BLOCKS: usize = 12;
const EXT2_SINGLE_INDIRECT_INDEX: usize = 12;
const EXT2_DOUBLE_INDIRECT_INDEX: usize = 13;
const EXT2_INODE_BLOCK_POINTERS: usize = 15;
// Payload budget, independent of filesystem block size; allocated on the heap.
const EXT2_BLOCK_CACHE_BUDGET: usize = 1024 * 1024;
const EXT2_INODE_CACHE_ENTRIES: usize = 64;
const EXT2_DENTRY_CACHE_ENTRIES: usize = 128;
const EXT2_FT_REG_FILE: u8 = 1;
const EXT2_FT_DIR: u8 = 2;
const EXT2_FT_SOCK: u8 = 6;

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
    // A frozen filesystem must not reclaim an unlinked inode on close.
    close_pending: bool,
    owner_pid: Word,
    inode: u32,
    size: u32,
    mode: u16,
}

impl FileHandle {
    const EMPTY: Self = Self {
        active: false,
        close_pending: false,
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
    links_count: u16,
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
            links_count: 0,
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
    block_cache: block_cache::BlockCache,
    writeback: writeback::Writeback,
    frozen_by: Option<Word>,
    inode_cache: [CachedInode; EXT2_INODE_CACHE_ENTRIES],
    inode_cache_next: usize,
    dentry_cache: [CachedDentry; EXT2_DENTRY_CACHE_ENTRIES],
    dentry_cache_next: usize,
    directory_cursors: directory::Cursors,
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::print!("[ext2-server] panic\n");
    loop {}
}

fn nanami_main() -> nanami_runtime::NanamiResult {
    nanami_runtime::print!("[ext2-server] bootstrap\n");
    let (block_port, timer_port) = connect_block_device()
        .map_err(|e| log_error("[ext2-server] block-device unavailable: ", e))?;
    let (block_shm, block_shm_size) =
        nanami_client::block::block_device_attach_shared_memory(block_port, BLOCK_SHM_BYTES)
            .map_err(|e| log_error("[ext2-server] block shm attach failed: ", e))?;
    let (block_size, block_count) = nanami_client::block::block_device_info(block_port)
        .map_err(|e| log_error("[ext2-server] block info failed: ", e))?;
    if block_size == 0 || block_size > block_shm_size || block_shm_size > EXT2_BLOCK_CACHE_BUDGET {
        return Err(log_error(
            "[ext2-server] invalid block geometry: ",
            RequestError::Protocol,
        ));
    }

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
        block_cache: block_cache::BlockCache::new(
            block_size,
            EXT2_BLOCK_CACHE_BUDGET,
            block_shm_size,
        ),
        writeback: writeback::Writeback::new(timer_port),
        frozen_by: None,
        inode_cache: [CachedInode::EMPTY; EXT2_INODE_CACHE_ENTRIES],
        inode_cache_next: 0,
        dentry_cache: [CachedDentry::EMPTY; EXT2_DENTRY_CACHE_ENTRIES],
        dentry_cache_next: 0,
        directory_cursors: directory::Cursors::new(),
    };
    mount_ext2(&mut runtime).map_err(|e| log_error("[ext2-server] mount failed: ", e))?;
    writeback::initialize(&mut runtime)
        .map_err(|e| log_error("[ext2-server] writeback init failed: ", e))?;

    nanami_client::registry::register_vfs_service()
        .map_err(|e| log_error("[ext2-server] register failed: ", e))?;
    nanami_runtime::print!("[ext2-server] service registered: vfs-service\n");

    let service_port = nanami_runtime::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let mut pending = (nanami_runtime::OS_RESPONSE_OK, 0, 0);
    let mut has_reply = false;
    let mut request_sequence = 0usize;
    let mut trace_reply = None;
    nanami_runtime::diagnostics::clock_info("vfs");
    loop {
        let event = if has_reply {
            has_reply = false;
            if option_env!("NANAMI_BOOT_IO_TRACE") == Some("1") {
                nanami_runtime::println!(
                    "[vfs.trace] reply-receive seq={} status={:#x}",
                    request_sequence,
                    pending.0
                );
            }
            match nanami_runtime::ipc::service_reply_receive_event(
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
            match nanami_runtime::ipc::service_receive_event(service_port) {
                Ok(event) => event,
                Err(e) => return Err(log_error("[ext2-server] receive failed: ", e)),
            }
        };

        // Report a captured handler span only after sending its reply. Combined
        // Reply-Receive duration includes idle time and must NOT be called IPC latency.
        if let Some((before, after, bytes, error)) = trace_reply.take() {
            nanami_runtime::diagnostics::rpc(
                nanami_runtime::diagnostics::RpcStage::VfsServe,
                before,
                after,
                bytes,
                error,
            );
        }
        match event {
            ServiceEvent::Request(request) => {
                let trace_before = nanami_runtime::diagnostics::stamp();
                if option_env!("NANAMI_BOOT_IO_TRACE") == Some("1") {
                    request_sequence = request_sequence.wrapping_add(1);
                    nanami_runtime::println!(
                        "[vfs.trace] request seq={} pid={} code={:#x} arg0={:#x} arg1={:#x}",
                        request_sequence,
                        request.identifier,
                        request.code,
                        request.arg0,
                        request.arg1
                    );
                }
                pending = handle_request(request, &mut runtime);
                if option_env!("NANAMI_BOOT_IO_TRACE") == Some("1") {
                    nanami_runtime::println!(
                        "[vfs.trace] handled seq={} status={:#x}",
                        request_sequence,
                        pending.0
                    );
                }
                if let Err(error) = writeback::after_request(&mut runtime) {
                    pending = (map_request_error_to_status(error), 0, 0);
                }
                if nanami_runtime::diagnostics::ENABLED {
                    trace_reply = Some((
                        trace_before,
                        nanami_runtime::diagnostics::stamp(),
                        if matches!(
                            request.code,
                            nanami_client::vfs::VFS_REQUEST_READ
                                | nanami_client::vfs::VFS_REQUEST_READ_DELEGATED
                        ) {
                            pending.1
                        } else {
                            0
                        },
                        pending.0 != nanami_runtime::OS_RESPONSE_OK,
                    ));
                }
                has_reply = true;
            }
            ServiceEvent::Notification { .. } => {
                runtime.writeback.armed = false;
                if let Err(error) = flush_blocks(&mut runtime) {
                    log_request_error("[ext2-server] writeback failed (writes disabled): ", error);
                }
            }
            ServiceEvent::Fault {
                identifier, reason, ..
            } => {
                nanami_runtime::print!("[ext2-server] fault id={}", identifier);
                nanami_runtime::print!(" reason={:#x}\n", reason);
            }
        }
    }
}

fn handle_request(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    if matches!(
        request.code,
        nanami_client::vfs::VFS_REQUEST_OPEN_COMPOUND
            | nanami_client::vfs::VFS_REQUEST_CREATE
            | nanami_client::vfs::VFS_REQUEST_MKDIR
            | nanami_client::vfs::VFS_REQUEST_WRITE
            | nanami_client::vfs::VFS_REQUEST_WRITE_DELEGATED
            | nanami_client::vfs::VFS_REQUEST_REMOVE
            | nanami_client::vfs::VFS_REQUEST_LINK
            | nanami_client::vfs::VFS_REQUEST_RENAME
            | nanami_client::vfs::VFS_REQUEST_FTRUNCATE
    ) {
        if let Err(error) = writeback::check_writable(runtime) {
            return (map_request_error_to_status(error), 0, 0);
        }
    }
    match request.code {
        nanami_client::vfs::VFS_REQUEST_FREEZE | nanami_client::vfs::VFS_REQUEST_THAW => {
            writeback::handle_freeze(request, runtime)
        }
        nanami_client::vfs::VFS_REQUEST_FSYNC | nanami_client::vfs::VFS_REQUEST_SYNC => {
            writeback::handle_sync(request, runtime)
        }
        nanami_client::vfs::VFS_REQUEST_CONTROL => handle_control(request, runtime),
        nanami_client::vfs::VFS_REQUEST_OPEN => handle_open(request, runtime),
        nanami_client::vfs::VFS_REQUEST_OPEN_COMPOUND => handle_open_compound(request, runtime),
        nanami_client::vfs::VFS_REQUEST_READ => handle_read(request, runtime),
        nanami_client::vfs::VFS_REQUEST_READ_DELEGATED => handle_read_delegated(request, runtime),
        nanami_client::vfs::VFS_REQUEST_STAT => handle_stat(request, runtime),
        nanami_client::vfs::VFS_REQUEST_CLOSE => handle_close(request, runtime),
        nanami_client::vfs::VFS_REQUEST_READ_DIR => handle_read_dir(request, runtime),
        nanami_client::vfs::VFS_REQUEST_FSTAT => handle_fstat(request, runtime),
        nanami_client::vfs::VFS_REQUEST_CREATE => handle_create(request, runtime, false),
        nanami_client::vfs::VFS_REQUEST_MKDIR => handle_create(request, runtime, true),
        nanami_client::vfs::VFS_REQUEST_WRITE => handle_write_file(request, runtime),
        nanami_client::vfs::VFS_REQUEST_WRITE_DELEGATED => handle_write_delegated(request, runtime),
        nanami_client::vfs::VFS_REQUEST_REMOVE => handle_remove(request, runtime),
        nanami_client::vfs::VFS_REQUEST_LINK => handle_link(request, runtime),
        nanami_client::vfs::VFS_REQUEST_RENAME => handle_rename(request, runtime),
        nanami_client::vfs::VFS_REQUEST_FTRUNCATE => handle_ftruncate(request, runtime),
        _ => (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_control(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    match request.arg0 {
        nanami_client::vfs::VFS_CONTROL_ATTACH_SHARED_MEMORY => {
            let size = if request.arg1 == 0 {
                nanami_client::vfs::VFS_DEFAULT_SHM_BYTES
            } else {
                request.arg1
            };
            match nanami_runtime::request_shared_memory(request.identifier, size) {
                Ok((local, peer)) => match session_for_pid(runtime, request.identifier) {
                    Some(index) => {
                        runtime.sessions[index] = ClientSession {
                            active: true,
                            pid: request.identifier,
                            shm_local: local,
                            shm_size: size,
                        };
                        (nanami_runtime::OS_RESPONSE_OK, peer, size)
                    }
                    None => (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
                },
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_client::vfs::VFS_CONTROL_ATTACH_DELEGATED_SHARED_MEMORY => {
            if request.arg1 == 0 || find_session(runtime, request.identifier).is_none() {
                return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            let size = if request.arg2 == 0 {
                nanami_client::vfs::VFS_DEFAULT_SHM_BYTES
            } else {
                request.arg2
            };
            if size > nanami_client::vfs::VFS_DELEGATE_VALUE_MASK {
                return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            if let Some(index) = find_delegated_session(runtime, request.identifier, request.arg1) {
                let session = runtime.delegated_sessions[index];
                return delegated_attach_response(index, session.peer_vaddr, session.shm_size);
            }
            let Some(index) = free_delegated_session(runtime) else {
                return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            };
            match nanami_runtime::request_shared_memory(request.arg1, size) {
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
        _ => (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn delegated_attach_response(index: usize, peer_vaddr: Word, size: Word) -> (Word, Word, Word) {
    let delegate_id = index as Word + 1;
    let packed = (delegate_id << nanami_client::vfs::VFS_DELEGATE_ID_SHIFT)
        | (size & nanami_client::vfs::VFS_DELEGATE_VALUE_MASK);
    (nanami_runtime::OS_RESPONSE_OK, peer_vaddr, packed)
}

fn handle_open(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let Some(session) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some(inode_no) = lookup_path(
        runtime,
        session,
        request.arg0 as usize,
        request.arg1 as usize,
    ) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    let Ok(inode) = read_inode(runtime, inode_no) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    match alloc_handle(runtime, request.identifier, inode_no, inode) {
        Some(handle) => (nanami_runtime::OS_RESPONSE_OK, handle as Word, 0),
        None => (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_open_compound(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let flags = request.arg2;
    let supported_flags = nanami_client::vfs::VFS_OPEN_CREATE
        | nanami_client::vfs::VFS_OPEN_TRUNCATE
        | nanami_client::vfs::VFS_OPEN_DIRECTORY;
    if (flags & !supported_flags) != 0 || !has_available_handle(runtime) {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let Some(session) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let path_offset = request.arg0 as usize;
    let path_len = request.arg1 as usize;
    let inode_no = match lookup_path(runtime, session, path_offset, path_len) {
        Some(inode_no) => inode_no,
        None if (flags & nanami_client::vfs::VFS_OPEN_CREATE) != 0 => {
            if (flags & nanami_client::vfs::VFS_OPEN_DIRECTORY) != 0 {
                return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
            }
            let Some((parent_inode, name_ptr, name_len)) =
                lookup_parent_path(runtime, session, path_offset, path_len)
            else {
                return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
            };
            match create_node(runtime, parent_inode, name_ptr, name_len, EXT2_S_IFREG) {
                Ok(inode_no) => inode_no,
                Err(status) => return (status, 0, 0),
            }
        }
        None => return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0),
    };
    let Ok(mut inode) = read_inode(runtime, inode_no) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    let kind = file_kind(inode.mode);
    if (flags & nanami_client::vfs::VFS_OPEN_DIRECTORY) != 0
        && kind != nanami_client::vfs::VFS_FILE_TYPE_DIRECTORY
    {
        return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    }
    if (flags & nanami_client::vfs::VFS_OPEN_TRUNCATE) != 0 {
        if kind == nanami_client::vfs::VFS_FILE_TYPE_DIRECTORY {
            return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
        }
        if let Err(error) = truncate::resize_inode(runtime, inode_no, &mut inode, 0) {
            return (map_request_error_to_status(error), 0, 0);
        }
    }
    match alloc_handle(runtime, request.identifier, inode_no, inode) {
        Some(handle) => {
            let opened = ((inode_no as Word) << nanami_client::vfs::VFS_OPEN_INODE_SHIFT)
                | (handle as Word & nanami_client::vfs::VFS_OPEN_HANDLE_MASK);
            (
                nanami_runtime::OS_RESPONSE_OK,
                opened,
                pack_stat(inode.size as Word, kind),
            )
        }
        None => (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_stat(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let Some(session) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    if option_env!("NANAMI_BOOT_IO_TRACE") == Some("1") {
        nanami_runtime::println!("[vfs.trace] stat lookup begin");
    }
    let Some(inode_no) = lookup_path(
        runtime,
        session,
        request.arg0 as usize,
        request.arg1 as usize,
    ) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    if option_env!("NANAMI_BOOT_IO_TRACE") == Some("1") {
        nanami_runtime::println!("[vfs.trace] stat inode begin inode={}", inode_no);
    }
    let Ok(inode) = read_inode(runtime, inode_no) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    (
        nanami_runtime::OS_RESPONSE_OK,
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
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let file = runtime.handles[handle];
    (
        nanami_runtime::OS_RESPONSE_OK,
        file.inode as Word,
        pack_stat(file.size as Word, file_kind(file.mode)),
    )
}

fn handle_create(
    request: ServiceRequest,
    runtime: &mut Ext2Runtime,
    is_directory: bool,
) -> (Word, Word, Word) {
    let Some(session) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((parent_inode, name_ptr, name_len)) = lookup_parent_path(
        runtime,
        session,
        request.arg0 as usize,
        request.arg1 as usize,
    ) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    if unsafe { lookup_in_directory(runtime, parent_inode, name_ptr, name_len).is_some() } {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let mode = if is_directory {
        EXT2_S_IFDIR
    } else if request.arg2 == nanami_client::vfs::VFS_FILE_TYPE_SOCKET {
        EXT2_S_IFSOCK
    } else if request.arg2 == 0 {
        EXT2_S_IFREG
    } else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    match create_node(runtime, parent_inode, name_ptr, name_len, mode) {
        Ok(inode) => (nanami_runtime::OS_RESPONSE_OK, inode as Word, 0),
        Err(status) => (status, 0, 0),
    }
}

fn handle_remove(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let Some(session) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((parent_inode, name_ptr, name_len)) = lookup_parent_path(
        runtime,
        session,
        request.arg0 as usize,
        request.arg1 as usize,
    ) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    match remove_node(runtime, parent_inode, name_ptr, name_len) {
        Ok(()) => (nanami_runtime::OS_RESPONSE_OK, 0, 0),
        Err(status) => (status, 0, 0),
    }
}

fn handle_link(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let Some(session) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((old_parent, old_name, old_name_len)) = lookup_parent_path(
        runtime,
        session,
        request.arg0 as usize,
        request.arg1 as usize,
    ) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    let Some((new_parent, new_name, new_name_len)) = lookup_parent_path(
        runtime,
        session,
        request.arg2 as usize,
        request.arg3 as usize,
    ) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    if unsafe { lookup_in_directory(runtime, new_parent, new_name, new_name_len).is_some() } {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    match link_node(
        runtime,
        old_parent,
        old_name,
        old_name_len,
        new_parent,
        new_name,
        new_name_len,
    ) {
        Ok(()) => (nanami_runtime::OS_RESPONSE_OK, 0, 0),
        Err(status) => (status, 0, 0),
    }
}

fn handle_rename(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    let Some(session) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((old_parent, old_name, old_name_len)) = lookup_parent_path(
        runtime,
        session,
        request.arg0 as usize,
        request.arg1 as usize,
    ) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    let Some((new_parent, new_name, new_name_len)) = lookup_parent_path(
        runtime,
        session,
        request.arg2 as usize,
        request.arg3 as usize,
    ) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    if old_parent == new_parent
        && old_name_len == new_name_len
        && unsafe { bytes_eq(old_name, new_name, old_name_len) }
    {
        return (nanami_runtime::OS_RESPONSE_OK, 0, 0);
    }
    if unsafe { lookup_in_directory(runtime, new_parent, new_name, new_name_len).is_some() } {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
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
        Ok(()) => (nanami_runtime::OS_RESPONSE_OK, 0, 0),
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
    nanami_runtime::print!("[ext2-server] mounted ext2 block-size={}", block_size);
    nanami_runtime::print!(
        " groups={} inode-table={}\n",
        runtime.group_count,
        runtime.groups[0].inode_table_block as usize
    );
    Ok(())
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
        links_count: r16(p + 26),
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
    w32_mem(p + 28, sectors);
    w16_mem(p + 26, inode.links_count);
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
    if (inode.mode & EXT2_S_IFMT) != EXT2_S_IFDIR {
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


fn create_node(
    runtime: &mut Ext2Runtime,
    parent_inode: u32,
    name: *const u8,
    name_len: usize,
    file_type: u16,
) -> Result<u32, Word> {
    let is_directory = file_type == EXT2_S_IFDIR;
    let inode_no = alloc_inode(runtime).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)? as u32;
    let mut inode = Ext2Inode {
        mode: file_type | if is_directory { 0o755 } else { 0o644 },
        size: if is_directory {
            runtime.block_size as u32
        } else {
            0
        },
        links_count: if is_directory { 2 } else { 1 },
        block: [0; EXT2_INODE_BLOCK_POINTERS],
    };
    if is_directory {
        let block = alloc_block(runtime).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
        inode.block[0] = block as u32;
        read_block(runtime, block).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
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
        write_block(runtime, block).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
    }
    write_inode(runtime, inode_no, inode).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
    insert_directory_entry(
        runtime,
        parent_inode,
        inode_no,
        name,
        name_len,
        ext2_file_type(inode.mode),
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
        .ok_or(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)?;
    let mut inode = read_inode(runtime, inode_no)
        .map_err(|_| nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)?;
    if (inode.mode & EXT2_S_IFMT) == EXT2_S_IFDIR && !is_directory_empty(runtime, inode)? {
        return Err(nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION);
    }
    read_block(runtime, block).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
    w32_mem(runtime.block_shm as usize + offset, 0);
    write_block(runtime, block).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
    unsafe {
        invalidate_cached_dentry(runtime, parent_inode, name, name_len);
        store_cached_dentry(runtime, parent_inode, name, name_len, None);
    }
    let pinned_socket = (inode.mode & EXT2_S_IFMT) == EXT2_S_IFSOCK
        && runtime.handles.iter().any(|handle| {
            (handle.active || handle.close_pending) && handle.inode == inode_no
        });
    if (inode.mode & EXT2_S_IFMT) != EXT2_S_IFDIR && (inode.links_count > 1 || pinned_socket) {
        inode.links_count -= 1;
        write_inode(runtime, inode_no, inode).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
    } else {
        free_inode_blocks(runtime, inode).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
        free_inode(runtime, inode_no as usize).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
        invalidate_cached_inode(runtime, inode_no);
        invalidate_cached_dentries_for_inode(runtime, inode_no);
    }
    Ok(())
}

fn release_socket_inode(runtime: &mut Ext2Runtime, inode_no: u32) -> Result<(), RequestError> {
    if runtime.handles.iter().any(|handle| {
        (handle.active || handle.close_pending) && handle.inode == inode_no
    }) {
        return Ok(());
    }
    let inode = read_inode(runtime, inode_no)?;
    if inode.links_count == 0 {
        writeback::check_writable(runtime)?;
        free_inode_blocks(runtime, inode)?;
        free_inode(runtime, inode_no as usize)?;
        invalidate_cached_inode(runtime, inode_no);
        invalidate_cached_dentries_for_inode(runtime, inode_no);
    }
    Ok(())
}

fn link_node(
    runtime: &mut Ext2Runtime,
    old_parent: u32,
    old_name: *const u8,
    old_name_len: usize,
    new_parent: u32,
    new_name: *const u8,
    new_name_len: usize,
) -> Result<(), Word> {
    let (inode_no, _, _) = find_directory_entry(runtime, old_parent, old_name, old_name_len)
        .ok_or(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)?;
    let mut inode = read_inode(runtime, inode_no)
        .map_err(|_| nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)?;
    if (inode.mode & EXT2_S_IFMT) == EXT2_S_IFDIR {
        return Err(nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION);
    }
    inode.links_count = inode
        .links_count
        .checked_add(1)
        .ok_or(nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION)?;
    write_inode(runtime, inode_no, inode).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
    if let Err(status) = insert_directory_entry(
        runtime,
        new_parent,
        inode_no,
        new_name,
        new_name_len,
        ext2_file_type(inode.mode),
    ) {
        inode.links_count -= 1;
        let _ = write_inode(runtime, inode_no, inode);
        return Err(status);
    }
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
            .ok_or(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)?;
    let inode = read_inode(runtime, inode_no)
        .map_err(|_| nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)?;
    let file_type = ext2_file_type(inode.mode);
    if file_type == EXT2_FT_DIR && is_directory_ancestor(runtime, inode_no, new_parent)? {
        return Err(nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION);
    }
    insert_directory_entry(
        runtime,
        new_parent,
        inode_no,
        new_name,
        new_name_len,
        file_type,
    )?;
    read_block(runtime, old_block).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
    w32_mem(runtime.block_shm as usize + old_offset, 0);
    write_block(runtime, old_block).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
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
    let mut dir_inode = read_inode(runtime, dir_inode_no)
        .map_err(|_| nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)?;
    if (dir_inode.mode & EXT2_S_IFMT) != EXT2_S_IFDIR || dir_inode.block[0] == 0 {
        return Err(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR);
    }
    let needed = align4(8 + name_len);
    let mut logical_block = 0usize;
    while logical_block < max_file_blocks(runtime) {
        let mut block = get_data_block(runtime, dir_inode, logical_block)
            .map_err(|_| nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)?
            as usize;
        if block == 0 {
            block = ensure_data_block(runtime, &mut dir_inode, logical_block)
                .map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)? as usize;
            dir_inode.size = ((logical_block + 1) * runtime.block_size) as u32;
            write_inode(runtime, dir_inode_no, dir_inode)
                .map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
            read_block(runtime, block).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
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
            write_block(runtime, block).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
            unsafe {
                invalidate_cached_dentry(runtime, dir_inode_no, name, name_len);
                store_cached_dentry(runtime, dir_inode_no, name, name_len, Some(child_inode));
            }
            return Ok(());
        }
        read_block(runtime, block).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
        let mut offset = 0usize;
        while offset < runtime.block_size {
            let Some(entry) = parse_directory_entry(
                runtime.block_shm as usize + offset,
                runtime.block_size - offset,
            ) else {
                return Err(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR);
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
                write_block(runtime, block).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
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
    Err(nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION)
}

fn find_directory_entry(
    runtime: &mut Ext2Runtime,
    dir_inode_no: u32,
    name: *const u8,
    name_len: usize,
) -> Option<(u32, usize, usize)> {
    let inode = read_inode(runtime, dir_inode_no).ok()?;
    if (inode.mode & EXT2_S_IFMT) != EXT2_S_IFDIR {
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
            .map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)? as usize;
        if block == 0 {
            break;
        }
        read_block(runtime, block).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
        let mut offset = 0usize;
        while offset < runtime.block_size {
            let Some(entry) = parse_directory_entry(
                runtime.block_shm as usize + offset,
                runtime.block_size - offset,
            ) else {
                return Err(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR);
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
        .map_err(|_| nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)?;
    if (inode.mode & EXT2_S_IFMT) != EXT2_S_IFDIR {
        return Err(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR);
    }
    let name = b"..";
    let mut logical_block = 0usize;
    while logical_block < max_file_blocks(runtime) {
        let block = get_data_block(runtime, inode, logical_block)
            .map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)? as usize;
        if block == 0 {
            break;
        }
        read_block(runtime, block).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
        let mut offset = 0usize;
        while offset < runtime.block_size {
            let Some(entry) = parse_directory_entry(
                runtime.block_shm as usize + offset,
                runtime.block_size - offset,
            ) else {
                return Err(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR);
            };
            if entry.inode != 0
                && entry.name_len == name.len()
                && unsafe { bytes_eq(entry.name_ptr, name.as_ptr(), name.len()) }
            {
                w32_mem(runtime.block_shm as usize + offset, new_parent_inode_no);
                write_block(runtime, block).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
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
    Err(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)
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
    Err(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)
}

fn parent_directory_inode(runtime: &mut Ext2Runtime, inode_no: u32) -> Result<u32, Word> {
    let inode = read_inode(runtime, inode_no)
        .map_err(|_| nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)?;
    if (inode.mode & EXT2_S_IFMT) != EXT2_S_IFDIR {
        return Err(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR);
    }
    let name = b"..";
    let mut logical_block = 0usize;
    while logical_block < max_file_blocks(runtime) {
        let block = get_data_block(runtime, inode, logical_block)
            .map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)? as usize;
        if block == 0 {
            break;
        }
        read_block(runtime, block).map_err(|_| nanami_runtime::OS_RESPONSE_FATAL)?;
        let mut offset = 0usize;
        while offset < runtime.block_size {
            let Some(entry) = parse_directory_entry(
                runtime.block_shm as usize + offset,
                runtime.block_size - offset,
            ) else {
                return Err(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR);
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
    Err(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)
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
        if !runtime.handles[i].active && !runtime.handles[i].close_pending {
            runtime.handles[i] = FileHandle {
                active: true,
                close_pending: false,
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
        if !runtime.handles[i].active && !runtime.handles[i].close_pending {
            return true;
        }
        i += 1;
    }
    false
}

fn file_kind(mode: u16) -> Word {
    match mode & EXT2_S_IFMT {
        EXT2_S_IFDIR => nanami_client::vfs::VFS_FILE_TYPE_DIRECTORY,
        EXT2_S_IFREG => nanami_client::vfs::VFS_FILE_TYPE_REGULAR,
        EXT2_S_IFSOCK => nanami_client::vfs::VFS_FILE_TYPE_SOCKET,
        _ => nanami_client::vfs::VFS_FILE_TYPE_UNKNOWN,
    }
}

fn ext2_file_type(mode: u16) -> u8 {
    match mode & EXT2_S_IFMT {
        EXT2_S_IFDIR => EXT2_FT_DIR,
        EXT2_S_IFREG => EXT2_FT_REG_FILE,
        EXT2_S_IFSOCK => EXT2_FT_SOCK,
        _ => 0,
    }
}

fn ext2_dirent_file_type_to_vfs(file_type: u8) -> Word {
    match file_type {
        1 => nanami_client::vfs::VFS_FILE_TYPE_REGULAR,
        2 => nanami_client::vfs::VFS_FILE_TYPE_DIRECTORY,
        EXT2_FT_SOCK => nanami_client::vfs::VFS_FILE_TYPE_SOCKET,
        _ => nanami_client::vfs::VFS_FILE_TYPE_UNKNOWN,
    }
}

fn pack_stat(size: Word, kind: Word) -> Word {
    (size & nanami_client::vfs::VFS_STAT_SIZE_MASK)
        | (kind << nanami_client::vfs::VFS_STAT_TYPE_SHIFT)
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

fn log_error(prefix: &str, e: RequestError) -> nanami_runtime::NanamiError {
    log_request_error(prefix, e);
    e.into()
}

fn log_request_error(prefix: &str, e: RequestError) {
    nanami_runtime::print!("{}{}\n", prefix, e);
}

fn map_request_error_to_status(e: RequestError) -> Word {
    match e {
        RequestError::InvalidArgument => nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Transport | RequestError::Protocol => nanami_runtime::OS_RESPONSE_FATAL,
        RequestError::Status(status) => status,
    }
}

nanami_runtime::nanami_entry!(nanami_main);
