use crate::{COLS, DEFAULT_TEXT_COLOR};

const ANSI_PARAM_MAX: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AnsiAction {
    None,
    DirtyLine,
    FlushLine,
    FlushLineAndRetry,
    ClearScreen,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParserState {
    Ground,
    Escape,
    EscapeIgnoreOne,
    Osc,
    OscEscape,
    Csi,
}

pub struct AnsiTerminal {
    line: [u8; COLS],
    colors: [u32; COLS],
    col: usize,
    saved_col: usize,
    color: u32,
    state: ParserState,
    params: [usize; ANSI_PARAM_MAX],
    param_count: usize,
    current_param: usize,
    have_param: bool,
    private_csi: bool,
}

impl AnsiTerminal {
    pub const fn new() -> Self {
        Self {
            line: [0; COLS],
            colors: [DEFAULT_TEXT_COLOR; COLS],
            col: 0,
            saved_col: 0,
            color: DEFAULT_TEXT_COLOR,
            state: ParserState::Ground,
            params: [0; ANSI_PARAM_MAX],
            param_count: 0,
            current_param: 0,
            have_param: false,
            private_csi: false,
        }
    }

    pub fn reset(&mut self) {
        self.line = [0; COLS];
        self.colors = [DEFAULT_TEXT_COLOR; COLS];
        self.col = 0;
        self.saved_col = 0;
        self.color = DEFAULT_TEXT_COLOR;
        self.state = ParserState::Ground;
        self.reset_csi();
    }

    pub fn line(&self) -> [u8; COLS] {
        self.line
    }

    pub fn colors(&self) -> [u32; COLS] {
        self.colors
    }

    pub fn col(&self) -> usize {
        self.col
    }

    pub fn process_byte(&mut self, byte: u8) -> AnsiAction {
        match self.state {
            ParserState::Ground => self.process_ground(byte),
            ParserState::Escape => self.process_escape(byte),
            ParserState::EscapeIgnoreOne => {
                self.state = ParserState::Ground;
                AnsiAction::None
            }
            ParserState::Osc => self.process_osc(byte),
            ParserState::OscEscape => self.process_osc_escape(byte),
            ParserState::Csi => self.process_csi(byte),
        }
    }

    pub fn clear_line(&mut self) {
        self.line = [0; COLS];
        self.colors = [DEFAULT_TEXT_COLOR; COLS];
        self.col = 0;
    }

    fn process_ground(&mut self, byte: u8) -> AnsiAction {
        match byte {
            0x1b => {
                self.state = ParserState::Escape;
                AnsiAction::None
            }
            b'\n' => AnsiAction::FlushLine,
            b'\r' => {
                self.col = 0;
                AnsiAction::DirtyLine
            }
            0x08 => {
                if self.col != 0 {
                    self.col -= 1;
                }
                AnsiAction::DirtyLine
            }
            0x20..=0x7e | b'\t' => {
                if byte == b'\t' {
                    self.write_spaces(4)
                } else {
                    self.write_printable(byte)
                }
            }
            _ => AnsiAction::None,
        }
    }

    fn process_escape(&mut self, byte: u8) -> AnsiAction {
        match byte {
            b'[' => {
                self.state = ParserState::Csi;
                self.reset_csi();
                AnsiAction::None
            }
            b']' | b'P' | b'^' | b'_' => {
                self.state = ParserState::Osc;
                AnsiAction::None
            }
            b'c' => {
                self.reset();
                AnsiAction::ClearScreen
            }
            b'7' | b's' => {
                self.saved_col = self.col;
                self.state = ParserState::Ground;
                AnsiAction::None
            }
            b'8' | b'u' => {
                self.col = self.saved_col.min(COLS.saturating_sub(1));
                self.state = ParserState::Ground;
                AnsiAction::DirtyLine
            }
            b'(' | b')' | b'*' | b'+' | b'-' | b'.' | b'/' => {
                self.state = ParserState::EscapeIgnoreOne;
                AnsiAction::None
            }
            _ => {
                self.state = ParserState::Ground;
                AnsiAction::None
            }
        }
    }

    fn process_osc(&mut self, byte: u8) -> AnsiAction {
        match byte {
            0x07 => {
                self.state = ParserState::Ground;
                AnsiAction::None
            }
            0x1b => {
                self.state = ParserState::OscEscape;
                AnsiAction::None
            }
            _ => AnsiAction::None,
        }
    }

    fn process_osc_escape(&mut self, byte: u8) -> AnsiAction {
        self.state = ParserState::Ground;
        if byte == b'\\' {
            AnsiAction::None
        } else {
            self.process_ground(byte)
        }
    }

    fn process_csi(&mut self, byte: u8) -> AnsiAction {
        match byte {
            b'0'..=b'9' => {
                self.have_param = true;
                self.current_param = self
                    .current_param
                    .saturating_mul(10)
                    .saturating_add((byte - b'0') as usize);
                AnsiAction::None
            }
            b';' => {
                self.push_param();
                AnsiAction::None
            }
            b':' => {
                self.push_param();
                AnsiAction::None
            }
            b'?' | b'>' | b'=' | b'<' => {
                self.private_csi = true;
                AnsiAction::None
            }
            0x20..=0x2f => AnsiAction::None,
            final_byte @ 0x40..=0x7e => {
                self.push_param();
                self.state = ParserState::Ground;
                self.apply_csi(final_byte)
            }
            _ => {
                self.state = ParserState::Ground;
                AnsiAction::None
            }
        }
    }

    fn apply_csi(&mut self, final_byte: u8) -> AnsiAction {
        if self.private_csi && matches!(final_byte, b'h' | b'l') {
            return AnsiAction::None;
        }

        match final_byte {
            b'@' => {
                let n = self.param_or_default(0, 1);
                self.insert_cells(n);
                AnsiAction::DirtyLine
            }
            b'A' => AnsiAction::DirtyLine,
            b'B' => AnsiAction::DirtyLine,
            b'C' => {
                let n = self.param_or_default(0, 1);
                self.col = self.col.saturating_add(n).min(COLS.saturating_sub(1));
                AnsiAction::DirtyLine
            }
            b'D' => {
                let n = self.param_or_default(0, 1);
                self.col = self.col.saturating_sub(n);
                AnsiAction::DirtyLine
            }
            b'E' => {
                let n = self.param_or_default(0, 1);
                self.col = 0;
                if n == 0 {
                    AnsiAction::DirtyLine
                } else {
                    AnsiAction::FlushLine
                }
            }
            b'F' => {
                self.col = 0;
                AnsiAction::DirtyLine
            }
            b'G' => {
                let n = self.param_or_default(0, 1);
                self.col = n.saturating_sub(1).min(COLS.saturating_sub(1));
                AnsiAction::DirtyLine
            }
            b'H' | b'f' => {
                let col = self.param_or_default(1, 1);
                self.col = col.saturating_sub(1).min(COLS.saturating_sub(1));
                AnsiAction::DirtyLine
            }
            b'J' => {
                let mode = self.param_or_default(0, 0);
                if mode == 2 || mode == 3 {
                    self.clear_line();
                    AnsiAction::ClearScreen
                } else {
                    self.erase_line_part(mode);
                    AnsiAction::DirtyLine
                }
            }
            b'K' => {
                let mode = self.param_or_default(0, 0);
                self.erase_line_part(mode);
                AnsiAction::DirtyLine
            }
            b'L' | b'M' => AnsiAction::DirtyLine,
            b'P' => {
                let n = self.param_or_default(0, 1);
                self.delete_cells(n);
                AnsiAction::DirtyLine
            }
            b'X' => {
                let n = self.param_or_default(0, 1);
                self.erase_cells(n);
                AnsiAction::DirtyLine
            }
            b'Z' => {
                let n = self.param_or_default(0, 1);
                let step = n.saturating_mul(8);
                self.col = self.col.saturating_sub(step);
                AnsiAction::DirtyLine
            }
            b'`' => {
                let n = self.param_or_default(0, 1);
                self.col = n.saturating_sub(1).min(COLS.saturating_sub(1));
                AnsiAction::DirtyLine
            }
            b'a' => {
                let n = self.param_or_default(0, 1);
                self.col = self.col.saturating_add(n).min(COLS.saturating_sub(1));
                AnsiAction::DirtyLine
            }
            b'm' => {
                self.apply_sgr();
                AnsiAction::DirtyLine
            }
            b'd' => AnsiAction::DirtyLine,
            b'e' => AnsiAction::DirtyLine,
            b's' => {
                self.saved_col = self.col;
                AnsiAction::None
            }
            b'u' => {
                self.col = self.saved_col.min(COLS.saturating_sub(1));
                AnsiAction::DirtyLine
            }
            _ => AnsiAction::None,
        }
    }

    fn insert_cells(&mut self, count: usize) {
        if self.col >= COLS || count == 0 {
            return;
        }
        let count = count.min(COLS - self.col);
        let mut i = COLS;
        while i > self.col + count {
            self.line[i - 1] = self.line[i - 1 - count];
            self.colors[i - 1] = self.colors[i - 1 - count];
            i -= 1;
        }
        let mut j = self.col;
        while j < self.col + count {
            self.line[j] = b' ';
            self.colors[j] = self.color;
            j += 1;
        }
    }

    fn delete_cells(&mut self, count: usize) {
        if self.col >= COLS || count == 0 {
            return;
        }
        let count = count.min(COLS - self.col);
        let mut i = self.col;
        while i + count < COLS {
            self.line[i] = self.line[i + count];
            self.colors[i] = self.colors[i + count];
            i += 1;
        }
        while i < COLS {
            self.line[i] = 0;
            self.colors[i] = DEFAULT_TEXT_COLOR;
            i += 1;
        }
    }

    fn erase_cells(&mut self, count: usize) {
        if self.col >= COLS || count == 0 {
            return;
        }
        let end = self.col.saturating_add(count).min(COLS);
        let mut i = self.col;
        while i < end {
            self.line[i] = 0;
            self.colors[i] = DEFAULT_TEXT_COLOR;
            i += 1;
        }
    }

    fn write_printable(&mut self, byte: u8) -> AnsiAction {
        if self.col >= COLS {
            return AnsiAction::FlushLineAndRetry;
        }
        self.line[self.col] = byte;
        self.colors[self.col] = self.color;
        self.col += 1;
        AnsiAction::DirtyLine
    }

    fn write_spaces(&mut self, count: usize) -> AnsiAction {
        if self.col.saturating_add(count) > COLS {
            return AnsiAction::FlushLineAndRetry;
        }
        let mut i = 0usize;
        while i < count {
            self.line[self.col] = b' ';
            self.colors[self.col] = self.color;
            self.col += 1;
            i += 1;
        }
        AnsiAction::DirtyLine
    }

    fn erase_line_part(&mut self, mode: usize) {
        let (start, end) = match mode {
            1 => (0, self.col.saturating_add(1).min(COLS)),
            2 => (0, COLS),
            _ => (self.col, COLS),
        };
        let mut i = start;
        while i < end {
            self.line[i] = 0;
            self.colors[i] = DEFAULT_TEXT_COLOR;
            i += 1;
        }
    }

    fn apply_sgr(&mut self) {
        if self.param_count == 0 {
            self.color = DEFAULT_TEXT_COLOR;
            return;
        }
        let mut i = 0usize;
        while i < self.param_count {
            let code = self.params[i];
            self.color = sgr_color(code, self.color);
            i += 1;
        }
    }

    fn reset_csi(&mut self) {
        self.params = [0; ANSI_PARAM_MAX];
        self.param_count = 0;
        self.current_param = 0;
        self.have_param = false;
        self.private_csi = false;
    }

    fn push_param(&mut self) {
        if self.param_count >= ANSI_PARAM_MAX {
            self.current_param = 0;
            self.have_param = false;
            return;
        }
        if self.have_param || self.param_count != 0 {
            self.params[self.param_count] = if self.have_param {
                self.current_param
            } else {
                0
            };
            self.param_count += 1;
        }
        self.current_param = 0;
        self.have_param = false;
    }

    fn param_or_default(&self, index: usize, default: usize) -> usize {
        if index < self.param_count && self.params[index] != 0 {
            self.params[index]
        } else {
            default
        }
    }
}

fn sgr_color(code: usize, current: u32) -> u32 {
    match code {
        0 | 39 => DEFAULT_TEXT_COLOR,
        1 => current,
        30 => 0x0028_2c34,
        31 => 0x00ff_6b6b,
        32 => 0x0087_d37c,
        33 => 0x00ff_d166,
        34 => 0x0061_afef,
        35 => 0x00c6_78dd,
        36 => 0x0056_d6c9,
        37 => 0x00e8_e0cf,
        90 => 0x0074_7d8c,
        91 => 0x00ff_8787,
        92 => 0x00a3_e88d,
        93 => 0x00ff_e08a,
        94 => 0x0080_c7ff,
        95 => 0x00d7_9bff,
        96 => 0x0076_f0e0,
        97 => 0x00ff_faf0,
        _ => current,
    }
}
