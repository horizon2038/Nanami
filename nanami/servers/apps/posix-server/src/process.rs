use libnanami::ipc::ServiceRequest;
use libnanami::Word;
use nanami_services::posix::*;

use crate::environment::init_default_environment;
use crate::fd::{inherit_fds, release_cloexec_session_fds, release_session_fds};
use crate::path::{path_basename, resolve_client_path};
use crate::state::*;

pub(crate) fn session_for_pid(runtime: &mut Runtime, owner_pid: Word) -> Option<usize> {
    if let Some(index) = runtime.cached_session(owner_pid) {
        return Some(index);
    }
    let mut empty = None;
    let mut i = 0usize;
    while i < runtime.sessions.len() {
        if runtime.sessions[i].active && runtime.sessions[i].owner_pid == owner_pid {
            runtime.cache_session(owner_pid, i);
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
    init_default_environment(&mut runtime.sessions[index]);
    runtime.cache_session(owner_pid, index);
    Some(index)
}

pub(crate) fn find_session(runtime: &mut Runtime, owner_pid: Word) -> Option<usize> {
    if let Some(index) = runtime.cached_session(owner_pid) {
        return Some(index);
    }
    let mut i = 0usize;
    while i < runtime.sessions.len() {
        if runtime.sessions[i].active && runtime.sessions[i].owner_pid == owner_pid {
            runtime.cache_session(owner_pid, i);
            return Some(i);
        }
        i += 1;
    }
    None
}

pub(crate) fn handle_getppid(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    match session_for_pid(runtime, request.identifier) {
        Some(index) => (
            libnanami::OS_RESPONSE_OK,
            runtime.sessions[index].posix_ppid,
            0,
        ),
        None => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

pub(crate) fn handle_get_native_pid(
    runtime: &mut Runtime,
    request: ServiceRequest,
) -> (Word, Word, Word) {
    match session_for_pid(runtime, request.identifier) {
        Some(_) => (libnanami::OS_RESPONSE_OK, request.identifier, 0),
        None => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

pub(crate) fn handle_getuid(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
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

pub(crate) fn handle_getgid(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
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

pub(crate) fn handle_getpgid(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
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

pub(crate) fn handle_getsid(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
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

pub(crate) fn handle_setpgid(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = session_for_pid(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let current_pid = runtime.sessions[index].posix_pid;
    if request.arg0 != 0 && request.arg0 != current_pid {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let pgid = if request.arg1 == 0 {
        current_pid
    } else {
        request.arg1
    };
    runtime.sessions[index].posix_pgid = pgid;
    (libnanami::OS_RESPONSE_OK, 0, 0)
}

pub(crate) fn handle_setsid(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
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

pub(crate) fn handle_spawn(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(parent_index) = session_for_pid(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((path, len)) = resolve_client_path(
        runtime,
        parent_index,
        request.arg0 as usize,
        request.arg1 as usize,
    ) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let image_name = path_basename(&path[..len]);
    let Ok(image_name_str) = core::str::from_utf8(image_name) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let native_pid = match libnanami::request_process_spawn(image_name_str) {
        Ok(pid) => pid,
        Err(e) => {
            crate::log_request_error("[posix-server] native spawn failed: ", e);
            return (crate::map_request_error_to_status(e), 0, 0);
        }
    };
    let Some(child_index) = create_child_session(runtime, native_pid, parent_index) else {
        libnanami::println!(
            "[posix-server] child session allocation failed native_pid={}",
            native_pid
        );
        cleanup_native_process(native_pid);
        return (libnanami::OS_RESPONSE_FATAL, 0, 0);
    };
    if !set_session_image_name(&mut runtime.sessions[child_index], image_name) {
        cleanup_native_process(native_pid);
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    (
        libnanami::OS_RESPONSE_OK,
        runtime.sessions[child_index].posix_pid,
        native_pid,
    )
}

pub(crate) fn handle_fork(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(parent_index) = session_for_pid(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let parent = runtime.sessions[parent_index];
    if parent.image_name_len == 0 {
        return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    }
    let image_name = &parent.image_name[..parent.image_name_len];
    let Ok(image_name_str) = core::str::from_utf8(image_name) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let native_pid = match libnanami::request_process_spawn(image_name_str) {
        Ok(pid) => pid,
        Err(e) => {
            crate::log_request_error("[posix-server] native fork spawn failed: ", e);
            return (crate::map_request_error_to_status(e), 0, 0);
        }
    };
    let Some(child_index) = create_child_session(runtime, native_pid, parent_index) else {
        libnanami::println!(
            "[posix-server] fork child session allocation failed native_pid={}",
            native_pid
        );
        cleanup_native_process(native_pid);
        return (libnanami::OS_RESPONSE_FATAL, 0, 0);
    };
    runtime.sessions[child_index].image_name = parent.image_name;
    runtime.sessions[child_index].image_name_len = parent.image_name_len;
    (
        libnanami::OS_RESPONSE_OK,
        runtime.sessions[child_index].posix_pid,
        native_pid,
    )
}

pub(crate) fn handle_exec(runtime: &mut Runtime, request: ServiceRequest) -> ReplyAction {
    let Some(index) = session_for_pid(runtime, request.identifier) else {
        return ReplyAction::error(libnanami::OS_RESPONSE_INVALID_ARGUMENT);
    };
    let Some((path, len)) =
        resolve_client_path(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return ReplyAction::error(libnanami::OS_RESPONSE_INVALID_ARGUMENT);
    };
    let image_name = path_basename(&path[..len]);
    if image_name.is_empty() || image_name.len() > IMAGE_NAME_MAX {
        return ReplyAction::error(libnanami::OS_RESPONSE_INVALID_ARGUMENT);
    }
    let Ok(image_name_str) = core::str::from_utf8(image_name) else {
        return ReplyAction::error(libnanami::OS_RESPONSE_INVALID_ARGUMENT);
    };
    let native_pid = match libnanami::request_process_spawn(image_name_str) {
        Ok(pid) => pid,
        Err(e) => {
            crate::log_request_error("[posix-server] native exec spawn failed: ", e);
            return ReplyAction::Reply(crate::map_request_error_to_status(e), 0, 0);
        }
    };

    let old_native_pid = runtime.sessions[index].owner_pid;
    if let Err(e) = libnanami::request_process_kill(old_native_pid, 0) {
        cleanup_native_process(native_pid);
        return ReplyAction::error(crate::map_request_error_to_status(e));
    }
    let _ = libnanami::request_process_reap(old_native_pid);

    runtime.sessions[index].owner_pid = native_pid;
    runtime.cache_session(native_pid, index);
    release_cloexec_session_fds(runtime, index);
    set_session_image_name(&mut runtime.sessions[index], image_name);
    ReplyAction::DropReply
}

pub(crate) fn handle_kill(runtime: &mut Runtime, request: ServiceRequest) -> ReplyAction {
    let Some(caller_index) = session_for_pid(runtime, request.identifier) else {
        return ReplyAction::error(libnanami::OS_RESPONSE_INVALID_ARGUMENT);
    };
    let target_posix_pid = request.arg0;
    let signal = request.arg1;
    if target_posix_pid == 0 {
        return ReplyAction::error(libnanami::OS_RESPONSE_INVALID_ARGUMENT);
    }

    let mut target_index = None;
    let mut i = 0usize;
    while i < runtime.sessions.len() {
        if runtime.sessions[i].active && runtime.sessions[i].posix_pid == target_posix_pid {
            target_index = Some(i);
            break;
        }
        i += 1;
    }
    let Some(target_index) = target_index else {
        return ReplyAction::error(libnanami::OS_RESPONSE_INVALID_ARGUMENT);
    };

    let caller = runtime.sessions[caller_index];
    let target = runtime.sessions[target_index];
    let same_user =
        caller.euid == POSIX_ROOT_UID || caller.euid == target.uid || caller.euid == target.euid;
    if !same_user {
        return ReplyAction::error(libnanami::OS_RESPONSE_PERMISSION_DENIED);
    }
    if signal == 0 {
        return ReplyAction::ok(0, 0);
    }
    match libnanami::request_process_kill(target.owner_pid, signal) {
        Ok(()) if target.owner_pid == request.identifier => ReplyAction::DropReply,
        Ok(()) => ReplyAction::ok(0, 0),
        Err(e) => ReplyAction::error(crate::map_request_error_to_status(e)),
    }
}

pub(crate) fn handle_waitpid(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
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
                        return (crate::map_request_error_to_status(e), 0, 0);
                    }
                    release_session_fds(runtime, i);
                    runtime.invalidate_session_cache(i);
                    runtime.sessions[i] = Session::EMPTY;
                    return (libnanami::OS_RESPONSE_OK, posix_pid, exit_code);
                }
                Ok((false, _)) => {}
                Err(e) => return (crate::map_request_error_to_status(e), 0, 0),
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

fn create_child_session(
    runtime: &mut Runtime,
    native_pid: Word,
    parent_index: usize,
) -> Option<usize> {
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
    runtime.sessions[child_index].image_name = parent.image_name;
    runtime.sessions[child_index].image_name_len = parent.image_name_len;
    runtime.sessions[child_index].env = parent.env;
    inherit_fds(runtime, parent_index, child_index);
}

fn set_session_image_name(session: &mut Session, image_name: &[u8]) -> bool {
    if image_name.is_empty() || image_name.len() > IMAGE_NAME_MAX {
        return false;
    }
    session.image_name = [0; IMAGE_NAME_MAX];
    session.image_name[..image_name.len()].copy_from_slice(image_name);
    session.image_name_len = image_name.len();
    true
}

fn cleanup_native_process(native_pid: Word) {
    let _ = libnanami::request_process_kill(native_pid, 0);
    let _ = libnanami::request_process_reap(native_pid);
}
