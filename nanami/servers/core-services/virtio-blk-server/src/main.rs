#![no_std]
#![no_main]

use core::ptr;
use core::sync::atomic::{fence, Ordering};

use libnanami::ipc::{ServiceEvent, ServiceRequest};
use libnanami::{self, RequestError, Word};

#[path = "app/arch.rs"]
mod arch;
#[cfg(target_arch = "x86_64")]
#[path = "app/pci.rs"]
mod pci;
#[path = "app/util.rs"]
mod util;

use arch::*;
#[cfg(target_arch = "x86_64")]
use pci::{
    configure_pci_command_for_intx, disable_pci_msi_capabilities, resolve_irq_number,
    scan_virtio_blk,
};
use util::{fail_device, log_request_error};

#[cfg(target_arch = "x86_64")]
const SLOT_IO_PCI_CFG: Word = 16;
#[cfg(target_arch = "x86_64")]
const SLOT_IO_VIRTIO: Word = 17;
#[cfg(target_arch = "x86_64")]
const SLOT_NOTIFICATION: Word = 18;
#[cfg(target_arch = "x86_64")]
const SLOT_INTERRUPT: Word = 19;
const SLOT_SERVICE_PORT: Word = 20;

#[cfg(target_arch = "x86_64")]
const VIRTIO_VENDOR_ID: u16 = 0x1af4;
#[cfg(target_arch = "x86_64")]
const VIRTIO_BLK_DEVICE_ID_LEGACY: u16 = 0x1001;
#[cfg(target_arch = "x86_64")]
const VIRTIO_BLK_DEVICE_ID_MODERN: u16 = 0x1042;

const VIRTIO_STATUS_ACKNOWLEDGE: u8 = 1;
const VIRTIO_STATUS_DRIVER: u8 = 2;
const VIRTIO_STATUS_DRIVER_OK: u8 = 4;
#[cfg(target_arch = "aarch64")]
const VIRTIO_STATUS_FEATURES_OK: u8 = 8;
const VIRTIO_STATUS_FAILED: u8 = 128;

const QUEUE_INDEX: u16 = 0;
const QUEUE_MEM_BYTES: usize = 16384;
const VIRTIO_QUEUE_ALIGN: usize = 4096;
const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;

const VIRTIO_BLK_T_IN: u32 = 0;
const VIRTIO_BLK_T_OUT: u32 = 1;
const VIRTIO_BLK_T_FLUSH: u32 = 4;
const VIRTIO_BLK_F_FLUSH: Word = 1 << 9;
const VIRTIO_BLK_STATUS_OK: u8 = 0;
const VIRTIO_SECTOR_BYTES: usize = 512;
const BLOCK_SIZE: usize = nanami_services::block::BLOCK_DEVICE_BLOCK_SIZE as usize;
const MAX_TRANSFER_BYTES: usize = nanami_services::block::BLOCK_DEVICE_DEFAULT_SHM_BYTES as usize;

const DMA_QUEUE_OFFSET: usize = 0;
const DMA_HEADER_OFFSET: usize = DMA_QUEUE_OFFSET + QUEUE_MEM_BYTES;
const DMA_DATA_OFFSET: usize = DMA_HEADER_OFFSET + core::mem::size_of::<VirtioBlkReqHeader>();
const DMA_STATUS_OFFSET: usize = DMA_DATA_OFFSET + MAX_TRANSFER_BYTES;
const DMA_TOTAL_BYTES: usize = 0x9000;

