//! Production POSIX namespace proxy, descriptor allocator and state; IPC mocked.
#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_runtime;
type Word = usize;
const OS_RESPONSE_OK: Word = 0;
const OS_RESPONSE_INVALID_ARGUMENT: Word = 1;
const OS_RESPONSE_INVALID_DESCRIPTOR: Word = 3;
const OS_RESPONSE_FATAL: Word = 5;
#[derive(Clone, Copy)]
enum RequestError { Status(Word) }
mod ipc {
    #[derive(Clone, Copy, Default)]
    pub struct ServiceRequest {
        pub identifier: usize, pub code: usize, pub arg0: usize,
        pub arg1: usize, pub arg2: usize, pub arg3: usize,
    }
}
use ipc::ServiceRequest;
mod posix { pub use nanami_uapi::posix::*; }
use posix::*;
mod vfs {
    pub use nanami_uapi::vfs::*;
    use super::*;
    pub enum PathError { Filesystem(Word), Transport(RequestError) }
    pub fn vfs_path_request(_: Word, offset: Word) -> Result<(Word, Word), PathError> {
        BACKEND.with(|state| {
            let mut state = state.borrow_mut();
            let request = unsafe { ((state.shm + offset) as *const PathRequest).read_unaligned() };
            state.requests.push(request);
            let descriptor = state.descriptor;
            let base = state.shm + request.output_offset;
            let prefix = if request.operation == VFS_PATH_OPEN_READONLY { std::mem::size_of::<PathOpenResult>() } else { 0 };
            unsafe {
                if prefix != 0 {
                    (base as *mut PathOpenResult).write_unaligned(PathOpenResult { descriptor });
                }
                std::ptr::copy_nonoverlapping(state.path.as_ptr(),
                    (base + prefix) as *mut u8, state.path.len());
            }
            Ok((state.reply_bytes.unwrap_or(prefix + state.path.len()),
                state.kind))
        })
    }
    pub fn vfs_close(_: Word, fd: Word) -> Result<(), RequestError> {
        BACKEND.with(|state| state.borrow_mut().closed.push(fd));
        Ok(())
    }
    pub fn vfs_fchmod(_: Word, _: Word, _: Word) -> Result<(), PathError> { Ok(()) }
}
use vfs::*;
struct Backend {
    shm: Word, requests: Vec<PathRequest>, closed: Vec<Word>,
    descriptor: Word, kind: Word, path: Vec<u8>, reply_bytes: Option<Word>,
}
thread_local! {
    static BACKEND: std::cell::RefCell<Backend> = std::cell::RefCell::new(Backend {
        shm: 0, requests: Vec::new(), closed: Vec::new(), descriptor: 7,
        kind: VFS_FILE_TYPE_REGULAR, path: b"/lib/file".to_vec(), reply_bytes: None,
    });
}
#[path = "../../nanami/servers/apps/posix-server/src/state.rs"]
mod state;
use state::*;
mod process {
    use super::*;
    pub fn find_session(runtime: &mut Runtime, owner: Word) -> Option<usize> {
        runtime.sessions.iter().position(|session| session.active && session.owner_pid == owner)
    }
}
use process::find_session;
mod flock { pub fn notify(_: &super::Runtime) {} }
fn map_request_error_to_status(error: RequestError) -> Word {
    match error { RequestError::Status(status) => status }
}
#[path = "../../nanami/servers/apps/posix-server/src/fd.rs"]
mod fd;
use fd::*;
#[path = "../../nanami/servers/apps/posix-server/src/namespace.rs"]
mod namespace;

