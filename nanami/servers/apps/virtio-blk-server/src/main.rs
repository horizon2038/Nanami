#![no_std]
#![no_main]

use core::ptr;
use core::sync::atomic::{fence, Ordering};

use libnanami::ipc::{ServiceEvent, ServiceRequest};
use libnanami::{self, RequestError, Word};

#[path = "app/arch.rs"]
mod arch;
#[path = "app/pci.rs"]
mod pci;
#[path = "app/util.rs"]
mod util;

use pci::{
    configure_pci_command_for_intx, disable_pci_msi_capabilities, resolve_irq_number,
    scan_virtio_blk,
};
use util::{fail_device, log_request_error};

const SLOT_IO_PCI_CFG: Word = 16;
const SLOT_IO_VIRTIO: Word = 17;
const SLOT_NOTIFICATION: Word = 18;
const SLOT_INTERRUPT: Word = 19;
const SLOT_SERVICE_PORT: Word = 20;

const VIRTIO_VENDOR_ID: u16 = 0x1af4;
const VIRTIO_BLK_DEVICE_ID_LEGACY: u16 = 0x1001;
const VIRTIO_BLK_DEVICE_ID_MODERN: u16 = 0x1042;

const VIRTIO_PCI_DEVICE_FEATURES: Word = 0x00;
const VIRTIO_PCI_GUEST_FEATURES: Word = 0x04;
const VIRTIO_PCI_QUEUE_ADDRESS: Word = 0x08;
const VIRTIO_PCI_QUEUE_SIZE: Word = 0x0c;
const VIRTIO_PCI_QUEUE_SELECT: Word = 0x0e;
const VIRTIO_PCI_QUEUE_NOTIFY: Word = 0x10;
const VIRTIO_PCI_DEVICE_STATUS: Word = 0x12;
const VIRTIO_PCI_ISR_STATUS: Word = 0x13;
const VIRTIO_PCI_LEGACY_DEVICE_CONFIG_BASE: Word = 0x14;

const VIRTIO_STATUS_ACKNOWLEDGE: u8 = 1;
const VIRTIO_STATUS_DRIVER: u8 = 2;
const VIRTIO_STATUS_DRIVER_OK: u8 = 4;
const VIRTIO_STATUS_FAILED: u8 = 128;

const QUEUE_INDEX: u16 = 0;
const QUEUE_MEM_BYTES: usize = 16384;
const VIRTIO_QUEUE_ALIGN: usize = 4096;
const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;

const VIRTIO_BLK_T_IN: u32 = 0;
const VIRTIO_BLK_T_OUT: u32 = 1;
const VIRTIO_BLK_STATUS_OK: u8 = 0;
const VIRTIO_SECTOR_BYTES: usize = 512;
const BLOCK_SIZE: usize = nanami_services::block::BLOCK_DEVICE_BLOCK_SIZE as usize;
const MAX_TRANSFER_BYTES: usize = nanami_services::block::BLOCK_DEVICE_DEFAULT_SHM_BYTES as usize;

const DMA_QUEUE_OFFSET: usize = 0;
const DMA_HEADER_OFFSET: usize = DMA_QUEUE_OFFSET + QUEUE_MEM_BYTES;
const DMA_DATA_OFFSET: usize = DMA_HEADER_OFFSET + core::mem::size_of::<VirtioBlkReqHeader>();
const DMA_STATUS_OFFSET: usize = DMA_DATA_OFFSET + MAX_TRANSFER_BYTES;
const DMA_TOTAL_BYTES: usize = 0x9000;

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
    queue_size: u16,
    used_idx: u16,
    queue_vaddr: usize,
    header_vaddr: usize,
    data_vaddr: usize,
    status_vaddr: usize,
    capacity_sectors: u64,
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[virtio-blk] panic\n");
    loop {}
}

fn vio_read(io_desc: Word, io_base: Word, offset: Word, width: Word) -> Result<Word, RequestError> {
    libnanami::io::io_read(io_desc, io_base + offset, width)
}

fn vio_write(
    io_desc: Word,
    io_base: Word,
    offset: Word,
    width: Word,
    value: Word,
) -> Result<(), RequestError> {
    libnanami::io::io_write(io_desc, io_base + offset, width, value)
}

fn read_device_status(io_desc: Word, io_base: Word) -> Result<u8, RequestError> {
    Ok(vio_read(io_desc, io_base, VIRTIO_PCI_DEVICE_STATUS, 1)? as u8)
}

fn write_device_status(io_desc: Word, io_base: Word, status: u8) -> Result<(), RequestError> {
    vio_write(
        io_desc,
        io_base,
        VIRTIO_PCI_DEVICE_STATUS,
        1,
        status as Word,
    )
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
    vio_write(
        io_desc,
        io_base,
        VIRTIO_PCI_QUEUE_NOTIFY,
        2,
        QUEUE_INDEX as Word,
    )
}

