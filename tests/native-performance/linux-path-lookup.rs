//! Production rooted lookup, wire decoding and RPC counts; transport is mocked.
#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_runtime;
use std::cell::RefCell;
type Word = usize;
const EBADF: i32 = 9;
const ENOENT: i32 = 2;
const ESRCH: i32 = 3;
const ENOTDIR: i32 = 20;
const EINVAL: i32 = 22;
const EIO: i32 = 5;
const ELOOP: i32 = 40;
const ENAMETOOLONG: i32 = 36;
const EEXIST: i32 = 17;
const ENOSPC: i32 = 28;
const EROFS: i32 = 30;
const EOPNOTSUPP: i32 = 95;
const LINUX_CWD_MAX: usize = 128;
const LINUX_AT_FDCWD: Word = (-100i32) as Word;
mod posix {
    pub use nanami_uapi::posix::*;
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
            if let Some(error) = p.error {
                return Err(PathError::Filesystem(error));
            }
            let metadata_bytes = if request.flags & VFS_PATH_WITH_METADATA != 0 {
                std::mem::size_of::<PathMetadata>()
            } else {
                0
            };
            unsafe {
                if metadata_bytes != 0 {
                    ((p.shm + request.output_offset) as *mut PathMetadata)
                        .write_unaligned(p.metadata);
                }
                std::ptr::copy_nonoverlapping(
                    p.path.as_ptr(),
                    (p.shm + request.output_offset + metadata_bytes) as *mut u8,
                    p.path.len(),
                );
            }
            Ok((
                p.reply_bytes.unwrap_or(metadata_bytes + p.path.len()),
                p.metadata.file_type,
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
    error: Option<Word>,
    reply_bytes: Option<Word>,
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
            error: None,
            reply_bytes: None,
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
}
impl LinuxFile {
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
            },
            buffer,
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
    fn set_cwd(&mut self, _: Word, _: &[u8]) -> bool {
        true
    }
}
fn map_request_error(error: i32) -> i32 {
    error
}
fn guest_root_for_process(_: &Runtime, _: Word) -> Result<Vec<u8>, i32> {
    Ok(b"/test/rootfs".to_vec())
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
fn read_c_string(_: &Runtime, _: Word, _: Word) -> Result<Word, i32> {
    unreachable!()
}
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
