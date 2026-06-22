#![no_std]
#![no_main]

use core::ptr;
use libnanami::ipc::{ServiceEvent, ServiceRequest};
use libnanami::{self, RequestError, Word};
use nanami_services::posix::*;

const SLOT_SERVICE_PORT: Word = 20;
const SLOT_VFS_SERVICE: Word = 23;
const VFS_SHM_BYTES: Word = 0x4000;
const MAX_SESSIONS: usize = 16;
const MAX_FDS: usize = 32;
const MAX_OPEN_FILES: usize = 128;
const PATH_MAX: usize = POSIX_PATH_MAX;
const MAX_COMPONENTS: usize = 16;
const VFS_PATH_OFFSET: usize = 0;
const VFS_PATH2_OFFSET: usize = 256;
const VFS_IO_OFFSET: usize = 512;

#[derive(Clone, Copy)]
struct Session {
    active: bool,
    owner_pid: Word,
    shm_local: Word,
    shm_size: Word,
    posix_pid: Word,
    posix_ppid: Word,
    posix_pgid: Word,
    posix_sid: Word,
    uid: Word,
    euid: Word,
    gid: Word,
    egid: Word,
    cwd: [u8; PATH_MAX],
    cwd_len: usize,
    fds: [FileDescriptor; MAX_FDS],
}

impl Session {
    const EMPTY: Self = Self {
        active: false,
        owner_pid: 0,
        shm_local: 0,
        shm_size: 0,
        posix_pid: 0,
        posix_ppid: 0,
        posix_pgid: 0,
        posix_sid: 0,
        uid: POSIX_ROOT_UID,
        euid: POSIX_ROOT_UID,
        gid: POSIX_ROOT_GID,
        egid: POSIX_ROOT_GID,
        cwd: [0; PATH_MAX],
        cwd_len: 0,
        fds: [FileDescriptor::EMPTY; MAX_FDS],
    };
}

#[derive(Clone, Copy)]
struct FileDescriptor {
    active: bool,
    open_file: usize,
    flags: Word,
}

impl FileDescriptor {
    const EMPTY: Self = Self {
        active: false,
        open_file: 0,
        flags: 0,
    };
}

#[derive(Clone, Copy)]
struct OpenFile {
    active: bool,
    kind: FdKind,
    offset: Word,
    vfs_handle: Word,
    ref_count: Word,
}

impl OpenFile {
    const EMPTY: Self = Self {
        active: false,
        kind: FdKind::Empty,
        offset: 0,
        vfs_handle: 0,
        ref_count: 0,
    };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FdKind {
    Empty,
    Regular,
    Directory,
    DevNull,
    DevZero,
}

struct Runtime {
    vfs_port: Word,
    vfs_shm: Word,
    vfs_shm_size: Word,
    sessions: [Session; MAX_SESSIONS],
    open_files: [OpenFile; MAX_OPEN_FILES],
    next_posix_pid: Word,
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[posix-server] panic\n");
    loop {}
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::print!("[posix-server] bootstrap\n");
    libnanami::ipc::init_ipc_tls()
        .map_err(|e| log_error("[posix-server] ipc tls init failed: ", e))?;

    let vfs_port = connect_vfs_service();
    let (vfs_shm, vfs_shm_size) = nanami_services::vfs::vfs_attach_shared_memory(vfs_port, VFS_SHM_BYTES)
        .map_err(|e| log_error("[posix-server] vfs shm attach failed: ", e))?;

    nanami_services::registry::register_posix_service()
        .map_err(|e| log_error("[posix-server] service register failed: ", e))?;
    libnanami::print!("[posix-server] service registered: posix-service\n");

    let mut runtime = Runtime {
        vfs_port,
        vfs_shm,
        vfs_shm_size,
        sessions: [Session::EMPTY; MAX_SESSIONS],
        open_files: [OpenFile::EMPTY; MAX_OPEN_FILES],
        next_posix_pid: 100,
    };

    let service_port = libnanami::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let mut pending = (libnanami::OS_RESPONSE_OK, 0, 0);
    let mut has_reply = false;

