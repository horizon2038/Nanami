use libnanami::{RequestError, Word};

pub const MMIO_PHYSICAL_BASE: Word = 0x0a00_0000;
pub const MMIO_REGION_BYTES: Word = 0x4000;
pub const MMIO_TRANSPORT_STRIDE: Word = 0x200;

pub const VIRTIO_MAGIC_VALUE: u32 = 0x7472_6976;
pub const VIRTIO_MMIO_VERSION: u32 = 2;
pub const VIRTIO_BLOCK_DEVICE_ID: u32 = 2;

pub const REG_MAGIC_VALUE: Word = 0x000;
pub const REG_VERSION: Word = 0x004;
pub const REG_DEVICE_ID: Word = 0x008;
pub const REG_VENDOR_ID: Word = 0x00c;
pub const REG_DEVICE_FEATURES: Word = 0x010;
pub const REG_DEVICE_FEATURES_SELECT: Word = 0x014;
pub const REG_DRIVER_FEATURES: Word = 0x020;
pub const REG_DRIVER_FEATURES_SELECT: Word = 0x024;
pub const REG_QUEUE_SELECT: Word = 0x030;
pub const REG_QUEUE_SIZE_MAX: Word = 0x034;
pub const REG_QUEUE_SIZE: Word = 0x038;
pub const REG_QUEUE_READY: Word = 0x044;
pub const REG_QUEUE_NOTIFY: Word = 0x050;
pub const REG_INTERRUPT_STATUS: Word = 0x060;
pub const REG_INTERRUPT_ACK: Word = 0x064;
pub const REG_DEVICE_STATUS: Word = 0x070;
pub const REG_QUEUE_DESC_LOW: Word = 0x080;
pub const REG_QUEUE_DRIVER_LOW: Word = 0x090;
pub const REG_QUEUE_DEVICE_LOW: Word = 0x0a0;
pub const REG_CONFIG_BASE: Word = 0x100;

pub fn read(
    _descriptor: Word,
    base: Word,
    offset: Word,
    _width: Word,
) -> Result<Word, RequestError> {
    if base == 0 || (offset & 3) != 0 {
        return Err(RequestError::InvalidArgument);
    }
    Ok(unsafe { ((base + offset) as *const u32).read_volatile() as Word })
}

pub fn write(
    _descriptor: Word,
    base: Word,
    offset: Word,
    _width: Word,
    value: Word,
) -> Result<(), RequestError> {
    if base == 0 || (offset & 3) != 0 {
        return Err(RequestError::InvalidArgument);
    }
    unsafe {
        ((base + offset) as *mut u32).write_volatile(value as u32);
    }
    Ok(())
}

pub fn acknowledge_interrupt(descriptor: Word, base: Word) -> Result<(), RequestError> {
    let pending = read(descriptor, base, REG_INTERRUPT_STATUS, 4)?;
    if pending != 0 {
        write(descriptor, base, REG_INTERRUPT_ACK, 4, pending)?;
    }
    Ok(())
}
