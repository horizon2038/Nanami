use crate::Word;
pub mod display;
pub mod honoka;

pub const DISPLAY_SERVICE_REQUEST_GET_SCREEN_INFO: Word = 0x5001;

pub const DISPLAY_SERVICE_REQUEST_PREPARE_SHARED_FRAMEBUFFER: Word = 0x5002;

pub const DISPLAY_SERVICE_REQUEST_PRESENT: Word = 0x5003;
pub const DISPLAY_SERVICE_REQUEST_GET_CAPABILITIES: Word = 0x5004;
pub const DISPLAY_CAP_NATIVE_SCANOUT: Word = 1;
pub const DISPLAY_CAP_VBLANK_FLIP: Word = 2;
pub const DISPLAY_CAP_CONNECTOR_INFO: Word = 4;
pub const DISPLAY_CAP_EDID: Word = 8;
pub const DISPLAY_CAP_MONITOR_MODES: Word = 16;
/// The selected display endpoint also implements `gpu` protocol version 1.
pub const DISPLAY_CAP_GPU_INTERFACE: Word = 32;
pub const DISPLAY_SERVICE_REQUEST_CONNECTOR_INFO: Word = 0x5005;
pub const DISPLAY_SERVICE_REQUEST_READ_EDID: Word = 0x5006;
pub const DISPLAY_SERVICE_REQUEST_MONITOR_MODE: Word = 0x5007;
