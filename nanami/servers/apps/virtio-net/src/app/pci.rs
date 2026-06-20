use super::arch::x86_64::*;
use super::*;

const PCI_COMMAND_IO_SPACE: u16 = 1 << 0;
const PCI_COMMAND_BUS_MASTER: u16 = 1 << 2;
const PCI_COMMAND_INTX_DISABLE: u16 = 1 << 10;
const PCI_STATUS_CAP_LIST: u16 = 1 << 4;
const PCI_CAP_ID_MSI: u8 = 0x05;
const PCI_CAP_ID_MSIX: u8 = 0x11;
const PCI_MSI_CONTROL_ENABLE: u16 = 1 << 0;
const PCI_MSIX_CONTROL_MASKALL: u16 = 1 << 14;
const PCI_MSIX_CONTROL_ENABLE: u16 = 1 << 15;

fn pci_cfg_addr(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    0x8000_0000u32
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xfc)
}

fn pci_cfg_read32(
    io_desc: Word,
    bus: u8,
    dev: u8,
    func: u8,
    offset: u8,
) -> Result<u32, RequestError> {
    let addr = pci_cfg_addr(bus, dev, func, offset);
    libnanami::io::io_write(io_desc, PCI_CFG_ADDR_PORT, 4, addr as Word)?;
    let data = libnanami::io::io_read(io_desc, PCI_CFG_DATA_PORT, 4)? as u32;
    Ok(data)
}

fn pci_cfg_read16(
    io_desc: Word,
    bus: u8,
    dev: u8,
    func: u8,
    offset: u8,
) -> Result<u16, RequestError> {
    let value = pci_cfg_read32(io_desc, bus, dev, func, offset)?;
    let shift = ((offset & 0x2) * 8) as u32;
    Ok(((value >> shift) & 0xffff) as u16)
}

fn pci_cfg_read8(
    io_desc: Word,
    bus: u8,
    dev: u8,
    func: u8,
    offset: u8,
) -> Result<u8, RequestError> {
    let value = pci_cfg_read32(io_desc, bus, dev, func, offset)?;
    let shift = ((offset & 0x3) * 8) as u32;
    Ok(((value >> shift) & 0xff) as u8)
}

fn pci_cfg_write32(
    io_desc: Word,
    bus: u8,
    dev: u8,
    func: u8,
    offset: u8,
    value: u32,
) -> Result<(), RequestError> {
    let addr = pci_cfg_addr(bus, dev, func, offset);
    libnanami::io::io_write(io_desc, PCI_CFG_ADDR_PORT, 4, addr as Word)?;
    libnanami::io::io_write(io_desc, PCI_CFG_DATA_PORT, 4, value as Word)
}

fn pci_cfg_write16(
    io_desc: Word,
    bus: u8,
    dev: u8,
    func: u8,
    offset: u8,
    value: u16,
) -> Result<(), RequestError> {
    let aligned = offset & 0xfc;
    let old = pci_cfg_read32(io_desc, bus, dev, func, aligned)?;
    let shift = ((offset & 0x2) * 8) as u32;
    let mask = !(0xffffu32 << shift);
    let next = (old & mask) | ((value as u32) << shift);
    pci_cfg_write32(io_desc, bus, dev, func, aligned, next)
}

