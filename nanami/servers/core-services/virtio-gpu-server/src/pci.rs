//! Modern PCI capability discovery; no hard-coded BAR or notification offsets.
use crate::queue::Transport;
use core::ptr;
use nanami_client::pci::{self, Address, Bar, Device};
use nanami_runtime::{RequestError, Word};

#[derive(Clone, Copy, Debug, Default)]
pub struct Region {
    pub bar: Word,
    pub offset: Word,
    pub bytes: Word,
}
#[derive(Clone, Copy, Debug)]
pub struct Capabilities {
    pub common: Region,
    pub notify: Region,
    pub config: Region,
    pub multiplier: Word,
}
pub fn matches(device: Device) -> bool {
    device.vendor_id == 0x1af4 && device.device_id == 0x1050 && device.class >> 16 == 3
}
pub fn capabilities(port: Word, address: Address) -> Result<Capabilities, RequestError> {
    if pci::read(port, address, 6, 2)? & (1 << 4) == 0 {
        return Err(RequestError::Unsupported);
    }
    let mut common = None;
    let mut notify = None;
    let mut config = None;
    let mut multiplier = 0;
    let mut visited = [false; 64];
    let mut offset = pci::read(port, address, 0x34, 1)? as Word;
    for _ in 0..48 {
        if offset == 0 {
            return Ok(Capabilities {
                common: common.ok_or(RequestError::Unsupported)?,
                notify: notify.ok_or(RequestError::Unsupported)?,
                config: config.ok_or(RequestError::Unsupported)?,
                multiplier,
            });
        }
        if offset < 0x40 || offset > 0xfc || offset & 3 != 0 || visited[offset / 4] {
            return Err(RequestError::Protocol);
        }
        visited[offset / 4] = true;
        let header = pci::read(port, address, offset, 4)?;
        if header & 0xff == 9 {
            let length = ((header >> 16) & 0xff) as Word;
            let kind = header >> 24;
            if matches!(kind, 1 | 2 | 4) {
                if length < 16 || offset + length > 256 || (kind == 2 && length < 20) {
                    return Err(RequestError::Protocol);
                }
                let bar = (pci::read(port, address, offset + 4, 4)? & 0xff) as Word;
                if bar < 6 {
                    let region = Region {
                        bar,
                        offset: pci::read(port, address, offset + 8, 4)? as Word,
                        bytes: pci::read(port, address, offset + 12, 4)? as Word,
                    };
                    match kind {
                        1 if common.is_none() => common = Some(region),
                        2 if notify.is_none() => {
                            notify = Some(region);
                            multiplier = pci::read(port, address, offset + 16, 4)? as Word;
                        }
                        4 if config.is_none() => config = Some(region),
                        _ => {}
                    }
                }
            }
        }
        offset = ((header >> 8) & 0xff) as Word;
    }
    Err(RequestError::Protocol)
}

fn map_region(
    port: Word,
    device: Device,
    region: Region,
    needed: Word,
    bars: &mut [Option<Bar>; 6],
) -> Result<Word, RequestError> {
    if region.bar >= 6 || region.offset & 3 != 0 || region.bytes < needed {
        return Err(RequestError::Protocol);
    }
    let bar = match bars[region.bar] {
        Some(bar) => bar,
        None => {
            let bar = pci::probe_bar(port, device.address, region.bar)?;
            bars[region.bar] = Some(bar);
            bar
        }
    };
    if bar.io
        || region
            .offset
            .checked_add(region.bytes)
            .is_none_or(|end| end > bar.bytes)
    {
        return Err(RequestError::Protocol);
    }
    let physical = bar
        .physical
        .checked_add(region.offset)
        .ok_or(RequestError::Protocol)?;
    let page_offset = physical & 4095;
    Ok(nanami_runtime::request_mmio(physical - page_offset, needed + page_offset)?.1 + page_offset)
}

pub struct Pci {
    common: Word,
    notify: Word,
    notify_bytes: Word,
    multiplier: Word,
}
impl Pci {
    pub fn new(port: Word, device: Device) -> Result<Self, RequestError> {
        if !matches(device) {
            return Err(RequestError::Unsupported);
        }
        let caps = capabilities(port, device.address)?;
        let mut bars = [None; 6];
        let common = map_region(port, device, caps.common, 56, &mut bars)?;
        // Bound MMIO reservation even if a device supplies a huge capability.
        // Offsets outside this mapped notification window are rejected.
        let notify_bytes = caps.notify.bytes.min(65536);
        let notify = map_region(port, device, caps.notify, notify_bytes.max(2), &mut bars)?;
        let config = map_region(port, device, caps.config, 16, &mut bars)?;
        if caps.multiplier != 0 && (!caps.multiplier.is_power_of_two() || caps.multiplier & 1 != 0)
        {
            return Err(RequestError::Protocol);
        }
        pci::disable_message_interrupts(port, device.address)?;
        let command = pci::read(port, device.address, 4, 2)?;
        pci::write(port, device.address, 4, 2, command | 6 | (1 << 10))?;
        // A fixed single scanout is selected from GET_DISPLAY_INFO later.
        let count = unsafe { ptr::read_volatile((config + 8) as *const u32) };
        if count == 0 || count > 16 {
            return Err(RequestError::Unsupported);
        }
        Ok(Self {
            common,
            notify,
            notify_bytes,
            multiplier: caps.multiplier,
        })
    }
}
impl Transport for Pci {
    fn read(&self, offset: Word, width: Word) -> u32 {
        assert!(offset + width <= 56 && offset % width == 0);
        unsafe {
            match width {
                1 => ptr::read_volatile((self.common + offset) as *const u8) as u32,
                2 => ptr::read_volatile((self.common + offset) as *const u16) as u32,
                4 => ptr::read_volatile((self.common + offset) as *const u32),
                _ => panic!("invalid virtio register width"),
            }
        }
    }
    fn write(&mut self, offset: Word, width: Word, value: u32) {
        assert!(offset + width <= 56 && offset % width == 0);
        unsafe {
            match width {
                1 => ptr::write_volatile((self.common + offset) as *mut u8, value as u8),
                2 => ptr::write_volatile((self.common + offset) as *mut u16, value as u16),
                4 => ptr::write_volatile((self.common + offset) as *mut u32, value),
                _ => panic!("invalid virtio register width"),
            }
        }
    }
    fn notify_offset(&self, queue_offset: u32) -> Result<Word, RequestError> {
        let offset = (queue_offset as Word)
            .checked_mul(self.multiplier)
            .ok_or(RequestError::Protocol)?;
        if offset
            .checked_add(2)
            .is_none_or(|end| end > self.notify_bytes)
            || offset & 1 != 0
        {
            return Err(RequestError::Protocol);
        }
        Ok(offset)
    }
    fn notify(&mut self, offset: Word) -> Result<(), RequestError> {
        if offset
            .checked_add(2)
            .is_none_or(|end| end > self.notify_bytes)
        {
            return Err(RequestError::Protocol);
        }
        unsafe {
            ptr::write_volatile((self.notify + offset) as *mut u16, 0);
        }
        Ok(())
    }
}
