#![no_std]
#![no_main]

#[cfg(not(target_arch = "x86_64"))]
compile_error!("ahci-server is an x86_64 platform driver");

use core::ptr;
use core::sync::atomic::{fence, Ordering};

use libnanami::ipc::{ServiceEvent, ServiceRequest};
use libnanami::{RequestError, Word};

const SLOT_PCI_CONFIG: Word = 16;
const SLOT_SERVICE_PORT: Word = 20;
const PCI_CONFIG_ADDRESS: Word = 0x0cf8;
const PCI_CONFIG_DATA: Word = 0x0cfc;

const PCI_CLASS_MASS_STORAGE: u8 = 0x01;
const PCI_SUBCLASS_SATA: u8 = 0x06;
const PCI_PROG_IF_AHCI: u8 = 0x01;
const PCI_COMMAND_MEMORY: u16 = 1 << 1;
const PCI_COMMAND_BUS_MASTER: u16 = 1 << 2;

const HBA_CAP: usize = 0x00;
const HBA_GHC: usize = 0x04;
const HBA_IS: usize = 0x08;
const HBA_PI: usize = 0x0c;
const HBA_CAP2: usize = 0x24;
const HBA_BOHC: usize = 0x28;
const HBA_GHC_AE: u32 = 1 << 31;
const HBA_CAP2_BOH: u32 = 1;
const HBA_BOHC_BOS: u32 = 1;
const HBA_BOHC_OOS: u32 = 1 << 1;
const HBA_BOHC_BB: u32 = 1 << 4;

const PORT_BASE: usize = 0x100;
const PORT_STRIDE: usize = 0x80;
const PORT_CLB: usize = 0x00;
const PORT_CLBU: usize = 0x04;
const PORT_FB: usize = 0x08;
const PORT_FBU: usize = 0x0c;
const PORT_IS: usize = 0x10;
const PORT_CMD: usize = 0x18;
const PORT_TFD: usize = 0x20;
const PORT_SIG: usize = 0x24;
const PORT_SSTS: usize = 0x28;
const PORT_SERR: usize = 0x30;
const PORT_CI: usize = 0x38;
const PORT_CMD_ST: u32 = 1;
const PORT_CMD_FRE: u32 = 1 << 4;
const PORT_CMD_FR: u32 = 1 << 14;
const PORT_CMD_CR: u32 = 1 << 15;
const PORT_TFD_DRQ: u32 = 1 << 3;
const PORT_TFD_BSY: u32 = 1 << 7;
const PORT_IS_TFES: u32 = 1 << 30;
const SATA_SIGNATURE: u32 = 0x0000_0101;

const ATA_IDENTIFY_DEVICE: u8 = 0xec;
const ATA_READ_DMA_EXT: u8 = 0x25;
const ATA_WRITE_DMA_EXT: u8 = 0x35;

const DMA_COMMAND_LIST_OFFSET: usize = 0x0000;
const DMA_RECEIVED_FIS_OFFSET: usize = 0x0400;
const DMA_COMMAND_TABLE_OFFSET: usize = 0x0500;
const DMA_DATA_OFFSET: usize = 0x1000;
const DMA_DATA_BYTES: usize = nanami_services::block::BLOCK_DEVICE_DEFAULT_SHM_BYTES as usize;
const DMA_TOTAL_BYTES: usize = DMA_DATA_OFFSET + DMA_DATA_BYTES;
const COMMAND_TABLE_PRDT_OFFSET: usize = 0x80;
const AHCI_MMIO_BYTES: Word = 0x2000;
const SECTOR_BYTES: usize = 512;
const BLOCK_BYTES: usize = nanami_services::block::BLOCK_DEVICE_BLOCK_SIZE as usize;
const POLL_LIMIT: usize = 10_000_000;
const MAX_AHCI_CONTROLLERS: usize = 16;

#[derive(Clone, Copy)]
struct PciAddress {
    bus: u8,
    device: u8,
    function: u8,
    abar: Word,
}

impl PciAddress {
    const EMPTY: Self = Self {
        bus: 0,
        device: 0,
        function: 0,
        abar: 0,
    };
}

#[derive(Clone, Copy)]
struct ClientSession {
    active: bool,
    pid: Word,
    shared_memory: Word,
    shared_size: Word,
}

impl ClientSession {
    const EMPTY: Self = Self {
        active: false,
        pid: 0,
        shared_memory: 0,
        shared_size: 0,
    };
}