pub(crate) fn configure_pci_command_for_intx(
    io_desc: Word,
    found: VirtioPciDevice,
) -> Result<(), RequestError> {
    let cmd_before = pci_cfg_read16(
        io_desc,
        found.bus,
        found.dev,
        found.func,
        PCI_CFG_COMMAND_OFFSET,
    )?;
    let int_pin = pci_cfg_read8(
        io_desc,
        found.bus,
        found.dev,
        found.func,
        PCI_CFG_INTERRUPT_PIN_OFFSET,
    )?;

    libnanami::print!("[virtio-net] pci cmd before=");
    libnanami::print!("{:#x}", cmd_before);
    libnanami::print!(" int-pin=");
    libnanami::print!("{}", int_pin as usize);
    libnanami::print!("\n");

    let mut cmd_after = cmd_before | PCI_COMMAND_IO_SPACE | PCI_COMMAND_BUS_MASTER;
    cmd_after &= !PCI_COMMAND_INTX_DISABLE;

    if cmd_after != cmd_before {
        pci_cfg_write16(
            io_desc,
            found.bus,
            found.dev,
            found.func,
            PCI_CFG_COMMAND_OFFSET,
            cmd_after,
        )?;
    }

    let cmd_verify = pci_cfg_read16(
        io_desc,
        found.bus,
        found.dev,
        found.func,
        PCI_CFG_COMMAND_OFFSET,
    )?;
    libnanami::print!("[virtio-net] pci cmd after =");
    libnanami::print!("{:#x}", cmd_verify);
    libnanami::print!("\n");
    Ok(())
}

pub(crate) fn disable_pci_msi_capabilities(
    io_desc: Word,
    found: VirtioPciDevice,
) -> Result<(), RequestError> {
    let status = pci_cfg_read16(
        io_desc,
        found.bus,
        found.dev,
        found.func,
        PCI_CFG_STATUS_OFFSET,
    )?;
    if (status & PCI_STATUS_CAP_LIST) == 0 {
        libnanami::print!("[virtio-net] pci caps: none\n");
        return Ok(());
    }

    let mut cap = pci_cfg_read8(
        io_desc,
        found.bus,
        found.dev,
        found.func,
        PCI_CFG_CAP_PTR_OFFSET,
    )? & 0xfc;
    let mut guard = 0usize;

    while cap >= 0x40 && guard < 48 {
        guard += 1;
        let cap_id = pci_cfg_read8(io_desc, found.bus, found.dev, found.func, cap)?;
        let cap_next = pci_cfg_read8(io_desc, found.bus, found.dev, found.func, cap + 1)? & 0xfc;

        if cap_id == PCI_CAP_ID_MSI {
            let ctrl = pci_cfg_read16(io_desc, found.bus, found.dev, found.func, cap + 2)?;
            let next = ctrl & !PCI_MSI_CONTROL_ENABLE;
            if next != ctrl {
                pci_cfg_write16(io_desc, found.bus, found.dev, found.func, cap + 2, next)?;
            }
            libnanami::print!("[virtio-net] pci msi ctrl=");
            libnanami::print!("{:#x}", ctrl);
            libnanami::print!(" -> ");
            libnanami::print!("{:#x}", next);
            libnanami::print!("\n");
        } else if cap_id == PCI_CAP_ID_MSIX {
            let ctrl = pci_cfg_read16(io_desc, found.bus, found.dev, found.func, cap + 2)?;
            let next = (ctrl & !PCI_MSIX_CONTROL_ENABLE) & !PCI_MSIX_CONTROL_MASKALL;
            if next != ctrl {
                pci_cfg_write16(io_desc, found.bus, found.dev, found.func, cap + 2, next)?;
            }
            libnanami::print!("[virtio-net] pci msix ctrl=");
            libnanami::print!("{:#x}", ctrl);
            libnanami::print!(" -> ");
            libnanami::print!("{:#x}", next);
            libnanami::print!("\n");
        }

        if cap_next == 0 || cap_next == cap {
            break;
        }
        cap = cap_next;
    }

    Ok(())
}