fn read_capacity_sectors(io_desc: Word, io_base: Word) -> Result<u64, RequestError> {
    let lo = vio_read(
        io_desc,
        io_base,
        VIRTIO_PCI_LEGACY_DEVICE_CONFIG_BASE,
        4,
    )? as u64;
    let hi = vio_read(
        io_desc,
        io_base,
        VIRTIO_PCI_LEGACY_DEVICE_CONFIG_BASE + 4,
        4,
    )? as u64;
    Ok(lo | (hi << 32))
}

fn submit_blk_request_dma(
    runtime: &mut BlockRuntime,
    dma_paddr_base: usize,
    request_type: u32,
    block_index: usize,
    bytes: usize,
) -> Result<(), RequestError> {
    let sector = block_index
        .checked_mul(BLOCK_SIZE / VIRTIO_SECTOR_BYTES)
        .ok_or(RequestError::InvalidArgument)? as u64;
    let sector_count = (bytes / VIRTIO_SECTOR_BYTES) as u64;
    if bytes == 0
        || bytes > MAX_TRANSFER_BYTES
        || bytes % VIRTIO_SECTOR_BYTES != 0
        || sector
            .checked_add(sector_count)
            .map_or(true, |end| end > runtime.capacity_sectors)
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
        (*desc.add(0)).next = 1;

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
                    libnanami::print!(" block=");
                    libnanami::print!("{}", block_index);
                    libnanami::print!(" sector=");
                    libnanami::print!("{}", sector as usize);
                    libnanami::print!(" bytes=");
                    libnanami::print!("{}", bytes);
                    libnanami::print!(" capacity=");
                    libnanami::print!("{}", runtime.capacity_sectors as usize);
                    libnanami::print!("\n");
                    Err(RequestError::Unsupported)
                };
            }
        }
    }
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
            (runtime.capacity_sectors / (BLOCK_SIZE / VIRTIO_SECTOR_BYTES) as u64) as Word,
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
    if count == 0 || offset + bytes > session.shm_size as usize || bytes > MAX_TRANSFER_BYTES {
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
    match submit_blk_request_dma(runtime, dma_paddr_base, VIRTIO_BLK_T_IN, block, bytes) {
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
    if count == 0 || offset + bytes > session.shm_size as usize || bytes > MAX_TRANSFER_BYTES {
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
    match submit_blk_request_dma(runtime, dma_paddr_base, VIRTIO_BLK_T_OUT, block, bytes) {
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

fn log_device_failure(msg: &str, io_desc: Word, io_base: Word) -> libnanami::NanamiError {
    libnanami::print!(msg);
    fail_device(io_desc, io_base);
    libnanami::NanamiError::UNKNOWN
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::print!("[virtio-blk] bootstrap start\n");

    let service_port_desc = libnanami::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let pci_io_desc = libnanami::ipc::process_slot_descriptor(SLOT_IO_PCI_CFG);
    let mut dev_io_desc = libnanami::ipc::process_slot_descriptor(SLOT_IO_VIRTIO);
    let notif_desc = libnanami::ipc::process_slot_descriptor(SLOT_NOTIFICATION);
    let irq_desc = libnanami::ipc::process_slot_descriptor(SLOT_INTERRUPT);

    let mut full_io_granted = false;
    match libnanami::request_io_port(0x0000, 0xffff, SLOT_IO_PCI_CFG) {
        Ok(()) => {
            full_io_granted = true;
            dev_io_desc = pci_io_desc;
            libnanami::print!("[virtio-blk] full io range granted\n");
        }
        Err(_) => match libnanami::request_io_port(0x0cf8, 0x0cff, SLOT_IO_PCI_CFG) {
            Ok(()) => libnanami::print!("[virtio-blk] pci cfg io ports granted\n"),
            Err(e) => {
                return Err(log_device_error(
                    "[virtio-blk] failed to request PCI cfg io ports: ",
                    e,
                    dev_io_desc,
                    0,
                ));
            }
        },
    }

    let found = match scan_virtio_blk(pci_io_desc) {
        Ok(v) => v,
        Err(_) => {
            return Err(log_device_failure(
                "[virtio-blk] virtio-blk pci device not found\n",
                dev_io_desc,
                0,
            ));
        }
    };
    libnanami::print!("[virtio-blk] found pci bus=");
    libnanami::print!("{}", found.bus as usize);
    libnanami::print!(" dev=");
    libnanami::print!("{}", found.dev as usize);
    libnanami::print!(" func=");
    libnanami::print!("{}", found.func as usize);
    libnanami::print!(" vid=");
    libnanami::print!("{:#x}", found.vendor_id);
    libnanami::print!(" did=");
    libnanami::print!("{:#x}", found.device_id);
    libnanami::print!(" io=");
    libnanami::print!("{:#x}", found.io_base);
    libnanami::print!(" irq=");
    libnanami::print!("{}", found.irq_line as usize);
    libnanami::print!("\n");

    if let Err(e) = configure_pci_command_for_intx(pci_io_desc, found) {
        return Err(log_device_error(
            "[virtio-blk] pci command configure failed: ",
            e,
            dev_io_desc,
            found.io_base as Word,
        ));
    }
    if let Err(e) = disable_pci_msi_capabilities(pci_io_desc, found) {
        return Err(log_device_error(
            "[virtio-blk] pci msi disable failed: ",
            e,
            dev_io_desc,
            found.io_base as Word,
        ));
    }

    let io_base = found.io_base as Word;
    match libnanami::request_io_port(io_base, io_base + 0xff, SLOT_IO_VIRTIO) {
        Ok(()) => libnanami::print!("[virtio-blk] virtio io range granted\n"),
        Err(e) => {
            if full_io_granted {
                libnanami::print!("[virtio-blk] virtio io range already covered by full range\n");
            } else {
                return Err(log_device_error(
                    "[virtio-blk] failed to request virtio io range: ",
                    e,
                    dev_io_desc,
                    io_base,
                ));
            }
        }
    }

    if let Ok(Some(irq_number)) = resolve_irq_number(pci_io_desc, found) {
        if libnanami::request_irq(irq_number, SLOT_NOTIFICATION, SLOT_INTERRUPT).is_ok() {
            if let Err(e) = libnanami::ipc::bind_current_thread_notification(notif_desc) {
                return Err(log_device_error(
                    "[virtio-blk] notification bind failed: ",
                    e,
                    dev_io_desc,
                    io_base,
                ));
            }
            let _ = libnanami::ipc::interrupt_ack(irq_desc);
            libnanami::print!("[virtio-blk] irq granted\n");
        }
    }

    let (dma_paddr_base, mut runtime) = match init_virtio_blk_with_dma_base(dev_io_desc, io_base) {
        Ok(v) => v,
        Err(e) => {
            return Err(log_device_error(
                "[virtio-blk] queue init failed: ",
                e,
                dev_io_desc,
                io_base,
            ));
        }
    };

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
                let _ = vio_read(runtime.io_desc, runtime.io_base, VIRTIO_PCI_ISR_STATUS, 1);
                let _ = libnanami::ipc::interrupt_ack(irq_desc);
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

fn init_virtio_blk_with_dma_base(
    io_desc: Word,
    io_base: Word,
) -> Result<(usize, BlockRuntime), RequestError> {
    vio_write(io_desc, io_base, VIRTIO_PCI_DEVICE_STATUS, 1, 0)?;
    vio_write(
        io_desc,
        io_base,
        VIRTIO_PCI_DEVICE_STATUS,
        1,
        (VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER) as Word,
    )?;

    let _features = vio_read(io_desc, io_base, VIRTIO_PCI_DEVICE_FEATURES, 4)?;
    vio_write(io_desc, io_base, VIRTIO_PCI_GUEST_FEATURES, 4, 0)?;
    let capacity_sectors = read_capacity_sectors(io_desc, io_base)?;

    vio_write(
        io_desc,
        io_base,
        VIRTIO_PCI_QUEUE_SELECT,
        2,
        QUEUE_INDEX as Word,
    )?;
    let queue_size = vio_read(io_desc, io_base, VIRTIO_PCI_QUEUE_SIZE, 2)? as u16;
    if capacity_sectors < 2 || queue_size < 3 || total_queue_bytes(queue_size) > QUEUE_MEM_BYTES {
        return Err(RequestError::Unsupported);
    }

    let (dma_paddr, dma_vaddr) = libnanami::request_dma(DMA_TOTAL_BYTES)?;
    let dma_paddr = dma_paddr as usize;
    let dma_vaddr = dma_vaddr as usize;
    unsafe {
        ptr::write_bytes(dma_vaddr as *mut u8, 0, DMA_TOTAL_BYTES);
    }

    vio_write(
        io_desc,
        io_base,
        VIRTIO_PCI_QUEUE_ADDRESS,
        4,
        ((dma_paddr + DMA_QUEUE_OFFSET) >> 12) as Word,
    )?;
    unsafe {
        *avail_flags_ptr((dma_vaddr + DMA_QUEUE_OFFSET) as *mut u8, queue_size) = 0;
        *used_flags_ptr((dma_vaddr + DMA_QUEUE_OFFSET) as *mut u8, queue_size) = 0;
    }

    vio_write(
        io_desc,
        io_base,
        VIRTIO_PCI_DEVICE_STATUS,
        1,
        (VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_DRIVER_OK) as Word,
    )?;
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
            queue_size,
            used_idx: 0,
            queue_vaddr: dma_vaddr + DMA_QUEUE_OFFSET,
            header_vaddr: dma_vaddr + DMA_HEADER_OFFSET,
            data_vaddr: dma_vaddr + DMA_DATA_OFFSET,
            status_vaddr: dma_vaddr + DMA_STATUS_OFFSET,
            capacity_sectors,
        },
    ))
}

libnanami::nanami_entry!(nanami_main);
