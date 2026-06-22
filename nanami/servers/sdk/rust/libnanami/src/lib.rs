#![no_std]

pub mod debug;
pub mod hal;
pub mod heap;
pub mod io;
pub mod ipc;

use a9n_abi::capability_call::ipc_port::MessageInfo;
pub use a9n_abi::Word;
use a9n_abi::{CapabilityDescriptor, CapabilityError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestError {
    InvalidArgument,
    Unsupported,
    Transport,
    Protocol,
    Status(Word),
}

impl core::fmt::Display for RequestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            RequestError::InvalidArgument => f.write_str("invalid-arg"),
            RequestError::Unsupported => f.write_str("unsupported"),
            RequestError::Transport => f.write_str("transport"),
            RequestError::Protocol => f.write_str("protocol"),
            RequestError::Status(status) => write!(f, "status={:#x}", status),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NanamiError(pub Word);

impl NanamiError {
    pub const UNKNOWN: Self = Self(1);
    pub const INVALID_ARGUMENT: Self = Self(2);
    pub const UNSUPPORTED: Self = Self(3);
    pub const TRANSPORT: Self = Self(4);
    pub const PROTOCOL: Self = Self(5);

    #[inline(always)]
    pub const fn value(self) -> Word {
        self.0
    }
}

impl From<RequestError> for NanamiError {
    fn from(value: RequestError) -> Self {
        match value {
            RequestError::InvalidArgument => Self::INVALID_ARGUMENT,
            RequestError::Unsupported => Self::UNSUPPORTED,
            RequestError::Transport => Self::TRANSPORT,
            RequestError::Protocol => Self::PROTOCOL,
            RequestError::Status(code) => Self(code),
        }
    }
}

pub type NanamiResult = core::result::Result<(), NanamiError>;

const OS_PORT_SLOT2_DESCRIPTOR: CapabilityDescriptor = 0x0802_0000_0000_0000;

const OS_REQUEST_IRQ_CONTROL: Word = 0x1001;
const OS_REQUEST_IO_PORT_CONTROL: Word = 0x1002;
const OS_REQUEST_SERVICE_REGISTER: Word = 0x1003;
const OS_REQUEST_PAGE_ALLOC: Word = 0x1004;
const OS_REQUEST_SERVICE_CONNECT: Word = 0x1005;
const OS_REQUEST_DMA_REQUEST: Word = 0x1006;
const OS_REQUEST_MMIO_REQUEST: Word = 0x1007;
const OS_REQUEST_SHARED_MEMORY_CREATE: Word = 0x1008;
const OS_REQUEST_SELF_PID: Word = 0x1009;
const OS_REQUEST_EXIT: Word = 0x100a;
const OS_REQUEST_INITIAL_FRAMEBUFFER_INFORMATION: Word = 0x100b;
const OS_REQUEST_NOTIFICATION_PORT_CREATE: Word = 0x100c;
const OS_REQUEST_NOTIFICATION_PORT_COPY: Word = 0x100d;
const OS_REQUEST_SHARED_FRAMEBUFFER_CREATE: Word = 0x100e;
const OS_REQUEST_HEAP_ALLOC: Word = 0x100f;
const OS_REQUEST_SERVICE_LIST: Word = 0x1010;
const OS_REQUEST_PROCESS_SPAWN: Word = 0x1011;
const OS_REQUEST_PROCESS_STATUS: Word = 0x1012;
const OS_REQUEST_PROCESS_REAP: Word = 0x1013;
const OS_REQUEST_MAPPING_RELEASE: Word = 0x1014;
const OS_REQUEST_DEBUG_PING: Word = 0x10ff;

pub const OS_RESPONSE_OK: Word = 0;
pub const OS_RESPONSE_INVALID_ARGUMENT: Word = 1;
pub const OS_RESPONSE_PERMISSION_DENIED: Word = 2;
pub const OS_RESPONSE_INVALID_DESCRIPTOR: Word = 3;
pub const OS_RESPONSE_ILLEGAL_OPERATION: Word = 4;
pub const OS_RESPONSE_FATAL: Word = 5;

const OS_RESPONSE_PONG_MAGIC: Word = 0x504f_4e47;
pub const PROCESS_SLOT_NOTIFICATION: Word = 21;

pub const FRAMEBUFFER_INFORMATION_REGION: Word = 0;
pub const FRAMEBUFFER_INFORMATION_GEOMETRY: Word = 1;
pub const FRAMEBUFFER_INFORMATION_FORMAT: Word = 2;
pub const FRAMEBUFFER_INFORMATION_COLOR_AND_ID: Word = 3;

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

        #[cfg(not(target_arch = "x86_64"))]
        compile_error!("nanami_entry! is currently supported only on x86_64");
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

pub fn request_mmio(
    physical_address: Word,
    size_bytes: Word,
) -> Result<(Word, Word), RequestError> {
    if physical_address == 0 || size_bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, paddr, vaddr) = call_os_port(
        OS_REQUEST_MMIO_REQUEST,
        physical_address,
        size_bytes,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((paddr, vaddr))
}

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

pub fn service_info_by_ordinal(ordinal: Word) -> Result<(Word, Word), RequestError> {
    let (status, owner_pid, service_kind) =
        call_os_port(OS_REQUEST_SERVICE_LIST, ordinal, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((owner_pid, service_kind))
}

pub fn request_process_spawn(image_name: &str) -> Result<Word, RequestError> {
    let (name0, name1, name2) = pack_name_24(image_name)?;
    let (status, child_pid, _) =
        call_os_port(OS_REQUEST_PROCESS_SPAWN, name0, name1, name2, 0, 4)?;
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

pub fn request_mapping_release(base_vaddr: Word, size_bytes: Word) -> Result<(), RequestError> {
    if base_vaddr == 0 || size_bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let (status, _, _) = call_os_port(
        OS_REQUEST_MAPPING_RELEASE,
        base_vaddr,
        size_bytes,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
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
