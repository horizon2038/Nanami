use crate::Word;

pub const BLOCK_DEVICE_REQUEST_CONTROL: Word = 0x9001;
pub const BLOCK_DEVICE_REQUEST_READ: Word = 0x9002;
pub const BLOCK_DEVICE_REQUEST_WRITE: Word = 0x9003;
// WRITE completion may leave data in a volatile device cache. FLUSH completes
// after all preceding writes reach the device's backing store (a RAM device
// remains inherently volatile).
pub const BLOCK_DEVICE_REQUEST_FLUSH: Word = 0x9004;

pub const BLOCK_DEVICE_CONTROL_ATTACH_SHARED_MEMORY: Word = 1;
pub const BLOCK_DEVICE_CONTROL_GET_INFO: Word = 2;

pub const BLOCK_DEVICE_DEFAULT_SHM_BYTES: Word = 0x4000;
pub const BLOCK_DEVICE_BLOCK_SIZE: Word = 1024;
