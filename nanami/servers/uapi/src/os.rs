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
// With peer=0, nonzero arg2/arg3 specify a source prefix and its length.
// Construct owned readonly RAM after copying that prefix and zeroing the tail.

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
/// Optional successful EXEC_MEMORY / SPAWN_MEMORY_FAULT_HANDLER_SUSPENDED
/// response arg1. All nonempty raw LOADs have disjoint page ranges and final
/// nonzero rights, with no stack overlap. Valid only for this stopped initial
/// image, before any main-image mutation. Older Alpha replies zero.
pub const OS_PROCESS_IMAGE_FINAL_PERMISSIONS: Word = 1;

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
/// Create a suspended raw task with a private snapshot of a supervised VM.
/// args: source PID, PCB destination slot, priority, options (zero or
/// OS_FORK_SHARE_READONLY). Zero retains the eager private-copy behavior.
pub const OS_REQUEST_PROCESS_FORK: Word = 0x102d;
/// Ordered, permission-checked copies using caller-owned transfer descriptors.
/// args: target PID, descriptor VA, count (1..=16), reserved (zero).
pub const OS_REQUEST_PROCESS_MEMORY_READ_VECTOR: Word = 0x102e;
pub const OS_REQUEST_PROCESS_MEMORY_WRITE_VECTOR: Word = 0x102f;
/// Replace a supervised task's native capability namespace with an empty root.
/// (target PID, zero, zero, zero) -> (). Only its reaper may request this;
/// the first successful call leaves the task suspended; repeats are no-ops.
/// Alpha retains its management
/// capabilities. The restriction survives exec and is inherited by fork/thread.
pub const OS_REQUEST_PROCESS_SEAL_CAPABILITIES: Word = 0x1030;
/// Query/detach private fork aliases: (PID, VA, bytes, mode) -> needs-detach.
/// DETACH requires the reaper to have quiesced every task sharing the VM.
/// Successful detach preserves bytes and R/W/X; only backing becomes private.
pub const OS_REQUEST_PROCESS_PRIVATE_WRITE: Word = 0x1031;
/// Borrow one sealed child's readable private RAM page at a caller-chosen VA.
/// (target PID, target page VA, local page VA, 0); no Frame descriptor is returned.
/// The trusted native supervisor retains the ordinary management Frame copy.
/// The page is pinned until MAPPING_RELEASE, not a live address-space view.
/// The supervisor must invalidate its cached view after backing/protection changes.
pub const OS_REQUEST_PROCESS_READ_VIEW: Word = 0x1032;
/// Borrow one currently writable, unshared private RAM page from a sealed child.
/// Same arguments/lifetime as READ_VIEW; the trusted supervisor must invalidate
/// before backing/protection changes. Never grants guest mapping authority.
pub const OS_REQUEST_PROCESS_WRITE_VIEW: Word = 0x1033;
/// Map native caller-owned immutable RAM into a sealed supervised task privately.
/// (target PID, destination VA, page-rounded bytes, descriptor VA) -> ().
/// Descriptor: [source VA, R/RX permissions]. Source must be owned read-only RAM;
/// it cannot be refilled while aliases live. DETACH precedes every private write.
/// On error, release any installed destination prefix; never fallback-copy over it.
pub const OS_REQUEST_PROCESS_PRIVATE_SNAPSHOT: Word = 0x1034;
/// Trusted supervisor: source VA, source bytes, final R/W/X permissions.
/// Ordinary MAP_ANONYMOUS retains its original two-word initializer contract.
// arg2 == 0 selects automatic heap placement; nonzero selects a fixed base.
// arg3 points to [source, source_bytes, permissions]; the DEFER flag adds a
// fourth word with eager prefix bytes.
pub const OS_REQUEST_PROCESS_MAP_INITIALIZED_PROTECTED: Word = 0x1035;
/// MAP_INITIALIZED_PROTECTED permission flag: a fourth descriptor word gives
/// eager prefix bytes. Only supervised sealed R mappings accept it. Every
/// byte is initialized; available huge leaves may be mapped in full, while
/// remaining small PTEs are populated by the supervisor on demand.
pub const OS_MAP_INITIALIZER_DEFER: Word = 1 << 8;
/// Create immutable owned RAM without mapping it into the native caller.
/// (0, capacity, source VA, initialized prefix bytes) -> (source address, 0).
/// The returned address identifies backing for Alpha copies/private snapshots;
/// it is not a CPU-accessible pointer. Self MAPPING_PROTECT(R) can map it later.
/// No writable mapping/share/view of this source is permitted.
pub const OS_REQUEST_MEMORY_SOURCE_CREATE: Word = 0x1036;
/// Populate initialized deferred snapshot backing, including a bounded cluster.
/// (supervised PID, page VA, required R/W/X bit, 0) -> populated/already-present.
pub const OS_REQUEST_PROCESS_POPULATE_SNAPSHOT: Word = 0x1037;
/// PRIVATE_SNAPSHOT permission flag: descriptor adds page-aligned eager bytes.
/// Only R snapshots accept it; remaining initialized pages retain logical R
/// while their PTE is deferred. A matching supervisor handles missing faults.
pub const OS_PRIVATE_SNAPSHOT_DEFER: Word = 1 << 8;
pub const OS_PRIVATE_WRITE_QUERY: Word = 0;
pub const OS_PRIVATE_WRITE_DETACH: Word = 1;
/// Sealed target, same supervisor quiescence contract as DETACH. A copied
/// pool-backed replacement may retain its old logical rights with an absent
/// PTE, ready for the caller's following PROTECT. The supervisor must support
/// POPULATE_SNAPSHOT faults if that later protection fails or is abandoned.
pub const OS_PRIVATE_WRITE_DETACH_DEFERRED: Word = 2;
// PRIVATE_WRITE response arg0 is a boolean: QUERY found shared private pages,
// or DETACH successfully repaired a nonempty set. No shared pages returns 0.
/// PROCESS_FORK arg3 opt-in (legacy name). Sealed tasks share eligible private
/// backing with read-only hardware leaves, including logically writable COW.
/// Their reaper must quiesce all source VM users and revoke writable views
/// before this fork, handle COW write faults, and DETACH before privileged
/// writes or granting hardware write access. Zero retains eager fork.
pub const OS_FORK_SHARE_READONLY: Word = 1;
pub const OS_MEMORY_COPY_VECTOR_MAX: usize = 16;

/// Native-Word wire record; addresses refer to target and caller respectively.
/// A failure stops at that record; preceding records have already completed.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessMemoryTransfer {
    pub target: Word,
    pub local: Word,
    pub bytes: Word,
}

/// MAP_ANONYMOUS arg3 may point to two caller Words: source VA and initialized
/// byte count. Copy that prefix once and zero only the rest, before publishing
/// each destination page. Zero arg3 retains ordinary anonymous zero fill.
pub const OS_MAP_INITIALIZER_WORDS: usize = 2;
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
