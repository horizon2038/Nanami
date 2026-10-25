use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};
pub use nanami_uapi::pci::*;

pub fn connect(slot: Word) -> Result<Word, RequestError> {
    // Registration happens asynchronously after driver-manager spawns PCI.
    // PCI runs at boot priority 31, including on a single CPU.
    crate::registry::connect_boot_service(PCI_SERVICE, slot)?;
    Ok(nanami_runtime::ipc::process_slot_descriptor(slot))
}

pub fn enumerate(port: Word, index: Word) -> Result<Option<Device>, RequestError> {
    let (status, address, info) = call_port(port, PCI_REQUEST_ENUMERATE, index, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if address == PCI_ENUM_END {
        return Ok(None);
    }
    Device::from_words(address, info)
        .map(Some)
        .ok_or(RequestError::Protocol)
}

/// Fetch UTF-8 diagnostic text without embedding the ID database in clients.
/// Unknown IDs return an empty string; callers should still display numeric IDs.
pub fn name<'a>(
    port: Word,
    index: Word,
    field: Word,
    buffer: &'a mut [u8],
) -> Result<&'a str, RequestError> {
    if field > PCI_NAME_INTERFACE {
        return Err(RequestError::InvalidArgument);
    }
    let mut offset = 0;
    let mut expected = None;
    loop {
        let (status, total, chunk) = call_port(port, PCI_REQUEST_NAME, index, field, offset, 0, 4)?;
        if status != OS_RESPONSE_OK {
            return Err(RequestError::Status(status));
        }
        if total > buffer.len()
            || expected.is_some_and(|expected| expected != total)
            || offset > total
        {
            return Err(RequestError::Protocol);
        }
        expected = Some(total);
        let count = (total - offset).min(PCI_NAME_CHUNK_BYTES);
        buffer[offset..offset + count].copy_from_slice(&(chunk as u64).to_le_bytes()[..count]);
        offset += count;
        if offset == total {
            return core::str::from_utf8(&buffer[..total]).map_err(|_| RequestError::Protocol);
        }
    }
}

pub fn read(port: Word, address: Address, offset: Word, width: Word) -> Result<u32, RequestError> {
    if !valid_register(address, offset, width) {
        return Err(RequestError::InvalidArgument);
    }
    let (status, value, _) = call_port(
        port,
        PCI_REQUEST_READ,
        address.packed(),
        offset,
        width,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(value as u32)
}

pub fn write(
    port: Word,
    address: Address,
    offset: Word,
    width: Word,
    value: u32,
) -> Result<(), RequestError> {
    if !valid_register(address, offset, width) {
        return Err(RequestError::InvalidArgument);
    }
    let (status, _, _) = call_port(
        port,
        PCI_REQUEST_WRITE,
        address.packed(),
        offset | (width << 16),
        value as Word,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

/// The owner must quiesce the device before probing: address decoding is briefly
/// disabled. The complete probe/restore sequence is one server transaction.
pub fn probe_bar(port: Word, address: Address, index: Word) -> Result<Bar, RequestError> {
    let (status, physical, flags) = call_port(
        port,
        PCI_REQUEST_PROBE_BAR,
        address.packed(),
        index,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    let bytes = flags & !3;
    if !bytes.is_power_of_two() || physical == 0 || physical & (bytes - 1) != 0 {
        return Err(RequestError::Protocol);
    }
    Ok(Bar {
        physical,
        bytes,
        io: flags & PCI_BAR_IO != 0,
        prefetchable: flags & PCI_BAR_PREFETCHABLE != 0,
    })
}

/// Current drivers use INTx. Preserve unrelated capability controls and never
/// write PCI status RW1C bits while disabling MSI/MSI-X.
pub fn disable_message_interrupts(port: Word, address: Address) -> Result<(), RequestError> {
    if read(port, address, 6, 2)? & (1 << 4) == 0 {
        return Ok(());
    }
    let mut cap = read(port, address, 0x34, 1)? as Word & !3;
    let mut seen = [false; 64];
    for _ in 0..48 {
        if cap == 0 {
            return Ok(());
        }
        if cap < 0x40 || seen[cap / 4] {
            return Err(RequestError::Protocol);
        }
        seen[cap / 4] = true;
        let value = read(port, address, cap, 4)?;
        let control = (value >> 16) as u16;
        let next = match value as u8 {
            5 => control & !1,
            0x11 => (control & !(1 << 15)) | (1 << 14),
            _ => control,
        };
        if next != control {
            write(port, address, cap + 2, 2, next as u32)?;
        }
        cap = ((value >> 8) as u8 & !3) as Word;
    }
    Err(RequestError::Protocol)
}