struct Fixture { runtime: Box<Runtime>, client: Vec<u8>, backend: Vec<u8> }
impl Fixture {
    fn new() -> Self {
        let mut client = vec![0; 32768];
        let mut backend = vec![0; 32768];
        BACKEND.with(|state| *state.borrow_mut() = Backend {
            shm: backend.as_mut_ptr() as Word, requests: Vec::new(), closed: Vec::new(),
            descriptor: 7, kind: VFS_FILE_TYPE_REGULAR, path: b"/lib/file".to_vec(), reply_bytes: None,
        });
        let mut runtime = Box::new(Runtime {
            vfs_port: 1, vfs_shm: backend.as_mut_ptr() as Word, vfs_shm_size: backend.len(),
            sessions: [Session::EMPTY; MAX_SESSIONS], open_files: [OpenFile::EMPTY; MAX_OPEN_FILES],
            next_posix_pid: 100, cached_owner_pid: 0, cached_session_index: INVALID_SESSION_INDEX,
        });
        runtime.sessions[0].active = true;
        runtime.sessions[0].owner_pid = 16;
        runtime.sessions[0].shm_local = client.as_mut_ptr() as Word;
        runtime.sessions[0].shm_size = client.len();
        let request = PathRequest {
            operation: VFS_PATH_OPEN_READONLY, directory: VFS_PATH_DIRECTORY_ROOT,
            root_offset: 128, root_bytes: 5, path_offset: 256, path_bytes: 4,
            target_offset: 512, target_bytes: std::mem::size_of::<Word>(),
            output_offset: 1024, output_bytes: 4096,
            flags: VFS_PATH_FOLLOW_FINAL | VFS_PATH_ALLOW_MISSING,
        };
        client[128..133].copy_from_slice(b"/root");
        client[256..260].copy_from_slice(b"file");
        unsafe { (client.as_mut_ptr() as *mut PathRequest).write_unaligned(request); }
        Self { runtime, client, backend }
    }
    fn request(&self) -> PathRequest { unsafe { (self.client.as_ptr() as *const PathRequest).read_unaligned() } }
    fn set_request(&mut self, request: PathRequest) {
        unsafe { (self.client.as_mut_ptr() as *mut PathRequest).write_unaligned(request); }
    }
    fn handle(&mut self, owner: Word) -> (Word, Word, Word) {
        namespace::handle(&mut self.runtime, ServiceRequest { identifier: owner,
            code: POSIX_REQUEST_OPEN_ROOTED, ..ServiceRequest::default() })
    }
    fn descriptor(&self) -> Word {
        unsafe { (self.client.as_ptr().add(1024) as *const PathOpenResult).read_unaligned().descriptor }
    }
}

#[test]
fn open_translates_backend_handle_into_the_callers_owned_fd_once() {
    for kind in [VFS_FILE_TYPE_REGULAR, VFS_FILE_TYPE_DIRECTORY] {
        let mut fixture = Fixture::new();
        BACKEND.with(|state| state.borrow_mut().kind = kind);
        let result = fixture.handle(16);
        assert_eq!(result.0, OS_RESPONSE_OK);
        assert_eq!(result.2, kind);
        assert_eq!(fixture.descriptor(), 3); // The real allocator reserves stdio.
        let file = fixture.runtime.open_files[fixture.runtime.sessions[0].fds[3].open_file];
        assert!(file.active);
        assert_eq!(file.vfs_handle, 7);
        assert_eq!(&fixture.client[1032..1041], b"/lib/file");
        BACKEND.with(|state| {
            assert_eq!(state.borrow().requests.len(), 1);
            assert!(state.borrow().closed.is_empty());
        });
    }
}

#[test]
fn declined_open_preserves_path_without_allocating_or_closing() {
    let mut fixture = Fixture::new();
    BACKEND.with(|state| state.borrow_mut().descriptor = VFS_PATH_OPEN_NONE);
    assert_eq!(fixture.handle(16).0, OS_RESPONSE_OK);
    assert_eq!(fixture.descriptor(), VFS_PATH_OPEN_NONE);
    assert!(!fixture.runtime.open_files.iter().any(|file| file.active));
    BACKEND.with(|state| assert!(state.borrow().closed.is_empty()));
}