    loop {
        let event = if has_reply {
            has_reply = false;
            match libnanami::ipc::service_reply_receive_event(service_port, pending.0, pending.1, pending.2) {
                Ok(e) => e,
                Err(e) => return Err(log_error("[posix-server] reply_receive failed: ", e)),
            }
        } else {
            match libnanami::ipc::service_receive_event(service_port) {
                Ok(e) => e,
                Err(e) => return Err(log_error("[posix-server] receive failed: ", e)),
            }
        };

        match event {
            ServiceEvent::Request(request) => {
                pending = handle_request(&mut runtime, request);
                has_reply = true;
            }
            ServiceEvent::Notification { .. } => {}
            ServiceEvent::Fault { identifier, reason, .. } => {
                libnanami::println!("[posix-server] fault id={} reason={:#x}", identifier, reason);
            }
        }
    }
}

fn connect_vfs_service() -> Word {
    let mut tries = 0usize;
    loop {
        match nanami_services::registry::connect_vfs_service(SLOT_VFS_SERVICE) {
            Ok(()) => return libnanami::ipc::process_slot_descriptor(SLOT_VFS_SERVICE),
            Err(e) => {
                if tries == 0 {
                    log_request_error("[posix-server] waiting vfs-service: ", e);
                }
                tries += 1;
                let mut spin = 0usize;
                while spin < 200_000 {
                    core::hint::spin_loop();
                    spin += 1;
                }
            }
        }
    }
}

fn handle_request(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    match request.code {
        POSIX_REQUEST_CONTROL => handle_control(runtime, request),
        POSIX_REQUEST_GETPID => match session_for_pid(runtime, request.identifier) {
            Some(index) => (libnanami::OS_RESPONSE_OK, runtime.sessions[index].posix_pid, 0),
            None => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
        },
        POSIX_REQUEST_GETCWD => handle_getcwd(runtime, request),
        POSIX_REQUEST_CHDIR => handle_chdir(runtime, request),
        POSIX_REQUEST_OPEN => handle_open(runtime, request),
        POSIX_REQUEST_CLOSE => handle_close(runtime, request),
        POSIX_REQUEST_DUP => handle_dup(runtime, request),
        POSIX_REQUEST_DUP2 => handle_dup2(runtime, request),
        POSIX_REQUEST_FCNTL => handle_fcntl(runtime, request),
        POSIX_REQUEST_READ => handle_read(runtime, request),
        POSIX_REQUEST_WRITE => handle_write(runtime, request),
        POSIX_REQUEST_STAT => handle_stat(runtime, request),
        POSIX_REQUEST_MKDIR => handle_mkdir(runtime, request),
        POSIX_REQUEST_UNLINK => handle_unlink(runtime, request),
        POSIX_REQUEST_RENAME => handle_rename(runtime, request),
        POSIX_REQUEST_FSTAT => handle_fstat(runtime, request),
        POSIX_REQUEST_READ_DIR => handle_read_dir(runtime, request),
        POSIX_REQUEST_SEEK => handle_seek(runtime, request),
        POSIX_REQUEST_RMDIR => handle_rmdir(runtime, request),
        POSIX_REQUEST_GETPPID => handle_getppid(runtime, request),
        POSIX_REQUEST_GET_NATIVE_PID => handle_get_native_pid(runtime, request),
        POSIX_REQUEST_GETUID => handle_getuid(runtime, request),
        POSIX_REQUEST_GETEUID => handle_getuid(runtime, request),
        POSIX_REQUEST_GETGID => handle_getgid(runtime, request),
        POSIX_REQUEST_GETEGID => handle_getgid(runtime, request),
        POSIX_REQUEST_GETPGID => handle_getpgid(runtime, request),
        POSIX_REQUEST_GETSID => handle_getsid(runtime, request),
        POSIX_REQUEST_SETPGID => handle_setpgid(runtime, request),
        POSIX_REQUEST_SETSID => handle_setsid(runtime, request),
        POSIX_REQUEST_SPAWN => handle_spawn(runtime, request),
        POSIX_REQUEST_WAITPID => handle_waitpid(runtime, request),
        POSIX_REQUEST_FORK
        | POSIX_REQUEST_EXEC
        | POSIX_REQUEST_KILL => handle_unsupported_process_lifecycle(runtime, request),
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_control(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    match request.arg0 {
        POSIX_CONTROL_ATTACH_SHARED_MEMORY => {
            let size = if request.arg1 == 0 { POSIX_DEFAULT_SHM_BYTES } else { request.arg1 };
            match libnanami::request_shared_memory(request.identifier, size) {
                Ok((local, peer)) => match session_for_pid(runtime, request.identifier) {
                    Some(index) => {
                        runtime.sessions[index].shm_local = local;
                        runtime.sessions[index].shm_size = size;
                        (libnanami::OS_RESPONSE_OK, peer, size)
                    }
                    None => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
                },
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn session_for_pid(runtime: &mut Runtime, owner_pid: Word) -> Option<usize> {
    let mut empty = None;
    let mut i = 0usize;
    while i < runtime.sessions.len() {
        if runtime.sessions[i].active && runtime.sessions[i].owner_pid == owner_pid {
            return Some(i);
        }
        if !runtime.sessions[i].active && empty.is_none() {
            empty = Some(i);
        }
        i += 1;
    }
    let index = empty?;
    runtime.sessions[index] = Session::EMPTY;
    runtime.sessions[index].active = true;
    runtime.sessions[index].owner_pid = owner_pid;
    runtime.sessions[index].posix_pid = runtime.next_posix_pid;
    runtime.next_posix_pid = runtime.next_posix_pid.saturating_add(1);
    runtime.sessions[index].posix_ppid = POSIX_PROCESS_ROOT_PID;
    runtime.sessions[index].posix_pgid = runtime.sessions[index].posix_pid;
    runtime.sessions[index].posix_sid = POSIX_PROCESS_ROOT_PID;
    runtime.sessions[index].uid = POSIX_ROOT_UID;
    runtime.sessions[index].euid = POSIX_ROOT_UID;
    runtime.sessions[index].gid = POSIX_ROOT_GID;
    runtime.sessions[index].egid = POSIX_ROOT_GID;
    runtime.sessions[index].cwd[0] = b'/';
    runtime.sessions[index].cwd_len = 1;
    Some(index)
}

fn create_child_session(runtime: &mut Runtime, native_pid: Word, parent_index: usize) -> Option<usize> {
    let mut i = 0usize;
    while i < runtime.sessions.len() {
        if runtime.sessions[i].active && runtime.sessions[i].owner_pid == native_pid {
            inherit_child_session(runtime, i, parent_index);
            return Some(i);
        }
        i += 1;
    }

    let mut empty = None;
    i = 0;
    while i < runtime.sessions.len() {
        if !runtime.sessions[i].active {
            empty = Some(i);
            break;
        }
        i += 1;
    }

    let index = empty?;
    runtime.sessions[index] = Session::EMPTY;
    runtime.sessions[index].active = true;
    runtime.sessions[index].owner_pid = native_pid;
    runtime.sessions[index].posix_pid = runtime.next_posix_pid;
    runtime.next_posix_pid = runtime.next_posix_pid.saturating_add(1);
    inherit_child_session(runtime, index, parent_index);
    Some(index)
}

fn inherit_child_session(runtime: &mut Runtime, child_index: usize, parent_index: usize) {
    let parent = runtime.sessions[parent_index];
    release_session_fds(runtime, child_index);
    runtime.sessions[child_index].posix_ppid = parent.posix_pid;
    runtime.sessions[child_index].posix_pgid = parent.posix_pgid;
    runtime.sessions[child_index].posix_sid = parent.posix_sid;
    runtime.sessions[child_index].uid = parent.uid;
    runtime.sessions[child_index].euid = parent.euid;
    runtime.sessions[child_index].gid = parent.gid;
    runtime.sessions[child_index].egid = parent.egid;
    runtime.sessions[child_index].cwd = parent.cwd;
    runtime.sessions[child_index].cwd_len = parent.cwd_len;
    inherit_fds(runtime, parent_index, child_index);
}

fn inherit_fds(runtime: &mut Runtime, parent_index: usize, child_index: usize) {
    let mut fd = 0usize;
    while fd < MAX_FDS {
        let parent_fd = runtime.sessions[parent_index].fds[fd];
        if parent_fd.active && (parent_fd.flags & POSIX_FD_CLOEXEC) == 0 {
            let open_index = parent_fd.open_file;
            if open_index < runtime.open_files.len() && runtime.open_files[open_index].active {
                runtime.open_files[open_index].ref_count =
                    runtime.open_files[open_index].ref_count.saturating_add(1);
                runtime.sessions[child_index].fds[fd] = parent_fd;
            }
        }
        fd += 1;
    }
}

fn find_session(runtime: &mut Runtime, owner_pid: Word) -> Option<usize> {
    let mut i = 0usize;
    while i < runtime.sessions.len() {
        if runtime.sessions[i].active && runtime.sessions[i].owner_pid == owner_pid {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn handle_getppid(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    match session_for_pid(runtime, request.identifier) {
        Some(index) => (libnanami::OS_RESPONSE_OK, runtime.sessions[index].posix_ppid, 0),
        None => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_get_native_pid(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    match session_for_pid(runtime, request.identifier) {
        Some(_) => (libnanami::OS_RESPONSE_OK, request.identifier, 0),
        None => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_getuid(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    match session_for_pid(runtime, request.identifier) {
        Some(index) => {
            let uid = if request.code == POSIX_REQUEST_GETEUID {
                runtime.sessions[index].euid
            } else {
                runtime.sessions[index].uid
            };
            (libnanami::OS_RESPONSE_OK, uid, 0)
        }
        None => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_getgid(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    match session_for_pid(runtime, request.identifier) {
        Some(index) => {
            let gid = if request.code == POSIX_REQUEST_GETEGID {
                runtime.sessions[index].egid
            } else {
                runtime.sessions[index].gid
            };
            (libnanami::OS_RESPONSE_OK, gid, 0)
        }
        None => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_getpgid(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = session_for_pid(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let session = runtime.sessions[index];
    let current_pid = session.posix_pid;
    if request.arg0 != 0 && request.arg0 != current_pid {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    (libnanami::OS_RESPONSE_OK, session.posix_pgid, 0)
}

fn handle_getsid(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = session_for_pid(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let session = runtime.sessions[index];
    let current_pid = session.posix_pid;
    if request.arg0 != 0 && request.arg0 != current_pid {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    (libnanami::OS_RESPONSE_OK, session.posix_sid, 0)
}

fn handle_setpgid(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = session_for_pid(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let current_pid = runtime.sessions[index].posix_pid;
    if request.arg0 != 0 && request.arg0 != current_pid {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let pgid = if request.arg1 == 0 { current_pid } else { request.arg1 };
    runtime.sessions[index].posix_pgid = pgid;
    (libnanami::OS_RESPONSE_OK, 0, 0)
}

fn handle_setsid(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = session_for_pid(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let pid = runtime.sessions[index].posix_pid;
    if runtime.sessions[index].posix_pgid == pid {
        return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    }
    runtime.sessions[index].posix_sid = pid;
    runtime.sessions[index].posix_pgid = pid;
    (libnanami::OS_RESPONSE_OK, pid, 0)
}

fn handle_unsupported_process_lifecycle(
    runtime: &mut Runtime,
    request: ServiceRequest,
) -> (Word, Word, Word) {
    match session_for_pid(runtime, request.identifier) {
        Some(_) => (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
        None => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_spawn(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(parent_index) = session_for_pid(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((path, len)) = resolve_client_path(runtime, parent_index, request.arg0 as usize, request.arg1 as usize) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let image_name = path_basename(&path[..len]);
    let Ok(image_name_str) = core::str::from_utf8(image_name) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let native_pid = match libnanami::request_process_spawn(image_name_str) {
        Ok(pid) => pid,
        Err(e) => {
            log_request_error("[posix-server] native spawn failed: ", e);
            return (map_request_error_to_status(e), 0, 0);
        }
    };
    let Some(child_index) = create_child_session(runtime, native_pid, parent_index) else {
        libnanami::println!("[posix-server] child session allocation failed native_pid={}", native_pid);
        return (libnanami::OS_RESPONSE_FATAL, 0, 0);
    };
    (
        libnanami::OS_RESPONSE_OK,
        runtime.sessions[child_index].posix_pid,
        native_pid,
    )
}

fn handle_waitpid(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(parent_index) = session_for_pid(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    if (request.arg1 & !POSIX_WAIT_NOHANG) != 0 {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }

    let parent_pid = runtime.sessions[parent_index].posix_pid;
    let target_pid = request.arg0;
    let mut matched = false;
    let mut i = 0usize;
    while i < runtime.sessions.len() {
        let child = runtime.sessions[i];
        let any_child = target_pid == 0 || target_pid == usize::MAX as Word;
        if child.active
            && child.posix_ppid == parent_pid
            && (any_child || child.posix_pid == target_pid)
        {
            matched = true;
            match libnanami::request_process_status(child.owner_pid) {
                Ok((true, exit_code)) => {
                    let posix_pid = child.posix_pid;
                    if let Err(e) = libnanami::request_process_reap(child.owner_pid) {
                        return (map_request_error_to_status(e), 0, 0);
                    }
                    release_session_fds(runtime, i);
                    runtime.sessions[i] = Session::EMPTY;
                    return (libnanami::OS_RESPONSE_OK, posix_pid, exit_code);
                }
                Ok((false, _)) => {}
                Err(e) => return (map_request_error_to_status(e), 0, 0),
            }
        }
        i += 1;
    }

    if matched && (request.arg1 & POSIX_WAIT_NOHANG) != 0 {
        return (libnanami::OS_RESPONSE_OK, 0, 0);
    }
    if matched {
        return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    }
    (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0)
}

fn handle_getcwd(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let session = runtime.sessions[index];
    let out_offset = request.arg0 as usize;
    let max_len = request.arg1 as usize;
    if out_offset.saturating_add(session.cwd_len) > session.shm_size as usize || session.cwd_len > max_len {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    unsafe { ptr::copy_nonoverlapping(session.cwd.as_ptr(), (session.shm_local as usize + out_offset) as *mut u8, session.cwd_len); }
    (libnanami::OS_RESPONSE_OK, session.cwd_len as Word, 0)
}

fn handle_chdir(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let Some((path, len)) = resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    match nanami_services::vfs::vfs_stat(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok((_, _, kind)) if kind == nanami_services::vfs::VFS_FILE_TYPE_DIRECTORY => {
            runtime.sessions[index].cwd[..len].copy_from_slice(&path[..len]);
            runtime.sessions[index].cwd_len = len;
            (libnanami::OS_RESPONSE_OK, 0, 0)
        }
        Ok(_) => (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

fn handle_open(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let Some((path, len)) = resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let kind = special_device_kind(&path[..len]);
    if kind != FdKind::Empty {
        if (request.arg2 & POSIX_O_DIRECTORY) != 0 {
            return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
        }
        return alloc_open_file_and_fd(runtime, index, kind, 0);
    }
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    if (request.arg2 & POSIX_O_TRUNC) != 0 {
        match nanami_services::vfs::vfs_stat(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
            Ok((_, _, kind)) if kind == nanami_services::vfs::VFS_FILE_TYPE_DIRECTORY => {
                return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
            }
            Ok(_) => {
                let _ = nanami_services::vfs::vfs_remove(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word);
                let _ = nanami_services::vfs::vfs_create(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word);
            }
            Err(_) => {}
        }
    }
    let mut handle = nanami_services::vfs::vfs_open(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word);
    if handle.is_err() && (request.arg2 & POSIX_O_CREAT) != 0 {
        let _ = nanami_services::vfs::vfs_create(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word);
        handle = nanami_services::vfs::vfs_open(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word);
    }
    match handle {
        Ok(vfs_handle) => {
            let fd_kind = match nanami_services::vfs::vfs_fstat(runtime.vfs_port, vfs_handle) {
                Ok((_, _, kind)) if kind == nanami_services::vfs::VFS_FILE_TYPE_DIRECTORY => FdKind::Directory,
                Ok(_) => FdKind::Regular,
                Err(e) => {
                    let _ = nanami_services::vfs::vfs_close(runtime.vfs_port, vfs_handle);
                    return (map_request_error_to_status(e), 0, 0);
                }
            };
            if (request.arg2 & POSIX_O_DIRECTORY) != 0 && fd_kind != FdKind::Directory {
                let _ = nanami_services::vfs::vfs_close(runtime.vfs_port, vfs_handle);
                return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
            }
            alloc_open_file_and_fd(runtime, index, fd_kind, vfs_handle)
        }
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

fn alloc_open_file_and_fd(
    runtime: &mut Runtime,
    session_index: usize,
    kind: FdKind,
    vfs_handle: Word,
) -> (Word, Word, Word) {
    let Some(open_index) = alloc_open_file(runtime, kind, vfs_handle) else {
        if matches!(kind, FdKind::Regular | FdKind::Directory) {
            let _ = nanami_services::vfs::vfs_close(runtime.vfs_port, vfs_handle);
        }
        return (libnanami::OS_RESPONSE_FATAL, 0, 0);
    };
    match alloc_fd(&mut runtime.sessions[session_index], open_index) {
        (libnanami::OS_RESPONSE_OK, fd, detail) => (libnanami::OS_RESPONSE_OK, fd, detail),
        _ => {
            release_open_file(runtime, open_index);
            (libnanami::OS_RESPONSE_FATAL, 0, 0)
        }
    }
}

fn alloc_open_file(runtime: &mut Runtime, kind: FdKind, vfs_handle: Word) -> Option<usize> {
    let mut i = 0usize;
    while i < runtime.open_files.len() {
        if !runtime.open_files[i].active {
            runtime.open_files[i] = OpenFile {
                active: true,
                kind,
                offset: 0,
                vfs_handle,
                ref_count: 1,
            };
            return Some(i);
        }
        i += 1;
    }
    None
}

fn alloc_fd(session: &mut Session, open_file: usize) -> (Word, Word, Word) {
    let mut fd = 3usize;
    while fd < session.fds.len() {
        if !session.fds[fd].active {
            session.fds[fd] = FileDescriptor {
                active: true,
                open_file,
                flags: 0,
            };
            return (libnanami::OS_RESPONSE_OK, fd as Word, 0);
        }
        fd += 1;
    }
    (libnanami::OS_RESPONSE_FATAL, 0, 0)
}

fn duplicate_fd(
    runtime: &mut Runtime,
    session_index: usize,
    old_fd: usize,
    new_fd: Option<usize>,
) -> (Word, Word, Word) {
    if old_fd >= MAX_FDS || !runtime.sessions[session_index].fds[old_fd].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let open_index = runtime.sessions[session_index].fds[old_fd].open_file;
    if open_index >= runtime.open_files.len() || !runtime.open_files[open_index].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }

    let target_fd = match new_fd {
        Some(fd) => {
            if fd >= MAX_FDS {
                return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            fd
        }
        None => match lowest_free_fd(&runtime.sessions[session_index]) {
            Some(fd) => fd,
            None => return (libnanami::OS_RESPONSE_FATAL, 0, 0),
        },
    };

    if target_fd == old_fd {
        return (libnanami::OS_RESPONSE_OK, target_fd as Word, 0);
    }
    if runtime.sessions[session_index].fds[target_fd].active {
        let old_target_open = runtime.sessions[session_index].fds[target_fd].open_file;
        runtime.sessions[session_index].fds[target_fd] = FileDescriptor::EMPTY;
        release_open_file(runtime, old_target_open);
    }

    runtime.open_files[open_index].ref_count =
        runtime.open_files[open_index].ref_count.saturating_add(1);
    runtime.sessions[session_index].fds[target_fd] = FileDescriptor {
        active: true,
        open_file: open_index,
        flags: 0,
    };
    (libnanami::OS_RESPONSE_OK, target_fd as Word, 0)
}

fn lowest_free_fd(session: &Session) -> Option<usize> {
    let mut fd = 0usize;
    while fd < MAX_FDS {
        if !session.fds[fd].active {
            return Some(fd);
        }
        fd += 1;
    }
    None
}

fn release_open_file(runtime: &mut Runtime, open_index: usize) {
    if open_index >= runtime.open_files.len() || !runtime.open_files[open_index].active {
        return;
    }
    if runtime.open_files[open_index].ref_count > 1 {
        runtime.open_files[open_index].ref_count -= 1;
        return;
    }
    let entry = runtime.open_files[open_index];
    if matches!(entry.kind, FdKind::Regular | FdKind::Directory) {
        let _ = nanami_services::vfs::vfs_close(runtime.vfs_port, entry.vfs_handle);
    }
    runtime.open_files[open_index] = OpenFile::EMPTY;
}

fn release_session_fds(runtime: &mut Runtime, session_index: usize) {
    let mut fd = 0usize;
    while fd < MAX_FDS {
        if runtime.sessions[session_index].fds[fd].active {
            let open_index = runtime.sessions[session_index].fds[fd].open_file;
            runtime.sessions[session_index].fds[fd] = FileDescriptor::EMPTY;
            release_open_file(runtime, open_index);
        }
        fd += 1;
    }
}

fn handle_close(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let fd = request.arg0 as usize;
    if fd >= MAX_FDS || !runtime.sessions[index].fds[fd].active { return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0); }
    let open_index = runtime.sessions[index].fds[fd].open_file;
    runtime.sessions[index].fds[fd] = FileDescriptor::EMPTY;
    release_open_file(runtime, open_index);
    (libnanami::OS_RESPONSE_OK, 0, 0)
}

fn handle_dup(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    duplicate_fd(runtime, index, request.arg0 as usize, None)
}

fn handle_dup2(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    duplicate_fd(
        runtime,
        index,
        request.arg0 as usize,
        Some(request.arg1 as usize),
    )
}

fn handle_fcntl(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let fd = request.arg0 as usize;
    if fd >= MAX_FDS || !runtime.sessions[index].fds[fd].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    match request.arg1 {
        POSIX_F_GETFD => (libnanami::OS_RESPONSE_OK, runtime.sessions[index].fds[fd].flags, 0),
        POSIX_F_SETFD => {
            if (request.arg2 & !POSIX_FD_CLOEXEC) != 0 {
                return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            runtime.sessions[index].fds[fd].flags = request.arg2 & POSIX_FD_CLOEXEC;
            (libnanami::OS_RESPONSE_OK, 0, 0)
        }
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_read(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let fd = request.arg0 as usize;
    let out_offset = request.arg1 as usize;
    let len = request.arg2 as usize;
    if fd >= MAX_FDS || !runtime.sessions[index].fds[fd].active { return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0); }
    if out_offset.saturating_add(len) > runtime.sessions[index].shm_size as usize { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); }
    let open_index = runtime.sessions[index].fds[fd].open_file;
    if open_index >= runtime.open_files.len() || !runtime.open_files[open_index].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let mut entry = runtime.open_files[open_index];
    match entry.kind {
        FdKind::DevNull => (libnanami::OS_RESPONSE_OK, 0, 0),
        FdKind::DevZero => {
            unsafe { ptr::write_bytes((runtime.sessions[index].shm_local as usize + out_offset) as *mut u8, 0, len); }
            (libnanami::OS_RESPONSE_OK, len as Word, 0)
        }
        FdKind::Regular => {
            let chunk = len.min(1024);
            match nanami_services::vfs::vfs_read(runtime.vfs_port, entry.vfs_handle, entry.offset, chunk as Word, VFS_IO_OFFSET as Word) {
                Ok(bytes) => {
                    unsafe {
                        ptr::copy_nonoverlapping((runtime.vfs_shm as usize + VFS_IO_OFFSET) as *const u8, (runtime.sessions[index].shm_local as usize + out_offset) as *mut u8, bytes as usize);
                    }
                    entry.offset = entry.offset.saturating_add(bytes);
                    runtime.open_files[open_index] = entry;
                    (libnanami::OS_RESPONSE_OK, bytes, 0)
                }
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        _ => (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
    }
}

fn handle_write(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let fd = request.arg0 as usize;
    let input_offset = request.arg1 as usize;
    let len = request.arg2 as usize;
    if fd >= MAX_FDS || !runtime.sessions[index].fds[fd].active { return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0); }
    if input_offset.saturating_add(len) > runtime.sessions[index].shm_size as usize { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); }
    let open_index = runtime.sessions[index].fds[fd].open_file;
    if open_index >= runtime.open_files.len() || !runtime.open_files[open_index].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let mut entry = runtime.open_files[open_index];
    match entry.kind {
        FdKind::DevNull => (libnanami::OS_RESPONSE_OK, len as Word, 0),
        FdKind::Regular => {
            let chunk = len.min(1024);
            unsafe {
                ptr::copy_nonoverlapping((runtime.sessions[index].shm_local as usize + input_offset) as *const u8, (runtime.vfs_shm as usize + VFS_IO_OFFSET) as *mut u8, chunk);
            }
            match nanami_services::vfs::vfs_write(runtime.vfs_port, entry.vfs_handle, entry.offset, chunk as Word, VFS_IO_OFFSET as Word) {
                Ok(bytes) => {
                    entry.offset = entry.offset.saturating_add(bytes);
                    runtime.open_files[open_index] = entry;
                    (libnanami::OS_RESPONSE_OK, bytes, 0)
                }
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        _ => (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
    }
}

fn handle_stat(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let Some((path, len)) = resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let dev_kind = special_device_kind(&path[..len]);
    if dev_kind == FdKind::DevNull {
        return (libnanami::OS_RESPONSE_OK, 0, pack_stat(0, POSIX_FILE_TYPE_CHAR_DEVICE, POSIX_DEV_NULL_MAJOR, POSIX_DEV_NULL_MINOR));
    }
    if dev_kind == FdKind::DevZero {
        return (libnanami::OS_RESPONSE_OK, 0, pack_stat(0, POSIX_FILE_TYPE_CHAR_DEVICE, POSIX_DEV_ZERO_MAJOR, POSIX_DEV_ZERO_MINOR));
    }
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    match nanami_services::vfs::vfs_stat(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok((inode, size, kind)) => (libnanami::OS_RESPONSE_OK, inode, pack_stat(size, vfs_kind_to_posix(kind), 0, 0)),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

fn handle_mkdir(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let Some((path, len)) = resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    match nanami_services::vfs::vfs_mkdir(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok(_) => (libnanami::OS_RESPONSE_OK, 0, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

fn handle_unlink(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let Some((path, len)) = resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    match nanami_services::vfs::vfs_stat(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok((_, _, kind)) if kind == nanami_services::vfs::VFS_FILE_TYPE_DIRECTORY => {
            return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
        }
        Ok(_) => {}
        Err(e) => return (map_request_error_to_status(e), 0, 0),
    }
    match nanami_services::vfs::vfs_remove(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

fn handle_rmdir(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let Some((path, len)) = resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    write_vfs_path(runtime, VFS_PATH_OFFSET, &path[..len]);
    match nanami_services::vfs::vfs_stat(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok((_, _, kind)) if kind == nanami_services::vfs::VFS_FILE_TYPE_DIRECTORY => {}
        Ok(_) => return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
        Err(e) => return (map_request_error_to_status(e), 0, 0),
    }
    match nanami_services::vfs::vfs_remove(runtime.vfs_port, VFS_PATH_OFFSET as Word, len as Word) {
        Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

fn handle_rename(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let Some((old_path, old_len)) = resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let Some((new_path, new_len)) = resolve_client_path(runtime, index, request.arg2 as usize, request.arg3 as usize) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    write_vfs_path(runtime, VFS_PATH_OFFSET, &old_path[..old_len]);
    write_vfs_path(runtime, VFS_PATH2_OFFSET, &new_path[..new_len]);
    match nanami_services::vfs::vfs_rename(runtime.vfs_port, VFS_PATH_OFFSET as Word, old_len as Word, VFS_PATH2_OFFSET as Word, new_len as Word) {
        Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

fn handle_fstat(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let fd = request.arg0 as usize;
    if fd >= MAX_FDS || !runtime.sessions[index].fds[fd].active { return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0); }
    let open_index = runtime.sessions[index].fds[fd].open_file;
    if open_index >= runtime.open_files.len() || !runtime.open_files[open_index].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let entry = runtime.open_files[open_index];
    match entry.kind {
        FdKind::DevNull => (libnanami::OS_RESPONSE_OK, 0, pack_stat(0, POSIX_FILE_TYPE_CHAR_DEVICE, POSIX_DEV_NULL_MAJOR, POSIX_DEV_NULL_MINOR)),
        FdKind::DevZero => (libnanami::OS_RESPONSE_OK, 0, pack_stat(0, POSIX_FILE_TYPE_CHAR_DEVICE, POSIX_DEV_ZERO_MAJOR, POSIX_DEV_ZERO_MINOR)),
        FdKind::Regular | FdKind::Directory => match nanami_services::vfs::vfs_fstat(runtime.vfs_port, entry.vfs_handle) {
            Ok((inode, size, kind)) => (libnanami::OS_RESPONSE_OK, inode, pack_stat(size, vfs_kind_to_posix(kind), 0, 0)),
            Err(e) => (map_request_error_to_status(e), 0, 0),
        },
        FdKind::Empty => (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0),
    }
}

fn handle_read_dir(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let fd = request.arg0 as usize;
    let max_entries = request.arg1;
    let out_offset = request.arg2 as usize;
    if fd >= MAX_FDS || !runtime.sessions[index].fds[fd].active { return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0); }
    let open_index = runtime.sessions[index].fds[fd].open_file;
    if open_index >= runtime.open_files.len() || !runtime.open_files[open_index].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let entry = runtime.open_files[open_index];
    if entry.kind != FdKind::Directory {
        return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    }
    if out_offset > runtime.sessions[index].shm_size as usize {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let record_bytes = nanami_services::vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES;
    let client_capacity = runtime.sessions[index]
        .shm_size
        .saturating_sub(out_offset as Word) as usize
        / record_bytes;
    let vfs_capacity = runtime.vfs_shm_size.saturating_sub(VFS_IO_OFFSET as Word) as usize / record_bytes;
    let capped_entries = (max_entries as usize).min(client_capacity).min(vfs_capacity) as Word;
    let bytes = (capped_entries as usize).saturating_mul(record_bytes);
    if out_offset.saturating_add(bytes) > runtime.sessions[index].shm_size as usize {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    if capped_entries == 0 {
        return (libnanami::OS_RESPONSE_OK, 0, entry.offset);
    }
    match nanami_services::vfs::vfs_read_dir(runtime.vfs_port, entry.vfs_handle, entry.offset, capped_entries, VFS_IO_OFFSET as Word) {
        Ok((entries, next_index)) => {
            let copy_bytes = (entries as usize).saturating_mul(record_bytes);
            unsafe {
                ptr::copy_nonoverlapping(
                    (runtime.vfs_shm as usize + VFS_IO_OFFSET) as *const u8,
                    (runtime.sessions[index].shm_local as usize + out_offset) as *mut u8,
                    copy_bytes,
                );
            }
            runtime.open_files[open_index].offset = next_index;
            (libnanami::OS_RESPONSE_OK, entries, next_index)
        }
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

fn handle_seek(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_session(runtime, request.identifier) else { return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0); };
    let fd = request.arg0 as usize;
    if fd >= MAX_FDS || !runtime.sessions[index].fds[fd].active { return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0); }
    let open_index = runtime.sessions[index].fds[fd].open_file;
    if open_index >= runtime.open_files.len() || !runtime.open_files[open_index].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let entry = runtime.open_files[open_index];
    let base = match request.arg2 {
        POSIX_SEEK_SET => 0,
        POSIX_SEEK_CUR => entry.offset,
        POSIX_SEEK_END => match entry.kind {
            FdKind::Regular | FdKind::Directory => match nanami_services::vfs::vfs_fstat(runtime.vfs_port, entry.vfs_handle) {
                Ok((_, size, _)) => size,
                Err(e) => return (map_request_error_to_status(e), 0, 0),
            },
            FdKind::DevNull | FdKind::DevZero => 0,
            FdKind::Empty => return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0),
        },
        _ => return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    };
    let new_offset = base.saturating_add(request.arg1);
    runtime.open_files[open_index].offset = new_offset;
    (libnanami::OS_RESPONSE_OK, new_offset, 0)
}

fn resolve_client_path(runtime: &Runtime, session_index: usize, path_offset: usize, path_len: usize) -> Option<([u8; PATH_MAX], usize)> {
    let session = runtime.sessions[session_index];
    if path_len == 0 || path_offset.checked_add(path_len)? > session.shm_size as usize || path_len > PATH_MAX {
        return None;
    }
    let input = unsafe { core::slice::from_raw_parts((session.shm_local as usize + path_offset) as *const u8, path_len) };
    resolve_path(&session.cwd[..session.cwd_len], input)
}

fn resolve_path(cwd: &[u8], arg: &[u8]) -> Option<([u8; PATH_MAX], usize)> {
    let mut raw = [0u8; PATH_MAX];
    let mut raw_len = 0usize;
    if arg.first() == Some(&b'/') {
        raw_len = copy_path_part(&mut raw, raw_len, arg)?;
    } else {
        raw_len = copy_path_part(&mut raw, raw_len, cwd)?;
        if raw_len != 1 { raw_len = push_path_byte(&mut raw, raw_len, b'/')?; }
        raw_len = copy_path_part(&mut raw, raw_len, arg)?;
    }
    normalize_path(&raw[..raw_len])
}

fn copy_path_part(dst: &mut [u8; PATH_MAX], mut len: usize, src: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i < src.len() {
        len = push_path_byte(dst, len, src[i])?;
        i += 1;
    }
    Some(len)
}

fn push_path_byte(dst: &mut [u8; PATH_MAX], len: usize, byte: u8) -> Option<usize> {
    if len >= PATH_MAX { return None; }
    dst[len] = byte;
    Some(len + 1)
}

fn normalize_path(raw: &[u8]) -> Option<([u8; PATH_MAX], usize)> {
    let mut starts = [0usize; MAX_COMPONENTS];
    let mut lens = [0usize; MAX_COMPONENTS];
    let mut count = 0usize;
    let mut i = 0usize;
    while i < raw.len() {
        while i < raw.len() && raw[i] == b'/' { i += 1; }
        let start = i;
        while i < raw.len() && raw[i] != b'/' { i += 1; }
        let len = i.saturating_sub(start);
        if len == 0 || (len == 1 && raw[start] == b'.') { continue; }
        if len == 2 && raw[start] == b'.' && raw[start + 1] == b'.' {
            count = count.saturating_sub(1);
            continue;
        }
        if count >= MAX_COMPONENTS { return None; }
        starts[count] = start;
        lens[count] = len;
        count += 1;
    }
    let mut out = [0u8; PATH_MAX];
    let mut out_len = 1usize;
    out[0] = b'/';
    let mut c = 0usize;
    while c < count {
        if out_len != 1 { out_len = push_path_byte(&mut out, out_len, b'/')?; }
        let mut j = 0usize;
        while j < lens[c] {
            out_len = push_path_byte(&mut out, out_len, raw[starts[c] + j])?;
            j += 1;
        }
        c += 1;
    }
    Some((out, out_len))
}

fn write_vfs_path(runtime: &Runtime, offset: usize, path: &[u8]) {
    unsafe { ptr::copy_nonoverlapping(path.as_ptr(), (runtime.vfs_shm as usize + offset) as *mut u8, path.len()); }
}

fn special_device_kind(path: &[u8]) -> FdKind {
    if bytes_eq(path, b"/dev/null") { FdKind::DevNull }
    else if bytes_eq(path, b"/dev/zero") { FdKind::DevZero }
    else { FdKind::Empty }
}

fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut i = 0usize;
    while i < a.len() { if a[i] != b[i] { return false; } i += 1; }
    true
}

fn path_basename(path: &[u8]) -> &[u8] {
    let mut start = 0usize;
    let mut i = 0usize;
    while i < path.len() {
        if path[i] == b'/' {
            start = i + 1;
        }
        i += 1;
    }
    &path[start..]
}

fn vfs_kind_to_posix(kind: Word) -> Word {
    match kind {
        nanami_services::vfs::VFS_FILE_TYPE_REGULAR => POSIX_FILE_TYPE_REGULAR,
        nanami_services::vfs::VFS_FILE_TYPE_DIRECTORY => POSIX_FILE_TYPE_DIRECTORY,
        _ => POSIX_FILE_TYPE_UNKNOWN,
    }
}

fn pack_stat(size: Word, kind: Word, major: Word, minor: Word) -> Word {
    (size & POSIX_STAT_SIZE_MASK)
        | ((kind & 0xff) << POSIX_STAT_TYPE_SHIFT)
        | ((major & 0xff) << POSIX_STAT_MAJOR_SHIFT)
        | ((minor & 0xffff) << POSIX_STAT_MINOR_SHIFT)
}

fn map_request_error_to_status(error: RequestError) -> Word {
    match error {
        RequestError::InvalidArgument => libnanami::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Status(status) => status,
        RequestError::Unsupported => libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Transport | RequestError::Protocol => libnanami::OS_RESPONSE_FATAL,
    }
}

fn log_error(prefix: &str, error: RequestError) -> libnanami::NanamiError {
    log_request_error(prefix, error);
    libnanami::NanamiError::from(error)
}

fn log_request_error(prefix: &str, error: RequestError) {
    libnanami::print!("{}", prefix);
    libnanami::println!("{:?}", error);
}

libnanami::nanami_entry!(nanami_main);
