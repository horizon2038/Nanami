//! Parse bounded USB configuration data; never trust a device's lengths or
//! confuse a composite device's non-HID endpoints with a boot HID interface.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootInterface {
    pub number: u8,
    pub protocol: u8,
    pub endpoint: u8,
    pub packet_size: u16,
    pub interval: u8,
}

pub fn boot_interfaces(data: &[u8], mut found: impl FnMut(BootInterface)) -> Result<u8, ()> {
    if data.len() < 9 || data[0] < 9 || data[1] != 2 {
        return Err(());
    }
    let total = u16::from_le_bytes([data[2], data[3]]) as usize;
    if total > data.len() || total < data[0] as usize || data[5] == 0 {
        return Err(());
    }
    let mut offset = data[0] as usize;
    let mut interface = None;
    while offset < total {
        if total - offset < 2 {
            return Err(());
        }
        let len = data[offset] as usize;
        if len < 2 || len > total - offset {
            return Err(());
        }
        let d = &data[offset..offset + len];
        match d[1] {
            4 => {
                if len < 9 {
                    return Err(());
                }
                interface = if d[3] == 0 && d[5] == 3 && d[6] == 1 && matches!(d[7], 1 | 2) {
                    Some((d[2], d[7]))
                } else {
                    None
                };
            }
            5 => {
                if len < 7 {
                    return Err(());
                }
                if let Some((number, protocol)) = interface {
                    let packet_size = u16::from_le_bytes([d[4], d[5]]);
                    let minimum = if protocol == 1 { 8 } else { 3 };
                    if d[2] & 0x80 != 0
                        && d[2] & 0x0f != 0
                        && d[2] & 0x70 == 0
                        && d[3] & 3 == 3
                        && packet_size >= minimum
                        && packet_size <= 1024
                        && d[6] != 0
                    {
                        found(BootInterface {
                            number,
                            protocol,
                            endpoint: d[2],
                            packet_size,
                            interval: d[6],
                        });
                        interface = None;
                    }
                }
            }
            _ => {}
        }
        offset += len;
    }
    Ok(data[5])
}
