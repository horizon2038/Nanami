use libnanami::ipc::ServiceRequest;
use libnanami::Word;
use nanami_services::posix::*;

use crate::state::*;

pub(crate) fn alloc_open_file_and_fd(
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

pub(crate) fn inherit_fds(runtime: &mut Runtime, parent_index: usize, child_index: usize) {
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

pub(crate) fn release_session_fds(runtime: &mut Runtime, session_index: usize) {
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

pub(crate) fn handle_close(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = crate::find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let fd = request.arg0 as usize;
    if fd >= MAX_FDS || !runtime.sessions[index].fds[fd].active {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    }
    let open_index = runtime.sessions[index].fds[fd].open_file;
    runtime.sessions[index].fds[fd] = FileDescriptor::EMPTY;
    release_open_file(runtime, open_index);
    (libnanami::OS_RESPONSE_OK, 0, 0)
}

pub(crate) fn handle_dup(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = crate::find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    duplicate_fd(runtime, index, request.arg0 as usize, None)
}

pub(crate) fn handle_dup2(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = crate::find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    duplicate_fd(
        runtime,
        index,
        request.arg0 as usize,
        Some(request.arg1 as usize),
    )
}

pub(crate) fn handle_fcntl(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = crate::find_session(runtime, request.identifier) else {
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