fn probe_virtio_at(
    io_desc: Word,
    bus: u8,
    dev: u8,
    func: u8,
) -> Result<Option<VirtioPciDevice>, RequestError> {
    let vendor_id = pci_cfg_read16(io_desc, bus, dev, func, 0x00)?;
    if vendor_id == 0xffff {
        return Ok(None);
    }
    let device_id = pci_cfg_read16(io_desc, bus, dev, func, 0x02)?;
    if vendor_id != VIRTIO_VENDOR_ID
        || !(device_id == VIRTIO_NET_DEVICE_ID_LEGACY || device_id == VIRTIO_NET_DEVICE_ID_MODERN)
    {
        return Ok(None);
    }

    let bar0 = pci_cfg_read32(io_desc, bus, dev, func, 0x10)?;
    if (bar0 & 0x1) == 0 {
        return Ok(None);
    }
    let io_base = (bar0 & 0xfffc) as u16;
    let irq_line = pci_cfg_read8(io_desc, bus, dev, func, PCI_CFG_INTERRUPT_LINE_OFFSET)?;
    let irq_pin = pci_cfg_read8(io_desc, bus, dev, func, PCI_CFG_INTERRUPT_PIN_OFFSET)?;
    Ok(Some(VirtioPciDevice {
        bus,
        dev,
        func,
        vendor_id,
        device_id,
        io_base,
        irq_line,
        irq_pin,
    }))
}

fn read_piix3_pirq_route(io_desc: Word, pirq_index: u8) -> Result<Option<u8>, RequestError> {
    if pirq_index >= 4 {
        return Ok(None);
    }

    let reg = PIIX3_PIRQ_ROUTE_BASE + pirq_index;
    let raw = pci_cfg_read8(io_desc, PIIX3_BUS, PIIX3_DEV, PIIX3_FUNC, reg)?;
    if (raw & 0x80) != 0 {
        return Ok(None);
    }

    let irq = raw & 0x0f;
    if irq == 0 {
        return Ok(None);
    }

    Ok(Some(irq))
}

pub(crate) fn resolve_irq_number(
    io_desc: Word,
    found: VirtioPciDevice,
) -> Result<Option<Word>, RequestError> {
    let line_irq = if found.irq_line != 0 && found.irq_line != 0xff {
        Some(found.irq_line as Word)
    } else {
        None
    };

    if found.irq_pin >= 1 && found.irq_pin <= 4 {
        let slot_addend = found.dev.wrapping_sub(1) & 0x03;
        let pci_intx = (found.irq_pin - 1) & 0x03;
        let pirq_index = (pci_intx + slot_addend) & 0x03;

        if let Some(routed_irq) = read_piix3_pirq_route(io_desc, pirq_index)? {
            libnanami::print!("[virtio-net] irq routing: PIRQ");
            libnanami::print!("{}", pirq_index as usize);
            libnanami::print!(" -> irq=");
            libnanami::print!("{}", routed_irq as usize);
            if let Some(line) = line_irq {
                libnanami::print!(" (line=");
                libnanami::print!("{}", line as usize);
                libnanami::print!(")");
            }
            libnanami::print!("\n");
            return Ok(Some(routed_irq as Word));
        }
    }

    Ok(line_irq)
}

pub(crate) fn scan_virtio_net(io_desc: Word) -> Result<VirtioPciDevice, RequestError> {
    let known = [(0u8, 3u8, 0u8), (0u8, 2u8, 0u8), (0u8, 4u8, 0u8)];
    let mut i = 0usize;
    while i < known.len() {
        let (b, d, f) = known[i];
        if let Some(v) = probe_virtio_at(io_desc, b, d, f)? {
            return Ok(v);
        }
        i += 1;
    }

    let bus = 0u8;
    let mut dev = 0u8;
    while dev < 32 {
        let mut func = 0u8;
        while func < 8 {
            if let Some(v) = probe_virtio_at(io_desc, bus, dev, func)? {
                return Ok(v);
            }

            if func == 0 {
                let vendor_id = pci_cfg_read16(io_desc, bus, dev, func, 0x00)?;
                if vendor_id == 0xffff {
                    break;
                }
                let header_type = pci_cfg_read8(io_desc, bus, dev, func, 0x0e)?;
                if (header_type & 0x80) == 0 {
                    break;
                }
            }

            func += 1;
        }
        dev += 1;
    }

    Err(RequestError::Unsupported)
}
