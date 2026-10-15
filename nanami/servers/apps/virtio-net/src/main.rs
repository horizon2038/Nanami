#![no_std]
#![no_main]

use core::cmp::min;
use core::ptr;
use core::sync::atomic::{fence, Ordering};

use nanami_runtime::ipc::ServiceEvent;
use nanami_runtime::{self, RequestError, Word};

#[path = "app/arch.rs"]
mod arch;
#[path = "app/pci.rs"]
mod pci;
#[path = "app/rx_backlog.rs"]
mod rx_backlog;
#[path = "app/rx_batch.rs"]
mod rx_batch;
#[path = "app/rx_softq.rs"]
mod rx_softq;
#[path = "app/tx_queue.rs"]
mod tx_queue;
#[path = "app/virtqueue.rs"]
mod virtqueue;
#[path = "app/util.rs"]
mod util;

use pci::{
    configure_pci_command_for_intx, disable_pci_msi_capabilities, resolve_irq_number,
    scan_virtio_net,
};
use rx_backlog::drain_rx_to_softq;
use rx_softq::{softq_is_empty, softq_pop_len, softq_pop_to_shared};
use tx_queue::{copy_from_shared_and_submit, submit_tx, TxQueue, TX_BUFFER_COUNT};
use util::{fail_device, log_request_error};
use virtqueue::*;

const SLOT_IO_PCI_CFG: Word = 16;
const SLOT_IO_VIRTIO: Word = 17;
const SLOT_NOTIFICATION: Word = 18;
const SLOT_INTERRUPT: Word = 19;
const SLOT_SERVICE_PORT: Word = 20;
const SLOT_CLIENT_RX_NOTIFICATION: Word = 24;

const VIRTIO_VENDOR_ID: u16 = 0x1af4;
const VIRTIO_NET_DEVICE_ID_LEGACY: u16 = 0x1000;
const VIRTIO_NET_DEVICE_ID_MODERN: u16 = 0x1041;

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

const NET_MAX_PACKET_BYTES: usize = nanami_client::net::NET_DEVICE_RX_FRAME_MAX;
const QUEUE_MEM_BYTES: usize = 16384;
const QUEUE_RX_INDEX: u16 = 0;
const QUEUE_TX_INDEX: u16 = 1;
const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;
const VIRTIO_NET_HDR_LEN: usize = 10;
const RX_SNAPSHOT_CAP: usize = 64;
const RX_BUFFER_COUNT: usize = 64;
const RX_SOFTQ_CAP: usize = 64;
const RX_POLL_BURST: usize = 16;
const ENABLE_IRQ_EVENT_LOG: bool = false;
const ENABLE_IRQ_RX_PACKET_DEBUG: bool = false;

struct NetRuntime {
    io_desc: Word,
    io_base: Word,
    // pci_cfg_desc: Word,
    // pci_bus: u8,
    // pci_dev: u8,
    // pci_func: u8,
    queue_ready: bool,
    link_up: bool,
    irq_enabled: bool,
    rx_queue_size: u16,
    rx_buffer_count: usize,
    tx_queue_size: u16,
    rx_used_idx: u16,
    tx_queue: TxQueue,
    rx_pending_len: usize,
    rx_packet_len: usize,
    rx_queue_vaddr: usize,
    tx_queue_vaddr: usize,
    rx_buf_vaddr_base: usize,
    tx_buf_vaddr: usize,
    shared_vaddr: usize,
    shared_size: usize,
    client_rx_notification: Word,
    mac_addr: [u8; 6],
}

const DMA_QUEUE0_OFFSET: usize = 0;
const DMA_QUEUE1_OFFSET: usize = DMA_QUEUE0_OFFSET + QUEUE_MEM_BYTES;
const DMA_RX_HDR_OFFSET: usize = DMA_QUEUE1_OFFSET + QUEUE_MEM_BYTES;
const DMA_RX_HDR_BYTES: usize = RX_BUFFER_COUNT * VIRTIO_NET_HDR_LEN;
const DMA_TX_HDR_OFFSET: usize = DMA_RX_HDR_OFFSET + DMA_RX_HDR_BYTES;
const DMA_RX_BUF_OFFSET: usize = DMA_TX_HDR_OFFSET + TX_BUFFER_COUNT * VIRTIO_NET_HDR_LEN;
const DMA_RX_BUF_BYTES: usize = RX_BUFFER_COUNT * NET_MAX_PACKET_BYTES;
const DMA_TX_BUF_OFFSET: usize = DMA_RX_BUF_OFFSET + DMA_RX_BUF_BYTES;
const DMA_TOTAL_BYTES: usize =
    (DMA_TX_BUF_OFFSET + TX_BUFFER_COUNT * NET_MAX_PACKET_BYTES + 4095) & !4095;

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

