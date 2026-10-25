//! PCI configuration transactions. Only pci-server touches CF8/CFC or ECAM.
use crate::Word;

pub const PCI_SERVICE: &str = "pci-service";
pub const PCI_REQUEST_READ: Word = 0xd101;
pub const PCI_REQUEST_WRITE: Word = 0xd102;
pub const PCI_REQUEST_ENUMERATE: Word = 0xd103;
pub const PCI_REQUEST_PROBE_BAR: Word = 0xd104;
/// (inventory ordinal, name field, byte offset) -> (total bytes, LE byte chunk).
pub const PCI_REQUEST_NAME: Word = 0xd105;
pub const PCI_NAME_VENDOR: Word = 0;
pub const PCI_NAME_DEVICE: Word = 1;
pub const PCI_NAME_CLASS: Word = 2;
pub const PCI_NAME_SUBCLASS: Word = 3;
pub const PCI_NAME_INTERFACE: Word = 4;
pub const PCI_NAME_CHUNK_BYTES: usize = 8;
pub const PCI_NAME_BUFFER_BYTES: usize = 512;
pub const PCI_ENUM_END: Word = Word::MAX;
pub const PCI_BAR_IO: Word = 1;
pub const PCI_BAR_PREFETCHABLE: Word = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Address {
    pub segment: u16,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl Address {
    pub const fn new(bus: u8, device: u8, function: u8) -> Self {
        Self {
            segment: 0,
            bus,
            device,
            function,
        }
    }

    pub fn packed(self) -> Word {
        ((self.segment as Word) << 32)
            | ((self.bus as Word) << 16)
            | ((self.device as Word) << 8)
            | self.function as Word
    }

    pub fn from_packed(value: Word) -> Option<Self> {
        if value & !0x0000_ffff_00ff_1f07 != 0 {
            return None;
        }
        Some(Self {
            segment: (value >> 32) as u16,
            bus: (value >> 16) as u8,
            device: (value >> 8) as u8,
            function: value as u8,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Device {
    pub address: Address,
    pub vendor_id: u16,
    pub device_id: u16,
    /// Base class, subclass and programming interface, in that order.
    pub class: u32,
    pub header: u8,
}

impl Device {
    pub fn packed_info(self) -> Word {
        self.vendor_id as Word
            | ((self.device_id as Word) << 16)
            | ((self.class as Word) << 32)
            | ((self.header as Word) << 56)
    }

    pub fn from_words(address: Word, info: Word) -> Option<Self> {
        Some(Self {
            address: Address::from_packed(address)?,
            vendor_id: info as u16,
            device_id: (info >> 16) as u16,
            class: ((info >> 32) & 0xff_ffff) as u32,
            header: (info >> 56) as u8,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bar {
    pub physical: Word,
    pub bytes: Word,
    pub io: bool,
    pub prefetchable: bool,
}

pub fn valid_register(address: Address, offset: Word, width: Word) -> bool {
    address.device < 32
        && address.function < 8
        && matches!(width, 1 | 2 | 4)
        && offset < 4096
        && offset % width == 0
        && width <= 4096 - offset
}
