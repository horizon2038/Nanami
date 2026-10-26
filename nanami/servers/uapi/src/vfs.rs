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
pub const VFS_PATH_RESPONSE_ERROR: Word = 0x9116_0001;
pub const VFS_PATH_RESOLVE: Word = 1;
pub const VFS_PATH_SYMLINK: Word = 2;
pub const VFS_PATH_READLINK: Word = 3;
pub const VFS_PATH_STAT: Word = 4;
pub const VFS_PATH_FOLLOW_FINAL: Word = 1;
pub const VFS_PATH_ALLOW_MISSING: Word = 2;
pub const VFS_PATH_DIRECTORY_ROOT: Word = Word::MAX;

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

pub const VFS_CONTROL_ATTACH_SHARED_MEMORY: Word = 1;
pub const VFS_CONTROL_ATTACH_DELEGATED_SHARED_MEMORY: Word = 2;

pub const VFS_DEFAULT_SHM_BYTES: Word = 0x4000;
pub const VFS_DELEGATE_ID_SHIFT: Word = 32;
pub const VFS_DELEGATE_VALUE_MASK: Word = 0xffff_ffff;
pub const VFS_OPEN_CREATE: Word = 1 << 0;
pub const VFS_OPEN_TRUNCATE: Word = 1 << 1;
pub const VFS_OPEN_DIRECTORY: Word = 1 << 2;
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
pub const VFS_STAT_SIZE_MASK: Word = 0x00ff_ffff_ffff_ffff;
