//! Bounded _CRS descriptor selection, including nonzero _PRT SourceIndex.
use super::{Error, Route};

pub(super) fn interrupt(mut bytes: &[u8], index: u32) -> Result<Route, Error> {
    for current in 0..=index {
        let header = *bytes.first().ok_or(Error::Malformed)?;
        let (prefix, length) = if header & 0x80 != 0 {
            let length = bytes.get(1..3).ok_or(Error::Malformed)?;
            (3, u16::from_le_bytes([length[0], length[1]]) as usize)
        } else {
            (1, (header & 7) as usize)
        };
        let payload = bytes.get(prefix..prefix + length).ok_or(Error::Malformed)?;
        if header >> 3 == 15 {
            return Err(Error::NoRoute);
        } // EndTag
        if current == index {
            return match header {
                0x22 | 0x23 => {
                    let mask = u16::from_le_bytes([payload[0], payload[1]]);
                    if mask.count_ones() != 1 {
                        return Err(Error::Unsupported);
                    }
                    let flags = payload.get(2).copied().unwrap_or(1);
                    Ok(Route {
                        gsi: mask.trailing_zeros(),
                        level: flags & 1 == 0,
                        low: flags & 8 != 0,
                    })
                }
                0x89 => {
                    if payload.len() < 6 || payload[1] != 1 {
                        return Err(Error::Unsupported);
                    }
                    if payload[0] & 1 == 0 {
                        return Err(Error::Unsupported);
                    } // must be a consumer
                      // An explicit ResourceSource names another interrupt
                      // controller; its interrupt number is not an IO-APIC GSI.
                    if !matches!(&payload[6..], [] | [0, 0]) {
                        return Err(Error::Unsupported);
                    }
                    Ok(Route {
                        gsi: u32::from_le_bytes(payload[2..6].try_into().unwrap()),
                        level: payload[0] & 2 == 0,
                        low: payload[0] & 4 != 0,
                    })
                }
                _ => Err(Error::Unsupported),
            };
        }
        bytes = &bytes[prefix + length..];
    }
    Err(Error::NoRoute)
}
