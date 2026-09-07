#![no_std]

pub const LOGICAL_SECTOR_BYTES: usize = 512;
pub const PARTITION_ENTRY_BYTES_MAX: usize = 16 * 1024;

/// On-disk (mixed-endian GPT) encoding of
/// 6e616e61-6d69-4f53-a000-4e414e414d49.
pub const NANAMI_ROOT_TYPE_GUID: [u8; 16] = [
    0x61, 0x6e, 0x61, 0x6e, 0x69, 0x6d, 0x53, 0x4f, 0xa0, 0x00, 0x4e, 0x41, 0x4e, 0x41, 0x4d, 0x49,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub first_usable_lba: u64,
    pub last_usable_lba: u64,
    pub partition_entry_lba: u64,
    pub partition_entry_count: u32,
    pub partition_entry_size: u32,
    pub partition_entries_crc32: u32,
}

impl Header {
    pub fn partition_entry_bytes(self) -> Result<usize, Error> {
        let bytes = (self.partition_entry_count as usize)
            .checked_mul(self.partition_entry_size as usize)
            .ok_or(Error::Unsupported)?;
        if bytes == 0 || bytes > PARTITION_ENTRY_BYTES_MAX {
            return Err(Error::Unsupported);
        }
        Ok(bytes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Partition {
    pub first_lba: u64,
    pub sector_count: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Unsupported,
    Ambiguous,
}

pub fn parse_primary_header(sector: &[u8], disk_sectors: u64) -> Result<Header, Error> {
    if sector.len() < LOGICAL_SECTOR_BYTES || &sector[..8] != b"EFI PART" {
        return Err(Error::Invalid);
    }
    let revision = read_u32(sector, 8)?;
    let header_size = read_u32(sector, 12)? as usize;
    if revision != 0x0001_0000 || !(92..=LOGICAL_SECTOR_BYTES).contains(&header_size) {
        return Err(Error::Unsupported);
    }

    let recorded_crc = read_u32(sector, 16)?;
    let mut crc = Crc32::new();
    crc.update(&sector[..16]);
    crc.update(&[0, 0, 0, 0]);
    crc.update(&sector[20..header_size]);
    if crc.finish() != recorded_crc {
        return Err(Error::Invalid);
    }

    let current_lba = read_u64(sector, 24)?;
    let backup_lba = read_u64(sector, 32)?;
    let first_usable_lba = read_u64(sector, 40)?;
    let last_usable_lba = read_u64(sector, 48)?;
    let partition_entry_lba = read_u64(sector, 72)?;
    let partition_entry_count = read_u32(sector, 80)?;
    let partition_entry_size = read_u32(sector, 84)?;
    let partition_entries_crc32 = read_u32(sector, 88)?;

    if disk_sectors < 2
        || current_lba != 1
        || backup_lba >= disk_sectors
        || first_usable_lba > last_usable_lba
        || last_usable_lba >= disk_sectors
        || partition_entry_lba < 2
        || partition_entry_size < 128
        || partition_entry_size % 8 != 0
    {
        return Err(Error::Invalid);
    }

    let header = Header {
        first_usable_lba,
        last_usable_lba,
        partition_entry_lba,
        partition_entry_count,
        partition_entry_size,
        partition_entries_crc32,
    };
    let entry_bytes = header.partition_entry_bytes()?;
    let entry_sectors = entry_bytes.div_ceil(LOGICAL_SECTOR_BYTES) as u64;
    if partition_entry_lba
        .checked_add(entry_sectors)
        .map_or(true, |end| end > first_usable_lba || end > disk_sectors)
    {
        return Err(Error::Invalid);
    }
    Ok(header)
}

pub fn find_nanami_root(header: Header, entries: &[u8]) -> Result<Partition, Error> {
    let entry_bytes = header.partition_entry_bytes()?;
    if entries.len() < entry_bytes
        || crc32(&entries[..entry_bytes]) != header.partition_entries_crc32
    {
        return Err(Error::Invalid);
    }

    let mut found = None;
    let entry_size = header.partition_entry_size as usize;
    for entry in entries[..entry_bytes].chunks_exact(entry_size) {
        if entry[..16] != NANAMI_ROOT_TYPE_GUID {
            continue;
        }
        let first_lba = read_u64(entry, 32)?;
        let last_lba = read_u64(entry, 40)?;
        if first_lba < header.first_usable_lba
            || first_lba > last_lba
            || last_lba > header.last_usable_lba
        {
            return Err(Error::Invalid);
        }
        let sector_count = last_lba
            .checked_sub(first_lba)
            .and_then(|value| value.checked_add(1))
            .ok_or(Error::Invalid)?;
        if found.is_some() {
            return Err(Error::Ambiguous);
        }
        found = Some(Partition {
            first_lba,
            sector_count,
        });
    }
    found.ok_or(Error::Invalid)
}

pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(bytes);
    crc.finish()
}

struct Crc32(u32);

impl Crc32 {
    const fn new() -> Self {
        Self(!0)
    }

    fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = 0u32.wrapping_sub(self.0 & 1);
                self.0 = (self.0 >> 1) ^ (0xedb8_8320 & mask);
            }
        }
    }

    const fn finish(self) -> u32 {
        !self.0
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    let value = bytes.get(offset..offset + 4).ok_or(Error::Invalid)?;
    Ok(u32::from_le_bytes(
        value.try_into().map_err(|_| Error::Invalid)?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, Error> {
    let value = bytes.get(offset..offset + 8).ok_or(Error::Invalid)?;
    Ok(u64::from_le_bytes(
        value.try_into().map_err(|_| Error::Invalid)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }
}
