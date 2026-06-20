#![no_std]

pub fn read_u16_be(bytes: &[u8]) -> u16 {
    ((bytes[0] as u16) << 8) | (bytes[1] as u16)
}

pub fn write_u16_be(bytes: &mut [u8], value: u16) {
    bytes[0] = (value >> 8) as u8;
    bytes[1] = (value & 0xff) as u8;
}

pub fn write_u32_be(bytes: &mut [u8], value: u32) {
    bytes[0] = ((value >> 24) & 0xff) as u8;
    bytes[1] = ((value >> 16) & 0xff) as u8;
    bytes[2] = ((value >> 8) & 0xff) as u8;
    bytes[3] = (value & 0xff) as u8;
}

pub fn read_u32_be(bytes: &[u8]) -> u32 {
    ((bytes[0] as u32) << 24)
        | ((bytes[1] as u32) << 16)
        | ((bytes[2] as u32) << 8)
        | (bytes[3] as u32)
}

pub fn checksum16(data: &[u8], mut sum: u32) -> u16 {
    let mut i = 0usize;
    while i + 1 < data.len() {
        sum += (((data[i] as u16) << 8) | (data[i + 1] as u16)) as u32;
        i += 2;
    }
    if i < data.len() {
        sum += ((data[i] as u16) << 8) as u32;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

pub fn ipv4_checksum(header: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0usize;
    while i + 1 < header.len() {
        if i == 10 {
            i += 2;
            continue;
        }
        sum += (((header[i] as u16) << 8) | (header[i + 1] as u16)) as u32;
        i += 2;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}
