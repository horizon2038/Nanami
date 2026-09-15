//! Configure PCI before spawning drivers: CF8/CFC is a global address latch,
//! not an SMP-safe per-process register. USB never accesses it at runtime.
use libnanami::{RequestError, Word};
use nanami_services::device::UsbControllerResource;

struct Pci {
    io: Word,
    address: u32,
}
impl Pci {
    fn select(&self, offset: u8) -> Result<(), RequestError> {
        libnanami::io::io_write(
            self.io,
            0xcf8,
            4,
            (self.address | (offset as u32 & !3)) as Word,
        )
    }
    fn read(&self, offset: u8) -> Result<u32, RequestError> {
        self.select(offset)?;
        Ok(libnanami::io::io_read(self.io, 0xcfc, 4)? as u32)
    }
    fn write(&self, offset: u8, value: u32) -> Result<(), RequestError> {
        self.select(offset)?;
        libnanami::io::io_write(self.io, 0xcfc, 4, value as Word)
    }
    fn write16(&self, offset: u8, value: u16) -> Result<(), RequestError> {
        self.select(offset)?;
        // Do not write adjacent PCI status RW1C bits back while updating command.
        libnanami::io::io_write(self.io, 0xcfc + (offset as Word & 2), 2, value as Word)
    }
}

pub fn prepare(bdf: Word) -> Result<UsbControllerResource, RequestError> {
    if bdf & !0x00ff_1f07 != 0 {
        return Err(RequestError::InvalidArgument);
    }
    let pci = Pci {
        io: libnanami::ipc::process_slot_descriptor(super::SLOT_PCI_CONFIG),
        address: 0x8000_0000
            | ((bdf as u32 >> 16) << 16)
            | (((bdf as u32 >> 8) & 31) << 11)
            | ((bdf as u32 & 7) << 8),
    };
    if pci.read(8)? >> 8 != 0x0c0330 {
        return Err(RequestError::Unsupported);
    }
    let low = pci.read(0x10)?;
    if low & 1 != 0 || !matches!(low & 6, 0 | 4) {
        return Err(RequestError::Unsupported);
    }
    let wide = low & 6 == 4;
    let high = if wide { pci.read(0x14)? } else { 0 };
    let physical = (low & !15) as u64 | ((high as u64) << 32);
    if physical == 0 {
        return Err(RequestError::Unsupported);
    }
    // Alpha currently materializes intervening MMIO capability chunks. A high
    // PCI hole can exhaust that directory; do not attempt the mapping or move
    // firmware-assigned BARs ourselves. Sparse high MMIO is separate work.
    if physical >= 0x1_0000_0000 {
        libnanami::println!(
            "[driver-manager] USB BAR {:#x} exceeds current low-MMIO support",
            physical
        );
        return Err(RequestError::Unsupported);
    }
    let command = pci.read(4)? as u16;
    pci.write16(4, command & !6)?;
    // Probe BAR size with decoding disabled; always restore the BAR on errors.
    let probe = (|| {
        pci.write(0x10, u32::MAX)?;
        if wide {
            pci.write(0x14, u32::MAX)?;
        }
        let lo = pci.read(0x10)? & !15;
        let hi = if wide { pci.read(0x14)? } else { u32::MAX };
        Ok::<u64, RequestError>((!((hi as u64) << 32 | lo as u64)).wrapping_add(1))
    })();
    pci.write(0x10, low)?;
    if wide {
        pci.write(0x14, high)?;
    }
    pci.write16(4, command)?;
    let bytes = probe?;
    if bytes < 0x1000
        || bytes > 0x100000
        || !bytes.is_power_of_two()
        || physical
            .checked_add(bytes)
            .is_none_or(|end| end > 0x1_0000_0000)
    {
        return Err(RequestError::Unsupported);
    }
    // The existing IRQ API supplies INTx, not MSI/MSI-X vectors. Disable both
    // message mechanisms, preserving the other capability control fields.
    if pci.read(4)? & (1 << 20) != 0 {
        let mut cap = (pci.read(0x34)? & 0xfc) as u8;
        for _ in 0..48 {
            if cap == 0 {
                break;
            }
            if cap < 0x40 {
                return Err(RequestError::Protocol);
            }
            let value = pci.read(cap)?;
            let control = (value >> 16) as u16;
            match value as u8 {
                5 => pci.write16(cap + 2, control & !1)?,
                0x11 => pci.write16(cap + 2, (control & !(1 << 15)) | (1 << 14))?,
                _ => {}
            }
            let next = ((value >> 8) & 0xfc) as u8;
            if next == cap {
                return Err(RequestError::Protocol);
            }
            cap = next;
        }
        if cap != 0 {
            return Err(RequestError::Protocol);
        }
    }
    let interrupt = pci.read(0x3c)?;
    let line = (interrupt & 0xff) as usize;
    let pin = (interrupt >> 8) as u8;
    let irq = (pin != 0 && line != 0 && line < 16).then_some(line);
    pci.write16(4, (command | 6) & !(1 << 10))?;
    libnanami::println!(
        "[driver-manager] USB PCI BAR={:#x} bytes={:#x} INTx={}",
        physical,
        bytes,
        line
    );
    Ok(UsbControllerResource {
        physical: physical as usize,
        bytes: bytes as usize,
        irq,
    })
}
