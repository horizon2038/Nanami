use libnanami::Word;

pub const SLOT_SERVICE_PORT: Word = 20;
pub const SLOT_POSIX_SERVICE: Word = 22;
pub const SLOT_TERMINAL_SERVICE: Word = 23;
pub const SLOT_NETWORK_SERVICE: Word = 24;

pub const ALTER_REQUEST_CONTROL: Word = 0xb101;
pub const ALTER_REQUEST_LOAD_ELF: Word = 0xb102;
pub const ALTER_REQUEST_STATUS: Word = 0xb103;
pub const ALTER_REQUEST_SPAWN_INITRAMFS: Word = 0xb104;
pub const ALTER_REQUEST_SPAWN_LINUX: Word = 0xb105;
pub const ALTER_REQUEST_KILL: Word = 0xb106;
pub const ALTER_REQUEST_KILL_TERMINAL: Word = 0xb107;

pub const ALTER_CONTROL_ATTACH_SHARED_MEMORY: Word = 1;
pub const ALTER_LAUNCH_FLAG_STRACE: Word = 1 << 0;
pub const ALTER_LAUNCH_FLAG_DIAGNOSTICS: Word = 1 << 1;
pub const ALTER_LAUNCH_FLAG_OS_FREEBSD: Word = 1 << 8;

pub const ALTER_DEFAULT_SHM_BYTES: Word = 0x10000;
pub const ALTER_PATH_MAX: usize = 256;
pub const ALTER_IO_OFFSET: usize = 512;
pub const ALTER_MANAGED_PROCESS_MAX: usize = 16;
pub const ALTER_PROCESS_MAPPING_MAX: usize = 32;
pub const ALTER_MANAGED_PCB_SLOT_BASE: Word = 40;
pub const ALTER_IMAGE_NAME_MAX: usize = 24;
pub const ALTER_LAUNCH_MAX_ARGS: usize = 8;
pub const ALTER_LAUNCH_MAX_ENVS: usize = 8;

pub const A9N_FAULT_INVALID_KERNEL_CALL: Word = 5;
