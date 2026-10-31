use crate::Word;

pub const VFS_REQUEST_CONTROL: Word = 0x9101;
pub const VFS_REQUEST_OPEN: Word = 0x9102;
pub const VFS_REQUEST_READ: Word = 0x9103;
pub const VFS_REQUEST_STAT: Word = 0x9104;
pub const VFS_REQUEST_CLOSE: Word = 0x9105;
pub const VFS_REQUEST_READ_DIR: Word = 0x9106;
pub const VFS_REQUEST_FSTAT: Word = 0x9107;
pub const VFS_REQUEST_CREATE: Word = 0x9108;
pub const VFS_REQUEST_WRITE: Word = 0x9109;
pub const VFS_REQUEST_REMOVE: Word = 0x910a;
pub const VFS_REQUEST_MKDIR: Word = 0x910b;
pub const VFS_REQUEST_RENAME: Word = 0x910c;
pub const VFS_REQUEST_OPEN_COMPOUND: Word = 0x910d;
pub const VFS_REQUEST_READ_DELEGATED: Word = 0x910e;
pub const VFS_REQUEST_LINK: Word = 0x910f;
pub const VFS_REQUEST_WRITE_DELEGATED: Word = 0x9110;
pub const VFS_REQUEST_FSYNC: Word = 0x9111;
pub const VFS_REQUEST_SYNC: Word = 0x9112;
pub const VFS_REQUEST_FTRUNCATE: Word = 0x9113;
pub const VFS_REQUEST_FREEZE: Word = 0x9114;
pub const VFS_REQUEST_THAW: Word = 0x9115;
pub const VFS_REQUEST_PATH: Word = 0x9116;
/// arg0: owned open handle, arg1: permission/special mode bits (0000..7777).
/// Returns the same precise filesystem errors as VFS_REQUEST_PATH.
pub const VFS_REQUEST_FCHMOD: Word = 0x9117;
/// Read-only shared file page: arg0 owned handle, arg1 aligned file offset,
/// arg2 peer PID. Returns a page-aligned alias in that peer. UNMAP releases
/// its lease after the peer has removed its alias; the open handle stays live.
/// The trusted mapper must restrict downstream aliases to read-only access.
pub const VFS_REQUEST_MAP_READ_PAGE: Word = 0x9118;
pub const VFS_REQUEST_UNMAP_READ_PAGE: Word = 0x9119;
/// arg0: owned handle. Writes FilesystemStats at offset zero of client SHM.
pub const VFS_REQUEST_FSTATFS: Word = 0x911a;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FilesystemStats {
    pub kind: u64,
    pub block_size: u64,
    pub blocks: u64,
    pub free_blocks: u64,
    pub available_blocks: u64,
    pub files: u64,
    pub free_files: u64,
    pub fsid: u64,
    pub name_max: u64,
    pub flags: u64,
}
/// Resizing a file with read-page leases requires VMA invalidation and is
/// currently refused before any on-disk mutation; callers may unmap and retry.
pub const VFS_RESPONSE_MAPPING_BUSY: Word = 0x9118_0001;
pub const VFS_PATH_RESPONSE_ERROR: Word = 0x9116_0001;
pub const VFS_PATH_RESOLVE: Word = 1;
pub const VFS_PATH_SYMLINK: Word = 2;
pub const VFS_PATH_READLINK: Word = 3;
pub const VFS_PATH_STAT: Word = 4;
/// target contains one little-endian Word of permission/special mode bits.
/// FOLLOW_FINAL selects whether the final symlink is dereferenced.
pub const VFS_PATH_CHMOD: Word = 5;
pub const VFS_PATH_FOLLOW_FINAL: Word = 1;
pub const VFS_PATH_ALLOW_MISSING: Word = 2;
/// RESOLVE output starts with PathMetadata, followed by the canonical path.
/// detail0 counts the entire output, detail1 retains the resolved file type.
/// An allowed missing final component has zeroed metadata (inode == 0).
/// Without this flag RESOLVE retains its original path-only reply format.
pub const VFS_PATH_WITH_METADATA: Word = 4;
pub const VFS_PATH_DIRECTORY_ROOT: Word = Word::MAX;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PathMetadata {
    pub inode: Word,
    pub size: Word,
    pub mode: Word,
    pub file_type: Word,
}