#[derive(Clone, Copy)]
struct AhciRuntime {
    abar: Word,
    port: usize,
    dma_physical: Word,
    dma_virtual: Word,
    disk_capacity_sectors: u64,
    partition_start_sector: u64,
    partition_sectors: u64,
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[ahci-server] panic\n");
    let _ = libnanami::request_exit();
    loop {}
}

fn config_address(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    0x8000_0000
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | (u32::from(offset) & 0xfc)
}

fn pci_read32(
    descriptor: Word,
    bus: u8,
    device: u8,
    function: u8,
    offset: u8,
) -> Result<u32, RequestError> {
    libnanami::io::io_write(
        descriptor,
        PCI_CONFIG_ADDRESS,
        4,
        config_address(bus, device, function, offset) as Word,
    )?;
    Ok(libnanami::io::io_read(descriptor, PCI_CONFIG_DATA, 4)? as u32)
}

fn pci_write32(
    descriptor: Word,
    address: PciAddress,
    offset: u8,
    value: u32,
) -> Result<(), RequestError> {
    libnanami::io::io_write(
        descriptor,
        PCI_CONFIG_ADDRESS,
        4,
        config_address(address.bus, address.device, address.function, offset) as Word,
    )?;
    libnanami::io::io_write(descriptor, PCI_CONFIG_DATA, 4, value as Word)
}

fn find_controllers(
    descriptor: Word,
) -> Result<([PciAddress; MAX_AHCI_CONTROLLERS], usize), RequestError> {
    let mut controllers = [PciAddress::EMPTY; MAX_AHCI_CONTROLLERS];
    let mut controller_count = 0usize;
    let mut pending_buses = [0u8; 256];
    let mut seen_buses = [false; 256];
    let mut head = 0usize;
    let mut tail = 1usize;
    seen_buses[0] = true;
    while head < tail {
        let bus = pending_buses[head];
        head += 1;
        let mut device = 0u8;
        while device < 32 {
            let function0_id = pci_read32(descriptor, bus, device, 0, 0x00)?;
            if function0_id as u16 == 0xffff {
                device += 1;
                continue;
            }
            let header = pci_read32(descriptor, bus, device, 0, 0x0c)?;
            let functions = if ((header >> 16) as u8 & 0x80) != 0 {
                8
            } else {
                1
            };
            let mut function = 0u8;
            while function < functions {
                let id = if function == 0 {
                    function0_id
                } else {
                    pci_read32(descriptor, bus, device, function, 0x00)?
                };
                if id as u16 != 0xffff {
                    let class = pci_read32(descriptor, bus, device, function, 0x08)?;
                    if (class >> 24) as u8 == PCI_CLASS_MASS_STORAGE
                        && (class >> 16) as u8 == PCI_SUBCLASS_SATA
                        && (class >> 8) as u8 == PCI_PROG_IF_AHCI
                    {
                        let bar5 = pci_read32(descriptor, bus, device, function, 0x24)?;
                        if (bar5 & 1) == 0 && (bar5 & 0x6) != 0x4 {
                            let abar = (bar5 & 0xffff_fff0) as Word;
                            if abar != 0 {
                                if controller_count == controllers.len() {
                                    return Err(RequestError::Unsupported);
                                }
                                controllers[controller_count] = PciAddress {
                                    bus,
                                    device,
                                    function,
                                    abar,
                                };
                                controller_count += 1;
                            }
                        }
                    }
                    if (class >> 24) as u8 == 0x06 && (class >> 16) as u8 == 0x04 {
                        let buses = pci_read32(descriptor, bus, device, function, 0x18)?;
                        let secondary = ((buses >> 8) & 0xff) as usize;
                        if secondary != 0 && !seen_buses[secondary] {
                            seen_buses[secondary] = true;
                            pending_buses[tail] = secondary as u8;
                            tail += 1;
                        }
                    }
                }
                function += 1;
            }
            device += 1;
        }
    }
    if controller_count == 0 {
        Err(RequestError::Unsupported)
    } else {
        Ok((controllers, controller_count))
    }
}

unsafe fn mmio_read32(base: Word, offset: usize) -> u32 {
    unsafe { ptr::read_volatile((base as usize + offset) as *const u32) }
}

unsafe fn mmio_write32(base: Word, offset: usize, value: u32) {
    unsafe { ptr::write_volatile((base as usize + offset) as *mut u32, value) };
}

fn port_offset(port: usize, register: usize) -> usize {
    PORT_BASE + port * PORT_STRIDE + register
}

