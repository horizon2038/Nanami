use super::arch::x86_64::*;
use super::*;

const PCI_COMMAND_IO_SPACE: u16 = 1 << 0;
const PCI_COMMAND_BUS_MASTER: u16 = 1 << 2;
const PCI_COMMAND_INTX_DISABLE: u16 = 1 << 10;
fn pci_cfg_read32(port: Word, bus: u8, dev: u8, func: u8, offset: u8) -> Result<u32, RequestError> {
    nanami_client::pci::read(
        port,
        nanami_client::pci::Address::new(bus, dev, func),
        offset as Word,
        4,
    )
}
fn pci_cfg_read16(port: Word, bus: u8, dev: u8, func: u8, offset: u8) -> Result<u16, RequestError> {
    Ok(nanami_client::pci::read(
        port,
        nanami_client::pci::Address::new(bus, dev, func),
        offset as Word,
        2,
    )? as u16)
}
fn pci_cfg_read8(port: Word, bus: u8, dev: u8, func: u8, offset: u8) -> Result<u8, RequestError> {
    Ok(nanami_client::pci::read(
        port,
        nanami_client::pci::Address::new(bus, dev, func),
        offset as Word,
        1,
    )? as u8)
}
fn pci_cfg_write16(
    port: Word,
    bus: u8,
    dev: u8,
    func: u8,
    offset: u8,
    value: u16,
) -> Result<(), RequestError> {
    nanami_client::pci::write(
        port,
        nanami_client::pci::Address::new(bus, dev, func),
        offset as Word,
        2,
        value as u32,
    )
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

    nanami_runtime::print!("[virtio-net] pci cmd before=");
    nanami_runtime::print!("{:#x}", cmd_before);
    nanami_runtime::print!(" int-pin=");
    nanami_runtime::print!("{}", int_pin as usize);
    nanami_runtime::print!("\n");

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
    nanami_runtime::print!("[virtio-net] pci cmd after =");
    nanami_runtime::print!("{:#x}", cmd_verify);
    nanami_runtime::print!("\n");
    Ok(())
}

pub(crate) fn disable_pci_msi_capabilities(
    port: Word,
    found: VirtioPciDevice,
) -> Result<(), RequestError> {
    nanami_client::pci::disable_message_interrupts(
        port,
        nanami_client::pci::Address::new(found.bus, found.dev, found.func),
    )
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
            nanami_runtime::print!("[virtio-net] irq routing: PIRQ");
            nanami_runtime::print!("{}", pirq_index as usize);
            nanami_runtime::print!(" -> irq=");
            nanami_runtime::print!("{}", routed_irq as usize);
            if let Some(line) = line_irq {
                nanami_runtime::print!(" (line=");
                nanami_runtime::print!("{}", line as usize);
                nanami_runtime::print!(")");
            }
            nanami_runtime::print!("\n");
            return Ok(Some(routed_irq as Word));
        }
    }

    Ok(line_irq)
}

pub(crate) fn scan_virtio_net(port: Word) -> Result<VirtioPciDevice, RequestError> {
    let mut index = 0;
    while let Some(device) = nanami_client::pci::enumerate(port, index)? {
        if device.address.segment == 0 && device.vendor_id == VIRTIO_VENDOR_ID {
            if let Some(found) = probe_virtio_at(
                port,
                device.address.bus,
                device.address.device,
                device.address.function,
            )? {
                return Ok(found);
            }
        }
        index += 1;
    }
    Err(RequestError::Unsupported)
}
