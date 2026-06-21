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

pub const VFS_CONTROL_ATTACH_SHARED_MEMORY: Word = 1;

pub const VFS_DEFAULT_SHM_BYTES: Word = 0x4000;
pub const VFS_FILE_TYPE_UNKNOWN: Word = 0;
pub const VFS_FILE_TYPE_REGULAR: Word = 1;
pub const VFS_FILE_TYPE_DIRECTORY: Word = 2;

pub const VFS_DIRECTORY_ENTRY_NAME_BYTES: usize = 256;
pub const VFS_DIRECTORY_ENTRY_RECORD_BYTES: usize = 32 + VFS_DIRECTORY_ENTRY_NAME_BYTES;
pub const VFS_DIRECTORY_ENTRY_INODE_OFFSET: usize = 0;
pub const VFS_DIRECTORY_ENTRY_TYPE_OFFSET: usize = 8;
pub const VFS_DIRECTORY_ENTRY_NAME_LEN_OFFSET: usize = 16;
pub const VFS_DIRECTORY_ENTRY_RECORD_LEN_OFFSET: usize = 24;
pub const VFS_DIRECTORY_ENTRY_NAME_OFFSET: usize = 32;

pub const VFS_STAT_TYPE_SHIFT: Word = 56;
pub const VFS_STAT_SIZE_MASK: Word = 0x00ff_ffff_ffff_ffff;
