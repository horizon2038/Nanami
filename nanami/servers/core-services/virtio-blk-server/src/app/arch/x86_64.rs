use libnanami::{RequestError, Word};

pub const PCI_CFG_ADDR_PORT: Word = 0x0cf8;
pub const PCI_CFG_DATA_PORT: Word = 0x0cfc;
pub const PCI_CFG_COMMAND_OFFSET: u8 = 0x04;
pub const PCI_CFG_STATUS_OFFSET: u8 = 0x06;
pub const PCI_CFG_CAP_PTR_OFFSET: u8 = 0x34;
pub const PCI_CFG_INTERRUPT_LINE_OFFSET: u8 = 0x3c;
pub const PCI_CFG_INTERRUPT_PIN_OFFSET: u8 = 0x3d;

pub const PIIX3_BUS: u8 = 0;
pub const PIIX3_DEV: u8 = 1;
pub const PIIX3_FUNC: u8 = 0;
pub const PIIX3_PIRQ_ROUTE_BASE: u8 = 0x60;

pub const REG_DEVICE_FEATURES: Word = 0x00;
pub const REG_DRIVER_FEATURES: Word = 0x04;
pub const REG_QUEUE_ADDRESS: Word = 0x08;
pub const REG_QUEUE_SIZE_MAX: Word = 0x0c;
pub const REG_QUEUE_SELECT: Word = 0x0e;
pub const REG_QUEUE_NOTIFY: Word = 0x10;
pub const REG_DEVICE_STATUS: Word = 0x12;
pub const REG_INTERRUPT_STATUS: Word = 0x13;
pub const REG_CONFIG_BASE: Word = 0x14;

pub fn read(descriptor: Word, base: Word, offset: Word, width: Word) -> Result<Word, RequestError> {
    libnanami::io::io_read(descriptor, base + offset, width)
}

pub fn write(
    descriptor: Word,
    base: Word,
    offset: Word,
    width: Word,
    value: Word,
) -> Result<(), RequestError> {
    libnanami::io::io_write(descriptor, base + offset, width, value)
}

pub fn acknowledge_interrupt(descriptor: Word, base: Word) -> Result<(), RequestError> {
    let _ = read(descriptor, base, REG_INTERRUPT_STATUS, 1)?;
    Ok(())
}