fn wait_clear(base: Word, offset: usize, mask: u32) -> Result<(), RequestError> {
    let mut spin = 0usize;
    while unsafe { mmio_read32(base, offset) } & mask != 0 {
        spin += 1;
        if spin >= POLL_LIMIT {
            return Err(RequestError::Transport);
        }
        if spin & 0x3fff == 0 {
            libnanami::yield_now();
        }
    }
    Ok(())
}

fn bios_handoff(base: Word) -> Result<(), RequestError> {
    if unsafe { mmio_read32(base, HBA_CAP2) } & HBA_CAP2_BOH == 0 {
        return Ok(());
    }
    let ownership = unsafe { mmio_read32(base, HBA_BOHC) } | HBA_BOHC_OOS;
    unsafe { mmio_write32(base, HBA_BOHC, ownership) };
    wait_clear(base, HBA_BOHC, HBA_BOHC_BOS | HBA_BOHC_BB)
}

fn is_active_sata_port(base: Word, port: usize, implemented: u32) -> bool {
    if implemented & (1u32 << port) == 0 {
        return false;
    }
    let status = unsafe { mmio_read32(base, port_offset(port, PORT_SSTS)) };
    let signature = unsafe { mmio_read32(base, port_offset(port, PORT_SIG)) };
    status & 0x0f == 3 && (status >> 8) & 0x0f == 1 && signature == SATA_SIGNATURE
}

fn configure_port(runtime: &AhciRuntime) -> Result<(), RequestError> {
    let base = runtime.abar;
    let port = runtime.port;
    let command_offset = port_offset(port, PORT_CMD);
    let mut command = unsafe { mmio_read32(base, command_offset) };
    command &= !(PORT_CMD_ST | PORT_CMD_FRE);
    unsafe { mmio_write32(base, command_offset, command) };
    wait_clear(base, command_offset, PORT_CMD_CR | PORT_CMD_FR)?;

    let command_list = runtime.dma_physical as u64 + DMA_COMMAND_LIST_OFFSET as u64;
    let received_fis = runtime.dma_physical as u64 + DMA_RECEIVED_FIS_OFFSET as u64;
    unsafe {
        mmio_write32(base, port_offset(port, PORT_CLB), command_list as u32);
        mmio_write32(
            base,
            port_offset(port, PORT_CLBU),
            (command_list >> 32) as u32,
        );
        mmio_write32(base, port_offset(port, PORT_FB), received_fis as u32);
        mmio_write32(
            base,
            port_offset(port, PORT_FBU),
            (received_fis >> 32) as u32,
        );
        mmio_write32(base, port_offset(port, PORT_SERR), u32::MAX);
        mmio_write32(base, port_offset(port, PORT_IS), u32::MAX);
        mmio_write32(base, HBA_IS, 1u32 << port);
    }

    command = unsafe { mmio_read32(base, command_offset) } | PORT_CMD_FRE;
    unsafe { mmio_write32(base, command_offset, command) };
    command |= PORT_CMD_ST;
    unsafe { mmio_write32(base, command_offset, command) };
    Ok(())
}

fn prepare_command(
    runtime: &AhciRuntime,
    command: u8,
    lba: u64,
    sectors: u16,
    bytes: usize,
    write: bool,
) -> Result<(), RequestError> {
    if bytes > DMA_DATA_BYTES
        || bytes > 0x0040_0000
        || (bytes != 0 && sectors == 0 && command != ATA_IDENTIFY_DEVICE)
    {
        return Err(RequestError::InvalidArgument);
    }
    unsafe {
        ptr::write_bytes(runtime.dma_virtual as *mut u8, 0, DMA_DATA_OFFSET);
        let command_list = (runtime.dma_virtual as usize + DMA_COMMAND_LIST_OFFSET) as *mut u32;
        let command_table = runtime.dma_virtual as usize + DMA_COMMAND_TABLE_OFFSET;
        let mut flags = 5u32;
        if write {
            flags |= 1 << 6;
        }
        if bytes != 0 {
            flags |= 1 << 16;
        }
        ptr::write_volatile(command_list, flags);
        ptr::write_volatile(command_list.add(1), 0);
        let command_table_physical = runtime.dma_physical as u64 + DMA_COMMAND_TABLE_OFFSET as u64;
        ptr::write_volatile(command_list.add(2), command_table_physical as u32);
        ptr::write_volatile(command_list.add(3), (command_table_physical >> 32) as u32);

        let fis = command_table as *mut u8;
        ptr::write(fis.add(0), 0x27);
        ptr::write(fis.add(1), 1 << 7);
        ptr::write(fis.add(2), command);
        ptr::write(fis.add(4), lba as u8);
        ptr::write(fis.add(5), (lba >> 8) as u8);
        ptr::write(fis.add(6), (lba >> 16) as u8);
        ptr::write(fis.add(7), 1 << 6);
        ptr::write(fis.add(8), (lba >> 24) as u8);
        ptr::write(fis.add(9), (lba >> 32) as u8);
        ptr::write(fis.add(10), (lba >> 40) as u8);
        ptr::write(fis.add(12), sectors as u8);
        ptr::write(fis.add(13), (sectors >> 8) as u8);

        if bytes != 0 {
            let prdt = (command_table + COMMAND_TABLE_PRDT_OFFSET) as *mut u32;
            let data_physical = runtime.dma_physical as u64 + DMA_DATA_OFFSET as u64;
            ptr::write_volatile(prdt, data_physical as u32);
            ptr::write_volatile(prdt.add(1), (data_physical >> 32) as u32);
            ptr::write_volatile(prdt.add(2), 0);
            ptr::write_volatile(prdt.add(3), (bytes as u32 - 1) | (1 << 31));
        }
    }
    fence(Ordering::Release);
    Ok(())
}