/// Word-aligned shared-buffer descriptor for namespace-rooted path operations.
/// POSIX uses a directory FD; VFS uses an owned directory handle in the same field.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PathRequest {
    pub operation: Word,
    pub directory: Word,
    pub root_offset: Word,
    pub root_bytes: Word,
    pub path_offset: Word,
    pub path_bytes: Word,
    pub target_offset: Word,
    pub target_bytes: Word,
    pub output_offset: Word,
    pub output_bytes: Word,
    pub flags: Word,
}

pub const VFS_PATH_ERROR_INVALID: Word = 1;
pub const VFS_PATH_ERROR_MISSING: Word = 2;
pub const VFS_PATH_ERROR_NOT_DIRECTORY: Word = 3;
pub const VFS_PATH_ERROR_LOOP: Word = 4;
pub const VFS_PATH_ERROR_TOO_LONG: Word = 5;
pub const VFS_PATH_ERROR_EXISTS: Word = 6;
pub const VFS_PATH_ERROR_IO: Word = 7;
pub const VFS_PATH_ERROR_NO_SPACE: Word = 8;
pub const VFS_PATH_ERROR_READ_ONLY: Word = 9;
pub const VFS_PATH_ERROR_UNSUPPORTED: Word = 10;

pub const VFS_CONTROL_ATTACH_SHARED_MEMORY: Word = 1;
pub const VFS_CONTROL_ATTACH_DELEGATED_SHARED_MEMORY: Word = 2;

pub const VFS_DEFAULT_SHM_BYTES: Word = 0x4000;
pub const VFS_DELEGATE_ID_SHIFT: Word = 32;
pub const VFS_DELEGATE_VALUE_MASK: Word = 0xffff_ffff;
pub const VFS_OPEN_CREATE: Word = 1 << 0;
pub const VFS_OPEN_TRUNCATE: Word = 1 << 1;
pub const VFS_OPEN_DIRECTORY: Word = 1 << 2;
/// Creation-mode argument: zero preserves the native client's default mode.
pub const VFS_MODE_PRESENT: Word = 1 << 16;
pub const VFS_OPEN_HANDLE_MASK: Word = 0xffff_ffff;
pub const VFS_OPEN_INODE_SHIFT: Word = 32;
pub const VFS_FILE_TYPE_UNKNOWN: Word = 0;
pub const VFS_FILE_TYPE_REGULAR: Word = 1;
pub const VFS_FILE_TYPE_DIRECTORY: Word = 2;
pub const VFS_FILE_TYPE_SOCKET: Word = 3;
pub const VFS_FILE_TYPE_SYMLINK: Word = 4;

pub const VFS_DIRECTORY_ENTRY_NAME_BYTES: usize = 256;
pub const VFS_DIRECTORY_ENTRY_RECORD_BYTES: usize = 32 + VFS_DIRECTORY_ENTRY_NAME_BYTES;
pub const VFS_DIRECTORY_ENTRY_INODE_OFFSET: usize = 0;
pub const VFS_DIRECTORY_ENTRY_TYPE_OFFSET: usize = 8;
pub const VFS_DIRECTORY_ENTRY_NAME_LEN_OFFSET: usize = 16;
pub const VFS_DIRECTORY_ENTRY_RECORD_LEN_OFFSET: usize = 24;
pub const VFS_DIRECTORY_ENTRY_NAME_OFFSET: usize = 32;

pub const VFS_STAT_TYPE_SHIFT: Word = 56;
/// STAT arg2 / FSTAT arg1: detail0 contains (mode << 32) | inode.
pub const VFS_STAT_INCLUDE_MODE: Word = 1;
/// FSTAT only: high 32 inode bits contain a non-repeating content token instead
/// of mode. Zero means uncacheable. Tokens are scoped to this VFS instance.
pub const VFS_STAT_INCLUDE_VERSION: Word = 2;
pub const VFS_STAT_SIZE_MASK: Word = 0x00ff_ffff_ffff_ffff;