#[derive(Clone, Copy)]
struct QueueInitResult {
    rx_qsize: u16,
    tx_qsize: u16,
    rx_queue_vaddr: usize,
    tx_queue_vaddr: usize,
    rx_buf_vaddr_base: usize,
    tx_buf_vaddr: usize,
}

static mut RX_SNAPSHOT: [u8; RX_SNAPSHOT_CAP] = [0; RX_SNAPSHOT_CAP];
static mut RX_SNAPSHOT_LEN: usize = 0;
static mut RX_SOFTQ_DATA: [[u8; NET_MAX_PACKET_BYTES]; RX_SOFTQ_CAP] =
    [[0; NET_MAX_PACKET_BYTES]; RX_SOFTQ_CAP];
static mut RX_SOFTQ_LEN: [usize; RX_SOFTQ_CAP] = [0; RX_SOFTQ_CAP];
static mut RX_SOFTQ_HEAD: usize = 0;
static mut RX_SOFTQ_TAIL: usize = 0;
static mut RX_SOFTQ_COUNT: usize = 0;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::print!("[virtio-net] panic\n");
    loop {}
}

fn log_device_error(
    prefix: &str,
    err: RequestError,
    io_desc: Word,
    io_base: Word,
) -> nanami_runtime::NanamiError {
    log_request_error(prefix, err);
    fail_device(io_desc, io_base);
    err.into()
}

fn log_device_failure(msg: &str, io_desc: Word, io_base: Word) -> nanami_runtime::NanamiError {
    nanami_runtime::print!(msg);
    fail_device(io_desc, io_base);
    nanami_runtime::NanamiError::UNKNOWN
}

fn vio_read(io_desc: Word, io_base: Word, offset: Word, width: Word) -> Result<Word, RequestError> {
    nanami_runtime::io::io_read(io_desc, io_base + offset, width)
}

fn vio_write(
    io_desc: Word,
    io_base: Word,
    offset: Word,
    width: Word,
    value: Word,
) -> Result<(), RequestError> {
    nanami_runtime::io::io_write(io_desc, io_base + offset, width, value)
}

fn notify_queue(io_desc: Word, io_base: Word, queue_index: u16) -> Result<(), RequestError> {
    vio_write(
        io_desc,
        io_base,
        VIRTIO_PCI_QUEUE_NOTIFY,
        2,
        queue_index as Word,
    )
}

fn select_queue_and_get_size(
    io_desc: Word,
    io_base: Word,
    queue_index: u16,
) -> Result<u16, RequestError> {
    vio_write(
        io_desc,
        io_base,
        VIRTIO_PCI_QUEUE_SELECT,
        2,
        queue_index as Word,
    )?;
    Ok(vio_read(io_desc, io_base, VIRTIO_PCI_QUEUE_SIZE, 2)? as u16)
}