fn submit(runtime: &AhciRuntime) -> Result<(), RequestError> {
    let base = runtime.abar;
    let port = runtime.port;
    wait_clear(
        base,
        port_offset(port, PORT_TFD),
        PORT_TFD_BSY | PORT_TFD_DRQ,
    )?;
    unsafe {
        mmio_write32(base, port_offset(port, PORT_IS), u32::MAX);
        mmio_write32(base, port_offset(port, PORT_CI), 1);
    }
    let mut spin = 0usize;
    loop {
        let issued = unsafe { mmio_read32(base, port_offset(port, PORT_CI)) };
        let status = unsafe { mmio_read32(base, port_offset(port, PORT_IS)) };
        if status & PORT_IS_TFES != 0 {
            return Err(RequestError::Transport);
        }
        if issued & 1 == 0 {
            fence(Ordering::Acquire);
            return Ok(());
        }
        spin += 1;
        if spin >= POLL_LIMIT {
            return Err(RequestError::Transport);
        }
        if spin & 0x3fff == 0 {
            libnanami::yield_now();
        }
    }
}

fn identify(runtime: &AhciRuntime) -> Result<u64, RequestError> {
    prepare_command(runtime, ATA_IDENTIFY_DEVICE, 0, 0, SECTOR_BYTES, false)?;
    submit(runtime)?;
    let words = (runtime.dma_virtual as usize + DMA_DATA_OFFSET) as *const u16;
    let lba48_supported = unsafe { ptr::read_unaligned(words.add(83)) } & (1 << 10) != 0;
    let sectors = if lba48_supported {
        let w100 = unsafe { ptr::read_unaligned(words.add(100)) } as u64;
        let w101 = unsafe { ptr::read_unaligned(words.add(101)) } as u64;
        let w102 = unsafe { ptr::read_unaligned(words.add(102)) } as u64;
        let w103 = unsafe { ptr::read_unaligned(words.add(103)) } as u64;
        w100 | (w101 << 16) | (w102 << 32) | (w103 << 48)
    } else {
        let low = unsafe { ptr::read_unaligned(words.add(60)) } as u64;
        let high = unsafe { ptr::read_unaligned(words.add(61)) } as u64;
        low | (high << 16)
    };
    if sectors == 0 {
        return Err(RequestError::Protocol);
    }
    Ok(sectors)
}

fn transfer_absolute(
    runtime: &AhciRuntime,
    lba: u64,
    bytes: usize,
    write: bool,
) -> Result<(), RequestError> {
    if bytes == 0 || bytes > DMA_DATA_BYTES || bytes % SECTOR_BYTES != 0 {
        return Err(RequestError::InvalidArgument);
    }
    let sectors = bytes / SECTOR_BYTES;
    if sectors > u16::MAX as usize
        || lba
            .checked_add(sectors as u64)
            .map_or(true, |end| end > runtime.disk_capacity_sectors)
    {
        return Err(RequestError::InvalidArgument);
    }
    prepare_command(
        runtime,
        if write {
            ATA_WRITE_DMA_EXT
        } else {
            ATA_READ_DMA_EXT
        },
        lba,
        sectors as u16,
        bytes,
        write,
    )?;
    submit(runtime)
}

