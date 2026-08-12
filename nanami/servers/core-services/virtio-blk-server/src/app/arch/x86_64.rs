use libnanami::Word;

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