#[test]
fn descriptor_exhaustion_resolves_without_acquiring_so_virtual_fallback_remains_possible() {
    for full_open_files in [false, true] {
        let mut fixture = Fixture::new();
        if full_open_files {
            for file in &mut fixture.runtime.open_files { file.active = true; }
        } else {
            // Ordinary opens reserve0..2 even when those slots are inactive.
            for descriptor in fixture.runtime.sessions[0].fds.iter_mut().skip(3) { descriptor.active = true; }
        }
        assert_eq!(fixture.handle(16).0, OS_RESPONSE_OK);
        assert_eq!(fixture.descriptor(), VFS_PATH_OPEN_NONE);
        assert_eq!(&fixture.client[1032..1041], b"/lib/file");
        BACKEND.with(|state| {
            let state = state.borrow();
            assert!(state.closed.is_empty());
            assert_eq!(state.requests[0].operation, VFS_PATH_RESOLVE);
        });
    }
}

#[test]
fn malformed_backend_replies_close_the_handle_before_returning_an_error() {
    for case in 0..4 {
        let mut fixture = Fixture::new();
        BACKEND.with(|state| {
            let mut state = state.borrow_mut();
            match case {
                0 => state.reply_bytes = Some(8),
                1 => state.reply_bytes = Some(4097),
                2 => state.path = b"relative".to_vec(),
                _ => state.kind = VFS_FILE_TYPE_SYMLINK,
            }
        });
        assert_eq!(fixture.handle(16).0, VFS_PATH_RESPONSE_ERROR);
        BACKEND.with(|state| assert_eq!(state.borrow().closed, [7]));
    }
}

#[test]
fn owner_and_relative_directory_validation_precede_backend_access() {
    let mut fixture = Fixture::new();
    assert_eq!(fixture.handle(17).0, VFS_PATH_RESPONSE_ERROR);
    let mut request = fixture.request(); request.directory = 3; fixture.set_request(request);
    assert_eq!(fixture.handle(16).0, VFS_PATH_RESPONSE_ERROR);
    BACKEND.with(|state| assert!(state.borrow().requests.is_empty()));
    fixture.runtime.sessions[0].fds[3] = FileDescriptor { active: true, open_file: 0, flags: 0 };
    fixture.runtime.open_files[0] = OpenFile { active: true, kind: FdKind::Directory, vfs_handle: 19, ..OpenFile::EMPTY };
    assert_eq!(fixture.handle(16).0, OS_RESPONSE_OK);
    BACKEND.with(|state| assert_eq!(state.borrow().requests[0].directory, 19));
}

#[test]
fn legacy_entry_rejects_open_before_acquiring_a_backend_handle() {
    let mut fixture = Fixture::new();
    let reply = namespace::handle(&mut fixture.runtime, ServiceRequest {
        identifier: 16, code: POSIX_REQUEST_PATH, ..ServiceRequest::default()
    });
    assert_eq!(reply.0, VFS_PATH_RESPONSE_ERROR);
    assert_eq!(reply.1, VFS_PATH_ERROR_UNSUPPORTED);
    BACKEND.with(|state| assert!(state.borrow().requests.is_empty()));
}

#[test]
fn invalid_open_options_are_rejected_even_when_only_resolution_would_be_possible() {
    for full in [false, true] {
        for case in 0..3 {
            let mut fixture = Fixture::new();
            if full { for fd in &mut fixture.runtime.sessions[0].fds { fd.active = true; } }
            let mut request = fixture.request();
            match case {
                0 => request.target_bytes = 0,
                1 => request.flags |= VFS_PATH_WITH_METADATA,
                _ => fixture.client[512..520].copy_from_slice(&VFS_OPEN_CREATE.to_le_bytes()),
            }
            fixture.set_request(request);
            assert_eq!(fixture.handle(16).0, VFS_PATH_RESPONSE_ERROR);
            BACKEND.with(|state| assert!(state.borrow().requests.is_empty()));
        }
    }
}
