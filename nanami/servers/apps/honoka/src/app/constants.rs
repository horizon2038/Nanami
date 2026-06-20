use libnanami::Word;

pub const SLOT_DISPLAY_SERVICE: Word = 22;
pub const SLOT_INPUT_SERVICE: Word = 23;
pub const SLOT_TIMER_SERVICE: Word = 24;
pub const SLOT_RTC_SERVICE: Word = 25;
pub const SLOT_SERVICE_PORT: Word = 20;
pub const SLOT_NOTIFICATION: Word = 21;
pub const SLOT_WINDOW_INPUT_NOTIFICATION_BASE: Word = 40;

pub const CONNECT_RETRY_MS: Word = 20;
pub const MAX_INPUT_EVENTS_PER_FRAME: usize = 64;
pub const MAX_COALESCED_MOUSE_MOVES: usize = 2;

pub const CURSOR_SIZE: i32 = 11;
pub const MENU_BAR_HEIGHT: i32 = 30;
pub const TITLE_BAR_HEIGHT: i32 = 30;
pub const MAX_WINDOWS: usize = 16;
