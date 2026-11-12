#![no_std]
#![no_main]

#[cfg(not(target_arch = "x86_64"))]
compile_error!("ahci-server is an x86_64 platform driver");

use core::ptr;
use core::sync::atomic::{fence, Ordering};

use nanami_runtime::ipc::{ServiceEvent, ServiceRequest};
use nanami_runtime::{RequestError, Word};

#[path = "app/dma.rs"]
mod dma;
use dma::*;

#[path = "app/completion.rs"]
mod completion;

#[path = "app/command_state.rs"]
mod command_state;

#[path = "app/session.rs"]
mod session;
use session::{ClientSession, handle_request};

const SLOT_PCI_CONFIG: Word = 16;
const SLOT_SERVICE_PORT: Word = 20;

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

const AHCI_MMIO_BYTES: Word = 0x2000;
const SECTOR_BYTES: usize = 512;
const BLOCK_BYTES: usize = nanami_client::block::BLOCK_DEVICE_BLOCK_SIZE as usize;
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

struct AhciRuntime {
    abar: Word,
    port: usize,
    dma_physical: Word,
    dma_virtual: Word,
    dma_data_bytes: usize,
    disk_capacity_sectors: u64,
    partition_start_sector: u64,
    partition_sectors: u64,
    flush_command: u8,
    commands: command_state::State,
}

const _: () = assert!(core::mem::size_of::<AhciRuntime>() == 9 * core::mem::size_of::<Word>());

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::print!("[ahci-server] panic\n");
    retain_dma()
}

fn retain_dma() -> ! {
    let notification =
        nanami_runtime::ipc::process_slot_descriptor(nanami_runtime::PROCESS_SLOT_NOTIFICATION);
    loop {
        let _ = nanami_runtime::ipc::notification_wait(notification);
    }
}

fn pci_read32(
    port: Word,
    bus: u8,
    device: u8,
    function: u8,
    offset: u8,
) -> Result<u32, RequestError> {
    nanami_client::pci::read(
        port,
        nanami_client::pci::Address::new(bus, device, function),
        offset as Word,
        4,
    )
}

