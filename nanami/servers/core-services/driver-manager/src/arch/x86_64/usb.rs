//! PCI discovery/configuration is serialized by pci-server.
use nanami_client::{
    device::UsbControllerResource,
    pci::{self, Address},
};
use nanami_runtime::{RequestError, Word};

pub fn prepare(bdf: Word) -> Result<(UsbControllerResource, u8), RequestError> {
    let address = Address::from_packed(bdf).ok_or(RequestError::InvalidArgument)?;
    let port = nanami_runtime::ipc::process_slot_descriptor(super::SLOT_PCI_SERVICE);
    if pci::read(port, address, 8, 4)? >> 8 != 0x0c0330 {
        return Err(RequestError::Unsupported);
    }
    let bar = pci::probe_bar(port, address, 0)?;
    if bar.io || bar.bytes < 0x1000 || bar.bytes > 0x100000 {
        return Err(RequestError::Unsupported);
    }
    let command = pci::read(port, address, 4, 2)? as u16;
    pci::disable_message_interrupts(port, address)?;
    let interrupt = pci::read(port, address, 0x3c, 4)?;
    let line = (interrupt & 0xff) as Word;
    let pin = (interrupt >> 8) as u8;
    let irq = ((1..=4).contains(&pin)
        && line != 0
        && line < 24
        && option_env!("NANAMI_USB_ACPI_IRQ") != Some("1"))
    .then_some(line);
    pci::write(port, address, 4, 2, ((command | 6) & !(1 << 10)) as u32)?;
    nanami_runtime::println!(
        "[driver-manager] USB PCI BAR={:#x} bytes={:#x} INTx={}",
        bar.physical,
        bar.bytes,
        line
    );
    Ok((
        UsbControllerResource {
            physical: bar.physical,
            bytes: bar.bytes,
            irq,
        },
        pin,
    ))
}