fn init_virtio_legacy_queues(
    io_desc: Word,
    io_base: Word,
) -> Result<QueueInitResult, RequestError> {
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

    let rx_qsize = select_queue_and_get_size(io_desc, io_base, QUEUE_RX_INDEX)?;
    let tx_qsize = select_queue_and_get_size(io_desc, io_base, QUEUE_TX_INDEX)?;
    if rx_qsize == 0 || tx_qsize == 0 {
        return Err(RequestError::Unsupported);
    }
    if rx_qsize < 2 || tx_qsize < 2 {
        return Err(RequestError::Unsupported);
    }
    if total_queue_bytes(rx_qsize) > QUEUE_MEM_BYTES
        || total_queue_bytes(tx_qsize) > QUEUE_MEM_BYTES
    {
        return Err(RequestError::Unsupported);
    }

    let (dma_paddr, dma_vaddr) = nanami_runtime::request_dma(DMA_TOTAL_BYTES)?;
    let dma_paddr = dma_paddr as usize;
    let dma_vaddr = dma_vaddr as usize;
    let rx_queue_vaddr = dma_vaddr + DMA_QUEUE0_OFFSET;
    let tx_queue_vaddr = dma_vaddr + DMA_QUEUE1_OFFSET;
    let rx_buf_vaddr_base = dma_vaddr + DMA_RX_BUF_OFFSET;
    let tx_buf_vaddr = dma_vaddr + DMA_TX_BUF_OFFSET;

    let rx_queue_paddr = dma_paddr + DMA_QUEUE0_OFFSET;
    let tx_queue_paddr = dma_paddr + DMA_QUEUE1_OFFSET;
    let rx_hdr_paddr_base = dma_paddr + DMA_RX_HDR_OFFSET;
    let tx_hdr_paddr = dma_paddr + DMA_TX_HDR_OFFSET;
    let rx_buf_paddr_base = dma_paddr + DMA_RX_BUF_OFFSET;
    let tx_buf_paddr = dma_paddr + DMA_TX_BUF_OFFSET;

    unsafe {
        ptr::write_bytes(dma_vaddr as *mut u8, 0, DMA_TOTAL_BYTES);
    }

    // RX queue (0)
    vio_write(
        io_desc,
        io_base,
        VIRTIO_PCI_QUEUE_SELECT,
        2,
        QUEUE_RX_INDEX as Word,
    )?;
    let rx_base = rx_queue_vaddr as *mut u8;
    let rx_pfn = (rx_queue_paddr >> 12) as Word;
    vio_write(io_desc, io_base, VIRTIO_PCI_QUEUE_ADDRESS, 4, rx_pfn)?;
    unsafe {
        let d = desc_ptr(rx_base);
        let mut i = 0usize;
        let rx_count = min(RX_BUFFER_COUNT, rx_qsize as usize / 2);
        while i < rx_count {
            let head = i * 2;
            let hdr_paddr = rx_hdr_paddr_base + i * VIRTIO_NET_HDR_LEN;
            let buf_paddr = rx_buf_paddr_base + i * NET_MAX_PACKET_BYTES;

            (*d.add(head)).addr = hdr_paddr as u64;
            (*d.add(head)).len = VIRTIO_NET_HDR_LEN as u32;
            (*d.add(head)).flags = DESC_F_WRITE | DESC_F_NEXT;
            (*d.add(head)).next = (head + 1) as u16;

            (*d.add(head + 1)).addr = buf_paddr as u64;
            (*d.add(head + 1)).len = NET_MAX_PACKET_BYTES as u32;
            (*d.add(head + 1)).flags = DESC_F_WRITE;
            (*d.add(head + 1)).next = 0;

            *avail_ring_ptr(rx_base, rx_qsize).add(i) = head as u16;
            i += 1;
        }
        *avail_flags_ptr(rx_base, rx_qsize) = 0;
        *used_flags_ptr(rx_base, rx_qsize) = 0;
        fence(Ordering::Release);
        *avail_idx_ptr(rx_base, rx_qsize) = rx_count as u16;
    }
    fence(Ordering::SeqCst);
    notify_queue(io_desc, io_base, QUEUE_RX_INDEX)?;

    // TX queue (1)
    vio_write(
        io_desc,
        io_base,
        VIRTIO_PCI_QUEUE_SELECT,
        2,
        QUEUE_TX_INDEX as Word,
    )?;
    let tx_base = tx_queue_vaddr as *mut u8;
    let tx_pfn = (tx_queue_paddr >> 12) as Word;
    vio_write(io_desc, io_base, VIRTIO_PCI_QUEUE_ADDRESS, 4, tx_pfn)?;
    unsafe {
        let d = desc_ptr(tx_base);
        for slot in 0..min(TX_BUFFER_COUNT, tx_qsize as usize / 2) {
            let head = slot * 2;
            (*d.add(head)).addr = (tx_hdr_paddr + slot * VIRTIO_NET_HDR_LEN) as u64;
            (*d.add(head)).len = VIRTIO_NET_HDR_LEN as u32;
            (*d.add(head)).flags = DESC_F_NEXT;
            (*d.add(head)).next = (head + 1) as u16;
            (*d.add(head + 1)).addr = (tx_buf_paddr + slot * NET_MAX_PACKET_BYTES) as u64;
            (*d.add(head + 1)).len = 0;
            (*d.add(head + 1)).flags = 0;
            (*d.add(head + 1)).next = 0;
        }
        // Completion is reclaimed on the next submission; TX-only interrupts
        // do not provide useful work for the client.
        *avail_flags_ptr(tx_base, tx_qsize) = 1; // VIRTQ_AVAIL_F_NO_INTERRUPT
        *used_flags_ptr(tx_base, tx_qsize) = 0;
    }

    vio_write(
        io_desc,
        io_base,
        VIRTIO_PCI_DEVICE_STATUS,
        1,
        (VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_DRIVER_OK) as Word,
    )?;
    let status_after = read_device_status(io_desc, io_base)?;
    nanami_runtime::print!("[virtio-net] status after DRIVER_OK=");
    nanami_runtime::print!("{:#x}", status_after);
    nanami_runtime::print!("\n");

    Ok(QueueInitResult {
        rx_qsize,
        tx_qsize,
        rx_queue_vaddr,
        tx_queue_vaddr,
        rx_buf_vaddr_base,
        tx_buf_vaddr,
    })
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

fn set_link_state(runtime: &mut NetRuntime, up: bool) -> Result<(), RequestError> {
    if !runtime.queue_ready {
        return Err(RequestError::Unsupported);
    }
    let mut status = read_device_status(runtime.io_desc, runtime.io_base)?;
    if up {
        status |= VIRTIO_STATUS_DRIVER_OK;
    } else {
        status &= !VIRTIO_STATUS_DRIVER_OK;
    }
    write_device_status(runtime.io_desc, runtime.io_base, status)?;
    runtime.link_up = up;
    Ok(())
}

#[inline(always)]
fn rx_buffer_vaddr(runtime: &NetRuntime, chain: usize) -> usize {
    runtime.rx_buf_vaddr_base + chain * NET_MAX_PACKET_BYTES
}

fn take_rx_chain(runtime: &mut NetRuntime) -> Option<(usize, usize)> {
    unsafe {
        let base = runtime.rx_queue_vaddr as *mut u8;
        let used_idx = ptr::read_volatile(used_idx_ptr(base, runtime.rx_queue_size));
        if used_idx == runtime.rx_used_idx {
            return None;
        }
        fence(Ordering::Acquire);
        let elem = ptr::read_volatile(
            used_ring_ptr(base, runtime.rx_queue_size)
                .add((runtime.rx_used_idx as usize) % runtime.rx_queue_size as usize),
        );
        runtime.rx_used_idx = runtime.rx_used_idx.wrapping_add(1);
        let head = elem.id as usize;
        let chain = head / 2;
        if head & 1 != 0 || chain >= runtime.rx_buffer_count {
            return None;
        }

        let total_len = elem.len as usize;
        runtime.rx_pending_len = total_len.saturating_sub(VIRTIO_NET_HDR_LEN);
        runtime.rx_packet_len = min(runtime.rx_pending_len, NET_MAX_PACKET_BYTES);
        if ENABLE_IRQ_RX_PACKET_DEBUG {
            let snap_len = min(runtime.rx_packet_len, RX_SNAPSHOT_CAP);
            if snap_len > 0 {
                ptr::copy_nonoverlapping(
                    rx_buffer_vaddr(runtime, chain) as *const u8,
                    ptr::addr_of_mut!(RX_SNAPSHOT) as *mut u8,
                    snap_len,
                );
            }
            RX_SNAPSHOT_LEN = snap_len;
        }
        Some((chain, runtime.rx_packet_len))
    }
}

fn recycle_rx_chain(runtime: &mut NetRuntime, chain: usize) {
    unsafe {
        let base = runtime.rx_queue_vaddr as *mut u8;
        let head = (chain * 2) as u16;
        let avail_idx = ptr::read_volatile(avail_idx_ptr(base, runtime.rx_queue_size));
        ptr::write_volatile(
            avail_ring_ptr(base, runtime.rx_queue_size)
                .add((avail_idx as usize) % runtime.rx_queue_size as usize),
            head,
        );
        fence(Ordering::SeqCst);
        ptr::write_volatile(
            avail_idx_ptr(base, runtime.rx_queue_size),
            avail_idx.wrapping_add(1),
        );
    }
}


fn recv_direct_to_shared(
    runtime: &mut NetRuntime,
    shared_offset: usize,
    requested_len: usize,
) -> Result<Option<usize>, RequestError> {
    if runtime.shared_vaddr == 0 || runtime.shared_size == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let Some((chain, packet_len)) = take_rx_chain(runtime) else {
        return Ok(None);
    };
    let n = min(packet_len, requested_len);
    let result = if shared_offset >= runtime.shared_size
        || shared_offset
            .checked_add(n)
            .is_none_or(|end| end > runtime.shared_size)
    {
        Err(RequestError::InvalidArgument)
    } else if n > 0 {
        unsafe {
            let src = rx_buffer_vaddr(runtime, chain) as *const u8;
            let dst = (runtime.shared_vaddr + shared_offset) as *mut u8;
            ptr::copy_nonoverlapping(src, dst, n);
        }
        Ok(n)
    } else {
        Ok(0)
    };
    recycle_rx_chain(runtime, chain);
    let _ = notify_queue(runtime.io_desc, runtime.io_base, QUEUE_RX_INDEX);
    result.map(Some)
}


fn read_device_mac(io_desc: Word, io_base: Word) -> Result<[u8; 6], RequestError> {
    let mut mac = [0u8; 6];
    let mut i = 0usize;
    while i < 6 {
        mac[i] = vio_read(
            io_desc,
            io_base,
            VIRTIO_PCI_LEGACY_DEVICE_CONFIG_BASE + i as Word,
            1,
        )? as u8;
        i += 1;
    }
    Ok(mac)
}

fn handle_net_request(
    runtime: &mut NetRuntime,
    _irq_desc: Word,
    request: nanami_runtime::ipc::ServiceRequest,
) -> (Word, Word, Word) {
    match request.code {
        nanami_client::net::NET_DEVICE_REQUEST_SEND => {
            if !runtime.queue_ready || !runtime.link_up {
                return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
            }
            if request.arg1 == 0 {
                return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            if request.arg1 > NET_MAX_PACKET_BYTES {
                return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            let send_result = if runtime.shared_vaddr != 0 && runtime.shared_size != 0 {
                copy_from_shared_and_submit(runtime, request.arg0, request.arg1)
            } else {
                submit_tx(runtime, request.arg1)
            };
            match send_result {
                Ok(n) => (nanami_runtime::OS_RESPONSE_OK, n, 0),
                Err(_) => (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
            }
        }
        nanami_client::net::NET_DEVICE_REQUEST_RECV => {
            if !runtime.queue_ready || !runtime.link_up {
                return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
            }
            if request.arg1 == 0 {
                return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            if runtime.shared_vaddr != 0 && runtime.shared_size != 0 {
                if softq_is_empty() {
                    match recv_direct_to_shared(runtime, request.arg0, request.arg1) {
                        Ok(Some(n)) => return (nanami_runtime::OS_RESPONSE_OK, n, 0),
                        Ok(None) => {}
                        Err(RequestError::InvalidArgument) => {
                            return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0)
                        }
                        Err(_) => return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
                    }
                }
                match softq_pop_to_shared(runtime, request.arg0, request.arg1) {
                    Ok(n) => (nanami_runtime::OS_RESPONSE_OK, n, 0),
                    Err(RequestError::InvalidArgument) => {
                        (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0)
                    }
                    Err(_) => (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
                }
            } else {
                if softq_is_empty() {
                    drain_rx_to_softq(runtime, RX_POLL_BURST);
                }
                let n = softq_pop_len(request.arg1);
                (nanami_runtime::OS_RESPONSE_OK, n, 0)
            }
        }
        nanami_client::net::NET_DEVICE_REQUEST_RECV_BATCH => {
            if !runtime.queue_ready || !runtime.link_up {
                return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
            }
            match rx_batch::recv_batch_to_shared(runtime, request.arg0, request.arg1) {
                Ok(count) => (nanami_runtime::OS_RESPONSE_OK, count, 0),
                Err(_) => (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
            }
        }
        nanami_client::net::NET_DEVICE_REQUEST_CONTROL => match request.arg0 {
            nanami_client::net::NET_DEVICE_CONTROL_GET_FEATURES => (
                nanami_runtime::OS_RESPONSE_OK,
                nanami_client::net::NET_DEVICE_FEATURE_RECV_BATCH,
                0,
            ),
            nanami_client::net::NET_DEVICE_CONTROL_LINK_UP => match set_link_state(runtime, true)
            {
                Ok(()) => (nanami_runtime::OS_RESPONSE_OK, 0, 0),
                Err(RequestError::InvalidArgument) => {
                    (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0)
                }
                Err(_) => (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
            },
            nanami_client::net::NET_DEVICE_CONTROL_LINK_DOWN => {
                match set_link_state(runtime, false) {
                    Ok(()) => (nanami_runtime::OS_RESPONSE_OK, 0, 0),
                    Err(RequestError::InvalidArgument) => {
                        (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0)
                    }
                    Err(_) => (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
                }
            }
            nanami_client::net::NET_DEVICE_CONTROL_ATTACH_SHARED_MEMORY => {
                // NET_DEVICE_REQUEST_CONTROL layout:
                // arg0=control_code, arg1=control_arg0(vaddr), arg2=control_arg1(size)
                if request.arg1 == 0 || request.arg2 == 0 {
                    return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
                }
                runtime.shared_vaddr = request.arg1;
                runtime.shared_size = request.arg2;
                nanami_runtime::print!("[virtio-net] shared memory attached vaddr=");
                nanami_runtime::print!("{:#x}", runtime.shared_vaddr);
                nanami_runtime::print!(" size=");
                nanami_runtime::print!("{:#x}", runtime.shared_size);
                nanami_runtime::print!("\n");
                (nanami_runtime::OS_RESPONSE_OK, 0, 0)
            }
            nanami_client::net::NET_DEVICE_CONTROL_GET_MAC => {
                let packed = (runtime.mac_addr[0] as Word)
                    | ((runtime.mac_addr[1] as Word) << 8)
                    | ((runtime.mac_addr[2] as Word) << 16)
                    | ((runtime.mac_addr[3] as Word) << 24)
                    | ((runtime.mac_addr[4] as Word) << 32)
                    | ((runtime.mac_addr[5] as Word) << 40);
                (nanami_runtime::OS_RESPONSE_OK, packed, 0)
            }
            nanami_client::net::NET_DEVICE_CONTROL_ATTACH_RX_NOTIFICATION => {
                let source_slot = if request.arg1 == 0 {
                    nanami_runtime::PROCESS_SLOT_NOTIFICATION
                } else {
                    request.arg1
                };
                match nanami_runtime::request_notification_port_copy(
                    request.identifier,
                    source_slot,
                    SLOT_CLIENT_RX_NOTIFICATION,
                    nanami_client::net::NET_NOTIFICATION_RX,
                ) {
                    Ok(()) => {
                        runtime.client_rx_notification =
                            nanami_runtime::ipc::process_slot_descriptor(SLOT_CLIENT_RX_NOTIFICATION);
                        (nanami_runtime::OS_RESPONSE_OK, runtime.irq_enabled as Word, 0)
                    }
                    Err(error) => (
                        match error {
                            RequestError::InvalidArgument => {
                                nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT
                            }
                            RequestError::Status(status) => status,
                            _ => nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION,
                        },
                        0,
                        0,
                    ),
                }
            }
            _ => (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
        },
        _ => (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_notification(runtime: &mut NetRuntime, irq_desc: Word, identifier: Word, value: Word) {
    if ENABLE_IRQ_EVENT_LOG {
        nanami_runtime::print!("[virtio-net] notification id=");
        nanami_runtime::print!("{}", identifier as usize);
        nanami_runtime::print!(" value=");
        nanami_runtime::print!("{:#x}", value as u32);
        nanami_runtime::print!("\n");
    }

    if runtime.irq_enabled {
        // Legacy virtio-pci INTx requires reading ISR status to de-assert the device interrupt.
        // Without this, later RX/TX events may not generate a new interrupt edge/level transition.
        match vio_read(runtime.io_desc, runtime.io_base, VIRTIO_PCI_ISR_STATUS, 1) {
            Ok(isr) => {
                if ENABLE_IRQ_RX_PACKET_DEBUG {
                    nanami_runtime::print!("[virtio-net][irq.dbg] isr=");
                    nanami_runtime::print!("{:#x}", isr as u8);
                    nanami_runtime::print!("\n");
                }
            }
            Err(e) => {
                log_request_error("[virtio-net][irq.dbg] isr read failed: ", e);
            }
        }

        let mut rx_count = drain_rx_to_softq(runtime, RX_SOFTQ_CAP);
        if let Err(e) = nanami_runtime::ipc::interrupt_ack(irq_desc) {
            log_request_error("[virtio-net] irq re-arm failed: ", e);
        }

        // A packet can arrive after the ring was drained but before INTx is re-armed.
        // Rechecking after the ack closes that masked-interrupt window.
        rx_count += drain_rx_to_softq(runtime, RX_SOFTQ_CAP);
        if rx_count != 0 && runtime.client_rx_notification != 0 {
            let _ = nanami_runtime::ipc::notification_notify(runtime.client_rx_notification);
        }
    }
}

fn nanami_main() -> nanami_runtime::NanamiResult {
    nanami_runtime::print!("[virtio-net] bootstrap start\n");

    let service_port_desc = nanami_runtime::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let pci_io_desc = nanami_runtime::ipc::process_slot_descriptor(SLOT_IO_PCI_CFG);
    let mut dev_io_desc = nanami_runtime::ipc::process_slot_descriptor(SLOT_IO_VIRTIO);
    let notif_desc = nanami_runtime::ipc::process_slot_descriptor(SLOT_NOTIFICATION);
    let irq_desc = nanami_runtime::ipc::process_slot_descriptor(SLOT_INTERRUPT);

    nanami_runtime::print!("[virtio-net] step: register service\n");
    match nanami_client::registry::register_net_device() {
        Ok(()) => nanami_runtime::print!("[virtio-net] service registered: net-device\n"),
        Err(e) => {
            return Err(log_device_error(
                "[virtio-net] service registration failed: ",
                e,
                dev_io_desc,
                0,
            ));
        }
    }

    let mut full_io_granted = false;
    match nanami_runtime::request_io_port(0x0000, 0xffff, SLOT_IO_PCI_CFG) {
        Ok(()) => {
            nanami_runtime::print!("[virtio-net] full io range granted\n");
            full_io_granted = true;
            dev_io_desc = pci_io_desc;
        }
        Err(_) => match nanami_runtime::request_io_port(0x0cf8, 0x0cff, SLOT_IO_PCI_CFG) {
            Ok(()) => nanami_runtime::print!("[virtio-net] pci cfg io ports granted\n"),
            Err(e) => {
                return Err(log_device_error(
                    "[virtio-net] failed to request PCI cfg io ports: ",
                    e,
                    dev_io_desc,
                    0,
                ));
            }
        },
    }

    nanami_runtime::print!("[virtio-net] step: scan pci\n");
    nanami_runtime::diagnostics::clock_info("virtio-net");
    let scan = nanami_runtime::diagnostics::measure(nanami_runtime::diagnostics::RpcStage::PciScan,
        0, || scan_virtio_net(pci_io_desc));
    nanami_runtime::diagnostics::flush_rpc(nanami_runtime::diagnostics::RpcStage::PciScan);
    let found = match scan {
        Ok(v) => v,
        Err(_) => {
            return Err(log_device_failure(
                "[virtio-net] virtio-net pci device not found\n",
                dev_io_desc,
                0,
            ));
        }
    };

    nanami_runtime::print!("[virtio-net] found pci bus=");
    nanami_runtime::print!("{}", found.bus as usize);
    nanami_runtime::print!(" dev=");
    nanami_runtime::print!("{}", found.dev as usize);
    nanami_runtime::print!(" func=");
    nanami_runtime::print!("{}", found.func as usize);
    nanami_runtime::print!(" vid=");
    nanami_runtime::print!("{:#x}", found.vendor_id);
    nanami_runtime::print!(" did=");
    nanami_runtime::print!("{:#x}", found.device_id);
    nanami_runtime::print!(" io=");
    nanami_runtime::print!("{:#x}", found.io_base);
    nanami_runtime::print!(" irq=");
    nanami_runtime::print!("{}", found.irq_line as usize);
    nanami_runtime::print!(" pin=");
    nanami_runtime::print!("{}", found.irq_pin as usize);
    nanami_runtime::print!("\n");
    if let Err(e) = configure_pci_command_for_intx(pci_io_desc, found) {
        return Err(log_device_error(
            "[virtio-net] pci command configure failed: ",
            e,
            dev_io_desc,
            found.io_base as Word,
        ));
    }
    if let Err(e) = disable_pci_msi_capabilities(pci_io_desc, found) {
        return Err(log_device_error(
            "[virtio-net] pci msi disable failed: ",
            e,
            dev_io_desc,
            found.io_base as Word,
        ));
    }

    nanami_runtime::print!("[virtio-net] step: request virtio io range\n");
    let io_min = found.io_base as Word;
    let io_max = io_min + 0xff;
    match nanami_runtime::request_io_port(io_min, io_max, SLOT_IO_VIRTIO) {
        Ok(()) => nanami_runtime::print!("[virtio-net] virtio io range granted\n"),
        Err(e) => {
            if full_io_granted {
                nanami_runtime::print!("[virtio-net] virtio io range already covered by full range\n",);
            } else {
                return Err(log_device_error(
                    "[virtio-net] failed to request virtio io range: ",
                    e,
                    dev_io_desc,
                    io_min,
                ));
            }
        }
    }

    nanami_runtime::print!("[virtio-net] step: request irq\n");
    let mut irq_enabled = false;
    let selected_irq = match resolve_irq_number(pci_io_desc, found) {
        Ok(v) => v,
        Err(e) => {
            log_request_error("[virtio-net] irq route resolve failed: ", e);
            if found.irq_line != 0 && found.irq_line != 0xff {
                Some(found.irq_line as Word)
            } else {
                None
            }
        }
    };

    if let Some(irq_number) = selected_irq {
        nanami_runtime::print!("[virtio-net] request irq number=");
        nanami_runtime::print!("{}", irq_number as usize);
        nanami_runtime::print!(" (line=");
        nanami_runtime::print!("{}", found.irq_line as usize);
        nanami_runtime::print!(", pin=");
        nanami_runtime::print!("{}", found.irq_pin as usize);
        nanami_runtime::print!(")\n");
        match nanami_runtime::request_irq(irq_number, SLOT_NOTIFICATION, SLOT_INTERRUPT) {
            Ok(()) => {
                irq_enabled = true;
                nanami_runtime::print!("[virtio-net] irq granted\n");
                match nanami_runtime::ipc::bind_current_thread_notification(notif_desc) {
                    Ok(()) => nanami_runtime::print!("[virtio-net] tcb notification bound\n"),
                    Err(e) => {
                        return Err(log_device_error(
                            "[virtio-net] tcb notification bind failed: ",
                            e,
                            dev_io_desc,
                            found.io_base as Word,
                        ));
                    }
                }
                match nanami_runtime::ipc::interrupt_ack(irq_desc) {
                    Ok(()) => nanami_runtime::print!("[virtio-net] irq armed\n"),
                    Err(e) => {
                        return Err(log_device_error(
                            "[virtio-net] irq arm failed: ",
                            e,
                            dev_io_desc,
                            found.io_base as Word,
                        ));
                    }
                }
            }
            Err(e) => {
                log_request_error("[virtio-net] irq request failed, fallback polling: ", e);
            }
        }
    } else {
        nanami_runtime::print!("[virtio-net] device irq unavailable, fallback polling\n");
    }

    nanami_runtime::print!("[virtio-net] step: init queues\n");
    let io_base = found.io_base as Word;
    let queue_init = match init_virtio_legacy_queues(dev_io_desc, io_base) {
        Ok(v) => v,
        Err(e) => {
            return Err(log_device_error(
                "[virtio-net] queue init failed: ",
                e,
                dev_io_desc,
                io_base,
            ));
        }
    };
    nanami_runtime::print!("[virtio-net] queues init done rx=");
    nanami_runtime::print!("{}", queue_init.rx_qsize as usize);
    nanami_runtime::print!(" tx=");
    nanami_runtime::print!("{}", queue_init.tx_qsize as usize);
    nanami_runtime::print!("\n");
    let mac_addr = match read_device_mac(dev_io_desc, io_base) {
        Ok(m) => m,
        Err(_) => [0; 6],
    };

    let mut runtime = NetRuntime {
        io_desc: dev_io_desc,
        io_base,
        // pci_cfg_desc: pci_io_desc,
        // pci_bus: found.bus,
        // pci_dev: found.dev,
        // pci_func: found.func,
        queue_ready: true,
        link_up: true,
        irq_enabled,
        rx_queue_size: queue_init.rx_qsize,
        rx_buffer_count: min(RX_BUFFER_COUNT, queue_init.rx_qsize as usize / 2),
        tx_queue_size: queue_init.tx_qsize,
        rx_used_idx: 0,
        tx_queue: TxQueue::new(queue_init.tx_qsize),
        rx_pending_len: 0,
        rx_packet_len: 0,
        rx_queue_vaddr: queue_init.rx_queue_vaddr,
        tx_queue_vaddr: queue_init.tx_queue_vaddr,
        rx_buf_vaddr_base: queue_init.rx_buf_vaddr_base,
        tx_buf_vaddr: queue_init.tx_buf_vaddr,
        shared_vaddr: 0,
        shared_size: 0,
        client_rx_notification: 0,
        mac_addr,
    };

    if runtime.irq_enabled {
        match nanami_runtime::ipc::interrupt_ack(irq_desc) {
            Ok(()) => nanami_runtime::print!("[virtio-net] irq re-armed before loop\n"),
            Err(e) => {
                return Err(log_device_error(
                    "[virtio-net] irq re-arm before loop failed: ",
                    e,
                    runtime.io_desc,
                    runtime.io_base,
                ));
            }
        }
    }

    nanami_runtime::print!("[virtio-net] step: enter service loop\n");
    let mut pending_status = (nanami_runtime::OS_RESPONSE_OK, 0, 0);
    let mut has_pending_reply = false;

    loop {
        if !runtime.irq_enabled && softq_is_empty() {
            drain_rx_to_softq(&mut runtime, RX_POLL_BURST);
        }
        let used_reply_receive = has_pending_reply;
        let event = if used_reply_receive {
            match nanami_runtime::ipc::service_reply_receive_event(
                service_port_desc,
                pending_status.0,
                pending_status.1,
                pending_status.2,
            ) {
                Ok(e) => e,
                Err(e) => {
                    return Err(log_device_error(
                        "[virtio-net] service reply_receive failed: ",
                        e,
                        runtime.io_desc,
                        runtime.io_base,
                    ));
                }
            }
        } else {
            match nanami_runtime::ipc::service_receive_event(service_port_desc) {
                Ok(e) => e,
                Err(e) => {
                    return Err(log_device_error(
                        "[virtio-net] initial service receive failed: ",
                        e,
                        runtime.io_desc,
                        runtime.io_base,
                    ));
                }
            }
        };
        if used_reply_receive {
            has_pending_reply = false;
        }

        match event {
            ServiceEvent::Request(request) => {
                pending_status = handle_net_request(&mut runtime, irq_desc, request);
                has_pending_reply = true;
            }
            ServiceEvent::Notification { identifier, value } => {
                handle_notification(&mut runtime, irq_desc, identifier, value);
            }
            ServiceEvent::Fault {
                identifier, reason, ..
            } => {
                nanami_runtime::print!("[virtio-net] fault event id=");
                nanami_runtime::print!("{}", identifier as usize);
                nanami_runtime::print!(" reason=");
                nanami_runtime::print!("{:#x}", reason as u32);
                nanami_runtime::print!("\n");
                has_pending_reply = false;
            }
        }
    }
}

nanami_runtime::nanami_entry!(nanami_main);
