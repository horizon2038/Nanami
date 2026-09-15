use core::ptr;

pub const USBCMD: usize = 0;
pub const USBSTS: usize = 4;
pub const PORTSC: usize = 0x400;
pub const PORT_CHANGE: u32 = 0x7f << 17;
pub const PORT_POWER: u32 = 1 << 9;
pub const PORT_RESET: u32 = 1 << 4;

pub unsafe fn read(base: usize, offset: usize) -> u32 {
    ptr::read_volatile((base + offset) as *const u32)
}
pub unsafe fn write(base: usize, offset: usize, value: u32) {
    ptr::write_volatile((base + offset) as *mut u32, value);
}
pub unsafe fn write64(base: usize, offset: usize, value: u64) {
    write(base, offset, value as u32);
    write(base, offset + 4, (value >> 32) as u32);
}

/// Do not echo PED (RW1C), change flags or reset/link-state strobes when
/// updating a port. Preserve only ordinary read/write controls.
pub fn port_controls(value: u32) -> u32 {
    value & ((1 << 9) | (3 << 14) | (7 << 25))
}
