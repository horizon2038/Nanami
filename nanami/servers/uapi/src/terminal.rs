use crate::Word;

pub const TERMINAL_SERVICE: &str = "terminal-service";

pub const TERMINAL_REQUEST_CONTROL: Word = 0xc101;
pub const TERMINAL_REQUEST_CREATE: Word = 0xc102;
pub const TERMINAL_REQUEST_WRITE_INPUT: Word = 0xc103;
pub const TERMINAL_REQUEST_READ_INPUT: Word = 0xc104;
pub const TERMINAL_REQUEST_WRITE_OUTPUT: Word = 0xc105;
pub const TERMINAL_REQUEST_READ_OUTPUT: Word = 0xc106;
pub const TERMINAL_REQUEST_GET_SIZE: Word = 0xc107;
pub const TERMINAL_REQUEST_ATTACH_OUTPUT_NOTIFICATION: Word = 0xc108;
pub const TERMINAL_REQUEST_ATTACH_INPUT_NOTIFICATION: Word = 0xc109;
pub const TERMINAL_REQUEST_CLEAR: Word = 0xc10a;
pub const TERMINAL_REQUEST_SET_ECHO: Word = 0xc10b;
pub const TERMINAL_REQUEST_SET_SIZE: Word = 0xc10c;
pub const TERMINAL_REQUEST_SET_OUTPUT_CRLF: Word = 0xc10d;

pub const TERMINAL_CONTROL_ATTACH_SHARED_MEMORY: Word = 1;
pub const TERMINAL_CLEAR_INPUT: Word = 1 << 0;
pub const TERMINAL_CLEAR_OUTPUT: Word = 1 << 1;

pub const TERMINAL_DEFAULT_SHM_BYTES: Word = 0x4000;
pub const TERMINAL_DEFAULT_COLUMNS: Word = 80;
pub const TERMINAL_DEFAULT_ROWS: Word = 24;

pub const TERMINAL_NOTIFICATION_OUTPUT: Word = 1 << 51;
pub const TERMINAL_NOTIFICATION_INPUT: Word = 1 << 52;

// Optional READ_INPUT arg3: three selected bytes, then control flags. Matching
// bytes are consumed and reported as bits 0..2 in reply detail1. Zero disables
// a selected byte; arg3=0 retains the original unfiltered read/peek operation.
pub const TERMINAL_INPUT_CONTROLS: Word = 1 << 24;
pub const TERMINAL_INPUT_FLUSH: Word = 1 << 25;
pub const TERMINAL_INPUT_ECHO: Word = 1 << 26;

pub const fn terminal_input_controls(bytes: [u8; 3], flush: bool, echo: bool) -> Word {
    let mut value = TERMINAL_INPUT_CONTROLS
        | bytes[0] as Word
        | ((bytes[1] as Word) << 8)
        | ((bytes[2] as Word) << 16);
    if flush {
        value |= TERMINAL_INPUT_FLUSH;
    }
    if echo {
        value |= TERMINAL_INPUT_ECHO;
    }
    value
}
