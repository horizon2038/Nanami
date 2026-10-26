//! Advisory locks belong to open descriptions, including dup/fork aliases.
use super::{io::map_request_error_to_status, process::find_session, state::*};
use nanami_client::posix::*;
use nanami_runtime::{ipc::ServiceRequest, Word};

const NOTIFICATION_BASE: Word = 32;

pub(crate) fn notify(runtime: &Runtime) {
    for session in &runtime.sessions {
        if session.active && session.lock_notification != 0 {
            let _ = nanami_runtime::ipc::notification_notify(session.lock_notification);
        }
    }
}

pub(crate) fn handle(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(session) = find_session(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let fd = request.arg0 as usize;
    if fd >= MAX_FDS || !runtime.sessions[session].fds[fd].active {
        return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let op = request.arg1;
    if !matches!(
        op,
        POSIX_FLOCK_SHARED | POSIX_FLOCK_EXCLUSIVE | POSIX_FLOCK_UNLOCK
    ) {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let index = runtime.sessions[session].fds[fd].open_file;
    if !matches!(
        runtime.open_files[index].kind,
        FdKind::Regular | FdKind::Directory
    ) {
        return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    }
    // Subscribe before checking for contention. A release received while Alter
    // parks remains pending on its notification port; no timer polling is used.
    if request.arg2 != 0 && runtime.sessions[session].lock_notification == 0 {
        let slot = NOTIFICATION_BASE + session as Word;
        if let Err(error) = nanami_runtime::request_notification_port_copy(
            request.identifier,
            request.arg2,
            slot,
            POSIX_NOTIFICATION_LOCK_CHANGE,
        ) {
            return (map_request_error_to_status(error), 0, 0);
        }
        runtime.sessions[session].lock_notification =
            nanami_runtime::ipc::process_slot_descriptor(slot);
    }
    if runtime.open_files[index].lock == op {
        return (nanami_runtime::OS_RESPONSE_OK, 0, 0);
    }
    if runtime.open_files[index].lock != 0 {
        runtime.open_files[index].lock = 0;
        notify(runtime);
    }
    if op == POSIX_FLOCK_UNLOCK {
        return (nanami_runtime::OS_RESPONSE_OK, 0, 0);
    }
    if runtime.open_files[index].lock_inode == 0 {
        match nanami_client::vfs::vfs_fstat(runtime.vfs_port, runtime.open_files[index].vfs_handle)
        {
            Ok((inode, _, _)) => runtime.open_files[index].lock_inode = inode,
            Err(error) => return (map_request_error_to_status(error), 0, 0),
        }
    }
    let inode = runtime.open_files[index].lock_inode;
    if runtime.open_files.iter().enumerate().any(|(other, file)| {
        other != index
            && file.active
            && file.lock_inode == inode
            && file.lock != 0
            && (op == POSIX_FLOCK_EXCLUSIVE || file.lock == POSIX_FLOCK_EXCLUSIVE)
    }) {
        return (POSIX_FLOCK_WOULD_BLOCK, 0, 0);
    }
    runtime.open_files[index].lock = op;
    (nanami_runtime::OS_RESPONSE_OK, 0, 0)
}
