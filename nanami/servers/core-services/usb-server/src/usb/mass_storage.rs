//! Wire formats for SCSI transparent / Bulk-Only Transport (not UAS).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BulkEndpoint {
    pub address: u8,
    pub packet: u16,
    pub burst: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interface {
    pub number: u8,
    pub input: BulkEndpoint,
    pub output: BulkEndpoint,
}

/// Accept one alternate-zero BOT interface with exactly one bulk endpoint in
/// each direction. SuperSpeed endpoints must carry their companion descriptor.
pub fn interface(data: &[u8], superspeed: bool) -> Result<Option<Interface>, ()> {
    if data.len() < 9 || data[0] < 9 || data[1] != 2 || data[5] == 0 {
        return Err(());
    }
    let total = u16::from_le_bytes([data[2], data[3]]) as usize;
    if total > data.len() || total < data[0] as usize {
        return Err(());
    }
    let mut result = None;
    let mut active = None;
    let mut endpoints = [None; 2];
    let mut offset = data[0] as usize;
    while offset <= total {
        let end = offset == total;
        if !end
            && (total - offset < 2 || data[offset] < 2 || data[offset] as usize > total - offset)
        {
            return Err(());
        }
        let descriptor = if end {
            &[][..]
        } else {
            &data[offset..offset + data[offset] as usize]
        };
        if end || descriptor[1] == 4 {
            if let Some(number) = active.take() {
                let [Some(output), Some(input)] = endpoints else {
                    return Err(());
                };
                if result
                    .replace(Interface {
                        number,
                        input,
                        output,
                    })
                    .is_some()
                {
                    return Err(());
                }
            }
            endpoints = [None; 2];
            if end {
                break;
            }
            if descriptor.len() < 9 {
                return Err(());
            }
            if descriptor[3] == 0 && descriptor[5..8] == [8, 6, 0x50] {
                if descriptor[4] != 2 {
                    return Err(());
                }
                active = Some(descriptor[2]);
            }
        } else if descriptor[1] == 5 && active.is_some() {
            if descriptor.len() < 7
                || descriptor[2] & 0x70 != 0
                || descriptor[2] & 15 == 0
                || descriptor[3] != 2
            {
                return Err(());
            }
            let packet = u16::from_le_bytes([descriptor[4], descriptor[5]]);
            if !matches!(packet, 8 | 16 | 32 | 64 | 512 | 1024) {
                return Err(());
            }
            let mut burst = 0;
            if superspeed {
                let next = offset + descriptor.len();
                let companion = data.get(next..next + 6).ok_or(())?;
                // Streams belong to UAS, not BOT. No periodic bandwidth here.
                if next + 6 > total
                    || companion[0] != 6
                    || companion[1] != 48
                    || companion[2] > 15
                    || companion[3..6] != [0, 0, 0]
                {
                    return Err(());
                }
                burst = companion[2];
            }
            let direction = (descriptor[2] >> 7) as usize;
            if endpoints[direction]
                .replace(BulkEndpoint {
                    address: descriptor[2],
                    packet,
                    burst,
                })
                .is_some()
            {
                return Err(());
            }
        }
        offset += descriptor.len();
    }
    Ok(result)
}

pub fn cbw(tag: u32, lun: u8, cdb: &[u8], bytes: usize, receive: bool) -> Result<[u8; 31], ()> {
    if lun > 15 || cdb.is_empty() || cdb.len() > 16 || bytes > u32::MAX as usize {
        return Err(());
    }
    let mut wire = [0; 31];
    wire[..4].copy_from_slice(&0x4342_5355u32.to_le_bytes());
    wire[4..8].copy_from_slice(&tag.to_le_bytes());
    wire[8..12].copy_from_slice(&(bytes as u32).to_le_bytes());
    wire[12] = if receive { 0x80 } else { 0 };
    wire[13] = lun;
    wire[14] = cdb.len() as u8;
    wire[15..15 + cdb.len()].copy_from_slice(cdb);
    Ok(wire)
}

/// A failed SCSI command is distinct from a BOT phase/protocol error.
pub fn csw(
    wire: &[u8],
    tag: u32,
    requested: usize,
    transferred: usize,
) -> Result<(bool, usize), ()> {
    if wire.len() != 13
        || wire[..4] != 0x5342_5355u32.to_le_bytes()
        || wire[4..8] != tag.to_le_bytes()
        || transferred > requested
    {
        return Err(());
    }
    let residue = u32::from_le_bytes(wire[8..12].try_into().unwrap()) as usize;
    if residue > requested || (wire[12] == 0 && requested - residue > transferred) {
        return Err(());
    }
    match wire[12] {
        0 => Ok((true, requested - residue)),
        1 => Ok((false, 0)),
        _ => Err(()),
    }
}

/// Use READ/WRITE(10) for ordinary flash media, (16) above its LBA limit.
pub fn read_write(lba: u64, sectors: u32, write: bool) -> Result<([u8; 16], usize), ()> {
    let last = lba
        .checked_add(sectors.checked_sub(1).ok_or(())? as u64)
        .ok_or(())?;
    let mut cdb = [0; 16];
    if last <= u32::MAX as u64 && sectors <= u16::MAX as u32 {
        cdb[0] = if write { 0x2a } else { 0x28 };
        cdb[2..6].copy_from_slice(&(lba as u32).to_be_bytes());
        cdb[7..9].copy_from_slice(&(sectors as u16).to_be_bytes());
        Ok((cdb, 10))
    } else {
        cdb[0] = if write { 0x8a } else { 0x88 };
        cdb[2..10].copy_from_slice(&lba.to_be_bytes());
        cdb[10..14].copy_from_slice(&sectors.to_be_bytes());
        Ok((cdb, 16))
    }
}

/// Convert a bounded block-service request to disk sectors, without ever
/// exposing the GPT/ESP or an adjacent partition to its client.
pub fn block_range(first: u64, sectors: u64, block: usize, count: usize) -> Option<(u64, usize)> {
    let relative = (block as u64).checked_mul(2)?;
    let length = (count as u64).checked_mul(2)?;
    let bytes = count.checked_mul(1024)?;
    if count == 0 || bytes > 0x4000 || relative.checked_add(length)? > sectors {
        return None;
    }
    Some((first.checked_add(relative)?, bytes))
}
