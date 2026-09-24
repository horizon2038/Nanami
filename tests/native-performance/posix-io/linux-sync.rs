//! Real Linux sync handlers and SDK calls; only fd lookup and IPC are mocked.
use super::*;
const EBADF: i32 = 9;
const EINVAL: i32 = 22;
const LINUX_O_RDONLY: Word = 0;
const LINUX_O_ACCMODE: Word = 3;
#[derive(Clone, Copy, PartialEq, Eq)]
enum LinuxFileKind {
    Posix,
    VirtualDirectory,
    VirtualFile,
}
enum VirtualNode {
    Root,
}
impl VirtualNode {
    fn id(self) -> Word {
        1
    }
}
#[derive(Clone, Copy)]
struct LinuxFile {
    kind: LinuxFileKind,
    posix_fd: Word,
    resource: Word,
}
struct Runtime {
    posix_port: Word,
    file: LinuxFile,
}
impl Runtime {
    fn linux_file(&self, pid: Word, fd: Word) -> Option<LinuxFile> {
        if (pid, fd) == (42, 7) {
            Some(self.file)
        } else {
            None
        }
    }
}
fn map_request_error(_error: RequestError) -> i32 {
    5
}
#[path = "../../../nanami/servers/apps/alter/shared/src/personality/linux/sync.rs"]
mod production;
#[path = "../../../nanami/servers/apps/alter/shared/src/personality/linux/truncate.rs"]
mod truncate;

#[test]
fn ftruncate_validates_access_and_signed_length_before_ipc() {
    let mut runtime = runtime(LinuxFileKind::Posix, 0);
    assert_eq!(truncate::sys_ftruncate(&mut runtime, 42, 99, 1), Err(EBADF));
    assert_eq!(truncate::sys_ftruncate(&mut runtime, 42, 7, 1), Err(EINVAL));
    runtime.file.resource = 2;
    assert_eq!(
        truncate::sys_ftruncate(&mut runtime, 42, 7, usize::MAX),
        Err(EINVAL)
    );
    runtime.file.kind = LinuxFileKind::VirtualFile;
    assert_eq!(truncate::sys_ftruncate(&mut runtime, 42, 7, 1), Err(EINVAL));
    BACKEND.with(|backend| assert!(backend.borrow().calls.is_empty()));
    runtime.file.kind = LinuxFileKind::Posix;
    assert_eq!(truncate::sys_ftruncate(&mut runtime, 42, 7, 1234), Ok(0));
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        assert_eq!(backend.calls[0].code, posix::POSIX_REQUEST_FTRUNCATE);
        assert_eq!(backend.calls[0].handle, 8);
        assert_eq!(backend.calls[0].buffer, 1234);
    });
    BACKEND.with(|backend| backend.borrow_mut().result = Some(Err(RequestError::Transport)));
    assert_eq!(truncate::sys_ftruncate(&mut runtime, 42, 7, 0), Err(5));
}

fn runtime(kind: LinuxFileKind, resource: Word) -> Runtime {
    reset();
    Runtime {
        posix_port: 20,
        file: LinuxFile {
            kind,
            posix_fd: 8,
            resource,
        },
    }
}

#[test]
fn regular_file_uses_the_posix_fd_and_bad_fds_never_issue_ipc() {
    let mut runtime = runtime(LinuxFileKind::Posix, 0);
    assert_eq!(
        production::sys_fsync(&mut runtime, 42, usize::MAX),
        Err(EBADF)
    );
    assert_eq!(production::sys_fsync(&mut runtime, 41, 7), Err(EBADF));
    BACKEND.with(|backend| assert!(backend.borrow().calls.is_empty()));
    assert_eq!(production::sys_fsync(&mut runtime, 42, 7), Ok(0));
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        assert_eq!(backend.calls[0].code, posix::POSIX_REQUEST_FSYNC);
        assert_eq!(backend.calls[0].handle, 8);
    });
}

#[test]
fn overlay_root_synchronizes_real_directory_entries_but_not_arbitrary_virtual_nodes() {
    let mut runtime = runtime(LinuxFileKind::VirtualDirectory, VirtualNode::Root.id());
    assert_eq!(production::sys_fsync(&mut runtime, 42, 7), Ok(0));
    BACKEND.with(|backend| assert_eq!(backend.borrow().calls[0].code, posix::POSIX_REQUEST_SYNC));
    runtime.file.resource = 10; // /proc, not the real guest-root overlay.
    assert_eq!(production::sys_fsync(&mut runtime, 42, 7), Err(EINVAL));
    runtime.file.kind = LinuxFileKind::VirtualFile;
    assert_eq!(production::sys_fsync(&mut runtime, 42, 7), Err(EINVAL));
    BACKEND.with(|backend| assert_eq!(backend.borrow().calls.len(), 1));
}

#[test]
fn fd_sync_reports_errors_while_linux_sync_has_no_error_return() {
    let mut runtime = runtime(LinuxFileKind::Posix, 0);
    BACKEND.with(|backend| backend.borrow_mut().result = Some(Err(RequestError::Transport)));
    assert_eq!(production::sys_fsync(&mut runtime, 42, 7), Err(5));
    assert_eq!(production::sys_sync(&mut runtime), Ok(0));
    runtime.file.kind = LinuxFileKind::VirtualDirectory;
    runtime.file.resource = VirtualNode::Root.id();
    assert_eq!(production::sys_fsync(&mut runtime, 42, 7), Err(5));
}
