//! Decode controller-local Speed IDs (xHCI 1.2b, section 7.2.2.1).
//! USB2 uses two bits per PSIV: 1 = FS, 2 = LS, 3 = HS, 0 = unsupported.
pub const DEFAULT_SPEEDS: u32 = (1 << 2) | (2 << 4) | (3 << 6);

pub fn classify(speeds: u32, id: u8) -> u8 {
    ((speeds >> (id * 2)) & 3) as u8
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Speeds {
    pub usb2: u32,
    pub usb3: u16,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Error {
    pub index: usize,
    pub reason: &'static str,
}

impl Speeds {
    pub fn parse(
        major: u8,
        minor: u8,
        entries: &[u32],
        mut unsupported: impl FnMut(usize, &'static str),
    ) -> Result<Self, Error> {
        let mut speeds = Self::default();
        if entries.is_empty() {
            // PSIC=0 implies defaults only for these protocol revisions.
            match (major, minor) {
                (2, 0) => speeds.usb2 = DEFAULT_SPEEDS,
                (3, 0) => speeds.usb3 = 1 << 4,
                (3, 0x10) => speeds.usb3 = (1 << 4) | (1 << 5),
                (3, 0x20) => speeds.usb3 = (1 << 4) | (1 << 5) | (1 << 6) | (1 << 7),
                _ => {}
            }
            return Ok(speeds);
        }
        // Explicit PSI entries replace, rather than supplement, the defaults.
        // Track even unsupported IDs, so duplicates cannot become valid later.
        let mut seen = 0u16;
        let mut index = 0;
        while index < entries.len() {
            let psi = entries[index];
            let id = psi & 15;
            let link = (psi >> 6) & 3;
            let error = |reason| Error { index, reason };
            if id == 0 {
                return Err(error("reserved PSIV zero"));
            }
            if seen & (1 << id) != 0 {
                return Err(error("duplicate PSIV"));
            }
            seen |= 1 << id;
            if psi >> 16 == 0 {
                return Err(error("zero speed mantissa"));
            }
            let width = match link {
                2 => {
                    let tx = *entries
                        .get(index + 1)
                        .ok_or_else(|| error("asymmetric Rx without Tx"))?;
                    let tx_error = |reason| Error {
                        index: index + 1,
                        reason,
                    };
                    if tx & 15 != id || (tx >> 6) & 3 != 3 {
                        return Err(tx_error("asymmetric Rx/Tx pairing mismatch"));
                    }
                    if tx >> 16 == 0 {
                        return Err(tx_error("zero speed mantissa"));
                    }
                    if (psi ^ tx) & ((1 << 8) | (3 << 14)) != 0 {
                        return Err(tx_error("asymmetric link attributes differ"));
                    }
                    2
                }
                3 => return Err(error("asymmetric Tx without Rx")),
                _ => 1,
            };
            let reason = match major {
                2 if psi & ((7 << 6) | (3 << 14)) != 0 => Some("unsupported USB2 link mode"),
                2 => {
                    let rate = (psi >> 16) as u64
                        * [1, 1000, 1_000_000, 1_000_000_000][((psi >> 4) & 3) as usize];
                    let class = match rate {
                        12_000_000 => 1,
                        1_500_000 => 2,
                        480_000_000 => 3,
                        _ => 0,
                    };
                    speeds.usb2 |= class << (id * 2);
                    (class == 0).then_some("unsupported USB2 bit rate")
                }
                3 if link == 1 || psi & (1 << 8) == 0 || (psi >> 14) & 3 > 1 => {
                    Some("unsupported USB3 link mode")
                }
                3 => {
                    // SSIC also speaks USB3, with valid rates below 5 Gbit/s.
                    // EP0/transfer handling depends on the protocol, not rate.
                    speeds.usb3 |= 1 << id;
                    None
                }
                _ => Some("unsupported protocol"),
            };
            if let Some(reason) = reason {
                for entry in index..index + width {
                    unsupported(entry, reason);
                }
            }
            index += width;
        }
        Ok(speeds)
    }
}
