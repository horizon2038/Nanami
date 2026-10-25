//! Bus discovery and BAR transactions, independent of the transport backend.
use alloc::vec::Vec;
use nanami_client::pci::{Address, Bar, Device};
use nanami_runtime::{RequestError, Word};

pub const MAX_DEVICES: usize = 512;
const COMMAND_IO: u32 = 1;
const COMMAND_MEMORY: u32 = 2;
const COMMAND_BUS_MASTER: u32 = 4;

pub trait Access {
    fn read(&mut self, address: Address, offset: Word, width: Word) -> Result<u32, RequestError>;
    fn write(
        &mut self,
        address: Address,
        offset: Word,
        width: Word,
        value: u32,
    ) -> Result<(), RequestError>;
}

pub fn scan(access: &mut impl Access) -> Result<Vec<Device>, RequestError> {
    let mut devices = Vec::new();
    let mut buses = [0u8; 256];
    let mut seen = [false; 256];
    seen[0] = true;
    let (mut head, mut tail) = (0, 1);
    while head < tail {
        let bus = buses[head];
        head += 1;
        for slot in 0..32 {
            let address = Address::new(bus, slot, 0);
            let first = access.read(address, 0, 4)?;
            if first as u16 == 0xffff {
                continue;
            }
            let header = (access.read(address, 0x0c, 4)? >> 16) as u8;
            let functions = if header & 0x80 != 0 { 8 } else { 1 };
            for function in 0..functions {
                let address = Address::new(bus, slot, function);
                let id = if function == 0 {
                    first
                } else {
                    access.read(address, 0, 4)?
                };
                if id as u16 == 0xffff {
                    continue;
                }
                if devices.len() == MAX_DEVICES {
                    return Err(RequestError::Unsupported);
                }
                let class = access.read(address, 8, 4)? >> 8;
                let header = (access.read(address, 0x0c, 4)? >> 16) as u8;
                devices
                    .try_reserve(1)
                    .map_err(|_| RequestError::Unsupported)?;
                devices.push(Device {
                    address,
                    vendor_id: id as u16,
                    device_id: (id >> 16) as u16,
                    class,
                    header,
                });
                if class >> 8 == 0x0604 {
                    let secondary = (access.read(address, 0x18, 4)? >> 8) as u8;
                    if secondary != 0 && !seen[secondary as usize] {
                        seen[secondary as usize] = true;
                        buses[tail] = secondary;
                        tail += 1;
                    }
                }
            }
        }
    }
    Ok(devices)
}

pub fn probe_bar(
    access: &mut impl Access,
    device: Device,
    index: Word,
) -> Result<Bar, RequestError> {
    let count = match device.header & 0x7f {
        0 => 6,
        1 => 2,
        _ => return Err(RequestError::Unsupported),
    };
    if index >= count {
        return Err(RequestError::InvalidArgument);
    }
    // Reject the high half of a preceding 64-bit BAR.
    let mut current = 0;
    while current < index {
        let low = access.read(device.address, 0x10 + current * 4, 4)?;
        current += if low & 7 == 4 { 2 } else { 1 };
    }
    if current != index {
        return Err(RequestError::InvalidArgument);
    }
    let offset = 0x10 + index * 4;
    let low = access.read(device.address, offset, 4)?;
    let io = low & 1 != 0;
    let wide = !io && low & 6 == 4;
    if (!io && !matches!(low & 6, 0 | 4)) || (wide && index + 1 == count) {
        return Err(RequestError::Unsupported);
    }
    let high = if wide {
        access.read(device.address, offset + 4, 4)?
    } else {
        0
    };
    let physical = if io {
        (low & !3) as u64
    } else {
        (low & !15) as u64 | ((high as u64) << 32)
    };
    if physical == 0 {
        return Err(RequestError::Unsupported);
    }
    let command = access.read(device.address, 4, 2)?;
    access.write(
        device.address,
        4,
        2,
        command & !(COMMAND_IO | COMMAND_MEMORY | COMMAND_BUS_MASTER),
    )?;
    let result = (|| {
        access.write(device.address, offset, 4, u32::MAX)?;
        if wide {
            access.write(device.address, offset + 4, 4, u32::MAX)?;
        }
        let mask_low = access.read(device.address, offset, 4)? & if io { !3 } else { !15 };
        let mask_high = if wide {
            access.read(device.address, offset + 4, 4)?
        } else {
            u32::MAX
        };
        let mask = ((mask_high as u64) << 32) | mask_low as u64;
        Ok::<u64, RequestError>((!mask).wrapping_add(1))
    })();
    // Attempt every restore even if an earlier restore failed. Never overwrite
    // adjacent W1C status flags by restoring command with a 32-bit store.
    let restore_low = access.write(device.address, offset, 4, low);
    let restore_high = if wide {
        access.write(device.address, offset + 4, 4, high)
    } else {
        Ok(())
    };
    // Keep decoding disabled if either BAR half could not be restored.
    restore_low?;
    restore_high?;
    access.write(device.address, 4, 2, command)?;
    let bytes = result?;
    if bytes < 4
        || !bytes.is_power_of_two()
        || physical & (bytes - 1) != 0
        || physical.checked_add(bytes).is_none()
    {
        return Err(RequestError::Protocol);
    }
    Ok(Bar {
        physical: physical as Word,
        bytes: bytes as Word,
        io,
        prefetchable: !io && low & 8 != 0,
    })
}