fn find_controllers(
    port: Word,
) -> Result<([PciAddress; MAX_AHCI_CONTROLLERS], usize), RequestError> {
    let mut controllers = [PciAddress::EMPTY; MAX_AHCI_CONTROLLERS];
    let mut count = 0;
    let mut index = 0;
    while let Some(device) = nanami_client::pci::enumerate(port, index)? {
        if device.address.segment == 0 && device.class == 0x010601 {
            let bar = nanami_client::pci::read(port, device.address, 0x24, 4)?;
            if bar & 7 == 0 && bar & !15 != 0 {
                if count == controllers.len() {
                    return Err(RequestError::Unsupported);
                }
                controllers[count] = PciAddress {
                    bus: device.address.bus,
                    device: device.address.device,
                    function: device.address.function,
                    abar: (bar & !15) as Word,
                };
                count += 1;
            }
        }
        index += 1;
    }
    if count == 0 {
        return Err(RequestError::Unsupported);
    }
    Ok((controllers, count))
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
            nanami_runtime::yield_now();
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

fn stop_port(runtime: &AhciRuntime) -> Result<(), RequestError> {
    runtime.commands.invalidate();
    let base = runtime.abar;
    let command_offset = port_offset(runtime.port, PORT_CMD);
    let mut command = unsafe { mmio_read32(base, command_offset) };
    command &= !PORT_CMD_ST;
    unsafe { mmio_write32(base, command_offset, command) };
    wait_clear(base, command_offset, PORT_CMD_CR)?;
    command = unsafe { mmio_read32(base, command_offset) } & !PORT_CMD_FRE;
    unsafe { mmio_write32(base, command_offset, command) };
    wait_clear(base, command_offset, PORT_CMD_FR)
}

fn configure_port(runtime: &AhciRuntime) -> Result<(), RequestError> {
    stop_port(runtime)?;
    let base = runtime.abar;
    let port = runtime.port;
    let command_offset = port_offset(port, PORT_CMD);

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

    let mut command = unsafe { mmio_read32(base, command_offset) } | PORT_CMD_FRE;
    unsafe { mmio_write32(base, command_offset, command) };
    command |= PORT_CMD_ST;
    unsafe { mmio_write32(base, command_offset, command) };
    Ok(())
}

fn submit(runtime: &AhciRuntime, bytes: usize) -> Result<(), RequestError> {
    let base = runtime.abar;
    let port = runtime.port;
    runtime.commands.submit(|| wait_clear(
        base,
        port_offset(port, PORT_TFD),
        PORT_TFD_BSY | PORT_TFD_DRQ,
    ), || unsafe {
        mmio_write32(base, port_offset(port, PORT_IS), u32::MAX);
        mmio_write32(base, port_offset(port, PORT_CI), 1);
    }, || completion::wait(
        |register| unsafe { mmio_read32(base, port_offset(port, match register {
            completion::Register::Issued => PORT_CI,
            completion::Register::Status => PORT_IS,
        })) },
        || {
            // PRDBC is coherent, private DMA RAM, reset before every command.
            // Reaching the byte count never proves completion: wait() still
            // requires fresh CI-clear and TFES checks. Flush has no data hint.
            bytes == 0 || unsafe {
                ptr::read_volatile((runtime.dma_virtual + DMA_COMMAND_LIST_OFFSET + 4) as *const u32)
                    as usize == bytes
            }
        },
        nanami_runtime::yield_now,
        POLL_LIMIT,
    ))
}

fn identify(runtime: &mut AhciRuntime) -> Result<u64, RequestError> {
    prepare_command(runtime, ATA_IDENTIFY_DEVICE, 0, 0, SECTOR_BYTES, false)?;
    submit(runtime, SECTOR_BYTES)?;
    let words = (runtime.dma_virtual as usize + DMA_DATA_OFFSET) as *const u16;
    let commands = unsafe { ptr::read_unaligned(words.add(83)) };
    runtime.flush_command = if commands & 0xc000 != 0x4000 {
        0
    } else if commands & (1 << 13) != 0 {
        0xea // FLUSH CACHE EXT
    } else if commands & (1 << 12) != 0 {
        0xe7 // FLUSH CACHE
    } else {
        0
    };
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
    transfer_absolute_to(runtime, lba, bytes, write, runtime.dma_physical + DMA_DATA_OFFSET)
}

fn transfer_absolute_to(
    runtime: &AhciRuntime,
    lba: u64,
    bytes: usize,
    write: bool,
    data_physical: Word,
) -> Result<(), RequestError> {
    if bytes == 0 || bytes > runtime.dma_data_bytes || bytes % SECTOR_BYTES != 0 {
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
    prepare_command_to(
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
        data_physical,
    )?;
    submit(runtime, bytes)
}

fn find_root_partition(runtime: &AhciRuntime) -> Result<nanami_gpt::Partition, RequestError> {
    transfer_absolute(runtime, 1, SECTOR_BYTES, false)?;
    let data = unsafe {
        core::slice::from_raw_parts(
            (runtime.dma_virtual as usize + DMA_DATA_OFFSET) as *const u8,
            runtime.dma_data_bytes,
        )
    };
    let header =
        nanami_gpt::parse_primary_header(&data[..SECTOR_BYTES], runtime.disk_capacity_sectors)
            .map_err(|_| RequestError::Unsupported)?;
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
    nanami_gpt::find_nanami_root(header, data).map_err(|error| match error {
        nanami_gpt::Error::Ambiguous => RequestError::Protocol,
        _ => RequestError::Unsupported,
    })
}

fn transfer(
    runtime: &AhciRuntime,
    block_index: usize,
    bytes: usize,
    write: bool,
    data_physical: Word,
) -> Result<(), RequestError> {
    if bytes == 0 || bytes > runtime.dma_data_bytes || bytes % SECTOR_BYTES != 0 {
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
    transfer_absolute_to(runtime, lba, bytes, write, data_physical)
}

fn status(error: RequestError) -> Word {
    match error {
        RequestError::InvalidArgument => nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Status(value) => value,
        RequestError::Transport | RequestError::Protocol => nanami_runtime::OS_RESPONSE_FATAL,
    }
}

fn initialize() -> Result<AhciRuntime, RequestError> {
    nanami_client::pci::connect(SLOT_PCI_CONFIG)?;
    let pci = nanami_runtime::ipc::process_slot_descriptor(SLOT_PCI_CONFIG);
    let (controllers, controller_count) = find_controllers(pci)?;
    let (dma_physical, dma_virtual, dma_data_bytes) = allocate_command_memory(nanami_runtime::request_dma)?;
    unsafe { ptr::write_bytes(dma_virtual as *mut u8, 0, DMA_DATA_OFFSET + dma_data_bytes) };
    let mut selected: Option<(AhciRuntime, PciAddress)> = None;

    for address in controllers[..controller_count].iter().copied() {
        let command_status = pci_read32(pci, address.bus, address.device, address.function, 0x04)?;
        let command = (command_status as u16) | PCI_COMMAND_MEMORY | PCI_COMMAND_BUS_MASTER;
        nanami_client::pci::write(
            pci,
            nanami_client::pci::Address::new(address.bus, address.device, address.function),
            4,
            2,
            command as u32,
        )?;

        let (_, abar) = nanami_runtime::request_mmio(address.abar, AHCI_MMIO_BYTES)?;
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
                dma_data_bytes,
                disk_capacity_sectors: 0,
                partition_start_sector: 0,
                partition_sectors: 0,
                flush_command: 0,
                commands: command_state::State::new(),
            };
            configure_port(&runtime)?;
            let partition = (|| {
                runtime.disk_capacity_sectors = identify(&mut runtime)?;
                find_root_partition(&runtime)
            })();
            // The next port shares this command/FIS buffer. Stop both DMA
            // engines even for a non-root disk before reusing it or yielding
            // storage ownership to USB. A timeout must not return its pages.
            stop_port(&runtime)?;
            let partition = match partition {
                Ok(partition) => partition,
                Err(RequestError::Unsupported) => continue,
                Err(error) => return Err(error),
            };
            runtime.partition_start_sector = partition.first_lba;
            runtime.partition_sectors = partition.sector_count;
            if selected.is_some() {
                nanami_runtime::print!("[ahci-server] multiple Nanami root partitions found\n");
                return Err(RequestError::Protocol);
            }
            selected = Some((runtime, address));
        }
    }

    let (runtime, address) = selected.ok_or(RequestError::Unsupported)?;
    nanami_runtime::println!(
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

fn nanami_main() -> nanami_runtime::NanamiResult {
    nanami_runtime::ipc::init_ipc_tls()?;
    if let Err(error) = serve() {
        nanami_runtime::println!(
            "[ahci-server] stopped status={:#x}; DMA mappings retained",
            error.0
        );
        retain_dma();
    }
    Ok(())
}

fn serve() -> nanami_runtime::NanamiResult {
    let runtime = match initialize() {
        Ok(runtime) => runtime,
        Err(error) => {
            nanami_runtime::println!("[ahci-server] root probe: {}", error);
            let count = if matches!(error, RequestError::Unsupported) {
                0
            } else {
                2
            };
            let _ = nanami_client::device::select_storage_root(count);
            return Err(error.into());
        }
    };
    if !nanami_client::device::select_storage_root(1)? {
        return Err(RequestError::Unsupported.into());
    }
    configure_port(&runtime)?;
    nanami_client::registry::register_block_device()?;
    nanami_runtime::print!("[ahci-server] service registered: block-device\n");

    let port = nanami_runtime::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let mut session = ClientSession::EMPTY;
    let mut reply = (nanami_runtime::OS_RESPONSE_OK, 0, 0);
    let mut has_reply = false;
    loop {
        let event = if has_reply {
            has_reply = false;
            nanami_runtime::ipc::service_reply_receive_event(port, reply.0, reply.1, reply.2)?
        } else {
            nanami_runtime::ipc::service_receive_event(port)?
        };
        match event {
            ServiceEvent::Request(request) => {
                reply = handle_request(request, &mut session, &runtime)?;
                has_reply = true;
            }
            ServiceEvent::Notification { .. } | ServiceEvent::Fault { .. } => {}
        }
    }
}

nanami_runtime::nanami_entry!(nanami_main);
