//! Production rooted lookup, wire decoding and RPC counts; transport is mocked.
#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_runtime;
use std::cell::RefCell;
thread_local! {
    static CWD_ALLOCATIONS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}
struct CountingAllocator;
unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        let _ = CWD_ALLOCATIONS.try_with(|count| {
            if let Some(value) = count.get() { count.set(Some(value + 1)); }
        });
        unsafe { std::alloc::GlobalAlloc::alloc(&std::alloc::System, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: std::alloc::Layout, bytes: usize) -> *mut u8 {
        let _ = CWD_ALLOCATIONS.try_with(|count| {
            if let Some(value) = count.get() { count.set(Some(value + 1)); }
        });
        unsafe { std::alloc::GlobalAlloc::realloc(&std::alloc::System, pointer, layout, bytes) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: std::alloc::Layout) {
        unsafe { std::alloc::GlobalAlloc::dealloc(&std::alloc::System, pointer, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
type Word = usize;
#[cfg(feature = "wait-shape")]
mod wait_shape {
    pub(super) fn path_step(_: &mut super::Runtime, _: usize, _: usize) {}
}
const EBADF: i32 = 9;
const ENOENT: i32 = 2;
const ESRCH: i32 = 3;
const ENOTDIR: i32 = 20;
const EINVAL: i32 = 22;
const EFAULT: i32 = 14;
const EIO: i32 = 5;
const ELOOP: i32 = 40;
const ENAMETOOLONG: i32 = 36;
const EEXIST: i32 = 17;
const ENOSPC: i32 = 28;
const EROFS: i32 = 30;
const EOPNOTSUPP: i32 = 95;
const EMFILE: i32 = 24;
const LINUX_O_ACCMODE: Word = 3;
const LINUX_O_CREAT: Word = 0o100;
const LINUX_O_TRUNC: Word = 0o1000;
const LINUX_O_APPEND: Word = 0o2000;
const LINUX_O_NONBLOCK: Word = 0o4000;
const LINUX_O_DSYNC: Word = 0o10000;
const LINUX_O_SYNC: Word = 0o4010000;
const LINUX_O_CLOEXEC: Word = 0o2000000;
const LINUX_O_LARGEFILE: Word = 0o100000;
const LINUX_O_DIRECTORY: Word = 0o200000;
const LINUX_O_NOFOLLOW: Word = 0o400000;
const LINUX_FD_CLOEXEC: Word = 1;
const OS_RESPONSE_INVALID_DESCRIPTOR: Word = 3;
enum RequestError { Status(Word) }
const LINUX_CWD_MAX: usize = 128;
const LINUX_AT_FDCWD: Word = (-100i32) as Word;
mod posix {
    pub use nanami_uapi::posix::*;
    pub fn posix_stat(_: usize, _: usize, _: usize) -> Result<(usize, usize, usize, usize, usize), super::RequestError> {
        super::PROVIDER.with(|provider| {
            let mut p = provider.borrow_mut();
            p.legacy_stats += 1;
            if let Some(code) = p.stat_error {
                return Err(super::RequestError::Status(code));
            }
            Ok((0, 0, p.stat_kind, 0, 0))
        })
    }
    pub fn posix_close(_: usize, fd: usize) -> Result<(), i32> {
        super::PROVIDER.with(|provider| provider.borrow_mut().closed.push(fd));
        Ok(())
    }
    pub fn posix_open_mode(_: usize, offset: usize, bytes: usize, flags: usize, _: usize) -> Result<usize, super::RequestError> {
        super::PROVIDER.with(|provider| {
            let mut p = provider.borrow_mut();
            p.legacy_opens += 1;
            p.backend_open_flags = Some(flags);
            let path = unsafe { std::slice::from_raw_parts((p.shm + offset) as *const u8, bytes) };
            p.opened_path[..bytes].copy_from_slice(path);
            p.opened_path_len = bytes;
        });
        Ok(21)
    }
}
mod arch {
    pub const CPU_INFO: &[u8] = b"test";
    pub const PROC_VERSION: &[u8] = b"test";
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/virtual_fs.rs"]
mod virtual_fs;
use virtual_fs::VirtualNode;
mod vfs {
    use super::*;
    pub use nanami_uapi::vfs::*;
    pub enum PathError {
        Transport(i32),
        Filesystem(Word),
    }
    pub fn posix_rooted_open_request(port: Word, descriptor: Word) -> Result<(Word, Word), PathError> {
        posix_path_request(port, descriptor)
    }
    pub fn posix_path_request(_: Word, descriptor: Word) -> Result<(Word, Word), PathError> {
        PROVIDER.with(|p| {
            let mut p = p.borrow_mut();
            let request = unsafe { ((p.shm + descriptor) as *const PathRequest).read_unaligned() };
            let root = unsafe {
                std::slice::from_raw_parts(
                    (p.shm + request.root_offset) as *const u8,
                    request.root_bytes,
                )
            };
            assert_eq!(root, b"/test/rootfs");
            let input = unsafe {
                std::slice::from_raw_parts(
                    (p.shm + request.path_offset) as *const u8,
                    request.path_bytes,
                )
            }
            .to_vec();
            p.inputs.push(input);
            p.requests.push(request);
            let target = unsafe { std::slice::from_raw_parts((p.shm + request.target_offset) as *const u8, request.target_bytes) };
            p.captured_target[..target.len()].copy_from_slice(target);
            p.captured_target_len = target.len();
            if let Some(error) = p.error {
                if p.error_operation.is_none_or(|operation| operation == request.operation) {
                    return Err(PathError::Filesystem(error));
                }
            }
            if request.operation == VFS_PATH_READLINK && matches!(p.reply_type.unwrap_or(p.metadata.file_type), VFS_FILE_TYPE_REGULAR | VFS_FILE_TYPE_DIRECTORY | VFS_FILE_TYPE_SOCKET) {
                return Err(PathError::Filesystem(VFS_PATH_ERROR_INVALID));
            }
            // Like the actual proxy, mutation replies carry details but no
            // path payload. Do not let this mock fabricate a data reply.
            if request.operation == VFS_PATH_OPEN_READONLY && p.unsupported_open {
                return Err(PathError::Filesystem(VFS_PATH_ERROR_UNSUPPORTED));
            }
            if !matches!(request.operation, VFS_PATH_RESOLVE | VFS_PATH_READLINK | VFS_PATH_OPEN_READONLY) {
                return Ok((0, 0));
            }
            let metadata_bytes = if request.operation == VFS_PATH_OPEN_READONLY {
                std::mem::size_of::<PathOpenResult>()
            } else if request.flags & VFS_PATH_WITH_METADATA != 0 {
                std::mem::size_of::<PathMetadata>()
            } else {
                0
            };
            let payload = if request.operation == VFS_PATH_READLINK {
                &p.link_target[..p.link_target.len().min(request.output_bytes)]
            } else { &p.path[..] };
            unsafe {
                if request.operation == VFS_PATH_OPEN_READONLY {
                    ((p.shm + request.output_offset) as *mut PathOpenResult)
                        .write_unaligned(PathOpenResult { descriptor: p.opened });
                } else if metadata_bytes != 0 {
                    ((p.shm + request.output_offset) as *mut PathMetadata)
                        .write_unaligned(p.metadata);
                }
                std::ptr::copy_nonoverlapping(
                    payload.as_ptr(),
                    (p.shm + request.output_offset + metadata_bytes) as *mut u8,
                    payload.len(),
                );
            }
            Ok((
                p.reply_bytes.unwrap_or(metadata_bytes + payload.len()),
                p.reply_type.unwrap_or(p.metadata.file_type),
            ))
        })
    }
}
use vfs::*;
struct Provider {
    shm: Word,
    requests: Vec<PathRequest>,
    inputs: Vec<Vec<u8>>,
    metadata: PathMetadata,
    path: Vec<u8>,
    link_target: Vec<u8>,
    error: Option<Word>,
    error_operation: Option<Word>,
    captured_target: [u8; 4096],
    captured_target_len: usize,
    reply_bytes: Option<Word>,
    reply_type: Option<Word>,
    opened: Word,
    unsupported_open: bool,
    closed: Vec<Word>,
    legacy_opens: usize,
    backend_open_flags: Option<Word>,
    legacy_stats: usize,
    stat_kind: Word,
    stat_error: Option<Word>,
    translate_host: bool,
    clobber_nofollow_stat: bool,
    nofollow_stat_calls: usize,
    opened_path: [u8; 256],
    opened_path_len: usize,
}
impl Provider {
    fn new(shm: Word) -> Self {
        Self {
            shm,
            requests: Vec::new(),
            inputs: Vec::new(),
            metadata: PathMetadata {
                inode: 42,
                size: 123,
                mode: 0o100640,
                file_type: VFS_FILE_TYPE_REGULAR,
            },
            path: b"/lib/value".to_vec(),
            link_target: b"target".to_vec(),
            error: None,
            error_operation: None,
            captured_target: [0; 4096],
            captured_target_len: 0,
            reply_bytes: None,
            reply_type: None,
            opened: 7,
            unsupported_open: false,
            closed: Vec::new(),
            legacy_opens: 0,
            backend_open_flags: None,
            legacy_stats: 0,
            stat_kind: posix::POSIX_FILE_TYPE_DIRECTORY,
            stat_error: None,
            translate_host: false,
            clobber_nofollow_stat: false,
            nofollow_stat_calls: 0,
            opened_path: [0; 256],
            opened_path_len: 0,
        }
    }
}
thread_local! { static PROVIDER: RefCell<Provider> = RefCell::new(Provider::new(0)); }
#[derive(Clone, Copy, PartialEq)]
enum LinuxFileKind {
    Empty,
    Posix,
    VirtualDirectory,
}
#[derive(Clone, Copy)]
struct LinuxFile {
    kind: LinuxFileKind,
    posix_fd: Word,
    resource: Word,
    flags: Word,
    status: Word,
}
impl LinuxFile {
    fn posix(fd: Word, flags: Word, status: Word) -> Self {
        Self { kind: LinuxFileKind::Posix, posix_fd: fd, resource: 0, flags, status }
    }
    fn has_posix_backend(&self) -> bool {
        self.kind == LinuxFileKind::Posix
    }
}
struct Rootfs {
    readonly: bool,
}
struct Process {
    cwd: [u8; LINUX_CWD_MAX],
    cwd_len: usize,
    rootfs: Rootfs,
}
struct Runtime {
    posix_port: Word,
    posix_shm: Word,
    posix_shm_size: Word,
    process: Process,
    file: LinuxFile,
    buffer: Box<[u8; 65536]>,
    allocation_full: bool,
    allocated: Vec<LinuxFile>,
    cwd_source: Word,
    deny_cwd: bool,
}
impl Runtime {
    fn new() -> Self {
        let mut buffer = Box::new([0; 65536]);
        let shm = buffer.as_mut_ptr() as Word;
        PROVIDER.with(|p| *p.borrow_mut() = Provider::new(shm));
        let mut process = Process {
            cwd: [0; LINUX_CWD_MAX],
            cwd_len: 4,
            rootfs: Rootfs { readonly: false },
        };
        process.cwd[..4].copy_from_slice(b"/lib");
        Self {
            posix_port: 1,
            posix_shm: shm,
            posix_shm_size: buffer.len(),
            process,
            file: LinuxFile {
                kind: LinuxFileKind::Posix,
                posix_fd: 9,
                resource: 0,
                flags: 0,
                status: 0,
            },
            buffer,
            allocation_full: false,
            allocated: Vec::new(),
            cwd_source: 0,
            deny_cwd: false,
        }
    }
    fn path(&mut self, path: &[u8]) -> usize {
        self.buffer[..path.len()].copy_from_slice(path);
        path.len()
    }
    fn managed_process(&self, pid: Word) -> Option<&Process> {
        (pid == 1).then_some(&self.process)
    }
    fn linux_file(&self, _: Word, fd: Word) -> Option<LinuxFile> {
        (fd == 3).then_some(self.file)
    }
    fn set_cwd(&mut self, pid: Word, path: &[u8]) -> bool {
        if self.deny_cwd || pid != 1 || path.len() >= self.process.cwd.len() {
            return false;
        }
        self.process.cwd[..path.len()].copy_from_slice(path);
        self.process.cwd_len = path.len();
        self.cwd_source = path.as_ptr() as Word;
        true
    }
    fn allocate_linux_file(&mut self, _: Word, file: LinuxFile, _: Word) -> Option<Word> {
        if self.allocation_full { return None; }
        self.allocated.push(file);
        Some(3)
    }
}
fn map_path_request_error(_: RequestError) -> i32 { EIO }
use opening::translate_open_flags;
fn translate_guest_path_for_vfs(runtime: &mut Runtime, _: Word, bytes: Word) -> Result<Word, i32> {
    if PROVIDER.with(|provider| provider.borrow().translate_host) {
        let prefix = b"/test/rootfs";
        runtime.buffer.copy_within(..bytes, prefix.len());
        runtime.buffer[..prefix.len()].copy_from_slice(prefix);
        return Ok(prefix.len() + bytes);
    }
    Ok(bytes)
}
fn current_virtual_node(runtime: &Runtime, _: Word, bytes: Word) -> Option<VirtualNode> {
    let path = unsafe { std::slice::from_raw_parts(runtime.posix_shm as *const u8, bytes) };
    if path == b"/" { Some(VirtualNode::Root) } else { None }
}
fn open_virtual_path(runtime: &mut Runtime, _: Word, bytes: Word, _: Word) -> Result<Option<Word>, i32> {
    Ok(if current_virtual_node(runtime, 1, bytes).is_some() || is_linux_virtual_path(runtime.posix_shm, bytes) {
        Some(33)
    } else { None })
}
fn open_virtual_node(_: &mut Runtime, _: Word, _: VirtualNode, _: Word) -> Result<Word, i32> { Ok(33) }
fn stat_current_path(runtime: &mut Runtime, _: Word, _: Word) -> Result<(Word, Word, Word, Word, Word), i32> {
    PROVIDER.with(|provider| {
        let mut p = provider.borrow_mut();
        p.nofollow_stat_calls += 1;
        if p.clobber_nofollow_stat { runtime.buffer[..256].fill(0xda); }
        if p.clobber_nofollow_stat && p.stat_error.is_some() { return Err(EIO); }
        let kind = p.metadata.file_type;
        Ok((0, 0, if kind == VFS_FILE_TYPE_SYMLINK { posix::POSIX_FILE_TYPE_SYMLINK } else { posix::POSIX_FILE_TYPE_REGULAR }, 0, 0))
    })
}
fn map_request_error(error: i32) -> i32 {
    error
}
fn guest_root_for_process(_: &Runtime, _: Word) -> Result<&[u8], i32> {
    Ok(b"/test/rootfs")
}
fn is_at_fdcwd(fd: Word) -> bool {
    fd as i32 == -100
}
fn is_linux_virtual_path(base: Word, len: Word) -> bool {
    let path = unsafe { std::slice::from_raw_parts(base as *const u8, len) };
    [b"/dev".as_slice(), b"/proc", b"/sys"]
        .iter()
        .any(|prefix| {
            path == *prefix || (path.starts_with(prefix) && path.get(prefix.len()) == Some(&b'/'))
        })
}
fn resolve_lexical_shm_path(_: &Runtime, _: Word, len: Word) -> Result<Word, i32> {
    Ok(len)
}
fn reject_virtual_fs_mutation(_: &Runtime, _: Word, _: Word, _: Word) -> Result<(), i32> {
    Ok(())
}
fn read_c_string(runtime: &Runtime, _: Word, pointer: Word) -> Result<Word, i32> {
    if pointer == 0 { return Err(EFAULT); }
    for index in 0..LINUX_CWD_MAX {
        let byte = unsafe { ((pointer + index) as *const u8).read() };
        if byte == 0 { return Ok(index); }
        unsafe { ((runtime.posix_shm + index) as *mut u8).write(byte); }
    }
    Err(ENAMETOOLONG)
}
// The real virtual/proc fallback is KVM-tested. Here use the production rooted
// request transport and a backend model so tests verify the wrapper's RPC and
// error order, without fabricating the candidate's negative type decision.
mod filesystem {
    use super::*;
    pub fn readlink_from_current_path(r: &mut Runtime, pid: Word, size: Word, output: Word, bytes: Word) -> Result<Word, i32> {
        if output == 0 && bytes != 0 { return Err(EFAULT); }
        if bytes == 0 { return Err(EINVAL); }
        let count = namespace::readlink_current_path(r, pid, size, bytes.min(4096))?;
        unsafe { std::ptr::copy_nonoverlapping(r.posix_shm as *const u8, output as *mut u8, count); }
        Ok(count)
    }
    pub fn creation_mode(_: &Runtime, _: Word, mode: Word) -> Result<Word, i32> { Ok(mode) }
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/readlink.rs"]
mod readlink;
mod procfs {
    use super::*;
    pub fn validate(_: &Runtime, _: Word, _: Word) -> Result<(), i32> {
        Ok(())
    }
    pub fn node(id: Word) -> Option<VirtualNode> {
        VirtualNode::from_id(id)
    }
    pub fn directory_path(_: Word, _: &mut [u8; 64]) -> Option<usize> {
        None
    }
}
mod metadata {
    use super::*;
    pub fn stat_resolved_path(
        _: &mut Runtime,
        _: Word,
        resolved: namespace::ResolvedPath,
    ) -> Result<(Word, Word, Word, Word, Word), i32> {
        if let Some(metadata) = resolved.metadata {
            return namespace::decode_metadata(metadata)?.ok_or(ENOENT);
        }
        Ok((999, 0, posix::POSIX_FILE_TYPE_REGULAR, 0, 0))
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/namespace.rs"]
mod namespace;
use namespace::resolve_at;

#[test]
fn direct_reply_decode_preserves_prefix_tail_and_public_readlink_staging() {
    let mut staging = Vec::new();
    for mode in 0..4 {
        let mut r = Runtime::new();
        if mode == 3 {
            PROVIDER.with(|provider| provider.borrow_mut().reply_type = Some(VFS_FILE_TYPE_SYMLINK));
        }
        r.buffer.fill(0xa5);
        let input = r.path(b"value");
        let mut expected = r.buffer[..256].to_vec();
        let canonical = b"/lib/value";
        let mut reply = Vec::new();
        if mode == 0 {
            for word in [42usize, 123, 0o100640, VFS_FILE_TYPE_REGULAR] {
                reply.extend_from_slice(&word.to_ne_bytes());
            }
        } else if mode == 1 {
            reply.extend_from_slice(&7usize.to_ne_bytes());
        }
        reply.extend_from_slice(if mode == 3 { b"target" } else { canonical });
        expected[..reply.len()].copy_from_slice(&reply);
        if mode != 3 {
            expected[..canonical.len()].copy_from_slice(canonical);
            expected[canonical.len()] = 0;
        }
        namespace::take_reply_staging_bytes();
        match mode {
            0 => { namespace::stat_at(&mut r, 1, LINUX_AT_FDCWD, input, true).unwrap(); }
            1 => assert_eq!(namespace::resolve_for_open(&mut r, 1, LINUX_AT_FDCWD, input, true, 0).unwrap().opened, Some(7)),
            2 => assert_eq!(namespace::resolve_at(&mut r, 1, LINUX_AT_FDCWD, input, true).unwrap(), canonical.len()),
            _ => { namespace::request_path(&mut r, 1, VFS_PATH_READLINK, VFS_PATH_DIRECTORY_ROOT,
                b"/link", &[], VFS_PATH_FOLLOW_FINAL, 128).unwrap(); }
        }
        staging.push(namespace::take_reply_staging_bytes());
        assert_eq!(&r.buffer[..256], expected);
    }
    println!("ROOTED-REPLY-DIRECT staged-reply-bytes-stat/open/resolve/readlink={staging:?}");
    assert_eq!(staging, [0, 0, 0, 6]);
}

#[test]
fn direct_reply_errors_keep_old_staged_prefix_and_owned_fd_cleanup() {
    for open in [false, true] {
        for fault in 0..3 {
            let mut r = Runtime::new();
            r.buffer.fill(0xa5);
            let input = r.path(b"value");
            let mut expected = r.buffer[..256].to_vec();
            let prefix = if open { std::mem::size_of::<PathOpenResult>() } else { std::mem::size_of::<PathMetadata>() };
            let path = if fault == 0 { b"relative".as_slice() } else { b"/lib/value".as_slice() };
            let mut wire = Vec::new();
            if open {
                wire.extend_from_slice(&7usize.to_ne_bytes());
            } else {
                for word in [42usize, 123, 0o100640, VFS_FILE_TYPE_REGULAR] {
                    wire.extend_from_slice(&word.to_ne_bytes());
                }
            }
            wire.extend_from_slice(path);
            let reported = if fault == 1 { prefix } else { wire.len() };
            PROVIDER.with(|provider| {
                let mut p = provider.borrow_mut();
                p.path = path.to_vec();
                p.reply_bytes = Some(reported);
                if fault == 2 {
                    p.reply_type = Some(if open { VFS_FILE_TYPE_SOCKET } else { VFS_FILE_TYPE_DIRECTORY });
                }
            });
            // Too-short open replies are rejected in the request layer before
            // staging. Other errors historically exposed the complete wire prefix.
            if !(open && fault == 1) {
                expected[..reported].copy_from_slice(&wire[..reported]);
            }
            if open {
                assert!(matches!(namespace::resolve_for_open(&mut r, 1, LINUX_AT_FDCWD, input, true, 0), Err(EIO)));
            } else {
                assert_eq!(namespace::stat_at(&mut r, 1, LINUX_AT_FDCWD, input, true), Err(EIO));
            }
            assert_eq!(&r.buffer[..256], expected, "open={open} fault={fault}");
            PROVIDER.with(|provider| assert_eq!(provider.borrow().closed, if open { vec![7] } else { vec![] }));
        }
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/opening.rs"]
mod opening;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/path_checks.rs"]
mod path_checks;

#[test]
fn cwd_commit_borrows_resolved_shm_and_removes_the_fchdir_vector() {
    let mut counts = Vec::new();
    let mut borrowed = Vec::new();
    for mode in 0..3 {
        let mut r = Runtime::new();
        let canonical = if mode == 2 { b"/".as_slice() } else { b"/cwd-target".as_slice() };
        PROVIDER.with(|provider| {
            let mut p = provider.borrow_mut();
            p.path = canonical.to_vec();
            p.metadata.file_type = if mode == 2 { VFS_FILE_TYPE_REGULAR } else { VFS_FILE_TYPE_DIRECTORY };
            p.requests.reserve(4);
            p.inputs.reserve(4);
        });
        let bytes = r.path(b"/some-link");
        CWD_ALLOCATIONS.with(|count| count.set(Some(0)));
        let result = if mode == 0 { namespace::fchdir(&mut r, 1, 3) } else { path_checks::chdir(&mut r, 1, bytes) };
        let allocations = CWD_ALLOCATIONS.with(|count| count.replace(None).unwrap());
        assert_eq!(result, Ok(0));
        assert_eq!(&r.process.cwd[..r.process.cwd_len], canonical);
        counts.push(allocations);
        borrowed.push(r.cwd_source == r.posix_shm);
    }
    println!("CWD-PATH-BORROW fchdir/chdir/virtual-allocations={counts:?} commit-from-shm={borrowed:?}");
    // One allocation remains in the transport mock's captured input Vec.
    assert_eq!(counts, [1, 1, 1]);
    assert_eq!(borrowed, [true, true, true]);
}

#[test]
fn legacy_chdir_stages_guest_cwd_across_translation_and_keeps_failure_state() {
    for kind in [posix::POSIX_FILE_TYPE_DIRECTORY, posix::POSIX_FILE_TYPE_REGULAR] {
        for error in [None, Some(99)] {
            let mut r = Runtime::new();
            PROVIDER.with(|provider| {
                let mut p = provider.borrow_mut();
                p.path = b"/guest-target".to_vec();
                p.metadata.file_type = VFS_FILE_TYPE_UNKNOWN;
                p.translate_host = true;
                p.stat_kind = kind;
                p.stat_error = error;
            });
            let bytes = r.path(b"/some-link");
            let result = path_checks::chdir(&mut r, 1, bytes);
            let expected = if error.is_some() { Err(EIO) } else if kind == posix::POSIX_FILE_TYPE_DIRECTORY { Ok(0) } else { Err(ENOTDIR) };
            assert_eq!(result, expected);
            assert!(r.buffer.starts_with(b"/test/rootfs/guest-target"));
            if result.is_ok() {
                assert_eq!(&r.process.cwd[..r.process.cwd_len], b"/guest-target");
                assert_ne!(r.cwd_source, r.posix_shm);
            } else {
                assert_eq!(&r.process.cwd[..r.process.cwd_len], b"/lib");
                assert_eq!(r.cwd_source, 0);
            }
        }
    }
    for fchdir in [false, true] {
        let mut r = Runtime::new();
        r.deny_cwd = true;
        PROVIDER.with(|provider| provider.borrow_mut().metadata.file_type = VFS_FILE_TYPE_DIRECTORY);
        let bytes = r.path(b"/some-link");
        let result = if fchdir { namespace::fchdir(&mut r, 1, 3) } else { path_checks::chdir(&mut r, 1, bytes) };
        assert_eq!(result, Err(if fchdir { ENAMETOOLONG } else { EINVAL }));
        assert_eq!(&r.process.cwd[..r.process.cwd_len], b"/lib");
    }
}

#[test]
fn chdir_uses_rooted_kind_and_preserves_the_canonical_cwd() {
    let mut runtime = Runtime::new();
    PROVIDER.with(|p| {
        let mut p = p.borrow_mut();
        p.path = b"/real-directory".to_vec();
        p.metadata.file_type = VFS_FILE_TYPE_DIRECTORY;
    });
    let bytes = runtime.path(b"/directory-link");
    assert_eq!(path_checks::chdir(&mut runtime, 1, bytes), Ok(0));
    assert_eq!(&runtime.process.cwd[..runtime.process.cwd_len], b"/real-directory");
    PROVIDER.with(|p| {
        let p = p.borrow();
        assert_eq!(p.requests.len(), 1);
        assert_eq!(p.legacy_stats, 0);
        assert_ne!(p.requests[0].flags & VFS_PATH_FOLLOW_FINAL, 0);
    });
}

#[test]
fn chdir_rejects_known_non_directories_without_a_second_lookup() {
    for kind in [VFS_FILE_TYPE_REGULAR, VFS_FILE_TYPE_SOCKET, VFS_FILE_TYPE_SYMLINK] {
        let mut runtime = Runtime::new();
        PROVIDER.with(|p| p.borrow_mut().metadata.file_type = kind);
        let bytes = runtime.path(b"/not-directory");
        assert_eq!(path_checks::chdir(&mut runtime, 1, bytes), Err(ENOTDIR));
        assert_eq!(&runtime.process.cwd[..runtime.process.cwd_len], b"/lib");
        PROVIDER.with(|p| assert_eq!(p.borrow().legacy_stats, 0));
    }
}

#[test]
fn unknown_kind_keeps_the_original_stat_and_its_errors() {
    for error in [None, Some(99)] {
        let mut runtime = Runtime::new();
        PROVIDER.with(|p| {
            let mut p = p.borrow_mut();
            p.metadata.file_type = VFS_FILE_TYPE_UNKNOWN;
            p.stat_error = error;
        });
        let bytes = runtime.path(b"/legacy-directory");
        assert_eq!(path_checks::chdir(&mut runtime, 1, bytes), if error.is_none() { Ok(0) } else { Err(EIO) });
        PROVIDER.with(|p| assert_eq!(p.borrow().legacy_stats, 1));
    }
}

#[test]
fn virtual_root_takes_precedence_over_the_backing_kind() {
    let mut runtime = Runtime::new();
    PROVIDER.with(|p| { let mut p = p.borrow_mut(); p.path = b"/".to_vec(); p.metadata.file_type = VFS_FILE_TYPE_REGULAR; });
    let bytes = runtime.path(b"/root-link");
    assert_eq!(path_checks::chdir(&mut runtime, 1, bytes), Ok(0));
    assert_eq!(&runtime.process.cwd[..runtime.process.cwd_len], b"/");
    PROVIDER.with(|p| assert_eq!(p.borrow().legacy_stats, 0));
}

#[test]
fn time_check_reuses_kind_and_preserves_dirfd_and_nofollow() {
    for kind in [VFS_FILE_TYPE_REGULAR, VFS_FILE_TYPE_DIRECTORY, VFS_FILE_TYPE_SOCKET] {
        let mut runtime = Runtime::new();
        PROVIDER.with(|p| p.borrow_mut().metadata.file_type = kind);
        let bytes = runtime.path(b"relative");
        assert_eq!(path_checks::check_times(&mut runtime, 1, 3, bytes, false), Ok(0));
        PROVIDER.with(|p| {
            let p = p.borrow();
            assert_eq!(p.requests.len(), 1);
            assert_eq!(p.requests[0].directory, 9);
            assert_eq!(p.requests[0].flags & VFS_PATH_FOLLOW_FINAL, 0);
            assert_eq!(p.legacy_stats, 0);
        });
    }
}

#[test]
fn time_check_keeps_unknown_missing_and_virtual_errors() {
    let mut runtime = Runtime::new();
    PROVIDER.with(|p| { let mut p = p.borrow_mut(); p.metadata.file_type = VFS_FILE_TYPE_UNKNOWN; p.stat_error = Some(99); });
    let bytes = runtime.path(b"/missing");
    assert_eq!(path_checks::check_times(&mut runtime, 1, LINUX_AT_FDCWD, bytes, true), Err(EIO));
    PROVIDER.with(|p| assert_eq!(p.borrow().legacy_stats, 1));
    let bytes = runtime.path(b"/proc/no-such-entry");
    assert_eq!(path_checks::check_times(&mut runtime, 1, LINUX_AT_FDCWD, bytes, true), Err(ENOENT));
}

#[test]
fn nofollow_time_check_on_a_symlink_keeps_legacy_stat_semantics() {
    let mut runtime = Runtime::new();
    PROVIDER.with(|p| { let mut p = p.borrow_mut(); p.metadata.file_type = VFS_FILE_TYPE_SYMLINK; p.stat_error = Some(99); });
    let bytes = runtime.path(b"/dangling");
    assert_eq!(path_checks::check_times(&mut runtime, 1, LINUX_AT_FDCWD, bytes, false), Err(EIO));
    PROVIDER.with(|p| assert_eq!(p.borrow().legacy_stats, 1));
}

#[test]
fn production_open_uses_one_request_and_keeps_local_fd_flags() {
    let enabled = option_env!("NANAMI_ROOTED_READONLY_OPEN") != Some("0");
    let mut runtime = Runtime::new();
    let path = b"value\0";
    assert_eq!(opening::sys_openat(&mut runtime, 1, 3, path.as_ptr() as Word,
        LINUX_O_CLOEXEC | LINUX_O_LARGEFILE, 0), Ok(3));
    assert_eq!(runtime.allocated.len(), 1);
    assert_eq!(runtime.allocated[0].flags, LINUX_FD_CLOEXEC);
    assert_eq!(runtime.allocated[0].status, LINUX_O_CLOEXEC | LINUX_O_LARGEFILE);
    assert_eq!(runtime.allocated[0].posix_fd, if enabled { 7 } else { 21 });
    PROVIDER.with(|p| {
        let p = p.borrow();
        assert_eq!(p.requests.len(), 1);
        assert_eq!(p.requests[0].operation, if enabled { VFS_PATH_OPEN_READONLY } else { VFS_PATH_RESOLVE });
        assert_eq!(p.legacy_opens, usize::from(!enabled));
    });
}

#[test]
fn translated_away_readonly_flags_use_one_request_and_keep_linux_descriptor_state() {
    let enabled = option_env!("NANAMI_ROOTED_READONLY_OPEN") != Some("0");
    // O_NOCTTY, O_NOATIME, O_PATH, O_EXCL without O_CREAT, and an unknown bit
    // were already omitted by legacy backend translation; this changes routing.
    for ignored in [0o400, 0o1000000, 0o10000000, 0o200, 1 << 31] {
        for extra in [0, LINUX_O_CLOEXEC, LINUX_O_DIRECTORY, LINUX_O_NOFOLLOW] {
            let flags = ignored | extra;
            let mut r = Runtime::new();
            if extra == LINUX_O_DIRECTORY {
                PROVIDER.with(|p| p.borrow_mut().metadata.file_type = VFS_FILE_TYPE_DIRECTORY);
            }
            assert_eq!(opening::sys_openat(&mut r, 1, 3, c"value".as_ptr() as Word, flags, 0), Ok(3));
            assert_eq!(r.allocated[0].status, flags);
            assert_eq!(r.allocated[0].flags, if extra == LINUX_O_CLOEXEC { LINUX_FD_CLOEXEC } else { 0 });
            PROVIDER.with(|p| {
                let p = p.borrow(); assert_eq!(p.requests.len(), 1);
                assert_eq!(p.requests[0].operation, if enabled { VFS_PATH_OPEN_READONLY } else { VFS_PATH_RESOLVE });
                assert_eq!(p.legacy_opens, usize::from(!enabled));
                if enabled {
                    let options = unsafe { ((p.shm + p.requests[0].target_offset) as *const Word).read_unaligned() };
                    assert_eq!(options, if extra == LINUX_O_DIRECTORY { VFS_OPEN_DIRECTORY } else { 0 });
                }
            });
        }
    }
}

#[test]
fn readonly_status_flags_keep_the_full_legacy_open_and_backend_flags() {
    for (linux, backend) in [(LINUX_O_NONBLOCK, posix::POSIX_O_NONBLOCK),
        (LINUX_O_APPEND, posix::POSIX_O_APPEND), (LINUX_O_SYNC, posix::POSIX_O_SYNC),
        (LINUX_O_DSYNC, posix::POSIX_O_SYNC), (LINUX_O_CREAT, posix::POSIX_O_CREAT),
        (LINUX_O_TRUNC, posix::POSIX_O_TRUNC)] {
        let mut r = Runtime::new();
        let flags = linux | LINUX_O_CLOEXEC | 0o400;
        assert_eq!(opening::sys_open(&mut r, 1, c"value".as_ptr() as Word, flags, 0), Ok(3));
        assert_eq!(r.allocated[0].status, flags);
        PROVIDER.with(|p| {
            let p = p.borrow(); assert_eq!(p.requests.len(), 1);
            assert_eq!(p.requests[0].operation, VFS_PATH_RESOLVE);
            assert_eq!(p.backend_open_flags, Some(backend));
        });
    }
}

#[test]
fn translated_away_flags_preserve_virtual_mount_precedence() {
    for path in [c"/dev/null", c"/proc/self"] {
        let mut r = Runtime::new();
        assert_eq!(opening::sys_open(&mut r, 1, path.as_ptr() as Word, 0o400 | 0o1000000, 0), Ok(33));
        PROVIDER.with(|p| { let p = p.borrow(); assert!(p.requests.is_empty()); assert_eq!(p.legacy_opens, 0); });
    }
}

#[test]
fn production_mutating_opens_retain_the_legacy_path() {
    for flags in [1, LINUX_O_CREAT, LINUX_O_TRUNC] {
        let mut runtime = Runtime::new(); let path = b"value\0";
        assert_eq!(opening::sys_open(&mut runtime, 1, path.as_ptr() as Word, flags, 0o600), Ok(3));
        PROVIDER.with(|p| {
            let p = p.borrow();
            assert_eq!(p.requests[0].operation, VFS_PATH_RESOLVE);
            assert_eq!(p.legacy_opens, 1);
        });
    }
}

#[test]
fn production_open_closes_posix_fd_on_local_descriptor_exhaustion() {
    for flags in [0, LINUX_O_CREAT] {
        let mut runtime = Runtime::new(); runtime.allocation_full = true;
        let path = b"value\0";
        assert_eq!(opening::sys_open(&mut runtime, 1, path.as_ptr() as Word, flags, 0), Err(EMFILE));
        PROVIDER.with(|p| assert_eq!(p.borrow().closed,
            [if flags == 0 && option_env!("NANAMI_ROOTED_READONLY_OPEN") != Some("0") { 7 } else { 21 }]));
    }
}

#[test]
fn production_open_preserves_virtual_priority_after_a_symlink_resolves_to_a_mount() {
    let mut runtime = Runtime::new();
    PROVIDER.with(|p| { let mut p = p.borrow_mut(); p.path = b"/dev".to_vec(); p.metadata.file_type = VFS_FILE_TYPE_DIRECTORY; });
    let path = b"link-to-dev\0";
    assert_eq!(opening::sys_open(&mut runtime, 1, path.as_ptr() as Word, 0, 0), Ok(33));
    assert!(runtime.allocated.is_empty());
    PROVIDER.with(|p| {
        let p = p.borrow();
        assert_eq!(p.legacy_opens, 0);
        assert_eq!(p.closed.len(), usize::from(option_env!("NANAMI_ROOTED_READONLY_OPEN") != Some("0")));
    });
}

#[test]
fn production_open_declined_nofollow_symlink_retains_loop_error() {
    let mut runtime = Runtime::new();
    PROVIDER.with(|p| { let mut p = p.borrow_mut(); p.opened = VFS_PATH_OPEN_NONE; p.metadata.file_type = VFS_FILE_TYPE_SYMLINK; });
    let path = b"link\0";
    assert_eq!(opening::sys_open(&mut runtime, 1, path.as_ptr() as Word, LINUX_O_NOFOLLOW, 0), Err(ELOOP));
    PROVIDER.with(|p| assert_eq!(p.borrow().legacy_opens, 0));
}

#[test]
fn nofollow_open_restores_path_after_stat_without_heap_snapshot() {
    let mut counts = Vec::new();
    for length in [1, 12, 127] {
        let mut r = Runtime::new();
        r.allocated.reserve(1);
        let mut path = vec![b'x'; length]; path[0] = b'/';
        PROVIDER.with(|p| {
            let mut p = p.borrow_mut();
            p.path = path.clone(); p.clobber_nofollow_stat = true;
            p.requests.reserve(1); p.inputs.reserve(1);
        });
        CWD_ALLOCATIONS.with(|count| count.set(Some(0)));
        let result = opening::sys_open(&mut r, 1, c"/raw".as_ptr() as Word, 1 | LINUX_O_NOFOLLOW, 0);
        counts.push(CWD_ALLOCATIONS.with(|count| count.replace(None).unwrap()));
        assert_eq!(result, Ok(if length == 1 { 33 } else { 3 }));
        assert_eq!(&r.buffer[..length], path.as_slice());
        PROVIDER.with(|p| {
            let p = p.borrow();
            assert_eq!(p.nofollow_stat_calls, 1);
            assert_eq!(p.requests[0].operation, VFS_PATH_RESOLVE);
            assert_eq!(p.requests[0].flags, VFS_PATH_ALLOW_MISSING);
            if length != 1 { assert_eq!(&p.opened_path[..p.opened_path_len], path.as_slice()); }
        });
    }
    println!("nofollow open allocations for canonical lengths1/12/127: {counts:?}");
    // Only the transport mock's owned request-input capture remains.
    assert_eq!(counts, [1, 1, 1]);
}

#[test]
fn nofollow_open_keeps_stat_error_and_symlink_order_after_scratch_overwrite() {
    for length in [2, 127] {
        for error in [false, true] {
            for symlink in [false, true] {
                let mut r = Runtime::new();
                let mut path = vec![b'x'; length]; path[0] = b'/';
                PROVIDER.with(|p| {
                    let mut p = p.borrow_mut(); p.path = path.clone();
                    p.clobber_nofollow_stat = true;
                    p.stat_error = error.then_some(99);
                    if symlink { p.metadata.file_type = VFS_FILE_TYPE_SYMLINK; }
                });
                let result = opening::sys_open(&mut r, 1, c"/raw".as_ptr() as Word, 1 | LINUX_O_NOFOLLOW, 0);
                assert_eq!(result, if symlink && !error { Err(ELOOP) } else { Ok(3) });
                assert_eq!(&r.buffer[..length], path.as_slice());
                assert!(r.buffer[length..256].iter().all(|byte| *byte == 0xda));
                PROVIDER.with(|p| {
                    let p = p.borrow(); assert_eq!(p.nofollow_stat_calls, 1);
                    assert_eq!(p.legacy_opens, usize::from(!(symlink && !error)));
                    if p.legacy_opens != 0 { assert_eq!(&p.opened_path[..p.opened_path_len], path.as_slice()); }
                });
            }
        }
    }
}
#[test]
fn readonly_open_returns_an_owned_descriptor_and_path_in_one_rooted_request() {
    for (kind, options) in [(VFS_FILE_TYPE_REGULAR, 0), (VFS_FILE_TYPE_DIRECTORY, VFS_OPEN_DIRECTORY)] {
        let mut runtime = Runtime::new();
        PROVIDER.with(|p| p.borrow_mut().metadata.file_type = kind);
        let bytes = runtime.path(b"value");
        let result = namespace::resolve_for_open(&mut runtime, 1, 3, bytes, true, options).unwrap();
        assert_eq!(result.opened, Some(7));
        assert_eq!(result.file_type, kind);
        assert_eq!(&runtime.buffer[..result.bytes + 1], b"/lib/value\0");
        PROVIDER.with(|p| {
            let p = p.borrow();
            assert_eq!(p.requests.len(), 1);
            let request = p.requests[0];
            assert_eq!(request.operation, VFS_PATH_OPEN_READONLY);
            assert_eq!(request.directory, 9);
            assert_ne!(request.flags & VFS_PATH_FOLLOW_FINAL, 0);
            assert_eq!(request.flags & VFS_PATH_WITH_METADATA, 0);
            let flags = unsafe { ((p.shm + request.target_offset) as *const Word).read_unaligned() };
            assert_eq!(flags, options);
        });
    }
}

#[test]
fn readonly_open_none_result_retains_canonical_path_and_nofollow_policy() {
    for kind in [VFS_FILE_TYPE_UNKNOWN, VFS_FILE_TYPE_SYMLINK, VFS_FILE_TYPE_SOCKET] {
        let mut runtime = Runtime::new();
        PROVIDER.with(|p| {
            let mut p = p.borrow_mut();
            p.opened = VFS_PATH_OPEN_NONE;
            p.metadata.file_type = kind;
        });
        let bytes = runtime.path(b"name");
        let result = namespace::resolve_for_open(&mut runtime, 1, LINUX_AT_FDCWD, bytes, false, 0).unwrap();
        assert_eq!(result.opened, None);
        assert_eq!(result.file_type, kind);
        assert_eq!(&runtime.buffer[..result.bytes], b"/lib/value");
        PROVIDER.with(|p| assert_eq!(p.borrow().requests[0].flags & VFS_PATH_FOLLOW_FINAL, 0));
    }
}

#[test]
fn readonly_open_unsupported_service_restores_the_owned_path_only_once() {
    let mut runtime = Runtime::new();
    PROVIDER.with(|p| p.borrow_mut().unsupported_open = true);
    let bytes = runtime.path(b"requested");
    let result = namespace::resolve_for_open(&mut runtime, 1, LINUX_AT_FDCWD, bytes, true, 0).unwrap();
    assert_eq!(result.opened, None);
    PROVIDER.with(|p| {
        let p = p.borrow();
        assert_eq!(p.requests.iter().map(|request| request.operation).collect::<Vec<_>>(),
                   [VFS_PATH_OPEN_READONLY, VFS_PATH_RESOLVE]);
        assert_eq!(p.inputs, [b"/lib/requested".to_vec(), b"/lib/requested".to_vec()]);
        assert!(p.closed.is_empty());
    });
}

#[test]
fn readonly_open_does_not_retry_other_errors_or_query_virtual_mounts() {
    for error in [VFS_PATH_ERROR_IO, VFS_PATH_ERROR_MISSING, VFS_PATH_ERROR_LOOP] {
        let mut runtime = Runtime::new();
        PROVIDER.with(|p| p.borrow_mut().error = Some(error));
        let bytes = runtime.path(b"value");
        assert!(namespace::resolve_for_open(&mut runtime, 1, LINUX_AT_FDCWD, bytes, true, 0).is_err());
        PROVIDER.with(|p| assert_eq!(p.borrow().requests.len(), 1));
    }
    let mut runtime = Runtime::new();
    let bytes = runtime.path(b"/proc/self");
    assert_eq!(namespace::resolve_for_open(&mut runtime, 1, LINUX_AT_FDCWD, bytes, true, 0).unwrap().opened, None);
    PROVIDER.with(|p| assert!(p.borrow().requests.is_empty()));
}

#[test]
fn readonly_open_malformed_replies_close_the_acquired_descriptor() {
    for case in 0..5 {
        let mut runtime = Runtime::new();
        PROVIDER.with(|p| {
            let mut p = p.borrow_mut();
            match case {
                0 => p.reply_bytes = Some(0),
                1 => p.reply_bytes = Some(std::mem::size_of::<PathOpenResult>()),
                2 => p.reply_bytes = Some(4096),
                3 => p.path = b"relative".to_vec(),
                _ => p.reply_type = Some(VFS_FILE_TYPE_SYMLINK),
            }
        });
        let bytes = runtime.path(b"value");
        assert!(matches!(namespace::resolve_for_open(&mut runtime, 1, LINUX_AT_FDCWD, bytes, true, 0), Err(EIO)));
        PROVIDER.with(|p| assert_eq!(p.borrow().closed, [7]));
    }
}

#[test]
fn readlink_regular_directory_and_socket_use_current_type_without_a_second_query() {
    for kind in [VFS_FILE_TYPE_REGULAR, VFS_FILE_TYPE_DIRECTORY, VFS_FILE_TYPE_SOCKET] {
        let mut r = Runtime::new();
        PROVIDER.with(|p| p.borrow_mut().metadata.file_type = kind);
        let path = b"/file\0";
        let mut output = [0xa5; 16];
        assert_eq!(readlink::sys_readlink(&mut r, 1, path.as_ptr() as Word, output.as_mut_ptr() as Word, output.len()), Err(EINVAL));
        assert_eq!(output, [0xa5; 16]);
        PROVIDER.with(|p| {
            let p = p.borrow();
            let enabled = option_env!("NANAMI_READLINK_KIND_FASTPATH")
                .is_none_or(|value| value == "1");
            assert_eq!(p.requests.len(), if enabled { 1 } else { 2 });
            assert_eq!(p.requests[0].operation, VFS_PATH_RESOLVE);
            assert_eq!(p.requests[0].flags & VFS_PATH_FOLLOW_FINAL, 0);
        });
    }
}

#[test]
fn readlink_symlink_unknown_and_changed_type_keep_the_original_lookup_and_payload() {
    for kind in [VFS_FILE_TYPE_SYMLINK, VFS_FILE_TYPE_UNKNOWN, 99] {
        let mut r = Runtime::new();
        PROVIDER.with(|p| p.borrow_mut().metadata.file_type = kind);
        let path = b"alias\0";
        let mut output = [0xa5; 4];
        assert_eq!(readlink::sys_readlinkat(&mut r, 1, 3, path.as_ptr() as Word, output.as_mut_ptr() as Word, 2), Ok(2));
        assert_eq!(output, [b't', b'a', 0xa5, 0xa5]);
        PROVIDER.with(|p| { let p=p.borrow(); assert_eq!(p.requests.len(), 2); assert_eq!(p.requests[0].directory, 9); });
    }
    let mut r = Runtime::new();
    let path = b"/file\0";
    let mut output = [0; 16];
    assert_eq!(readlink::sys_readlink(&mut r, 1, path.as_ptr() as Word, output.as_mut_ptr() as Word, 16), Err(EINVAL));
    PROVIDER.with(|p| p.borrow_mut().metadata.file_type = VFS_FILE_TYPE_SYMLINK);
    assert_eq!(readlink::sys_readlink(&mut r, 1, path.as_ptr() as Word, output.as_mut_ptr() as Word, 16), Ok(6));
    assert_eq!(&output[..6], b"target");
}

#[test]
fn readlink_lookup_and_output_error_order_remains_unchanged() {
    for kind in [VFS_FILE_TYPE_REGULAR, VFS_FILE_TYPE_SYMLINK] {
        let mut r = Runtime::new();
        PROVIDER.with(|p| p.borrow_mut().metadata.file_type = kind);
        let path = b"/file\0";
        assert_eq!(readlink::sys_readlink(&mut r, 1, path.as_ptr() as Word, 0, 1), Err(EFAULT));
        assert_eq!(readlink::sys_readlink(&mut r, 1, path.as_ptr() as Word, 0, 0), Err(EINVAL));
    }
    let mut r = Runtime::new();
    let relative = b"file\0";
    assert_eq!(readlink::sys_readlinkat(&mut r, 1, 64, relative.as_ptr() as Word, 0, 1), Err(EBADF));
    PROVIDER.with(|p| { assert!(p.borrow().requests.is_empty()); p.borrow_mut().error=Some(VFS_PATH_ERROR_MISSING); });
    assert_eq!(readlink::sys_readlink(&mut r, 1, relative.as_ptr() as Word, 0, 1), Err(ENOENT));
}

#[test]
fn readlink_virtual_resolution_does_not_reject_the_fallback() {
    let mut r = Runtime::new();
    PROVIDER.with(|p| p.borrow_mut().metadata.file_type = VFS_FILE_TYPE_SYMLINK);
    let path = b"/proc/self\0";
    let mut output = [0; 16];
    assert_eq!(readlink::sys_readlink(&mut r, 1, path.as_ptr() as Word, output.as_mut_ptr() as Word, 16), Ok(6));
    PROVIDER.with(|p| { let p=p.borrow(); assert_eq!(p.requests.len(), 1); assert_eq!(p.requests[0].operation, VFS_PATH_READLINK); });
}

#[test]
fn successful_and_negative_stats_need_one_lookup_and_no_persistent_cache() {
    let mut runtime = Runtime::new();
    let bytes = runtime.path(b"value");
    let stat = namespace::stat_at(&mut runtime, 1, LINUX_AT_FDCWD, bytes, true).unwrap();
    assert_eq!(
        stat,
        (
            42 | (0o100640 << 32),
            123,
            posix::POSIX_FILE_TYPE_REGULAR,
            0,
            0
        )
    );
    PROVIDER.with(|p| {
        let mut p = p.borrow_mut();
        assert_eq!(p.requests.len(), 1);
        assert_eq!(p.inputs[0], b"/lib/value");
        assert_ne!(p.requests[0].flags & VFS_PATH_WITH_METADATA, 0);
        p.metadata = PathMetadata::default();
    });
    let bytes = runtime.path(b"value");
    assert_eq!(
        namespace::stat_at(&mut runtime, 1, LINUX_AT_FDCWD, bytes, true),
        Err(ENOENT)
    );
    PROVIDER.with(|p| assert_eq!(p.borrow().requests.len(), 2));
}
#[test]
fn legacy_resolution_retains_its_path_only_wire_format() {
    let mut runtime = Runtime::new();
    let bytes = runtime.path(b"value");
    let count = namespace::resolve_at(&mut runtime, 1, LINUX_AT_FDCWD, bytes, true).unwrap();
    assert_eq!(&runtime.buffer[..count + 1], b"/lib/value\0");
    PROVIDER.with(|p| {
        let p = p.borrow();
        assert_eq!(p.requests[0].flags & VFS_PATH_WITH_METADATA, 0);
        assert_eq!(p.requests[0].output_bytes, 127);
    });
}
#[test]
fn dirfds_symlink_policy_and_kind_translation_are_preserved() {
    let mut runtime = Runtime::new();
    PROVIDER.with(|p| {
        p.borrow_mut().metadata = PathMetadata {
            inode: 7,
            size: 11,
            mode: 0o120777,
            file_type: VFS_FILE_TYPE_SYMLINK,
        }
    });
    let bytes = runtime.path(b"link");
    let stat = namespace::stat_at(&mut runtime, 1, 3, bytes, false).unwrap();
    assert_eq!(stat.2, posix::POSIX_FILE_TYPE_SYMLINK);
    assert_eq!(stat.0 >> 32, 0o120777);
    PROVIDER.with(|p| {
        let p = p.borrow();
        assert_eq!(p.requests[0].directory, 9);
        assert_eq!(p.requests[0].flags & VFS_PATH_FOLLOW_FINAL, 0);
        assert_eq!(p.inputs[0], b"link");
    });
    assert_eq!(
        namespace::decode_metadata(PathMetadata {
            inode: 8,
            size: 0,
            mode: 0o140777,
            file_type: VFS_FILE_TYPE_SOCKET
        })
        .unwrap()
        .unwrap()
        .2,
        posix::POSIX_FILE_TYPE_SOCKET
    );
}
#[test]
fn virtual_mounts_never_issue_a_disk_lookup() {
    let mut runtime = Runtime::new();
    let bytes = runtime.path(b"/proc/self/status");
    assert_eq!(
        namespace::stat_at(&mut runtime, 1, LINUX_AT_FDCWD, bytes, true)
            .unwrap()
            .0,
        999
    );
    PROVIDER.with(|p| assert!(p.borrow().requests.is_empty()));
}
#[test]
fn short_oversized_and_inconsistent_metadata_fail_without_reading_stale_bytes() {
    for count in [0, 31, 32, 4096] {
        let mut runtime = Runtime::new();
        PROVIDER.with(|p| p.borrow_mut().reply_bytes = Some(count));
        let bytes = runtime.path(b"value");
        assert_eq!(
            namespace::stat_at(&mut runtime, 1, LINUX_AT_FDCWD, bytes, true),
            Err(EIO)
        );
    }
    for value in [
        PathMetadata {
            inode: 0,
            size: 1,
            mode: 0,
            file_type: 0,
        },
        PathMetadata {
            inode: 1,
            size: 0,
            mode: Word::MAX,
            file_type: 1,
        },
        PathMetadata {
            inode: Word::MAX,
            size: 0,
            mode: 0,
            file_type: 1,
        },
        PathMetadata {
            inode: 1,
            size: 0,
            mode: 0,
            file_type: 99,
        },
    ] {
        assert!(matches!(namespace::decode_metadata(value), Err(EIO)));
    }
}

#[test]
fn symlink_keeps_target_across_resolution_without_path_vector() {
    let mut counts = Vec::new();
    for length in [1, 12, 127] {
        let mut r = Runtime::new();
        let mut target = vec![b't'; length]; target.push(0);
        let input = c"/new-link";
        PROVIDER.with(|p| {
            let mut p = p.borrow_mut();
            p.path = b"/canonical-link".to_vec();
            p.requests.reserve(2); p.inputs.reserve(2);
        });
        CWD_ALLOCATIONS.with(|count| count.set(Some(0)));
        let result = namespace::symlink(&mut r, 1, target.as_ptr() as Word, LINUX_AT_FDCWD, input.as_ptr() as Word);
        counts.push(CWD_ALLOCATIONS.with(|count| count.replace(None).unwrap()));
        assert_eq!(result, Ok(0));
        assert_eq!(&r.buffer[..15], b"/canonical-link");
        PROVIDER.with(|p| {
            let p = p.borrow();
            assert_eq!(p.inputs, [b"/new-link".to_vec(), b"/canonical-link".to_vec()]);
            assert_eq!(p.requests.iter().map(|r| (r.operation, r.flags, r.output_bytes)).collect::<Vec<_>>(), [(VFS_PATH_RESOLVE, VFS_PATH_ALLOW_MISSING, LINUX_CWD_MAX - 1), (VFS_PATH_SYMLINK, 0, 0)]);
            assert_eq!(&p.captured_target[..p.captured_target_len], &target[..length]);
        });
    }
    println!("symlink allocations for target lengths1/12/127: {counts:?}");
    // One target Vec and two transport mock input captures remain.
    assert_eq!(counts, [3, 3, 3]);
}

#[test]
fn symlink_keeps_validation_resolution_and_mutation_error_order() {
    for mode in 0..6 {
        let mut r = Runtime::new();
        let target = if mode == 2 { 0 } else { c"target".as_ptr() as Word };
        let path = if mode <= 3 { 0 } else { c"/new-link".as_ptr() as Word };
        if mode == 1 { r.process.rootfs.readonly = true; }
        if mode >= 4 {
            PROVIDER.with(|p| {
                let mut p = p.borrow_mut();
                p.error = Some(VFS_PATH_ERROR_MISSING);
                p.error_operation = Some(if mode == 4 { VFS_PATH_RESOLVE } else { VFS_PATH_SYMLINK });
            });
        }
        let pid = if mode == 0 { 99 } else { 1 };
        let expected = match mode { 0 => ESRCH, 1 => EROFS, 2 | 3 => EFAULT, _ => ENOENT };
        assert_eq!(namespace::symlink(&mut r, pid, target, LINUX_AT_FDCWD, path), Err(expected));
        PROVIDER.with(|p| assert_eq!(p.borrow().requests.len(), if mode < 4 { 0 } else { mode - 3 }));
    }
}

#[test]
fn current_readlink_path_allocation() {
    let mut counts = Vec::new();
    for len in [1, 12, 127] {
        let mut r = Runtime::new();
        let path = vec![b'x'; len];
        let input = r.path(&path);
        PROVIDER.with(|p| {
            let mut p = p.borrow_mut();
            p.metadata.file_type = VFS_FILE_TYPE_SYMLINK;
            p.requests.reserve(1); p.inputs.reserve(1);
        });
        CWD_ALLOCATIONS.with(|count| count.set(Some(0)));
        let result = namespace::readlink_current_path(&mut r, 1, input, 4096);
        counts.push(CWD_ALLOCATIONS.with(|count| count.replace(None).unwrap()));
        assert_eq!(result, Ok(6));
        assert_eq!(&r.buffer[..6], b"target");
        PROVIDER.with(|p| assert_eq!(p.borrow().inputs, [path]));
    }
    println!("current-readlink allocations for 1/12/127-byte paths: {counts:?}");
    // The remaining allocation captures the request input in the transport mock.
    assert_eq!(counts, [1, 1, 1]);
}

#[test]
fn current_readlink_matches_owned_request_full_buffer_on_limits_and_errors() {
    for path_len in [0, 1, 127, 128] {
        for limit in [0, 1, 4096, 4097] {
            for fault in 0..3 {
                let mut results = Vec::new();
                for owned in [true, false] {
                    let mut r = Runtime::new();
                    let path = vec![b'x'; path_len];
                    let input = r.path(&path);
                    PROVIDER.with(|p| {
                        let mut p = p.borrow_mut();
                        p.metadata.file_type = VFS_FILE_TYPE_SYMLINK;
                        p.link_target = vec![b'z'; 4096];
                        if fault == 1 { p.error = Some(VFS_PATH_ERROR_MISSING); }
                        if fault == 2 { p.reply_bytes = Some(limit + 1); }
                    });
                    let result = if owned {
                        namespace::request_path(&mut r, 1, VFS_PATH_READLINK, VFS_PATH_DIRECTORY_ROOT, &path, &[], 0, limit).map(|d| d.0)
                    } else { namespace::readlink_current_path(&mut r, 1, input, limit) };
                    let requests = PROVIDER.with(|p| { let p = p.borrow(); (p.requests.len(), p.inputs.clone()) });
                    results.push((result, r.buffer.to_vec(), requests));
                }
                assert_eq!(results[0], results[1], "path_len={path_len} limit={limit} fault={fault}");
            }
        }
    }
}

#[test]
fn readlink_output_can_fill_its_limit_and_preserves_the_following_byte() {
    let mut r = Runtime::new();
    r.buffer[4096] = 0xa5;
    PROVIDER.with(|p| { let mut p=p.borrow_mut(); p.link_target = vec![b'x'; 4096]; p.metadata.file_type = VFS_FILE_TYPE_SYMLINK; });
    assert_eq!(
        namespace::request_path(
            &mut r,
            1,
            VFS_PATH_READLINK,
            VFS_PATH_DIRECTORY_ROOT,
            b"/link",
            &[],
            0,
            4096
        )
        .unwrap()
        .0,
        4096
    );
    assert!(r.buffer[..4096].iter().all(|b| *b == b'x'));
    assert_eq!(r.buffer[4096], 0xa5);
}

#[test]
fn request_limits_precede_writes_and_nonoutput_replies_preserve_existing_bytes() {
    for size in [
        0,
        128,
        0xc000 + std::mem::size_of::<PathRequest>(),
        0xd400 - 1,
    ] {
        let mut r = Runtime::new();
        r.posix_shm_size = size;
        r.buffer.fill(0xa5);
        assert_eq!(
            namespace::request_path(
                &mut r,
                1,
                VFS_PATH_RESOLVE,
                VFS_PATH_DIRECTORY_ROOT,
                b"/x",
                &[],
                0,
                127
            ),
            Err(ENAMETOOLONG)
        );
        assert!(r.buffer.iter().all(|b| *b == 0xa5));
        PROVIDER.with(|p| assert!(p.borrow().requests.is_empty()));
    }
    let mut r = Runtime::new();
    let path = [b'x'; LINUX_CWD_MAX];
    let target = [b'x'; 4097];
    for (path, target, count) in [
        (&path[..], &[][..], 127),
        (&b"/x"[..], &target[..], 127),
        (&b"/x"[..], &[][..], 4097),
    ] {
        assert_eq!(
            namespace::request_path(
                &mut r,
                1,
                VFS_PATH_RESOLVE,
                VFS_PATH_DIRECTORY_ROOT,
                path,
                target,
                0,
                count
            ),
            Err(ENAMETOOLONG)
        );
    }
    PROVIDER.with(|p| assert!(p.borrow().requests.is_empty()));
    r.buffer[..8].copy_from_slice(b"preserve");
    let mode = 0o6754usize.to_le_bytes();
    assert_eq!(
        namespace::request_path(
            &mut r,
            1,
            VFS_PATH_CHMOD,
            VFS_PATH_DIRECTORY_ROOT,
            b"/x",
            &mode,
            VFS_PATH_FOLLOW_FINAL,
            0
        ),
        Ok((0, 0))
    );
    assert_eq!(&r.buffer[..8], b"preserve");
}

#[test]
fn maximum_canonical_reply_and_mismatched_metadata_type_are_validated() {
    let mut r = Runtime::new();
    let path = format!("/{}", "x".repeat(LINUX_CWD_MAX - 2));
    let reply_bytes = std::mem::size_of::<PathMetadata>() + path.len();
    r.buffer[reply_bytes + 1] = 0xa5;
    PROVIDER.with(|p| p.borrow_mut().path = path.as_bytes().to_vec());
    let bytes = r.path(b"value");
    namespace::stat_at(&mut r, 1, LINUX_AT_FDCWD, bytes, true).unwrap();
    assert_eq!(&r.buffer[..path.len()], path.as_bytes());
    assert_eq!(r.buffer[path.len()], 0);
    assert_eq!(r.buffer[reply_bytes + 1], 0xa5);
    for (path, reply_type) in [
        (&b"relative"[..], None),
        (&b"/valid"[..], Some(VFS_FILE_TYPE_DIRECTORY)),
    ] {
        let mut r = Runtime::new();
        PROVIDER.with(|p| {
            let mut p = p.borrow_mut();
            p.path = path.to_vec();
            p.reply_type = reply_type;
        });
        let bytes = r.path(b"value");
        assert_eq!(
            namespace::stat_at(&mut r, 1, LINUX_AT_FDCWD, bytes, true),
            Err(EIO)
        );
    }
}
