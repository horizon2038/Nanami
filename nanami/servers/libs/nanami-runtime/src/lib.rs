#![no_std]

pub mod debug;
pub mod diagnostics;
pub mod hal;
pub mod heap;
pub mod io;
pub mod ipc;
mod mmio;
mod platform;
pub mod service;
mod version;

pub use platform::{request_nanami_info_platform, NanamiPlatformInfo};
pub use version::{request_kernel_version, request_nanami_version};

pub use a9n_abi::capability_call::address_space::Attribute;
use a9n_abi::capability_call::ipc_port::MessageInfo;
pub use a9n_abi::Word;
use a9n_abi::{CapabilityDescriptor, CapabilityError};

pub use nanami_uapi::os::*;
pub use nanami_uapi::{NanamiError, NanamiResult, RequestError};

#[inline(always)]
pub fn yield_now() {
    a9n_abi::arch::yield_call::yield_call();
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NanamiMemoryInfo {
    pub total_bytes: Word,
    pub free_bytes: Word,
}

impl NanamiMemoryInfo {
    pub const fn used_bytes(self) -> Word {
        self.total_bytes.saturating_sub(self.free_bytes)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NanamiProcessInfo {
    pub running: Word,
    pub exited: Word,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NanamiSmpInfo {
    pub online_cores: Word,
    pub active_core_mask: Word,
    pub enabled: bool,
}

impl NanamiProcessInfo {
    pub const fn total(self) -> Word {
        self.running.saturating_add(self.exited)
    }
}

const PROCESS_ARGUMENT_MAX: usize = 64;
const PROCESS_ARGUMENT_BYTES_MAX: usize = 4096;

static mut PROCESS_ARGC: usize = 0;
static mut PROCESS_ARGV_BASE: *const Word = core::ptr::null();
static mut PROCESS_ENVC: usize = 0;
static mut PROCESS_ENVP_BASE: *const Word = core::ptr::null();

#[doc(hidden)]
pub unsafe fn __init_process_arguments(initial_stack: usize) {
    PROCESS_ARGC = 0;
    PROCESS_ARGV_BASE = core::ptr::null();
    PROCESS_ENVC = 0;
    PROCESS_ENVP_BASE = core::ptr::null();

    if initial_stack == 0 {
        return;
    }

    let stack = initial_stack as *const Word;
    let argc = core::ptr::read(stack) as usize;
    if argc > PROCESS_ARGUMENT_MAX {
        return;
    }

    let argv = stack.add(1);
    let mut i = 0usize;
    while i < argc {
        if core::ptr::read(argv.add(i)) == 0 {
            return;
        }
        i += 1;
    }
    if core::ptr::read(argv.add(argc)) != 0 {
        return;
    }

    let envp = argv.add(argc + 1);
    let mut envc = 0usize;
    while envc < PROCESS_ARGUMENT_MAX {
        if core::ptr::read(envp.add(envc)) == 0 {
            break;
        }
        envc += 1;
    }
    if envc >= PROCESS_ARGUMENT_MAX {
        return;
    }

    PROCESS_ARGC = argc;
    PROCESS_ARGV_BASE = argv;
    PROCESS_ENVC = envc;
    PROCESS_ENVP_BASE = envp;
}

pub fn process_argc() -> usize {
    unsafe { PROCESS_ARGC }
}

pub fn process_arg(index: usize) -> Option<&'static [u8]> {
    unsafe {
        if index >= PROCESS_ARGC || PROCESS_ARGV_BASE.is_null() {
            return None;
        }
        let ptr = core::ptr::read(PROCESS_ARGV_BASE.add(index)) as *const u8;
        c_string_bytes(ptr)
    }
}

pub fn process_env_count() -> usize {
    unsafe { PROCESS_ENVC }
}

pub fn process_env(index: usize) -> Option<&'static [u8]> {
    unsafe {
        if index >= PROCESS_ENVC || PROCESS_ENVP_BASE.is_null() {
            return None;
        }
        let ptr = core::ptr::read(PROCESS_ENVP_BASE.add(index)) as *const u8;
        c_string_bytes(ptr)
    }
}

pub fn process_env_value(name: &[u8]) -> Option<&'static [u8]> {
    let mut i = 0usize;
    while i < process_env_count() {
        let env = process_env(i)?;
        if env.len() > name.len() && &env[..name.len()] == name && env[name.len()] == b'=' {
            return Some(&env[name.len() + 1..]);
        }
        i += 1;
    }
    None
}

unsafe fn c_string_bytes(ptr: *const u8) -> Option<&'static [u8]> {
    if ptr.is_null() {
        return None;
    }
    let mut len = 0usize;
    while len < PROCESS_ARGUMENT_BYTES_MAX {
        if core::ptr::read(ptr.add(len)) == 0 {
            return Some(core::slice::from_raw_parts(ptr, len));
        }
        len += 1;
    }
    None
}

#[macro_export]
macro_rules! print {
    ($s:expr) => {{
        $crate::debug::print(core::format_args!("{}", $s));
    }};
    ($($arg:tt)*) => {{
        $crate::debug::print(core::format_args!($($arg)*));
    }};
}

#[macro_export]
macro_rules! println {
    () => {{
        $crate::debug::println(core::format_args!(""));
    }};
    ($s:expr) => {{
        $crate::debug::println(core::format_args!("{}", $s));
    }};
    ($($arg:tt)*) => {{
        $crate::debug::println(core::format_args!($($arg)*));
    }};
}

#[macro_export]
macro_rules! nanami_entry {
    ($entry:path) => {
        #[cfg(target_arch = "x86_64")]
        $crate::define_x86_64_entry!($entry);

        #[cfg(target_arch = "aarch64")]
        $crate::define_aarch64_entry!($entry);

        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        compile_error!("nanami_entry! is supported only on x86_64 and aarch64");
    };
}

