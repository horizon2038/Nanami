//! Read-only Intel ACPI OpRegion/VBT wiring. No SCI/mailbox ownership changes.
use nanami_runtime::RequestError;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Wiring {
    pub ddc_pin: u8,
    pub aux: Option<u8>,
}
fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes(slice.try_into().ok()?))
}
fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes(slice.try_into().ok()?))
}
fn u64_at(bytes: &[u8], offset: usize) -> Option<u64> {
    let slice = bytes.get(offset..offset.checked_add(8)?)?;
    Some(u64::from_le_bytes(slice.try_into().ok()?))
}

/// OpRegion 2.0 uses an absolute RVDA; 2.1+ uses a base-relative address.
pub fn extended_vbt(
    opregion: &[u8],
    physical: usize,
) -> Result<Option<(usize, usize)>, RequestError> {
    if opregion.len() < 8192 || &opregion[..16] != b"IntelGraphicsMem" {
        return Err(RequestError::Protocol);
    }
    let size_kb = u32_at(opregion, 16).ok_or(RequestError::Protocol)?;
    if !(8..=64).contains(&size_kb) {
        return Err(RequestError::Protocol);
    }
    let major = opregion[23];
    let minor = opregion[22];
    let mailboxes = u32_at(opregion, 88).ok_or(RequestError::Protocol)?;
    if major < 2 || mailboxes & 4 == 0 {
        return Ok(None);
    }
    let address = u64_at(opregion, 0x3ba).ok_or(RequestError::Protocol)? as usize;
    let len = u32_at(opregion, 0x3c2).ok_or(RequestError::Protocol)? as usize;
    if address == 0 || len == 0 {
        return Ok(None);
    }
    if len > 65536 {
        return Err(RequestError::Protocol);
    }
    let address = if major > 2 || minor >= 1 {
        if address < size_kb as usize * 1024 {
            return Err(RequestError::Protocol);
        }
        physical
            .checked_add(address)
            .ok_or(RequestError::Protocol)?
    } else {
        address
    };
    address.checked_add(len).ok_or(RequestError::Protocol)?;
    Ok(Some((address, len)))
}

pub fn embedded_vbt(opregion: &[u8]) -> Result<&[u8], RequestError> {
    if opregion.len() < 8192
        || &opregion[..16] != b"IntelGraphicsMem"
        || !(8..=64).contains(&u32_at(opregion, 16).ok_or(RequestError::Protocol)?)
    {
        return Err(RequestError::Protocol);
    }
    let mailboxes = u32_at(opregion, 88).ok_or(RequestError::Protocol)?;
    let end = if mailboxes & (1 << 4) != 0 {
        0x1c00
    } else {
        8192
    };
    opregion.get(0x400..end).ok_or(RequestError::Protocol)
}

pub fn wiring(vbt: &[u8], port: u8) -> Result<Option<Wiring>, RequestError> {
    if vbt.get(..4) != Some(b"$VBT") {
        return Err(RequestError::Protocol);
    }
    let header = u16_at(vbt, 22).ok_or(RequestError::Protocol)? as usize;
    let len = u16_at(vbt, 24).ok_or(RequestError::Protocol)? as usize;
    if header < 48 || header > len || len > vbt.len() {
        return Err(RequestError::Protocol);
    }
    let vbt = &vbt[..len];
    if vbt.iter().fold(0u8, |sum, value| sum.wrapping_add(*value)) != 0 {
        return Err(RequestError::Protocol);
    }
    let bdb = u32_at(vbt, 28).ok_or(RequestError::Protocol)? as usize;
    if bdb < header {
        return Err(RequestError::Protocol);
    }
    let bdb = vbt.get(bdb..).ok_or(RequestError::Protocol)?;
    if bdb.get(..15) != Some(b"BIOS_DATA_BLOCK") {
        return Err(RequestError::Protocol);
    }
    let version = u16_at(bdb, 16).ok_or(RequestError::Protocol)?;
    let header = u16_at(bdb, 18).ok_or(RequestError::Protocol)? as usize;
    let size = u16_at(bdb, 20).ok_or(RequestError::Protocol)? as usize;
    if version < 158 || header < 22 || header > size || size > bdb.len() {
        return Err(RequestError::Unsupported);
    }
    let bdb = &bdb[..size];
    let mut offset = header;
    while offset < size {
        let id = bdb[offset];
        let len = u16_at(bdb, offset + 1).ok_or(RequestError::Protocol)? as usize;
        offset = offset.checked_add(3).ok_or(RequestError::Protocol)?;
        let block = bdb
            .get(offset..offset.checked_add(len).ok_or(RequestError::Protocol)?)
            .ok_or(RequestError::Protocol)?;
        if id == 2 {
            if block.len() < 5 {
                return Err(RequestError::Protocol);
            }
            let stride = block[4] as usize;
            if stride < 33 || (block.len() - 5) % stride != 0 {
                return Err(RequestError::Protocol);
            }
            let mut result: Option<Wiring> = None;
            for child in block[5..].chunks_exact(stride) {
                let child_port = match child[16] {
                    0 | 10 => 0,
                    1 | 7 => 1,
                    2 | 8 => 2,
                    3 | 9 => 3,
                    11 | 12 => 4,
                    _ => continue,
                };
                if child_port != port || u16_at(child, 2) == Some(0) {
                    continue;
                }
                let aux = match child[25] {
                    0 => None,
                    0x40 => Some(0),
                    0x10 => Some(1),
                    0x20 => Some(2),
                    0x30 => Some(3),
                    0x50 => Some(4),
                    _ => return Err(RequestError::Unsupported),
                };
                let mut candidate = Wiring {
                    ddc_pin: child[19],
                    aux,
                };
                // HDMI and DP child records may describe the same physical
                // DDI. Merge unspecified fields, but never choose arbitrarily
                // between two explicit, conflicting board routes.
                if let Some(previous) = result {
                    if previous.ddc_pin != 0
                        && candidate.ddc_pin != 0
                        && previous.ddc_pin != candidate.ddc_pin
                    {
                        return Err(RequestError::Protocol);
                    }
                    if previous.aux.is_some()
                        && candidate.aux.is_some()
                        && previous.aux != candidate.aux
                    {
                        return Err(RequestError::Protocol);
                    }
                    if candidate.ddc_pin == 0 {
                        candidate.ddc_pin = previous.ddc_pin;
                    }
                    candidate.aux = candidate.aux.or(previous.aux);
                }
                result = Some(candidate);
            }
            return Ok(result);
        }
        offset += len;
    }
    Ok(None)
}
