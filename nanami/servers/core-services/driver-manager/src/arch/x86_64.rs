use libnanami::{RequestError, Word};

use super::super::TimerSelection;

#[path = "x86_64/acpi.rs"]
mod acpi;

const SLOT_PCI_CONFIG: Word = 16;
const PCI_CONFIG_ADDRESS: Word = 0x0cf8;
const PCI_CONFIG_DATA: Word = 0x0cfc;

const PCI_CLASS_MASS_STORAGE: u8 = 0x01;
const PCI_SUBCLASS_SATA: u8 = 0x06;
const PCI_PROG_IF_AHCI: u8 = 0x01;

fn config_address(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    0x8000_0000
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | (u32::from(offset) & 0xfc)
}

fn read_config32(
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

fn has_ahci_controller(descriptor: Word) -> Result<bool, RequestError> {
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
            let function0_id = read_config32(descriptor, bus, device, 0, 0x00)?;
            if function0_id as u16 == 0xffff {
                device += 1;
                continue;
            }
            let header = read_config32(descriptor, bus, device, 0, 0x0c)?;
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
                    read_config32(descriptor, bus, device, function, 0x00)?
                };
                if (id as u16) != 0xffff {
                    let class = read_config32(descriptor, bus, device, function, 0x08)?;
                    if (class >> 24) as u8 == PCI_CLASS_MASS_STORAGE
                        && (class >> 16) as u8 == PCI_SUBCLASS_SATA
                        && (class >> 8) as u8 == PCI_PROG_IF_AHCI
                    {
                        libnanami::println!(
                            "[driver-manager] PCI AHCI at {:02x}:{:02x}.{}",
                            bus,
                            device,
                            function
                        );
                        return Ok(true);
                    }
                    if (class >> 24) as u8 == 0x06 && (class >> 16) as u8 == 0x04 {
                        let buses = read_config32(descriptor, bus, device, function, 0x18)?;
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
    Ok(false)
}

pub fn select_storage_driver() -> Result<Option<&'static str>, RequestError> {
    libnanami::request_io_port(0x0cf8, 0x0cff, SLOT_PCI_CONFIG)?;
    let descriptor = libnanami::ipc::process_slot_descriptor(SLOT_PCI_CONFIG);
    let storage_image = if has_ahci_controller(descriptor)? {
        "./bin/ahci-server"
    } else {
        libnanami::print!(
            "[driver-manager] no PCI AHCI controller; selecting virtio-blk fallback\n"
        );
        "./bin/virtio-blk-server"
    };
    Ok(Some(storage_image))
}

pub fn select_timer_driver() -> Result<TimerSelection, RequestError> {
    let (rsdp, _) =
        libnanami::request_driver_platform_info(libnanami::DRIVER_PLATFORM_INFO_RSDP_ADDRESS)?;
    let hpet_mmio_base = match acpi::find_hpet_mmio_base(rsdp) {
        Ok(Some(address)) => {
            libnanami::println!(
                "[driver-manager] ACPI HPET at {:#x} (RSDP {:#x})",
                address,
                rsdp
            );
            address
        }
        Ok(None) => 0,
        Err(error) => {
            libnanami::println!(
                "[driver-manager] ACPI scan failed at RSDP {:#x}: {}; selecting PIT fallback",
                rsdp,
                error
            );
            0
        }
    };

    let timer_image = if hpet_mmio_base != 0 {
        "./bin/hpet-server"
    } else {
        libnanami::print!("[driver-manager] no ACPI HPET; selecting PIT fallback\n");
        "./bin/timer-server"
    };

    Ok(TimerSelection {
        image: Some(timer_image),
        hpet_mmio_base,
    })
}
