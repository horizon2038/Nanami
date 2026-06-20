use libnanami::Word;

pub const SLOT_IO_PORT: Word = 16;
pub const SLOT_NOTIFICATION: Word = 18;
pub const SLOT_INTERRUPT_KBD: Word = 19;
pub const SLOT_INTERRUPT_MOUSE: Word = 24;
pub const SLOT_INPUT_SERVICE: Word = 23;
pub const SLOT_INPUT_NOTIFICATION: Word = 25;

pub const PS2_DATA_PORT: Word = 0x60;
pub const PS2_STATUS_PORT: Word = 0x64;
pub const PS2_COMMAND_PORT: Word = 0x64;

pub const PS2_CONTROLLER_ENABLE_AUX: Word = 0xA8;
pub const PS2_CONTROLLER_READ_CONFIG: Word = 0x20;
pub const PS2_CONTROLLER_WRITE_CONFIG: Word = 0x60;
pub const PS2_CONTROLLER_WRITE_TO_MOUSE: Word = 0xD4;

pub const PS2_MOUSE_CMD_DISABLE_DATA_REPORTING: Word = 0xF5;
pub const PS2_MOUSE_CMD_SET_DEFAULTS: Word = 0xF6;
pub const PS2_MOUSE_CMD_SET_SAMPLE_RATE: Word = 0xF3;
pub const PS2_MOUSE_CMD_SET_RESOLUTION: Word = 0xE8;
pub const PS2_MOUSE_CMD_SET_SCALING_1_TO_1: Word = 0xE6;
pub const PS2_MOUSE_CMD_ENABLE_DATA_REPORTING: Word = 0xF4;
pub const PS2_MOUSE_ACK: u8 = 0xFA;
pub const PS2_MOUSE_RESEND: u8 = 0xFE;

pub const PS2_MOUSE_SAMPLE_RATE: Word = 200;
pub const PS2_MOUSE_RESOLUTION: Word = 3;

pub const MAX_PS2_BYTES_PER_DRAIN: usize = 128;
pub const MAX_KEY_EVENTS_PER_BATCH: usize = 32;
pub const MAX_MOUSE_BUTTON_EVENTS_PER_BATCH: usize = 32;
pub const PS2_ACK_TIMEOUT: usize = 500_000;
pub const HEARTBEAT_IRQ_INTERVAL: usize = 65536;
