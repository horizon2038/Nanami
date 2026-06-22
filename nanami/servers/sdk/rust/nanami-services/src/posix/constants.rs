use crate::Word;

pub const POSIX_REQUEST_CONTROL: Word = 0xa101;
pub const POSIX_REQUEST_GETPID: Word = 0xa102;
pub const POSIX_REQUEST_GETCWD: Word = 0xa103;
pub const POSIX_REQUEST_CHDIR: Word = 0xa104;
pub const POSIX_REQUEST_OPEN: Word = 0xa105;
pub const POSIX_REQUEST_CLOSE: Word = 0xa106;
pub const POSIX_REQUEST_READ: Word = 0xa107;
pub const POSIX_REQUEST_WRITE: Word = 0xa108;
pub const POSIX_REQUEST_STAT: Word = 0xa109;
pub const POSIX_REQUEST_MKDIR: Word = 0xa10a;
pub const POSIX_REQUEST_UNLINK: Word = 0xa10b;
pub const POSIX_REQUEST_RENAME: Word = 0xa10c;
pub const POSIX_REQUEST_FSTAT: Word = 0xa10d;
pub const POSIX_REQUEST_READ_DIR: Word = 0xa10e;
pub const POSIX_REQUEST_SEEK: Word = 0xa10f;
pub const POSIX_REQUEST_RMDIR: Word = 0xa110;
pub const POSIX_REQUEST_GETPPID: Word = 0xa111;
pub const POSIX_REQUEST_GET_NATIVE_PID: Word = 0xa112;
pub const POSIX_REQUEST_GETUID: Word = 0xa113;
pub const POSIX_REQUEST_GETEUID: Word = 0xa114;
pub const POSIX_REQUEST_GETGID: Word = 0xa115;
pub const POSIX_REQUEST_GETEGID: Word = 0xa116;
pub const POSIX_REQUEST_GETPGID: Word = 0xa117;
pub const POSIX_REQUEST_GETSID: Word = 0xa118;
pub const POSIX_REQUEST_FORK: Word = 0xa119;
pub const POSIX_REQUEST_EXEC: Word = 0xa11a;
pub const POSIX_REQUEST_WAITPID: Word = 0xa11b;
pub const POSIX_REQUEST_KILL: Word = 0xa11c;
pub const POSIX_REQUEST_SETPGID: Word = 0xa11d;
pub const POSIX_REQUEST_SETSID: Word = 0xa11e;
pub const POSIX_REQUEST_SPAWN: Word = 0xa11f;
pub const POSIX_REQUEST_DUP: Word = 0xa120;
pub const POSIX_REQUEST_DUP2: Word = 0xa121;
pub const POSIX_REQUEST_FCNTL: Word = 0xa122;
pub const POSIX_REQUEST_GETENV: Word = 0xa123;
pub const POSIX_REQUEST_SETENV: Word = 0xa124;
pub const POSIX_REQUEST_UNSETENV: Word = 0xa125;
pub const POSIX_REQUEST_ENV_COUNT: Word = 0xa126;
pub const POSIX_REQUEST_ENV_AT: Word = 0xa127;

pub const POSIX_CONTROL_ATTACH_SHARED_MEMORY: Word = 1;
pub const POSIX_WAIT_NOHANG: Word = 1;

pub const POSIX_DEFAULT_SHM_BYTES: Word = 0x4000;
pub const POSIX_PATH_MAX: usize = 128;
pub const POSIX_ENV_NAME_MAX: usize = 32;
pub const POSIX_ENV_VALUE_MAX: usize = 128;

pub const POSIX_O_CREAT: Word = 1 << 0;
pub const POSIX_O_TRUNC: Word = 1 << 1;
pub const POSIX_O_DIRECTORY: Word = 1 << 2;

pub const POSIX_SEEK_SET: Word = 0;
pub const POSIX_SEEK_CUR: Word = 1;
pub const POSIX_SEEK_END: Word = 2;

pub const POSIX_F_GETFD: Word = 1;
pub const POSIX_F_SETFD: Word = 2;
pub const POSIX_FD_CLOEXEC: Word = 1 << 0;

pub const POSIX_PROCESS_ROOT_PID: Word = 1;
pub const POSIX_ROOT_UID: Word = 0;
pub const POSIX_ROOT_GID: Word = 0;
pub const POSIX_PAGE_SIZE: Word = 4096;

pub const POSIX_PROT_NONE: Word = 0;
pub const POSIX_PROT_READ: Word = 1 << 0;
pub const POSIX_PROT_WRITE: Word = 1 << 1;
pub const POSIX_PROT_EXEC: Word = 1 << 2;

pub const POSIX_MAP_PRIVATE: Word = 1 << 0;
pub const POSIX_MAP_SHARED: Word = 1 << 1;
pub const POSIX_MAP_ANONYMOUS: Word = 1 << 2;

pub const POSIX_FILE_TYPE_UNKNOWN: Word = 0;
pub const POSIX_FILE_TYPE_REGULAR: Word = 1;
pub const POSIX_FILE_TYPE_DIRECTORY: Word = 2;
pub const POSIX_FILE_TYPE_CHAR_DEVICE: Word = 3;
pub const POSIX_FILE_TYPE_BLOCK_DEVICE: Word = 4;

pub const POSIX_DEV_NULL_MAJOR: Word = 1;
pub const POSIX_DEV_NULL_MINOR: Word = 3;
pub const POSIX_DEV_ZERO_MAJOR: Word = 1;
pub const POSIX_DEV_ZERO_MINOR: Word = 5;

pub const POSIX_STAT_SIZE_MASK: Word = 0x0000_0000_ffff_ffff;
pub const POSIX_STAT_MINOR_SHIFT: Word = 32;
pub const POSIX_STAT_TYPE_SHIFT: Word = 48;
pub const POSIX_STAT_MAJOR_SHIFT: Word = 56;