#[cfg(target_arch = "x86_64")]
#[derive(Clone, Copy)]
struct VirtioPciDevice {
    bus: u8,
    dev: u8,
    func: u8,
    vendor_id: u16,
    device_id: u16,
    io_base: u16,
    irq_line: u8,
    irq_pin: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VirtqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VirtqUsedElem {
    id: u32,
    len: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioBlkReqHeader {
    request_type: u32,
    reserved: u32,
    sector: u64,
}

#[derive(Clone, Copy)]
struct ClientSession {
    active: bool,
    pid: Word,
    shm_local: Word,
    shm_size: Word,
}

impl ClientSession {
    const EMPTY: Self = Self {
        active: false,
        pid: 0,
        shm_local: 0,
        shm_size: 0,
    };
}

struct BlockRuntime {
    io_desc: Word,
    io_base: Word,
    notification_desc: Word,
    interrupt_desc: Word,
    irq_registered: bool,
    irq_wait_enabled: bool,
    queue_size: u16,
    used_idx: u16,
    queue_vaddr: usize,
    header_vaddr: usize,
    data_vaddr: usize,
    status_vaddr: usize,
    disk_capacity_sectors: u64,
    partition_start_sector: u64,
    partition_sectors: u64,
    flush_supported: bool,
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[virtio-blk] panic\n");
    loop {}
}

fn vio_read(io_desc: Word, io_base: Word, offset: Word, width: Word) -> Result<Word, RequestError> {
    arch::read(io_desc, io_base, offset, width)
}

fn vio_write(
    io_desc: Word,
    io_base: Word,
    offset: Word,
    width: Word,
    value: Word,
) -> Result<(), RequestError> {
    arch::write(io_desc, io_base, offset, width, value)
}

fn read_device_status(io_desc: Word, io_base: Word) -> Result<u8, RequestError> {
    Ok(vio_read(io_desc, io_base, REG_DEVICE_STATUS, 1)? as u8)
}

fn write_device_status(io_desc: Word, io_base: Word, status: u8) -> Result<(), RequestError> {
    vio_write(io_desc, io_base, REG_DEVICE_STATUS, 1, status as Word)
}

fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

fn used_offset(queue_size: u16) -> usize {
    let q = queue_size as usize;
    let avail_bytes = 2 + 2 + q * 2 + 2;
    align_up(
        q * core::mem::size_of::<VirtqDesc>() + avail_bytes,
        VIRTIO_QUEUE_ALIGN,
    )
}

fn total_queue_bytes(queue_size: u16) -> usize {
    let q = queue_size as usize;
    used_offset(queue_size) + (2 + 2 + q * core::mem::size_of::<VirtqUsedElem>() + 2)
}

unsafe fn desc_ptr(base: *mut u8) -> *mut VirtqDesc {
    base as *mut VirtqDesc
}

unsafe fn avail_flags_ptr(base: *mut u8, queue_size: u16) -> *mut u16 {
    let _ = queue_size;
    base.add(core::mem::size_of::<VirtqDesc>() * queue_size as usize) as *mut u16
}

unsafe fn avail_idx_ptr(base: *mut u8, queue_size: u16) -> *mut u16 {
    let _ = queue_size;
    base.add(core::mem::size_of::<VirtqDesc>() * queue_size as usize + 2) as *mut u16
}

unsafe fn avail_ring_ptr(base: *mut u8, queue_size: u16) -> *mut u16 {
    base.add(core::mem::size_of::<VirtqDesc>() * queue_size as usize + 4) as *mut u16
}

unsafe fn used_flags_ptr(base: *mut u8, queue_size: u16) -> *mut u16 {
    base.add(used_offset(queue_size)) as *mut u16
}

unsafe fn used_idx_ptr(base: *mut u8, queue_size: u16) -> *mut u16 {
    base.add(used_offset(queue_size) + 2) as *mut u16
}

unsafe fn used_ring_ptr(base: *mut u8, queue_size: u16) -> *mut VirtqUsedElem {
    base.add(used_offset(queue_size) + 4) as *mut VirtqUsedElem
}

fn notify_queue(io_desc: Word, io_base: Word) -> Result<(), RequestError> {
    vio_write(io_desc, io_base, REG_QUEUE_NOTIFY, 2, QUEUE_INDEX as Word)
}

fn read_capacity_sectors(io_desc: Word, io_base: Word) -> Result<u64, RequestError> {
    let lo = vio_read(io_desc, io_base, REG_CONFIG_BASE, 4)? as u64;
    let hi = vio_read(io_desc, io_base, REG_CONFIG_BASE + 4, 4)? as u64;
    Ok(lo | (hi << 32))
}

fn submit_blk_request_dma(
    runtime: &mut BlockRuntime,
    dma_paddr_base: usize,
    request_type: u32,
    sector: u64,
    bytes: usize,
) -> Result<(), RequestError> {
    let sector_count = (bytes / VIRTIO_SECTOR_BYTES) as u64;
    if (bytes == 0 && request_type != VIRTIO_BLK_T_FLUSH)
        || bytes > MAX_TRANSFER_BYTES
        || bytes % VIRTIO_SECTOR_BYTES != 0
        || sector
            .checked_add(sector_count)
            .map_or(true, |end| end > runtime.disk_capacity_sectors)
    {
        return Err(RequestError::InvalidArgument);
    }

    let header_paddr = dma_paddr_base + DMA_HEADER_OFFSET;
    let data_paddr = dma_paddr_base + DMA_DATA_OFFSET;
    let status_paddr = dma_paddr_base + DMA_STATUS_OFFSET;

    unsafe {
        ptr::write(
            runtime.header_vaddr as *mut VirtioBlkReqHeader,
            VirtioBlkReqHeader {
                request_type,
                reserved: 0,
                sector,
            },
        );
        ptr::write(runtime.status_vaddr as *mut u8, 0xff);

        let base = runtime.queue_vaddr as *mut u8;
        let desc = desc_ptr(base);
        (*desc.add(0)).addr = header_paddr as u64;
        (*desc.add(0)).len = core::mem::size_of::<VirtioBlkReqHeader>() as u32;
        (*desc.add(0)).flags = DESC_F_NEXT;
        (*desc.add(0)).next = if request_type == VIRTIO_BLK_T_FLUSH { 2 } else { 1 };

        (*desc.add(1)).addr = data_paddr as u64;
        (*desc.add(1)).len = bytes as u32;
        (*desc.add(1)).flags = DESC_F_NEXT
            | if request_type == VIRTIO_BLK_T_IN {
                DESC_F_WRITE
            } else {
                0
            };
        (*desc.add(1)).next = 2;

        (*desc.add(2)).addr = status_paddr as u64;
        (*desc.add(2)).len = 1;
        (*desc.add(2)).flags = DESC_F_WRITE;
        (*desc.add(2)).next = 0;

        let avail_idx = ptr::read_volatile(avail_idx_ptr(base, runtime.queue_size));
        *avail_ring_ptr(base, runtime.queue_size)
            .add((avail_idx as usize) % runtime.queue_size as usize) = 0;
        ptr::write_volatile(
            avail_idx_ptr(base, runtime.queue_size),
            avail_idx.wrapping_add(1),
        );
    }
    fence(Ordering::SeqCst);
    notify_queue(runtime.io_desc, runtime.io_base)?;

    loop {
        unsafe {
            let base = runtime.queue_vaddr as *mut u8;
            let used_idx = ptr::read_volatile(used_idx_ptr(base, runtime.queue_size));
            if used_idx != runtime.used_idx {
                fence(Ordering::Acquire);
                let used = ptr::read_volatile(
                    used_ring_ptr(base, runtime.queue_size)
                        .add((runtime.used_idx as usize) % runtime.queue_size as usize),
                );
                runtime.used_idx = runtime.used_idx.wrapping_add(1);
                if used.id != 0 {
                    libnanami::print!("[virtio-blk] unexpected used id=");
                    libnanami::print!("{}", used.id as usize);
                    libnanami::print!(" len=");
                    libnanami::print!("{}", used.len as usize);
                    libnanami::print!("\n");
                    return Err(RequestError::Unsupported);
                }
                let status = ptr::read_volatile(runtime.status_vaddr as *const u8);
                return if status == VIRTIO_BLK_STATUS_OK {
                    Ok(())
                } else {
                    libnanami::print!("[virtio-blk] request failed status=");
                    libnanami::print!("{:#x}", status);
                    libnanami::print!(" type=");
                    libnanami::print!("{}", request_type as usize);
                    libnanami::print!(" sector=");
                    libnanami::print!("{}", sector as usize);
                    libnanami::print!(" bytes=");
                    libnanami::print!("{}", bytes);
                    libnanami::print!(" capacity=");
                    libnanami::print!("{}", runtime.disk_capacity_sectors as usize);
                    libnanami::print!("\n");
                    Err(RequestError::Unsupported)
                };
            }
        }

        if runtime.irq_wait_enabled {
            let wait_result = libnanami::ipc::notification_wait(runtime.notification_desc)
                .and_then(|_| {
                    arch::acknowledge_interrupt(runtime.io_desc, runtime.io_base)?;
                    libnanami::ipc::interrupt_ack(runtime.interrupt_desc)
                });
            if let Err(error) = wait_result {
                log_request_error("[virtio-blk] irq wait failed, fallback polling: ", error);
                runtime.irq_wait_enabled = false;
            }
        } else {
            libnanami::yield_now();
        }
    }
}

fn submit_partition_request_dma(
    runtime: &mut BlockRuntime,
    dma_paddr_base: usize,
    request_type: u32,
    block_index: usize,
    bytes: usize,
) -> Result<(), RequestError> {
    let sector_count = (bytes / VIRTIO_SECTOR_BYTES) as u64;
    let partition_sector = block_index
        .checked_mul(BLOCK_SIZE / VIRTIO_SECTOR_BYTES)
        .ok_or(RequestError::InvalidArgument)? as u64;
    if partition_sector
        .checked_add(sector_count)
        .map_or(true, |end| end > runtime.partition_sectors)
    {
        return Err(RequestError::InvalidArgument);
    }
    let sector = runtime
        .partition_start_sector
        .checked_add(partition_sector)
        .ok_or(RequestError::InvalidArgument)?;
    submit_blk_request_dma(runtime, dma_paddr_base, request_type, sector, bytes)
}

#[cfg(target_arch = "x86_64")]
fn select_nanami_root(
    runtime: &mut BlockRuntime,
    dma_paddr_base: usize,
) -> Result<(), RequestError> {
    submit_blk_request_dma(
        runtime,
        dma_paddr_base,
        VIRTIO_BLK_T_IN,
        1,
        VIRTIO_SECTOR_BYTES,
    )?;
    let sector = unsafe {
        core::slice::from_raw_parts(runtime.data_vaddr as *const u8, VIRTIO_SECTOR_BYTES)
    };
    let header = nanami_gpt::parse_primary_header(sector, runtime.disk_capacity_sectors)
        .map_err(|_| RequestError::Protocol)?;
    let entry_bytes = header
        .partition_entry_bytes()
        .map_err(|_| RequestError::Unsupported)?;
    let transfer_bytes = entry_bytes.div_ceil(VIRTIO_SECTOR_BYTES) * VIRTIO_SECTOR_BYTES;
    submit_blk_request_dma(
        runtime,
        dma_paddr_base,
        VIRTIO_BLK_T_IN,
        header.partition_entry_lba,
        transfer_bytes,
    )?;
    let entries =
        unsafe { core::slice::from_raw_parts(runtime.data_vaddr as *const u8, transfer_bytes) };
    let partition =
        nanami_gpt::find_nanami_root(header, entries).map_err(|_| RequestError::Protocol)?;
    runtime.partition_start_sector = partition.first_lba;
    runtime.partition_sectors = partition.sector_count;
    libnanami::println!(
        "[virtio-blk] Nanami root LBA={} sectors={}",
        partition.first_lba,
        partition.sector_count
    );
    Ok(())
}

fn handle_control(
    request: ServiceRequest,
    session: &mut ClientSession,
    runtime: &BlockRuntime,
) -> (Word, Word, Word) {
    match request.arg0 {
        nanami_services::block::BLOCK_DEVICE_CONTROL_ATTACH_SHARED_MEMORY => {
            let size = if request.arg1 == 0 {
                nanami_services::block::BLOCK_DEVICE_DEFAULT_SHM_BYTES
            } else {
                request.arg1
            };
            match libnanami::request_shared_memory(request.identifier, size) {
                Ok((local, peer)) => {
                    *session = ClientSession {
                        active: true,
                        pid: request.identifier,
                        shm_local: local,
                        shm_size: size,
                    };
                    libnanami::print!("[virtio-blk] shm attached pid=");
                    libnanami::print!("{}", request.identifier as usize);
                    libnanami::print!(" local=");
                    libnanami::print!("{:#x}", local);
                    libnanami::print!(" peer=");
                    libnanami::print!("{:#x}", peer);
                    libnanami::print!("\n");
                    (libnanami::OS_RESPONSE_OK, peer, size)
                }
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_services::block::BLOCK_DEVICE_CONTROL_GET_INFO => (
            libnanami::OS_RESPONSE_OK,
            BLOCK_SIZE as Word,
            (runtime.partition_sectors / (BLOCK_SIZE / VIRTIO_SECTOR_BYTES) as u64) as Word,
        ),
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_read(
    request: ServiceRequest,
    session: &ClientSession,
    runtime: &mut BlockRuntime,
    dma_paddr_base: usize,
) -> (Word, Word, Word) {
    if !session.active || session.pid != request.identifier {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let block = request.arg0 as usize;
    let count = request.arg1 as usize;
    let offset = request.arg2 as usize;
    let bytes = match count.checked_mul(BLOCK_SIZE) {
        Some(v) => v,
        None => return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    };
    if count == 0
        || offset
            .checked_add(bytes)
            .map_or(true, |end| end > session.shm_size as usize)
        || bytes > MAX_TRANSFER_BYTES
    {
        libnanami::print!("[virtio-blk] invalid read block=");
        libnanami::print!("{}", block);
        libnanami::print!(" count=");
        libnanami::print!("{}", count);
        libnanami::print!(" offset=");
        libnanami::print!("{}", offset);
        libnanami::print!(" shm=");
        libnanami::print!("{}", session.shm_size as usize);
        libnanami::print!("\n");
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    match submit_partition_request_dma(runtime, dma_paddr_base, VIRTIO_BLK_T_IN, block, bytes) {
        Ok(()) => unsafe {
            ptr::copy_nonoverlapping(
                runtime.data_vaddr as *const u8,
                (session.shm_local as usize + offset) as *mut u8,
                bytes,
            );
            (libnanami::OS_RESPONSE_OK, bytes as Word, 0)
        },
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

fn handle_write(
    request: ServiceRequest,
    session: &ClientSession,
    runtime: &mut BlockRuntime,
    dma_paddr_base: usize,
) -> (Word, Word, Word) {
    if !session.active || session.pid != request.identifier {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let block = request.arg0 as usize;
    let count = request.arg1 as usize;
    let offset = request.arg2 as usize;
    let bytes = match count.checked_mul(BLOCK_SIZE) {
        Some(v) => v,
        None => return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    };
    if count == 0
        || offset
            .checked_add(bytes)
            .map_or(true, |end| end > session.shm_size as usize)
        || bytes > MAX_TRANSFER_BYTES
    {
        libnanami::print!("[virtio-blk] invalid write block=");
        libnanami::print!("{}", block);
        libnanami::print!(" count=");
        libnanami::print!("{}", count);
        libnanami::print!(" offset=");
        libnanami::print!("{}", offset);
        libnanami::print!(" shm=");
        libnanami::print!("{}", session.shm_size as usize);
        libnanami::print!("\n");
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    unsafe {
        ptr::copy_nonoverlapping(
            (session.shm_local as usize + offset) as *const u8,
            runtime.data_vaddr as *mut u8,
            bytes,
        );
    }
    match submit_partition_request_dma(runtime, dma_paddr_base, VIRTIO_BLK_T_OUT, block, bytes) {
        Ok(()) => (libnanami::OS_RESPONSE_OK, bytes as Word, 0),
        Err(e) => (map_request_error_to_status(e), 0, 0),
    }
}

fn handle_request(
    request: ServiceRequest,
    session: &mut ClientSession,
    runtime: &mut BlockRuntime,
    dma_paddr_base: usize,
) -> (Word, Word, Word) {
    match request.code {
        nanami_services::block::BLOCK_DEVICE_REQUEST_CONTROL => {
            handle_control(request, session, runtime)
        }
        nanami_services::block::BLOCK_DEVICE_REQUEST_READ => {
            handle_read(request, session, runtime, dma_paddr_base)
        }
        nanami_services::block::BLOCK_DEVICE_REQUEST_WRITE => {
            handle_write(request, session, runtime, dma_paddr_base)
        }
        nanami_services::block::BLOCK_DEVICE_REQUEST_FLUSH => {
            if !session.active || session.pid != request.identifier {
                return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            // Neither FLUSH nor CONFIG_WCE negotiated means writethrough in
            // the virtio specification. Otherwise await an actual FLUSH request.
            let result = if runtime.flush_supported {
                submit_blk_request_dma(runtime, dma_paddr_base, VIRTIO_BLK_T_FLUSH, 0, 0)
            } else {
                Ok(())
            };
            match result {
                Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
                Err(error) => (map_request_error_to_status(error), 0, 0),
            }
        }
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn map_request_error_to_status(error: RequestError) -> Word {
    match error {
        RequestError::Status(status) => status,
        RequestError::InvalidArgument => libnanami::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Transport | RequestError::Protocol => libnanami::OS_RESPONSE_FATAL,
    }
}

fn log_device_error(
    prefix: &str,
    err: RequestError,
    io_desc: Word,
    io_base: Word,
) -> libnanami::NanamiError {
    log_request_error(prefix, err);
    fail_device(io_desc, io_base);
    err.into()
}

#[cfg(target_arch = "x86_64")]
fn log_device_failure(msg: &str, io_desc: Word, io_base: Word) -> libnanami::NanamiError {
    libnanami::print!(msg);
    fail_device(io_desc, io_base);
    libnanami::NanamiError::UNKNOWN
}

struct PreparedTransport {
    descriptor: Word,
    base: Word,
    notification: Word,
    interrupt: Word,
    irq_registered: bool,
    irq_wait_enabled: bool,
}

#[cfg(target_arch = "x86_64")]
fn prepare_transport() -> Result<PreparedTransport, libnanami::NanamiError> {
    let pci_io_desc = libnanami::ipc::process_slot_descriptor(SLOT_IO_PCI_CFG);
    let mut dev_io_desc = libnanami::ipc::process_slot_descriptor(SLOT_IO_VIRTIO);
    let notification = libnanami::ipc::process_slot_descriptor(SLOT_NOTIFICATION);
    let interrupt = libnanami::ipc::process_slot_descriptor(SLOT_INTERRUPT);

    let mut full_io_granted = false;
    match libnanami::request_io_port(0x0000, 0xffff, SLOT_IO_PCI_CFG) {
        Ok(()) => {
            full_io_granted = true;
            dev_io_desc = pci_io_desc;
            libnanami::print!("[virtio-blk] full io range granted\n");
        }
        Err(_) => match libnanami::request_io_port(0x0cf8, 0x0cff, SLOT_IO_PCI_CFG) {
            Ok(()) => libnanami::print!("[virtio-blk] pci cfg io ports granted\n"),
            Err(error) => {
                return Err(log_device_error(
                    "[virtio-blk] failed to request PCI cfg io ports: ",
                    error,
                    dev_io_desc,
                    0,
                ));
            }
        },
    }

    let found = scan_virtio_blk(pci_io_desc).map_err(|_| {
        log_device_failure(
            "[virtio-blk] virtio-blk pci device not found\n",
            dev_io_desc,
            0,
        )
    })?;
    libnanami::println!(
        "[virtio-blk] found pci bus={} dev={} func={} vid={:#x} did={:#x} io={:#x} irq={}",
        found.bus,
        found.dev,
        found.func,
        found.vendor_id,
        found.device_id,
        found.io_base,
        found.irq_line
    );

    configure_pci_command_for_intx(pci_io_desc, found).map_err(|error| {
        log_device_error(
            "[virtio-blk] pci command configure failed: ",
            error,
            dev_io_desc,
            found.io_base as Word,
        )
    })?;
    disable_pci_msi_capabilities(pci_io_desc, found).map_err(|error| {
        log_device_error(
            "[virtio-blk] pci msi disable failed: ",
            error,
            dev_io_desc,
            found.io_base as Word,
        )
    })?;

    let io_base = found.io_base as Word;
    if let Err(error) = libnanami::request_io_port(io_base, io_base + 0xff, SLOT_IO_VIRTIO) {
        if full_io_granted {
            libnanami::print!("[virtio-blk] virtio io range already covered by full range\n");
        } else {
            return Err(log_device_error(
                "[virtio-blk] failed to request virtio io range: ",
                error,
                dev_io_desc,
                io_base,
            ));
        }
    } else {
        libnanami::print!("[virtio-blk] virtio io range granted\n");
    }

    let mut irq_registered = false;
    let mut irq_wait_enabled = false;
    if let Ok(Some(irq_number)) = resolve_irq_number(pci_io_desc, found) {
        if libnanami::request_irq(irq_number, SLOT_NOTIFICATION, SLOT_INTERRUPT).is_ok() {
            irq_registered = true;
            libnanami::ipc::bind_current_thread_notification(notification).map_err(|error| {
                log_device_error(
                    "[virtio-blk] notification bind failed: ",
                    error,
                    dev_io_desc,
                    io_base,
                )
            })?;
            match libnanami::ipc::interrupt_ack(interrupt) {
                Ok(()) => {
                    irq_wait_enabled = true;
                    libnanami::print!("[virtio-blk] irq granted\n");
                }
                Err(error) => {
                    log_request_error("[virtio-blk] irq arm failed, fallback polling: ", error);
                }
            }
        }
    }

    Ok(PreparedTransport {
        descriptor: dev_io_desc,
        base: io_base,
        notification,
        interrupt,
        irq_registered,
        irq_wait_enabled,
    })
}

#[cfg(target_arch = "aarch64")]
fn prepare_transport() -> Result<PreparedTransport, libnanami::NanamiError> {
    let (_, mapped_base) =
        libnanami::request_mmio(MMIO_PHYSICAL_BASE, MMIO_REGION_BYTES).map_err(|error| {
            log_request_error("[virtio-blk] virtio-mmio mapping failed: ", error);
            libnanami::NanamiError::UNKNOWN
        })?;

    let mut offset = 0;
    while offset < MMIO_REGION_BYTES {
        let base = mapped_base + offset;
        let magic = arch::read(0, base, REG_MAGIC_VALUE, 4).unwrap_or(0) as u32;
        let version = arch::read(0, base, REG_VERSION, 4).unwrap_or(0) as u32;
        let device_id = arch::read(0, base, REG_DEVICE_ID, 4).unwrap_or(0) as u32;
        if magic == VIRTIO_MAGIC_VALUE
            && version == VIRTIO_MMIO_VERSION
            && device_id == VIRTIO_BLOCK_DEVICE_ID
        {
            let vendor_id = arch::read(0, base, REG_VENDOR_ID, 4).unwrap_or(0);
            libnanami::println!(
                "[virtio-blk] found virtio-mmio paddr={:#x} vaddr={:#x} vendor={:#x}",
                MMIO_PHYSICAL_BASE + offset,
                base,
                vendor_id
            );
            return Ok(PreparedTransport {
                descriptor: 0,
                base,
                notification: 0,
                interrupt: 0,
                irq_registered: false,
                irq_wait_enabled: false,
            });
        }
        offset += MMIO_TRANSPORT_STRIDE;
    }

    libnanami::print!("[virtio-blk] virtio-mmio block device not found\n");
    Err(libnanami::NanamiError::UNKNOWN)
}

fn initialize_root() -> Result<(Word, BlockRuntime), libnanami::NanamiError> {
    let transport = prepare_transport()?;

    let (dma_paddr_base, mut runtime) = match init_virtio_blk_with_dma_base(
        transport.descriptor,
        transport.base,
        transport.notification,
        transport.interrupt,
        transport.irq_registered,
        transport.irq_wait_enabled,
    ) {
        Ok(v) => v,
        Err(e) => {
            return Err(log_device_error(
                "[virtio-blk] queue init failed: ",
                e,
                transport.descriptor,
                transport.base,
            ));
        }
    };

    #[cfg(target_arch = "x86_64")]
    select_nanami_root(&mut runtime, dma_paddr_base).map_err(|e| {
        log_device_error(
            "[virtio-blk] Nanami GPT root selection failed: ",
            e,
            runtime.io_desc,
            runtime.io_base,
        )
    })?;

    Ok((dma_paddr_base, runtime))
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::print!("[virtio-blk] bootstrap start\n");
    let service_port_desc = libnanami::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let (dma_paddr_base, mut runtime) = match initialize_root() {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = nanami_services::device::select_storage_root(2);
            return Err(error);
        }
    };
    if !nanami_services::device::select_storage_root(1)? {
        return Err(RequestError::Unsupported.into());
    }

    nanami_services::registry::register_block_device().map_err(|e| {
        log_device_error(
            "[virtio-blk] service registration failed: ",
            e,
            runtime.io_desc,
            runtime.io_base,
        )
    })?;
    libnanami::print!("[virtio-blk] service registered: block-device\n");

    let mut session = ClientSession::EMPTY;
    let mut pending = (libnanami::OS_RESPONSE_OK, 0, 0);
    let mut has_reply = false;
    loop {
        let event = if has_reply {
            has_reply = false;
            match libnanami::ipc::service_reply_receive_event(
                service_port_desc,
                pending.0,
                pending.1,
                pending.2,
            ) {
                Ok(event) => event,
                Err(e) => {
                    return Err(log_device_error(
                        "[virtio-blk] reply_receive failed: ",
                        e,
                        runtime.io_desc,
                        runtime.io_base,
                    ));
                }
            }
        } else {
            match libnanami::ipc::service_receive_event(service_port_desc) {
                Ok(event) => event,
                Err(e) => {
                    return Err(log_device_error(
                        "[virtio-blk] receive failed: ",
                        e,
                        runtime.io_desc,
                        runtime.io_base,
                    ));
                }
            }
        };

        match event {
            ServiceEvent::Request(request) => {
                pending = handle_request(request, &mut session, &mut runtime, dma_paddr_base);
                has_reply = true;
            }
            ServiceEvent::Notification { .. } => {
                if runtime.irq_registered {
                    let _ = arch::acknowledge_interrupt(runtime.io_desc, runtime.io_base);
                    if libnanami::ipc::interrupt_ack(runtime.interrupt_desc).is_err() {
                        runtime.irq_wait_enabled = false;
                    }
                }
            }
            ServiceEvent::Fault {
                identifier, reason, ..
            } => {
                libnanami::print!("[virtio-blk] fault id=");
                libnanami::print!("{}", identifier as usize);
                libnanami::print!(" reason=");
                libnanami::print!("{:#x}", reason);
                libnanami::print!("\n");
                has_reply = false;
            }
        }
    }
}

#[cfg(target_arch = "aarch64")]
fn write_mmio_address(
    descriptor: Word,
    base: Word,
    low_register: Word,
    address: usize,
) -> Result<(), RequestError> {
    vio_write(descriptor, base, low_register, 4, address as Word)?;
    vio_write(
        descriptor,
        base,
        low_register + 4,
        4,
        ((address as u64) >> 32) as Word,
    )
}

fn init_virtio_blk_with_dma_base(
    io_desc: Word,
    io_base: Word,
    notification_desc: Word,
    interrupt_desc: Word,
    irq_registered: bool,
    irq_wait_enabled: bool,
) -> Result<(usize, BlockRuntime), RequestError> {
    vio_write(io_desc, io_base, REG_DEVICE_STATUS, 1, 0)?;
    let base_status = VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER;
    vio_write(io_desc, io_base, REG_DEVICE_STATUS, 1, base_status as Word)?;

    #[cfg(target_arch = "x86_64")]
    let flush_supported = {
        let features = vio_read(io_desc, io_base, REG_DEVICE_FEATURES, 4)?;
        vio_write(io_desc, io_base, REG_DRIVER_FEATURES, 4, features & VIRTIO_BLK_F_FLUSH)?;
        features & VIRTIO_BLK_F_FLUSH != 0
    };
    #[cfg(target_arch = "aarch64")]
    let flush_supported = {
        // Virtio MMIO v2 requires negotiation of VIRTIO_F_VERSION_1 (bit 32).
        vio_write(io_desc, io_base, REG_DEVICE_FEATURES_SELECT, 4, 0)?;
        let low_features = vio_read(io_desc, io_base, REG_DEVICE_FEATURES, 4)?;
        vio_write(io_desc, io_base, REG_DRIVER_FEATURES_SELECT, 4, 0)?;
        vio_write(io_desc, io_base, REG_DRIVER_FEATURES, 4, low_features & VIRTIO_BLK_F_FLUSH)?;

        vio_write(io_desc, io_base, REG_DEVICE_FEATURES_SELECT, 4, 1)?;
        let high_features = vio_read(io_desc, io_base, REG_DEVICE_FEATURES, 4)?;
        if (high_features & 1) == 0 {
            return Err(RequestError::Unsupported);
        }
        vio_write(io_desc, io_base, REG_DRIVER_FEATURES_SELECT, 4, 1)?;
        vio_write(io_desc, io_base, REG_DRIVER_FEATURES, 4, 1)?;

        let feature_status = base_status | VIRTIO_STATUS_FEATURES_OK;
        write_device_status(io_desc, io_base, feature_status)?;
        if (read_device_status(io_desc, io_base)? & VIRTIO_STATUS_FEATURES_OK) == 0 {
            return Err(RequestError::Unsupported);
        }
        low_features & VIRTIO_BLK_F_FLUSH != 0
    };
    let capacity_sectors = read_capacity_sectors(io_desc, io_base)?;

    vio_write(io_desc, io_base, REG_QUEUE_SELECT, 2, QUEUE_INDEX as Word)?;
    let queue_size = vio_read(io_desc, io_base, REG_QUEUE_SIZE_MAX, 2)? as u16;
    if capacity_sectors < 2 || queue_size < 3 || total_queue_bytes(queue_size) > QUEUE_MEM_BYTES {
        return Err(RequestError::Unsupported);
    }

    let (dma_paddr, dma_vaddr) = libnanami::request_dma(DMA_TOTAL_BYTES)?;
    let dma_paddr = dma_paddr as usize;
    let dma_vaddr = dma_vaddr as usize;
    unsafe {
        ptr::write_bytes(dma_vaddr as *mut u8, 0, DMA_TOTAL_BYTES);
    }

    #[cfg(target_arch = "x86_64")]
    vio_write(
        io_desc,
        io_base,
        REG_QUEUE_ADDRESS,
        4,
        ((dma_paddr + DMA_QUEUE_OFFSET) >> 12) as Word,
    )?;
    #[cfg(target_arch = "aarch64")]
    {
        let desc_address = dma_paddr + DMA_QUEUE_OFFSET;
        let driver_address = desc_address + queue_size as usize * core::mem::size_of::<VirtqDesc>();
        let device_address = desc_address + used_offset(queue_size);
        if vio_read(io_desc, io_base, REG_QUEUE_READY, 4)? != 0 {
            return Err(RequestError::Unsupported);
        }
        vio_write(io_desc, io_base, REG_QUEUE_SIZE, 4, queue_size as Word)?;
        write_mmio_address(io_desc, io_base, REG_QUEUE_DESC_LOW, desc_address)?;
        write_mmio_address(io_desc, io_base, REG_QUEUE_DRIVER_LOW, driver_address)?;
        write_mmio_address(io_desc, io_base, REG_QUEUE_DEVICE_LOW, device_address)?;
        vio_write(io_desc, io_base, REG_QUEUE_READY, 4, 1)?;
    }
    unsafe {
        *avail_flags_ptr((dma_vaddr + DMA_QUEUE_OFFSET) as *mut u8, queue_size) = 0;
        *used_flags_ptr((dma_vaddr + DMA_QUEUE_OFFSET) as *mut u8, queue_size) = 0;
    }

    #[cfg(target_arch = "x86_64")]
    let ready_status = base_status | VIRTIO_STATUS_DRIVER_OK;
    #[cfg(target_arch = "aarch64")]
    let ready_status = base_status | VIRTIO_STATUS_FEATURES_OK | VIRTIO_STATUS_DRIVER_OK;
    write_device_status(io_desc, io_base, ready_status)?;
    libnanami::print!("[virtio-blk] queue ready qsize=");
    libnanami::print!("{}", queue_size as usize);
    libnanami::print!(" blocks=");
    libnanami::print!(
        "{}",
        (capacity_sectors / (BLOCK_SIZE / VIRTIO_SECTOR_BYTES) as u64) as usize
    );
    libnanami::print!(" sectors=");
    libnanami::print!("{}", capacity_sectors as usize);
    libnanami::print!(" dma_paddr=");
    libnanami::print!("{:#x}", dma_paddr);
    libnanami::print!(" dma_vaddr=");
    libnanami::print!("{:#x}", dma_vaddr);
    libnanami::print!("\n");

    Ok((
        dma_paddr,
        BlockRuntime {
            io_desc,
            io_base,
            notification_desc,
            interrupt_desc,
            irq_registered,
            irq_wait_enabled,
            queue_size,
            used_idx: 0,
            queue_vaddr: dma_vaddr + DMA_QUEUE_OFFSET,
            header_vaddr: dma_vaddr + DMA_HEADER_OFFSET,
            data_vaddr: dma_vaddr + DMA_DATA_OFFSET,
            status_vaddr: dma_vaddr + DMA_STATUS_OFFSET,
            disk_capacity_sectors: capacity_sectors,
            partition_start_sector: 0,
            partition_sectors: capacity_sectors,
            flush_supported,
        },
    ))
}

libnanami::nanami_entry!(nanami_main);
