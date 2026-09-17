use core::ptr::read_volatile;

use libnanami::{RequestError, Word};

const PAGE_SIZE: Word = 0x1000;
const RSDP_V1_BYTES: usize = 20;
const RSDP_V2_BYTES: usize = 36;
const SDT_HEADER_BYTES: usize = 36;
const HPET_TABLE_BYTES: usize = 56;
const HPET_GAS_OFFSET: Word = 40;
const GAS_SYSTEM_MEMORY: u8 = 0;
const MAX_RSDP_BYTES: usize = PAGE_SIZE as usize;
const MAX_SDT_BYTES: usize = 1024 * 1024;
const MAX_MAPPED_PAGES: usize = 128;

#[derive(Clone, Copy)]
struct MappedPage {
    physical: Word,
    virtual_address: Word,
}

impl MappedPage {
    const EMPTY: Self = Self {
        physical: 0,
        virtual_address: 0,
    };
}

#[derive(Clone, Copy)]
enum RootTable {
    Xsdt(Word),
    Rsdt(Word),
}

struct AcpiMapper {
    pages: [MappedPage; MAX_MAPPED_PAGES],
    mapped_count: usize,
}

impl AcpiMapper {
    const fn new() -> Self {
        Self {
            pages: [MappedPage::EMPTY; MAX_MAPPED_PAGES],
            mapped_count: 0,
        }
    }

    fn map_page(&mut self, physical_address: Word) -> Result<Word, RequestError> {
        if physical_address == 0 || physical_address & (PAGE_SIZE - 1) != 0 {
            return Err(RequestError::InvalidArgument);
        }

        let mut index = 0usize;
        while index < self.mapped_count {
            let page = self.pages[index];
            if page.physical == physical_address {
                return Ok(page.virtual_address);
            }
            index += 1;
        }

        if self.mapped_count == self.pages.len() {
            return Err(RequestError::Unsupported);
        }
        let (_, virtual_address) = libnanami::request_mmio(physical_address, PAGE_SIZE)
            .inspect_err(|error| {
                libnanami::println!(
                    "[driver-manager] ACPI map failed page={:#x} bytes={:#x}: {}",
                    physical_address,
                    PAGE_SIZE,
                    error
                );
            })?;
        self.pages[self.mapped_count] = MappedPage {
            physical: physical_address,
            virtual_address,
        };
        self.mapped_count += 1;
        Ok(virtual_address)
    }

    fn read_u8(&mut self, physical_address: Word) -> Result<u8, RequestError> {
        let page = physical_address & !(PAGE_SIZE - 1);
        let offset = physical_address - page;
        let virtual_page = self.map_page(page)?;
        Ok(unsafe { read_volatile((virtual_page + offset) as *const u8) })
    }

    fn read_bytes(
        &mut self,
        physical_address: Word,
        output: &mut [u8],
    ) -> Result<(), RequestError> {
        let mut index = 0usize;
        while index < output.len() {
            let address = physical_address
                .checked_add(index as Word)
                .ok_or(RequestError::InvalidArgument)?;
            output[index] = self.read_u8(address)?;
            index += 1;
        }
        Ok(())
    }