fn find_root_partition(runtime: &AhciRuntime) -> Result<nanami_gpt::Partition, RequestError> {
    transfer_absolute(runtime, 1, SECTOR_BYTES, false)?;
    let data = unsafe {
        core::slice::from_raw_parts(
            (runtime.dma_virtual as usize + DMA_DATA_OFFSET) as *const u8,
            DMA_DATA_BYTES,
        )
    };
    let header =
        nanami_gpt::parse_primary_header(&data[..SECTOR_BYTES], runtime.disk_capacity_sectors)
            .map_err(|_| RequestError::Protocol)?;
    let entry_bytes = header
        .partition_entry_bytes()
        .map_err(|_| RequestError::Unsupported)?;
    let transfer_bytes = entry_bytes.div_ceil(SECTOR_BYTES) * SECTOR_BYTES;
    transfer_absolute(runtime, header.partition_entry_lba, transfer_bytes, false)?;
    let data = unsafe {
        core::slice::from_raw_parts(
            (runtime.dma_virtual as usize + DMA_DATA_OFFSET) as *const u8,
            transfer_bytes,
        )
    };
    nanami_gpt::find_nanami_root(header, data).map_err(|_| RequestError::Protocol)
}

fn transfer(
    runtime: &AhciRuntime,
    block_index: usize,
    bytes: usize,
    write: bool,
) -> Result<(), RequestError> {
    if bytes == 0 || bytes > DMA_DATA_BYTES || bytes % SECTOR_BYTES != 0 {
        return Err(RequestError::InvalidArgument);
    }
    let sectors = bytes / SECTOR_BYTES;
    let partition_lba = block_index
        .checked_mul(BLOCK_BYTES / SECTOR_BYTES)
        .ok_or(RequestError::InvalidArgument)? as u64;
    if sectors > u16::MAX as usize
        || partition_lba
            .checked_add(sectors as u64)
            .map_or(true, |end| end > runtime.partition_sectors)
    {
        return Err(RequestError::InvalidArgument);
    }
    let lba = runtime
        .partition_start_sector
        .checked_add(partition_lba)
        .ok_or(RequestError::InvalidArgument)?;
    transfer_absolute(runtime, lba, bytes, write)
}

