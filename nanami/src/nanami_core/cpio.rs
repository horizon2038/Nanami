use nun::CapabilityError;

const NEWC_HEADER_SIZE: usize = 110;

pub struct CpioEntry<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
}

pub fn for_each_newc_entry<'a, F>(archive: &'a [u8], mut f: F) -> Result<(), CapabilityError>
where
    F: FnMut(CpioEntry<'a>) -> Result<(), CapabilityError>,
{
    let mut offset = 0usize;

    while offset + NEWC_HEADER_SIZE <= archive.len() {
        let header = &archive[offset..offset + NEWC_HEADER_SIZE];
        if &header[0..6] != b"070701" && &header[0..6] != b"070702" {
            return Err(CapabilityError::InvalidArgument);
        }

        let file_size = parse_hex_u32(&header[54..62])? as usize;
        let name_size = parse_hex_u32(&header[94..102])? as usize;
        if name_size == 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let name_start = offset + NEWC_HEADER_SIZE;
        let name_end = name_start
            .checked_add(name_size)
            .ok_or(CapabilityError::InvalidArgument)?;
        if name_end > archive.len() {
            return Err(CapabilityError::InvalidArgument);
        }

        let name_raw = &archive[name_start..name_end - 1];
        let name = core::str::from_utf8(name_raw).map_err(|_| CapabilityError::InvalidArgument)?;

        let data_start = align4(name_end);
        let data_end = data_start
            .checked_add(file_size)
            .ok_or(CapabilityError::InvalidArgument)?;
        if data_end > archive.len() {
            return Err(CapabilityError::InvalidArgument);
        }

        if name == "TRAILER!!!" {
            return Ok(());
        }

        f(CpioEntry {
            name,
            data: &archive[data_start..data_end],
        })?;

        offset = align4(data_end);
    }

    Err(CapabilityError::InvalidArgument)
}

#[inline(always)]
fn align4(value: usize) -> usize {
    (value + 3) & !3
}

fn parse_hex_u32(slice: &[u8]) -> Result<u32, CapabilityError> {
    let mut out = 0u32;
    for &c in slice {
        out <<= 4;
        out |= match c {
            b'0'..=b'9' => (c - b'0') as u32,
            b'a'..=b'f' => (c - b'a' + 10) as u32,
            b'A'..=b'F' => (c - b'A' + 10) as u32,
            _ => return Err(CapabilityError::InvalidArgument),
        };
    }
    Ok(out)
}
