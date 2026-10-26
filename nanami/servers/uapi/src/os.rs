use crate::Word;

pub const OS_PORT_SLOT2_DESCRIPTOR: Word = 0x0802_0000_0000_0000;

pub const OS_REQUEST_IRQ_CONTROL: Word = 0x1001;

pub const OS_REQUEST_IO_PORT_CONTROL: Word = 0x1002;

pub const OS_REQUEST_SERVICE_REGISTER: Word = 0x1003;

pub const OS_REQUEST_PAGE_ALLOC: Word = 0x1004;

pub const OS_REQUEST_SERVICE_CONNECT: Word = 0x1005;

pub const OS_REQUEST_DMA_REQUEST: Word = 0x1006;

pub const OS_REQUEST_MMIO_REQUEST: Word = 0x1007;

pub const OS_REQUEST_SHARED_MEMORY_CREATE: Word = 0x1008;

pub const OS_REQUEST_SELF_PID: Word = 0x1009;

pub const OS_REQUEST_EXIT: Word = 0x100a;

pub const OS_REQUEST_INITIAL_FRAMEBUFFER_INFORMATION: Word = 0x100b;

pub const OS_REQUEST_NOTIFICATION_PORT_CREATE: Word = 0x100c;

pub const OS_REQUEST_NOTIFICATION_PORT_COPY: Word = 0x100d;

pub const OS_REQUEST_SHARED_FRAMEBUFFER_CREATE: Word = 0x100e;

pub const OS_REQUEST_HEAP_ALLOC: Word = 0x100f;

pub const OS_REQUEST_SERVICE_LIST: Word = 0x1010;

pub const OS_REQUEST_PROCESS_SPAWN: Word = 0x1011;

pub const OS_REQUEST_PROCESS_STATUS: Word = 0x1012;

pub const OS_REQUEST_PROCESS_REAP: Word = 0x1013;

pub const OS_REQUEST_MAPPING_RELEASE: Word = 0x1014;

pub const OS_REQUEST_PROCESS_KILL: Word = 0x1015;

pub const OS_REQUEST_PROCESS_SPAWN_FAULT_HANDLER: Word = 0x1016;

pub const OS_REQUEST_PROCESS_MEMORY_READ: Word = 0x1017;

pub const OS_REQUEST_PROCESS_MEMORY_WRITE: Word = 0x1018;

pub const OS_REQUEST_PROCESS_MAP_ANONYMOUS: Word = 0x1019;

pub const OS_REQUEST_PROCESS_SPAWN_FAULT_HANDLER_SUSPENDED: Word = 0x101a;

pub const OS_REQUEST_NANAMI_CONTROL: Word = 0x101b;

pub const OS_REQUEST_PROCESS_SPAWN_MEMORY: Word = 0x101c;

pub const OS_REQUEST_PROCESS_SPAWN_MEMORY_FAULT_HANDLER_SUSPENDED: Word = 0x101d;

pub const OS_REQUEST_PROCESS_SPAWN_MEMORY_SUSPENDED: Word = 0x101e;

pub const OS_REQUEST_PROCESS_EXEC_MEMORY: Word = 0x101f;

pub const OS_REQUEST_PROCESS_MEMORY_CLONE: Word = 0x1020;

pub const OS_REQUEST_PROCESS_MEMORY_COPY_WITHIN: Word = 0x1021;

pub const OS_REQUEST_PROCESS_ALIVE: Word = 0x1022;

pub const OS_REQUEST_NANAMI_INFO: Word = 0x1023;

pub const OS_REQUEST_DRIVER_PLATFORM_INFO: Word = 0x1024;

/// Share an entire caller-owned RAM allocation, preserving the caller mapping.
/// (peer PID, page-aligned caller VA, byte length) -> (caller VA, peer VA).
pub const OS_REQUEST_MAPPING_SHARE: Word = 0x1029;

/// Change a supervised process's mapping permissions without discarding backing.
/// (target PID, page-aligned VA, byte length, R/W/X bits) -> ()
pub const OS_REQUEST_PROCESS_MAPPING_PROTECT: Word = 0x102a;
/// Atomic u32 compare/exchange on supervised RAM: (PID, VA, expected, desired) -> previous.
pub const OS_REQUEST_PROCESS_MEMORY_COMPARE_EXCHANGE: Word = 0x102b;
/// Create a suspended raw task sharing a supervised task's address space.
/// args: source PID, PCB destination slot, priority, reserved (zero).
pub const OS_REQUEST_PROCESS_THREAD_CREATE: Word = 0x102c;
/// High bit of MEMORY_READ/WRITE byte count: enforce guest read/write access.
/// Clear retains the supervisor's privileged image/debugging-copy semantics.
pub const OS_MEMORY_COPY_USER_ACCESS: Word = 1 << (Word::BITS - 1);
/// MAPPING_RELEASE arg3: ignore pages without allocated backing (virtual reservations).
pub const OS_MAPPING_RELEASE_ALLOW_HOLES: Word = 1;

pub const OS_REQUEST_DEBUG_PING: Word = 0x10ff;

pub const DRIVER_PLATFORM_INFO_RSDP_ADDRESS: Word = 1;

pub const OS_RESPONSE_OK: Word = 0;

pub const OS_RESPONSE_INVALID_ARGUMENT: Word = 1;

pub const OS_RESPONSE_PERMISSION_DENIED: Word = 2;

pub const OS_RESPONSE_INVALID_DESCRIPTOR: Word = 3;

pub const OS_RESPONSE_ILLEGAL_OPERATION: Word = 4;

pub const OS_RESPONSE_FATAL: Word = 5;

pub const OS_RESPONSE_PONG_MAGIC: Word = 0x504f_4e47;

pub const PROCESS_SLOT_NOTIFICATION: Word = 21;

pub const FRAMEBUFFER_INFORMATION_REGION: Word = 0;

pub const FRAMEBUFFER_INFORMATION_GEOMETRY: Word = 1;

pub const FRAMEBUFFER_INFORMATION_FORMAT: Word = 2;

pub const FRAMEBUFFER_INFORMATION_COLOR_AND_ID: Word = 3;

pub const NANAMI_INFO_MEMORY: Word = 1;

pub const NANAMI_INFO_PROCESS: Word = 2;

pub const NANAMI_INFO_SMP: Word = 3;
pub const NANAMI_INFO_ARCHITECTURE_NAME: Word = 4;
pub const NANAMI_INFO_PLATFORM_NAME: Word = 5;
pub const NANAMI_INFO_KERNEL_VERSION: Word = 6;
pub const NANAMI_INFO_OS_VERSION: Word = 7;