fn handle_control(
    request: ServiceRequest,
    session: &mut ClientSession,
    runtime: &AhciRuntime,
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
                        shared_memory: local,
                        shared_size: size,
                    };
                    (libnanami::OS_RESPONSE_OK, peer, size)
                }
                Err(error) => (status(error), 0, 0),
            }
        }
        nanami_services::block::BLOCK_DEVICE_CONTROL_GET_INFO => (
            libnanami::OS_RESPONSE_OK,
            BLOCK_BYTES as Word,
            (runtime.partition_sectors / (BLOCK_BYTES / SECTOR_BYTES) as u64) as Word,
        ),
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_transfer(
    request: ServiceRequest,
    session: &ClientSession,
    runtime: &AhciRuntime,
    write: bool,
) -> (Word, Word, Word) {
    if !session.active || session.pid != request.identifier {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let bytes = match (request.arg1 as usize).checked_mul(BLOCK_BYTES) {
        Some(bytes) => bytes,
        None => return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    };
    let offset = request.arg2 as usize;
    if request.arg1 == 0
        || offset
            .checked_add(bytes)
            .map_or(true, |end| end > session.shared_size)
        || bytes > DMA_DATA_BYTES
    {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }

    let dma_data = runtime.dma_virtual as usize + DMA_DATA_OFFSET;
    if write {
        unsafe {
            ptr::copy_nonoverlapping(
                (session.shared_memory as usize + offset) as *const u8,
                dma_data as *mut u8,
                bytes,
            )
        };
    }
    match transfer(runtime, request.arg0 as usize, bytes, write) {
        Ok(()) => {
            if !write {
                unsafe {
                    ptr::copy_nonoverlapping(
                        dma_data as *const u8,
                        (session.shared_memory as usize + offset) as *mut u8,
                        bytes,
                    )
                };
            }
            (libnanami::OS_RESPONSE_OK, bytes, 0)
        }
        Err(error) => (status(error), 0, 0),
    }
}

fn handle_request(
    request: ServiceRequest,
    session: &mut ClientSession,
    runtime: &AhciRuntime,
) -> (Word, Word, Word) {
    match request.code {
        nanami_services::block::BLOCK_DEVICE_REQUEST_CONTROL => {
            handle_control(request, session, runtime)
        }
        nanami_services::block::BLOCK_DEVICE_REQUEST_READ => {
            handle_transfer(request, session, runtime, false)
        }
        nanami_services::block::BLOCK_DEVICE_REQUEST_WRITE => {
            handle_transfer(request, session, runtime, true)
        }
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn status(error: RequestError) -> Word {
    match error {
        RequestError::InvalidArgument => libnanami::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Status(value) => value,
        RequestError::Transport | RequestError::Protocol => libnanami::OS_RESPONSE_FATAL,
    }
}

fn initialize() -> Result<AhciRuntime, RequestError> {
    libnanami::request_io_port(0x0cf8, 0x0cff, SLOT_PCI_CONFIG)?;
    let pci = libnanami::ipc::process_slot_descriptor(SLOT_PCI_CONFIG);
    let (controllers, controller_count) = find_controllers(pci)?;
    let (dma_physical, dma_virtual) = libnanami::request_dma(DMA_TOTAL_BYTES)?;
    unsafe { ptr::write_bytes(dma_virtual as *mut u8, 0, DMA_TOTAL_BYTES) };
    let mut selected: Option<(AhciRuntime, PciAddress)> = None;

    for address in controllers[..controller_count].iter().copied() {
        let command_status = pci_read32(pci, address.bus, address.device, address.function, 0x04)?;
        let command = (command_status as u16) | PCI_COMMAND_MEMORY | PCI_COMMAND_BUS_MASTER;
        pci_write32(
            pci,
            address,
            0x04,
            (command_status & 0xffff_0000) | command as u32,
        )?;

        let (_, abar) = libnanami::request_mmio(address.abar, AHCI_MMIO_BYTES)?;
        bios_handoff(abar)?;
        unsafe { mmio_write32(abar, HBA_GHC, mmio_read32(abar, HBA_GHC) | HBA_GHC_AE) };
        let implemented = unsafe { mmio_read32(abar, HBA_PI) };
        let port_count = ((unsafe { mmio_read32(abar, HBA_CAP) } & 0x1f) + 1) as usize;
        for port in 0..port_count.min(32) {
            if !is_active_sata_port(abar, port, implemented) {
                continue;
            }
            let mut runtime = AhciRuntime {
                abar,
                port,
                dma_physical,
                dma_virtual,
                disk_capacity_sectors: 0,
                partition_start_sector: 0,
                partition_sectors: 0,
            };
            if configure_port(&runtime).is_err() {
                continue;
            }
            runtime.disk_capacity_sectors = match identify(&runtime) {
                Ok(sectors) => sectors,
                Err(_) => continue,
            };
            let partition = match find_root_partition(&runtime) {
                Ok(partition) => partition,
                Err(_) => continue,
            };
            runtime.partition_start_sector = partition.first_lba;
            runtime.partition_sectors = partition.sector_count;
            if selected.is_some() {
                libnanami::print!("[ahci-server] multiple Nanami root partitions found\n");
                return Err(RequestError::Protocol);
            }
            selected = Some((runtime, address));
        }
    }

    let (runtime, address) = selected.ok_or(RequestError::Unsupported)?;
    configure_port(&runtime)?;
    libnanami::println!(
        "[ahci-server] root PCI {:02x}:{:02x}.{} abar={:#x} port={} LBA={} sectors={}",
        address.bus,
        address.device,
        address.function,
        address.abar,
        runtime.port,
        runtime.partition_start_sector,
        runtime.partition_sectors
    );
    Ok(runtime)
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::ipc::init_ipc_tls()?;
    let runtime = initialize()?;
    nanami_services::registry::register_block_device()?;
    libnanami::print!("[ahci-server] service registered: block-device\n");

    let port = libnanami::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let mut session = ClientSession::EMPTY;
    let mut reply = (libnanami::OS_RESPONSE_OK, 0, 0);
    let mut has_reply = false;
    loop {
        let event = if has_reply {
            has_reply = false;
            libnanami::ipc::service_reply_receive_event(port, reply.0, reply.1, reply.2)?
        } else {
            libnanami::ipc::service_receive_event(port)?
        };
        match event {
            ServiceEvent::Request(request) => {
                reply = handle_request(request, &mut session, &runtime);
                has_reply = true;
            }
            ServiceEvent::Notification { .. } | ServiceEvent::Fault { .. } => {}
        }
    }
}

libnanami::nanami_entry!(nanami_main);