pub fn nanami_exit(result: NanamiResult) -> ! {
    let (is_ok, error_value) = match result {
        Ok(()) => (1usize, 0usize),
        Err(error) => (0usize, error.value()),
    };
    let _ = request_exit_with_status(is_ok, error_value);
    loop {
        core::hint::spin_loop();
    }
}

pub fn request_pages(page_count: Word) -> Result<(), RequestError> {
    if page_count == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, _, _) = call_os_port(OS_REQUEST_PAGE_ALLOC, page_count, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn request_heap(size_bytes: Word) -> Result<(Word, Word), RequestError> {
    if size_bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, base, mapped_size) = call_os_port(OS_REQUEST_HEAP_ALLOC, size_bytes, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if base == 0 || mapped_size == 0 {
        return Err(RequestError::Protocol);
    }
    Ok((base, mapped_size))
}

pub fn request_dma(size_bytes: Word) -> Result<(Word, Word), RequestError> {
    if size_bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, paddr, vaddr) = call_os_port(OS_REQUEST_DMA_REQUEST, size_bytes, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((paddr, vaddr))
}

pub use mmio::{request_mmio, request_mmio_with_mapping};

pub fn request_shared_memory(
    peer_pid: Word,
    size_bytes: Word,
) -> Result<(Word, Word), RequestError> {
    if peer_pid == 0 || size_bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, local_vaddr, peer_vaddr) = call_os_port(
        OS_REQUEST_SHARED_MEMORY_CREATE,
        peer_pid,
        size_bytes,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((local_vaddr, peer_vaddr))
}

pub fn request_shared_framebuffer_memory(
    peer_pid: Word,
    physical_address: Word,
    size_bytes: Word,
) -> Result<(Word, Word), RequestError> {
    if peer_pid == 0 || physical_address == 0 || size_bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, local_vaddr, peer_vaddr) = call_os_port(
        OS_REQUEST_SHARED_FRAMEBUFFER_CREATE,
        peer_pid,
        physical_address,
        size_bytes,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((local_vaddr, peer_vaddr))
}

/// Share existing owned RAM (including DMA RAM) with a peer. Both mappings
/// remain Normal/WB; this is not an arbitrary-physical-address mapping API.
pub fn request_mapping_share(
    peer_pid: Word,
    local_vaddr: Word,
    bytes: Word,
) -> Result<Word, RequestError> {
    if peer_pid == 0 || local_vaddr == 0 || local_vaddr & 4095 != 0 || bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, local, peer) =
        call_os_port(OS_REQUEST_MAPPING_SHARE, peer_pid, local_vaddr, bytes, 0, 4)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if local != local_vaddr || peer == 0 || peer & 4095 != 0 {
        return Err(RequestError::Protocol);
    }
    Ok(peer)
}

/// Allocate zeroed, reclaimable owned RAM, ready to share with supervised guests.
pub fn request_memory_create(bytes: Word) -> Result<Word, RequestError> {
    if bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, local, peer) = call_os_port(OS_REQUEST_SHARED_MEMORY_CREATE, 0, bytes, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if local == 0 || local & 4095 != 0 || peer != 0 {
        return Err(RequestError::Protocol);
    }
    Ok(local)
}

/// Construct reclaimable owned RAM with a copied prefix, zero suffix and
/// readonly leaves. Requires a nonempty source and the matching Alpha API.
pub fn request_memory_create_readonly(
    bytes: Word, source: Word, source_bytes: Word,
) -> Result<Word, RequestError> {
    create_readonly_memory(bytes, source, source_bytes, OS_REQUEST_SHARED_MEMORY_CREATE)
}

/// Immutable backing for Alpha copies/snapshots, with no native CPU mapping.
/// Map R with self MAPPING_PROTECT before dereferencing the returned address.
pub fn request_memory_create_source(
    bytes: Word, source: Word, source_bytes: Word,
) -> Result<Word, RequestError> {
    create_readonly_memory(bytes, source, source_bytes, OS_REQUEST_MEMORY_SOURCE_CREATE)
}

fn create_readonly_memory(bytes: Word, source: Word, source_bytes: Word, code: Word)
    -> Result<Word, RequestError> {
    let capacity = bytes.checked_add(4095).ok_or(RequestError::InvalidArgument)? & !4095;
    if bytes == 0 || source == 0 || source_bytes == 0 || source_bytes > capacity
        || source.checked_add(source_bytes).is_none()
    { return Err(RequestError::InvalidArgument); }
    let (status, local, peer) = call_os_port(
        code, 0, bytes, source, source_bytes, 5,
    )?;
    if status != OS_RESPONSE_OK { return Err(RequestError::Status(status)); }
    if local == 0 || local & 4095 != 0 || local.checked_add(capacity).is_none() || peer != 0 {
        return Err(RequestError::Protocol);
    }
    Ok(local)
}