    fn read_u32(&mut self, physical_address: Word) -> Result<u32, RequestError> {
        let mut bytes = [0u8; 4];
        self.read_bytes(physical_address, &mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn read_u64(&mut self, physical_address: Word) -> Result<u64, RequestError> {
        let mut bytes = [0u8; 8];
        self.read_bytes(physical_address, &mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn signature(&mut self, physical_address: Word, expected: &[u8]) -> Result<bool, RequestError> {
        let mut index = 0usize;
        while index < expected.len() {
            let address = physical_address
                .checked_add(index as Word)
                .ok_or(RequestError::InvalidArgument)?;
            if self.read_u8(address)? != expected[index] {
                return Ok(false);
            }
            index += 1;
        }
        Ok(true)
    }

    fn checksum(&mut self, physical_address: Word, length: usize) -> Result<u8, RequestError> {
        let mut sum = 0u8;
        let mut index = 0usize;
        while index < length {
            let address = physical_address
                .checked_add(index as Word)
                .ok_or(RequestError::InvalidArgument)?;
            sum = sum.wrapping_add(self.read_u8(address)?);
            index += 1;
        }
        Ok(sum)
    }

    fn sdt_length(
        &mut self,
        physical_address: Word,
        signature: &[u8; 4],
    ) -> Result<usize, RequestError> {
        if !self.signature(physical_address, signature)? {
            return Err(RequestError::Protocol);
        }
        let length_address = physical_address
            .checked_add(4)
            .ok_or(RequestError::InvalidArgument)?;
        let length = self.read_u32(length_address)? as usize;
        if !(SDT_HEADER_BYTES..=MAX_SDT_BYTES).contains(&length) {
            return Err(RequestError::Protocol);
        }
        if self.checksum(physical_address, length)? != 0 {
            return Err(RequestError::Protocol);
        }
        Ok(length)
    }
}

fn root_table(mapper: &mut AcpiMapper, rsdp: Word) -> Result<RootTable, RequestError> {
    if !mapper.signature(rsdp, b"RSD PTR ")? || mapper.checksum(rsdp, RSDP_V1_BYTES)? != 0 {
        return Err(RequestError::Protocol);
    }

    let revision = mapper.read_u8(rsdp.checked_add(15).ok_or(RequestError::InvalidArgument)?)?;
    if revision >= 2 {
        let length =
            mapper.read_u32(rsdp.checked_add(20).ok_or(RequestError::InvalidArgument)?)? as usize;
        if !(RSDP_V2_BYTES..=MAX_RSDP_BYTES).contains(&length)
            || mapper.checksum(rsdp, length)? != 0
        {
            return Err(RequestError::Protocol);
        }
        let xsdt =
            mapper.read_u64(rsdp.checked_add(24).ok_or(RequestError::InvalidArgument)?)? as Word;
        if xsdt != 0 {
            return Ok(RootTable::Xsdt(xsdt));
        }
    }

    let rsdt = mapper.read_u32(rsdp.checked_add(16).ok_or(RequestError::InvalidArgument)?)? as Word;
    if rsdt == 0 {
        return Err(RequestError::Protocol);
    }
    Ok(RootTable::Rsdt(rsdt))
}

fn hpet_base(mapper: &mut AcpiMapper, table: Word) -> Result<Word, RequestError> {
    let length = mapper.sdt_length(table, b"HPET")?;
    if length < HPET_TABLE_BYTES {
        return Err(RequestError::Protocol);
    }
    let address_space = mapper.read_u8(
        table
            .checked_add(HPET_GAS_OFFSET)
            .ok_or(RequestError::InvalidArgument)?,
    )?;
    if address_space != GAS_SYSTEM_MEMORY {
        return Err(RequestError::Unsupported);
    }
    let address = mapper.read_u64(
        table
            .checked_add(HPET_GAS_OFFSET + 4)
            .ok_or(RequestError::InvalidArgument)?,
    )? as Word;
    if address == 0 {
        return Err(RequestError::Protocol);
    }
    Ok(address)
}

pub fn find_hpet_mmio_base(rsdp: Word) -> Result<Option<Word>, RequestError> {
    if rsdp == 0 {
        return Ok(None);
    }

    let mut mapper = AcpiMapper::new();
    let (root, root_address, signature, entry_bytes) = match root_table(&mut mapper, rsdp)? {
        RootTable::Xsdt(address) => (RootTable::Xsdt(address), address, b"XSDT", 8usize),
        RootTable::Rsdt(address) => (RootTable::Rsdt(address), address, b"RSDT", 4usize),
    };
    let root_length = mapper.sdt_length(root_address, signature)?;
    let payload_bytes = root_length - SDT_HEADER_BYTES;
    if payload_bytes % entry_bytes != 0 {
        return Err(RequestError::Protocol);
    }

    let mut index = 0usize;
    while index < payload_bytes / entry_bytes {
        let entry_address = root_address
            .checked_add(SDT_HEADER_BYTES as Word)
            .and_then(|address| address.checked_add((index * entry_bytes) as Word))
            .ok_or(RequestError::InvalidArgument)?;
        let table = match root {
            RootTable::Xsdt(_) => mapper.read_u64(entry_address)? as Word,
            RootTable::Rsdt(_) => mapper.read_u32(entry_address)? as Word,
        };
        if table != 0 && mapper.signature(table, b"HPET")? {
            return hpet_base(&mut mapper, table).map(Some);
        }
        index += 1;
    }
    Ok(None)
}
