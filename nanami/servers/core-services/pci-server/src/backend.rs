//! One thread owns both legacy PCI configuration ports and ECAM mappings.
use crate::config::Access;
use alloc::vec::Vec;
use core::ptr;
use nanami_client::{
    firmware::EcamRegion,
    pci::{valid_register, Address},
};
use nanami_runtime::{RequestError, Word};

const CONFIG_IO_SLOT: Word = 16;
const CONFIG_ADDRESS: Word = 0xcf8;
const CONFIG_DATA: Word = 0xcfc;
const PAGE_BYTES: Word = 4096;

pub struct Backend {
    io: Word,
    regions: Vec<EcamRegion>,
    pages: Vec<(Word, Word)>,
}

impl Backend {
    pub fn new(rsdp: Word) -> Result<Self, RequestError> {
        nanami_runtime::request_io_port(CONFIG_ADDRESS, CONFIG_DATA + 3, CONFIG_IO_SLOT)?;
        let regions = nanami_client::firmware::find_pci_ecam(rsdp)?;
        Ok(Self {
            io: nanami_runtime::ipc::process_slot_descriptor(CONFIG_IO_SLOT),
            regions,
            pages: Vec::new(),
        })
    }

    fn ecam(&mut self, address: Address, offset: Word) -> Result<Word, RequestError> {
        let physical = self
            .regions
            .iter()
            .find_map(|region| {
                region.address(
                    address.segment,
                    address.bus,
                    address.device,
                    address.function,
                )
            })
            .ok_or(RequestError::Unsupported)?;
        if let Some((_, mapped)) = self.pages.iter().find(|(page, _)| *page == physical) {
            return Ok(*mapped + offset);
        }
        if self.pages.len() == 512 {
            return Err(RequestError::Unsupported);
        }
        self.pages
            .try_reserve(1)
            .map_err(|_| RequestError::Unsupported)?;
        let (_, mapped) = nanami_runtime::request_mmio(physical, PAGE_BYTES)?;
        self.pages.push((physical, mapped));
        Ok(mapped + offset)
    }

    fn select(&self, address: Address, offset: Word) -> Result<(), RequestError> {
        let value = 0x8000_0000
            | ((address.bus as Word) << 16)
            | ((address.device as Word) << 11)
            | ((address.function as Word) << 8)
            | (offset & !3);
        nanami_runtime::io::io_write(self.io, CONFIG_ADDRESS, 4, value)
    }
}

impl Access for Backend {
    fn read(&mut self, address: Address, offset: Word, width: Word) -> Result<u32, RequestError> {
        if !valid_register(address, offset, width) {
            return Err(RequestError::InvalidArgument);
        }
        if address.segment == 0 && offset < 256 {
            self.select(address, offset)?;
            return Ok(
                nanami_runtime::io::io_read(self.io, CONFIG_DATA + (offset & 3), width)? as u32,
            );
        }
        let pointer = self.ecam(address, offset)?;
        Ok(unsafe {
            match width {
                1 => ptr::read_volatile(pointer as *const u8) as u32,
                2 => ptr::read_volatile(pointer as *const u16) as u32,
                4 => ptr::read_volatile(pointer as *const u32),
                _ => unreachable!(),
            }
        })
    }

    fn write(
        &mut self,
        address: Address,
        offset: Word,
        width: Word,
        value: u32,
    ) -> Result<(), RequestError> {
        if !valid_register(address, offset, width) {
            return Err(RequestError::InvalidArgument);
        }
        if address.segment == 0 && offset < 256 {
            self.select(address, offset)?;
            return nanami_runtime::io::io_write(
                self.io,
                CONFIG_DATA + (offset & 3),
                width,
                value as Word,
            );
        }
        let pointer = self.ecam(address, offset)?;
        unsafe {
            match width {
                1 => ptr::write_volatile(pointer as *mut u8, value as u8),
                2 => ptr::write_volatile(pointer as *mut u16, value as u16),
                4 => ptr::write_volatile(pointer as *mut u32, value),
                _ => unreachable!(),
            }
        }
        Ok(())
    }
}