/// Share owned RAM at a fixed address in a process supervised by the caller.
pub fn request_mapping_share_at(
    peer_pid: Word,
    local_vaddr: Word,
    bytes: Word,
    peer_vaddr: Word,
) -> Result<Word, RequestError> {
    if peer_pid == 0
        || local_vaddr == 0
        || peer_vaddr == 0
        || (local_vaddr | peer_vaddr) & 4095 != 0
        || bytes == 0
    {
        return Err(RequestError::InvalidArgument);
    }
    let (status, local, peer) = call_os_port(
        OS_REQUEST_MAPPING_SHARE,
        peer_pid,
        local_vaddr,
        bytes,
        peer_vaddr,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if local != local_vaddr || peer != peer_vaddr {
        return Err(RequestError::Protocol);
    }
    Ok(peer)
}

pub fn get_self_pid() -> Result<Word, RequestError> {
    let (status, pid, _) = call_os_port(OS_REQUEST_SELF_PID, 0, 0, 0, 0, 1)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(pid)
}

pub fn request_exit() -> Result<(), RequestError> {
    request_exit_with_status(0, 0)
}

pub fn request_exit_with_status(is_ok: Word, error_value: Word) -> Result<(), RequestError> {
    let (status, _, _) = call_os_port(OS_REQUEST_EXIT, is_ok, error_value, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn request_initial_framebuffer_information(
    information_selector: Word,
) -> Result<(Word, Word), RequestError> {
    let (status, detail0, detail1) = call_os_port(
        OS_REQUEST_INITIAL_FRAMEBUFFER_INFORMATION,
        information_selector,
        0,
        0,
        0,
        2,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((detail0, detail1))
}

pub fn request_notification_port_create(
    notification_slot: Word,
    identifier: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_os_port(
        OS_REQUEST_NOTIFICATION_PORT_CREATE,
        notification_slot,
        identifier,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn request_notification_port_copy(
    source_pid: Word,
    source_notification_slot: Word,
    destination_slot: Word,
    identifier: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_os_port(
        OS_REQUEST_NOTIFICATION_PORT_COPY,
        source_pid,
        source_notification_slot,
        destination_slot,
        identifier,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn register_service_by_name(name: &str, service_slot: Word) -> Result<(), RequestError> {
    let _ = register_service_by_name_with_pid(name, service_slot)?;
    Ok(())
}

pub fn register_service_by_name_with_pid(
    name: &str,
    service_slot: Word,
) -> Result<Word, RequestError> {
    let (name0, name1, name2) = pack_service_name_24(name)?;
    let (status, registered_pid, _) = call_os_port(
        OS_REQUEST_SERVICE_REGISTER,
        name0,
        name1,
        name2,
        service_slot,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(registered_pid)
}

pub fn connect_service_by_name(name: &str, destination_slot: Word) -> Result<(), RequestError> {
    let _ = connect_service_by_name_with_pid(name, destination_slot)?;
    Ok(())
}

pub fn connect_service_by_name_with_pid(
    name: &str,
    destination_slot: Word,
) -> Result<Word, RequestError> {
    let (name0, name1, name2) = pack_service_name_24(name)?;
    let (status, pid, _) = call_os_port(
        OS_REQUEST_SERVICE_CONNECT,
        destination_slot,
        name0,
        name1,
        name2,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(pid)
}

pub fn service_info_by_ordinal(
    ordinal: Word,
) -> Result<nanami_uapi::registry::ServiceInfo, RequestError> {
    use nanami_uapi::registry::{ServiceInfo, SERVICE_NAME_BYTES};
    let (status, owner_pid, name_len) = call_os_port(OS_REQUEST_SERVICE_LIST, ordinal, 0, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if name_len == 0 || name_len > SERVICE_NAME_BYTES {
        return Err(RequestError::InvalidArgument);
    }
    let mut info = ServiceInfo {
        owner_pid,
        name_len,
        name: [0; SERVICE_NAME_BYTES],
    };
    for chunk in 0..name_len.div_ceil(2 * core::mem::size_of::<Word>()) {
        let (status, first, second) =
            call_os_port(OS_REQUEST_SERVICE_LIST, ordinal, chunk + 1, 0, 0, 3)?;
        if status != OS_RESPONSE_OK {
            return Err(RequestError::Status(status));
        }
        for (offset, byte) in first
            .to_le_bytes()
            .into_iter()
            .chain(second.to_le_bytes())
            .enumerate()
        {
            let index = chunk * 2 * core::mem::size_of::<Word>() + offset;
            if index < SERVICE_NAME_BYTES {
                info.name[index] = byte;
            }
        }
    }
    if core::str::from_utf8(&info.name[..name_len]).is_err() || info.name[..name_len].contains(&0) {
        return Err(RequestError::InvalidArgument);
    }
    Ok(info)
}

pub fn request_process_spawn(image_name: &str) -> Result<Word, RequestError> {
    let (name0, name1, name2) = pack_name_24(image_name)?;
    let (status, child_pid, _) = call_os_port(OS_REQUEST_PROCESS_SPAWN, name0, name1, name2, 0, 4)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(child_pid)
}

pub fn request_process_spawn_memory(
    image_vaddr: Word,
    size_bytes: Word,
    priority: Word,
) -> Result<Word, RequestError> {
    if image_vaddr == 0 || size_bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, child_pid, _) = call_os_port(
        OS_REQUEST_PROCESS_SPAWN_MEMORY,
        image_vaddr,
        size_bytes,
        priority,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(child_pid)
}

pub fn request_process_spawn_memory_fault_handler_suspended(
    image_vaddr: Word,
    size_bytes: Word,
    priority: Word,
    pcb_destination_slot: Word,
) -> Result<Word, RequestError> {
    request_process_spawn_memory_fault_handler_suspended_with_permissions(
        image_vaddr, size_bytes, priority, pcb_destination_slot,
    ).map(|(pid, _)| pid)
}

/// The optional acknowledgement describes only the stopped initial image.
/// A zero/unknown reply keeps the supervisor's ordinary protection pass.
pub fn request_process_spawn_memory_fault_handler_suspended_with_permissions(
    image_vaddr: Word, size_bytes: Word, priority: Word, pcb_destination_slot: Word,
) -> Result<(Word, bool), RequestError> {
    if image_vaddr == 0 || size_bytes == 0 || pcb_destination_slot == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, child_pid, permissions) = call_os_port(
        OS_REQUEST_PROCESS_SPAWN_MEMORY_FAULT_HANDLER_SUSPENDED,
        image_vaddr,
        size_bytes,
        priority,
        pcb_destination_slot,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((child_pid, permissions == OS_PROCESS_IMAGE_FINAL_PERMISSIONS))
}

/// Drop a supervised guest's direct native capability access. The first call
/// leaves it stopped; repeating an already successful seal is a no-op.
/// Its supervisor retains the PCB and Alpha retains all memory-management caps.
pub fn request_process_seal_capabilities(pid: Word) -> Result<(), RequestError> {
    let (status, _, _) = call_os_port(OS_REQUEST_PROCESS_SEAL_CAPABILITIES, pid, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn request_process_thread_create(
    source_pid: Word,
    pcb_destination_slot: Word,
    priority: Word,
) -> Result<Word, RequestError> {
    let (status, pid, _) = call_os_port(
        OS_REQUEST_PROCESS_THREAD_CREATE,
        source_pid,
        pcb_destination_slot,
        priority,
        0,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(pid)
}

/// Snapshot the current VM, without opening or loading its executable again.
/// The new PCB remains suspended until its supervisor installs the registers.
pub fn request_process_fork(
    source_pid: Word,
    pcb_destination_slot: Word,
    priority: Word,
) -> Result<Word, RequestError> {
    request_process_fork_options(source_pid, pcb_destination_slot, priority, 0)
}

/// Opt-in private COW (legacy function name). The supervisor must quiesce all
/// source VM users and revoke writable views before forking, repair COW write
/// faults, and query/detach before privileged writes or hardware upgrades.
pub fn request_process_fork_shared_readonly(
    source_pid: Word,
    pcb_destination_slot: Word,
    priority: Word,
) -> Result<Word, RequestError> {
    request_process_fork_options(
        source_pid,
        pcb_destination_slot,
        priority,
        OS_FORK_SHARE_READONLY,
    )
}

fn request_process_fork_options(
    source_pid: Word,
    pcb_destination_slot: Word,
    priority: Word,
    options: Word,
) -> Result<Word, RequestError> {
    let (status, pid, _) = call_os_port(
        OS_REQUEST_PROCESS_FORK,
        source_pid,
        pcb_destination_slot,
        priority,
        options,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(pid)
}

/// Query is side-effect free. DETACH requires all tasks sharing the target VM
/// to be inactive through the current fault or a completed synchronous stop.
/// QUERY returns whether shared private pages exist; DETACH returns whether
/// any were successfully repaired. DEFERRED additionally permits absent
/// replacement PTEs until PROTECT or POPULATE_SNAPSHOT; it requires a sealed
/// target and a supervisor supporting those missing faults. A failing detach
/// never reports success.
pub fn request_process_private_write(
    pid: Word,
    address: Word,
    bytes: Word,
    mode: Word,
) -> Result<bool, RequestError> {
    if pid == 0
        || bytes == 0
        || address.checked_add(bytes).is_none()
        || !matches!(mode, OS_PRIVATE_WRITE_QUERY | OS_PRIVATE_WRITE_DETACH | OS_PRIVATE_WRITE_DETACH_DEFERRED)
    {
        return Err(RequestError::InvalidArgument);
    }
    let (status, needed, _) = call_os_port(
        OS_REQUEST_PROCESS_PRIVATE_WRITE,
        pid,
        address,
        bytes,
        mode,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if needed > 1 {
        return Err(RequestError::Protocol);
    }
    Ok(needed != 0)
}

/// Alias immutable caller-owned RAM privately into a sealed supervised task.
/// The caller must never refill its source and must detach before private writes.
/// Errors may leave an owned destination prefix which the caller must release.
pub fn request_process_map_private_snapshot(
    pid: Word,
    base: Word,
    bytes: Word,
    source: Word,
    permissions: Word,
) -> Result<(), RequestError> {
    map_private_snapshot(pid, base, bytes, source, permissions, None)
}

pub fn request_process_map_private_snapshot_prefix(
    pid: Word, base: Word, bytes: Word, source: Word, eager_bytes: Word,
) -> Result<(), RequestError> {
    map_private_snapshot(pid, base, bytes, source, 1, Some(eager_bytes))
}

fn map_private_snapshot(pid: Word, base: Word, bytes: Word, source: Word,
    permissions: Word, eager: Option<Word>) -> Result<(), RequestError> {
    if pid == 0
        || base == 0
        || source == 0
        || bytes == 0
        || (base | source | bytes) & 4095 != 0
        || base.checked_add(bytes).is_none()
        || source.checked_add(bytes).is_none()
        || (permissions != 1 && permissions != 5)
    {
        return Err(RequestError::InvalidArgument);
    }
    if eager.is_some_and(|prefix| permissions != 1 || prefix == 0
        || prefix > bytes || prefix & 4095 != 0) { return Err(RequestError::InvalidArgument); }
    let descriptor = [source, permissions | if eager.is_some() { OS_PRIVATE_SNAPSHOT_DEFER } else { 0 }, eager.unwrap_or(0)];
    let (status, _, _) = call_os_port(
        OS_REQUEST_PROCESS_PRIVATE_SNAPSHOT,
        pid,
        base,
        bytes,
        descriptor.as_ptr() as Word,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

/// Complete a missing immutable-snapshot PTE, preserving current logical rights.
pub fn request_process_populate_snapshot(pid: Word, page: Word, required: Word)
    -> Result<bool, RequestError> {
    if pid == 0 || page == 0 || page & 4095 != 0 || !matches!(required, 1 | 2 | 4) {
        return Err(RequestError::InvalidArgument);
    }
    let (status, populated, _) = call_os_port(OS_REQUEST_PROCESS_POPULATE_SNAPSHOT,
        pid, page, required, 0, 4)?;
    if status != OS_RESPONSE_OK { return Err(RequestError::Status(status)); }
    if populated > 1 { return Err(RequestError::Protocol); }
    Ok(populated != 0)
}

/// Allocate initialized private pages at a fixed address. The source prefix is
/// copied directly by Alpha; only the remaining bytes are zeroed.
pub fn request_process_map_initialized_at(
    target_pid: Word,
    base: Word,
    mapped_bytes: Word,
    source: Word,
    source_bytes: Word,
) -> Result<(), RequestError> {
    request_map_initialized(target_pid, base, mapped_bytes, source, source_bytes, None)
}

/// Trusted supervisor only: publish freshly initialized pages with final rights.
/// Select the address using Alpha's ordinary automatic heap reservation.
pub fn request_process_map_anonymous_protected(
    target_pid: Word,
    size_bytes: Word,
    permissions: Word,
) -> Result<(Word, Word), RequestError> {
    if target_pid == 0 || size_bytes == 0 || permissions == 0 || permissions & !7 != 0 {
        return Err(RequestError::InvalidArgument);
    }
    let mapped = size_bytes.checked_add(4095).ok_or(RequestError::InvalidArgument)? & !4095;
    let initializer = [0, 0, permissions];
    let (status, base, bytes) = call_os_port(
        OS_REQUEST_PROCESS_MAP_INITIALIZED_PROTECTED,
        target_pid,
        mapped,
        0,
        initializer.as_ptr() as Word,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if base == 0 || base & 4095 != 0 || bytes != mapped || base.checked_add(bytes).is_none() {
        return Err(RequestError::Protocol);
    }
    Ok((base, bytes))
}

/// Trusted supervisor only: publish freshly initialized pages with final rights.
pub fn request_process_map_initialized_protected_at(
    target_pid: Word,
    base: Word,
    mapped_bytes: Word,
    source: Word,
    source_bytes: Word,
    permissions: Word,
) -> Result<(), RequestError> {
    if permissions == 0 || permissions & !7 != 0 {
        return Err(RequestError::InvalidArgument);
    }
    request_map_initialized(target_pid, base, mapped_bytes, source, source_bytes, Some(permissions))
}

/// Initialize private readonly backing directly, retaining lazy small leaves.
/// Errors can leave an owned prefix; ordinary whole-range cleanup still owns it.
pub fn request_process_map_initialized_deferred_at(
    target_pid: Word, base: Word, mapped_bytes: Word, source: Word,
    source_bytes: Word, eager_bytes: Word,
) -> Result<(), RequestError> {
    if target_pid == 0 || base == 0 || mapped_bytes == 0
        || (base | mapped_bytes | eager_bytes) & 4095 != 0
        || eager_bytes == 0 || eager_bytes > mapped_bytes
        || base.checked_add(mapped_bytes).is_none() || source_bytes > mapped_bytes
        || (source_bytes != 0 && source == 0) || source.checked_add(source_bytes).is_none()
    { return Err(RequestError::InvalidArgument); }
    let initializer = [source, source_bytes, 1 | OS_MAP_INITIALIZER_DEFER, eager_bytes];
    let (status, address, bytes) = call_os_port(OS_REQUEST_PROCESS_MAP_INITIALIZED_PROTECTED,
        target_pid, mapped_bytes, base, initializer.as_ptr() as Word, 5)?;
    if status != OS_RESPONSE_OK { return Err(RequestError::Status(status)); }
    if address != base || bytes != mapped_bytes { return Err(RequestError::InvalidArgument); }
    Ok(())
}

fn request_map_initialized(
    target_pid: Word,
    base: Word,
    mapped_bytes: Word,
    source: Word,
    source_bytes: Word,
    permissions: Option<Word>,
) -> Result<(), RequestError> {
    if base == 0
        || mapped_bytes == 0
        || (base | mapped_bytes) & 4095 != 0
        || source_bytes > mapped_bytes
        || (source_bytes != 0 && source == 0)
    {
        return Err(RequestError::InvalidArgument);
    }
    let initializer = [source, source_bytes, permissions.unwrap_or(7)];
    let (status, address, bytes) = call_os_port(
        if permissions.is_some() { OS_REQUEST_PROCESS_MAP_INITIALIZED_PROTECTED } else { OS_REQUEST_PROCESS_MAP_ANONYMOUS },
        target_pid,
        mapped_bytes,
        base,
        initializer.as_ptr() as Word,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if address != base || bytes != mapped_bytes {
        return Err(RequestError::InvalidArgument);
    }
    Ok(())
}

pub fn request_process_spawn_memory_suspended(
    image_vaddr: Word,
    size_bytes: Word,
    priority: Word,
    pcb_destination_slot: Word,
) -> Result<Word, RequestError> {
    if image_vaddr == 0 || size_bytes == 0 || pcb_destination_slot == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, child_pid, _) = call_os_port(
        OS_REQUEST_PROCESS_SPAWN_MEMORY_SUSPENDED,
        image_vaddr,
        size_bytes,
        priority,
        pcb_destination_slot,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(child_pid)
}

pub fn request_process_exec_memory(
    target_pid: Word,
    image_vaddr: Word,
    size_bytes: Word,
    priority: Word,
) -> Result<Word, RequestError> {
    request_process_exec_memory_with_permissions(target_pid, image_vaddr, size_bytes, priority)
        .map(|(entry, _)| entry)
}

/// Only a successful initial-image acknowledgement may omit redundant LOAD
/// protections. It conveys no authority for later mmap/mprotect operations.
pub fn request_process_exec_memory_with_permissions(
    target_pid: Word, image_vaddr: Word, size_bytes: Word, priority: Word,
) -> Result<(Word, bool), RequestError> {
    if target_pid == 0 || image_vaddr == 0 || size_bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, entry_point, permissions) = call_os_port(
        OS_REQUEST_PROCESS_EXEC_MEMORY,
        target_pid,
        image_vaddr,
        size_bytes,
        priority,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((entry_point, permissions == OS_PROCESS_IMAGE_FINAL_PERMISSIONS))
}

pub fn request_process_spawn_fault_handler(
    image_name: &str,
    pcb_destination_slot: Word,
) -> Result<Word, RequestError> {
    request_process_spawn_fault_handler_impl(
        OS_REQUEST_PROCESS_SPAWN_FAULT_HANDLER,
        image_name,
        pcb_destination_slot,
    )
}

pub fn request_process_spawn_fault_handler_suspended(
    image_name: &str,
    pcb_destination_slot: Word,
) -> Result<Word, RequestError> {
    request_process_spawn_fault_handler_impl(
        OS_REQUEST_PROCESS_SPAWN_FAULT_HANDLER_SUSPENDED,
        image_name,
        pcb_destination_slot,
    )
}

fn request_process_spawn_fault_handler_impl(
    request_code: Word,
    image_name: &str,
    pcb_destination_slot: Word,
) -> Result<Word, RequestError> {
    if pcb_destination_slot == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (name0, name1, name2) = pack_name_24(image_name)?;
    let (status, child_pid, _) =
        call_os_port(request_code, name0, name1, name2, pcb_destination_slot, 5)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(child_pid)
}

pub fn request_process_status(pid: Word) -> Result<(bool, Word), RequestError> {
    if pid == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, exited, exit_code) = call_os_port(OS_REQUEST_PROCESS_STATUS, pid, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((exited != 0, exit_code))
}

pub fn request_process_alive(pid: Word) -> Result<bool, RequestError> {
    if pid == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, alive, _) = call_os_port(OS_REQUEST_PROCESS_ALIVE, pid, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(alive != 0)
}

pub fn request_process_reap(pid: Word) -> Result<(), RequestError> {
    if pid == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, _, _) = call_os_port(OS_REQUEST_PROCESS_REAP, pid, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn request_process_kill(pid: Word, signal: Word) -> Result<(), RequestError> {
    if pid == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, _, _) = call_os_port(OS_REQUEST_PROCESS_KILL, pid, signal, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn request_process_memory_read(
    target_pid: Word,
    target_vaddr: Word,
    local_vaddr: Word,
    size_bytes: Word,
) -> Result<(), RequestError> {
    if target_pid == 0 || target_vaddr == 0 || local_vaddr == 0 || size_bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, _, _) = call_os_port(
        OS_REQUEST_PROCESS_MEMORY_READ,
        target_pid,
        target_vaddr,
        local_vaddr,
        size_bytes,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn request_process_memory_read_checked(
    pid: Word,
    target: Word,
    destination: Word,
    bytes: Word,
) -> Result<(), RequestError> {
    if bytes == 0 || bytes & OS_MEMORY_COPY_USER_ACCESS != 0 {
        return Err(RequestError::InvalidArgument);
    }
    request_process_memory_read(pid, target, destination, bytes | OS_MEMORY_COPY_USER_ACCESS)
}

/// Borrow one readable private RAM page from a sealed supervised task.
/// `local` must be a reserved, otherwise unused page in the caller's layout.
/// Release it with request_mapping_release(local, 4096) before reusing the VA.
/// It pins backing, not the target mapping: the caller owns cache invalidation.
/// On any error, conservatively clean/quarantine `local` before another grant.
pub fn request_process_read_view(pid: Word, target: Word, local: Word) -> Result<(), RequestError> {
    request_process_page_view(OS_REQUEST_PROCESS_READ_VIEW, pid, target, local)
}

/// Borrow writable private RAM, not a mapping with elevated guest permissions.
/// The same invalidation, lifetime and error-cleanup rules as READ_VIEW apply.
pub fn request_process_write_view(
    pid: Word,
    target: Word,
    local: Word,
) -> Result<(), RequestError> {
    request_process_page_view(OS_REQUEST_PROCESS_WRITE_VIEW, pid, target, local)
}

fn request_process_page_view(
    code: Word,
    pid: Word,
    target: Word,
    local: Word,
) -> Result<(), RequestError> {
    if pid == 0 || target == 0 || local == 0 || (target | local) & 4095 != 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, _, _) = call_os_port(code, pid, target, local, 0, 5)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn request_process_memory_write_checked(
    pid: Word,
    target: Word,
    source: Word,
    bytes: Word,
) -> Result<(), RequestError> {
    if bytes == 0 || bytes & OS_MEMORY_COPY_USER_ACCESS != 0 {
        return Err(RequestError::InvalidArgument);
    }
    request_process_memory_write(pid, target, source, bytes | OS_MEMORY_COPY_USER_ACCESS)
}

pub fn request_process_memory_readv_checked(
    pid: Word,
    transfers: &[ProcessMemoryTransfer],
) -> Result<(), RequestError> {
    request_process_memory_vector(OS_REQUEST_PROCESS_MEMORY_READ_VECTOR, pid, transfers)
}

pub fn request_process_memory_writev_checked(
    pid: Word,
    transfers: &[ProcessMemoryTransfer],
) -> Result<(), RequestError> {
    request_process_memory_vector(OS_REQUEST_PROCESS_MEMORY_WRITE_VECTOR, pid, transfers)
}

fn request_process_memory_vector(
    operation: Word,
    pid: Word,
    transfers: &[ProcessMemoryTransfer],
) -> Result<(), RequestError> {
    if pid == 0 || transfers.is_empty() || transfers.len() > OS_MEMORY_COPY_VECTOR_MAX {
        return Err(RequestError::InvalidArgument);
    }
    // Entry validation belongs to the server, in order: rejecting a later
    // record here would prevent earlier records from completing.
    let (status, _, _) = call_os_port(
        operation,
        pid,
        transfers.as_ptr() as Word,
        transfers.len() as Word,
        0,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn request_process_memory_write(
    target_pid: Word,
    target_vaddr: Word,
    local_vaddr: Word,
    size_bytes: Word,
) -> Result<(), RequestError> {
    if target_pid == 0 || target_vaddr == 0 || local_vaddr == 0 || size_bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, _, _) = call_os_port(
        OS_REQUEST_PROCESS_MEMORY_WRITE,
        target_pid,
        target_vaddr,
        local_vaddr,
        size_bytes,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn request_process_memory_clone(
    source_pid: Word,
    destination_pid: Word,
    base_vaddr: Word,
    size_bytes: Word,
) -> Result<(), RequestError> {
    if source_pid == 0 || destination_pid == 0 || base_vaddr == 0 || size_bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, _, _) = call_os_port(
        OS_REQUEST_PROCESS_MEMORY_CLONE,
        source_pid,
        destination_pid,
        base_vaddr,
        size_bytes,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn request_process_memory_copy_within(
    pid: Word,
    source_vaddr: Word,
    destination_vaddr: Word,
    size_bytes: Word,
) -> Result<(), RequestError> {
    if pid == 0 || source_vaddr == 0 || destination_vaddr == 0 || size_bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, _, _) = call_os_port(
        OS_REQUEST_PROCESS_MEMORY_COPY_WITHIN,
        pid,
        source_vaddr,
        destination_vaddr,
        size_bytes,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn request_process_map_anonymous(
    target_pid: Word,
    size_bytes: Word,
) -> Result<(Word, Word), RequestError> {
    if target_pid == 0 || size_bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, base, mapped_size) = call_os_port(
        OS_REQUEST_PROCESS_MAP_ANONYMOUS,
        target_pid,
        size_bytes,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((base, mapped_size))
}

pub fn request_process_map_anonymous_at(
    target_pid: Word,
    base_vaddr: Word,
    size_bytes: Word,
) -> Result<(Word, Word), RequestError> {
    if target_pid == 0 || base_vaddr == 0 || size_bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, base, mapped_size) = call_os_port(
        OS_REQUEST_PROCESS_MAP_ANONYMOUS,
        target_pid,
        size_bytes,
        base_vaddr,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((base, mapped_size))
}

pub fn request_mapping_release(base_vaddr: Word, size_bytes: Word) -> Result<(), RequestError> {
    if base_vaddr == 0 || size_bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, _, _) = call_os_port(OS_REQUEST_MAPPING_RELEASE, base_vaddr, size_bytes, 0, 0, 3)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn request_process_mapping_protect(
    target_pid: Word,
    base_vaddr: Word,
    bytes: Word,
    permissions: Word,
) -> Result<(), RequestError> {
    if target_pid == 0 || base_vaddr & 4095 != 0 || bytes == 0 || permissions & !7 != 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, _, _) = call_os_port(
        OS_REQUEST_PROCESS_MAPPING_PROTECT,
        target_pid,
        base_vaddr,
        bytes,
        permissions,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn request_process_memory_compare_exchange_u32(
    target_pid: Word,
    address: Word,
    expected: u32,
    desired: u32,
) -> Result<u32, RequestError> {
    if target_pid == 0 || address == 0 || address & 3 != 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, previous, _) = call_os_port(
        OS_REQUEST_PROCESS_MEMORY_COMPARE_EXCHANGE,
        target_pid,
        address,
        expected as Word,
        desired as Word,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    u32::try_from(previous).map_err(|_| RequestError::Protocol)
}

pub fn request_process_mapping_release(
    target_pid: Word,
    base_vaddr: Word,
    size_bytes: Word,
) -> Result<(), RequestError> {
    if target_pid == 0 || base_vaddr == 0 || size_bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, _, _) = call_os_port(
        OS_REQUEST_MAPPING_RELEASE,
        base_vaddr,
        size_bytes,
        target_pid,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

/// Release a supervised virtual range, including unpopulated reservations.
pub fn request_process_mapping_release_sparse(
    target_pid: Word,
    base_vaddr: Word,
    size_bytes: Word,
) -> Result<(), RequestError> {
    if target_pid == 0 || base_vaddr == 0 || size_bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, _, _) = call_os_port(
        OS_REQUEST_MAPPING_RELEASE,
        base_vaddr,
        size_bytes,
        target_pid,
        OS_MAPPING_RELEASE_ALLOW_HOLES,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn request_nanami_control(property: &str, value: &str) -> Result<(), RequestError> {
    let (property0, property1) = pack_control_text_16(property)?;
    let (value0, value1) = pack_control_text_16(value)?;
    let (status, _, _) = call_os_port(
        OS_REQUEST_NANAMI_CONTROL,
        property0,
        property1,
        value0,
        value1,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn request_nanami_info_memory() -> Result<NanamiMemoryInfo, RequestError> {
    let (status, total_bytes, free_bytes) =
        call_os_port(OS_REQUEST_NANAMI_INFO, NANAMI_INFO_MEMORY, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if free_bytes > total_bytes {
        return Err(RequestError::Protocol);
    }
    Ok(NanamiMemoryInfo {
        total_bytes,
        free_bytes,
    })
}

pub fn request_nanami_info_process() -> Result<NanamiProcessInfo, RequestError> {
    let (status, running, exited) =
        call_os_port(OS_REQUEST_NANAMI_INFO, NANAMI_INFO_PROCESS, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(NanamiProcessInfo { running, exited })
}

pub fn request_nanami_info_smp() -> Result<NanamiSmpInfo, RequestError> {
    let (status, online_cores, active_core_mask) =
        call_os_port(OS_REQUEST_NANAMI_INFO, NANAMI_INFO_SMP, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    let valid_core_mask = if online_cores >= Word::BITS as Word {
        Word::MAX
    } else {
        (1usize << online_cores) - 1
    };
    if online_cores == 0 || active_core_mask & !valid_core_mask != 0 {
        return Err(RequestError::Protocol);
    }
    Ok(NanamiSmpInfo {
        online_cores,
        active_core_mask,
        enabled: online_cores > 1,
    })
}

/// Query raw architecture boot data from a component explicitly granted
/// `platform=1` by boot policy. On x86_64 this exposes the RSDP address, not a
/// kernel-discovered device address. Alpha rejects every other process.
pub fn request_driver_platform_info(kind: Word) -> Result<(Word, Word), RequestError> {
    let (status, detail0, detail1) =
        call_os_port(OS_REQUEST_DRIVER_PLATFORM_INFO, kind, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((detail0, detail1))
}

pub fn request_irq(
    irq_number: Word,
    notification_slot: Word,
    interrupt_slot: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_os_port(
        OS_REQUEST_IRQ_CONTROL,
        irq_number,
        notification_slot,
        interrupt_slot,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn request_io_port(
    range_min: Word,
    range_max: Word,
    io_slot: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_os_port(
        OS_REQUEST_IO_PORT_CONTROL,
        range_min,
        range_max,
        io_slot,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn ping(token: Word) -> Result<Word, RequestError> {
    let (status, echoed, magic) = call_os_port(OS_REQUEST_DEBUG_PING, token, 0, 0, 0, 5)?;

    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if magic != OS_RESPONSE_PONG_MAGIC {
        return Err(RequestError::Protocol);
    }

    Ok(echoed)
}

pub(crate) fn map_capability_error(error: CapabilityError) -> RequestError {
    match error {
        CapabilityError::InvalidArgument => RequestError::InvalidArgument,
        _ => RequestError::Transport,
    }
}

pub(crate) fn call_port(
    target_port: CapabilityDescriptor,
    request_code: Word,
    arg0: Word,
    arg1: Word,
    arg2: Word,
    arg3: Word,
    message_length: u8,
) -> Result<(Word, Word, Word), RequestError> {
    ipc::init_ipc_tls()?;

    let mut info;
    let mut sender_id;
    loop {
        let ipc_buffer = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
        ipc_buffer.configure_message(4, request_code);
        ipc_buffer.configure_message(5, arg0);
        ipc_buffer.configure_message(6, arg1);
        ipc_buffer.configure_message(7, arg2);
        ipc_buffer.configure_message(8, arg3);
        ipc_buffer.configure_message(9, 0);

        info = MessageInfo::normal(true, message_length, 0);
        sender_id = 0;
        a9n_abi::arch::ipc_port::call(target_port, &mut info, &mut sender_id)
            .map_err(map_capability_error)?;

        if info.is_notification() {
            ipc::preserve_interrupted_notification(sender_id);
            continue;
        }
        if !info.is_normal() || info.message_length() < 3 {
            return Err(RequestError::Protocol);
        }
        break;
    }

    let ipc_buffer = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    Ok((
        ipc_buffer.get_message(4),
        ipc_buffer.get_message(5),
        ipc_buffer.get_message(6),
    ))
}

pub fn call_service_port(
    target_port: CapabilityDescriptor,
    request_code: Word,
    arg0: Word,
    arg1: Word,
    arg2: Word,
    arg3: Word,
    message_length: u8,
) -> Result<(Word, Word, Word), RequestError> {
    call_port(
        target_port,
        request_code,
        arg0,
        arg1,
        arg2,
        arg3,
        message_length,
    )
}

fn call_os_port(
    request_code: Word,
    arg0: Word,
    arg1: Word,
    arg2: Word,
    arg3: Word,
    message_length: u8,
) -> Result<(Word, Word, Word), RequestError> {
    call_port(
        OS_PORT_SLOT2_DESCRIPTOR,
        request_code,
        arg0,
        arg1,
        arg2,
        arg3,
        message_length,
    )
}

fn pack_name_24(name: &str) -> Result<(Word, Word, Word), RequestError> {
    const NAME_BYTES: usize = 24;
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > NAME_BYTES {
        return Err(RequestError::InvalidArgument);
    }
    let mut raw = [0u8; NAME_BYTES];
    raw[..bytes.len()].copy_from_slice(bytes);

    let mut chunk0 = [0u8; core::mem::size_of::<Word>()];
    let mut chunk1 = [0u8; core::mem::size_of::<Word>()];
    let mut chunk2 = [0u8; core::mem::size_of::<Word>()];
    chunk0.copy_from_slice(&raw[0..8]);
    chunk1.copy_from_slice(&raw[8..16]);
    chunk2.copy_from_slice(&raw[16..24]);
    Ok((
        Word::from_le_bytes(chunk0),
        Word::from_le_bytes(chunk1),
        Word::from_le_bytes(chunk2),
    ))
}

fn pack_service_name_24(name: &str) -> Result<(Word, Word, Word), RequestError> {
    pack_name_24(name)
}

fn pack_control_text_16(text: &str) -> Result<(Word, Word), RequestError> {
    const TEXT_BYTES: usize = 16;
    let bytes = text.as_bytes();
    if bytes.is_empty() || bytes.len() > TEXT_BYTES {
        return Err(RequestError::InvalidArgument);
    }
    let mut raw = [0u8; TEXT_BYTES];
    raw[..bytes.len()].copy_from_slice(bytes);

    let mut chunk0 = [0u8; core::mem::size_of::<Word>()];
    let mut chunk1 = [0u8; core::mem::size_of::<Word>()];
    chunk0.copy_from_slice(&raw[0..8]);
    chunk1.copy_from_slice(&raw[8..16]);
    Ok((Word::from_le_bytes(chunk0), Word::from_le_bytes(chunk1)))
}
