//! A bounded, streaming VT-style screen. IPC chunk boundaries have no meaning.
pub mod keys;
mod parser;
mod screen;
mod style;
pub use screen::Cell;
use screen::Screen;
use style::Style;

#[derive(Clone, Copy)]
enum State {
    Ground,
    Escape,
    Charset,
    String,
    StringEscape,
    Csi,
}

pub struct Terminal {
    screen: Screen,
    alternate: Option<(Screen, Style)>,
    style: Style,
    saved_style: Style,
    state: State,
    params: [usize; 16],
    count: usize,
    private: bool,
    ignored: bool,
    origin: bool,
    wrap: bool,
    insert: bool,
    pub cursor_visible: bool,
    pub application_cursor: bool,
    response: [u8; 32],
    response_len: usize,
    view_offset: usize,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            screen: Screen::new(cols.clamp(1, 512), rows.clamp(1, 256)),
            alternate: None,
            style: Style::DEFAULT,
            saved_style: Style::DEFAULT,
            state: State::Ground,
            params: [0; 16],
            count: 1,
            private: false,
            ignored: false,
            origin: false,
            wrap: true,
            insert: false,
            cursor_visible: true,
            application_cursor: false,
            response: [0; 32],
            response_len: 0,
            view_offset: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.cols(), self.rows());
    }
    pub fn cols(&self) -> usize {
        self.screen.cols
    }
    pub fn rows(&self) -> usize {
        self.screen.rows
    }
    pub fn cursor(&self) -> (usize, usize) {
        (self.screen.x, self.screen.y)
    }
    pub fn row(&self, row: usize) -> &[Cell] {
        &self.screen.cells[row * self.cols()..(row + 1) * self.cols()]
    }
    pub fn history_len(&self) -> usize {
        self.screen.history.len()
    }
    pub fn output_row(&self, row: usize) -> &[Cell] {
        if row < self.history_len() {
            &self.screen.history[row]
        } else {
            self.row(row - self.history_len())
        }
    }
    pub fn view_row(&self, row: usize) -> &[Cell] {
        self.output_row(self.history_len().saturating_sub(self.view_offset) + row)
    }
    pub fn at_bottom(&self) -> bool {
        self.view_offset == 0
    }
    pub fn scroll(&mut self, rows: i16) -> bool {
        let old = self.view_offset;
        self.view_offset = if rows >= 0 {
            self.view_offset
                .saturating_add(rows as usize)
                .min(self.history_len())
        } else {
            self.view_offset
                .saturating_sub(rows.unsigned_abs() as usize)
        };
        self.view_offset != old
    }
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.view_offset = 0;
        let (cols, rows) = (cols.clamp(1, 512), rows.clamp(1, 256));
        self.screen.resize(cols, rows);
        if let Some((screen, _)) = &mut self.alternate {
            screen.resize(cols, rows);
        }
    }
    pub fn take_response(&mut self) -> ([u8; 32], usize) {
        let len = self.response_len;
        self.response_len = 0;
        (self.response, len)
    }
    fn respond(&mut self, bytes: &[u8]) {
        let len = bytes.len().min(self.response.len() - self.response_len);
        self.response[self.response_len..self.response_len + len].copy_from_slice(&bytes[..len]);
        self.response_len += len;
    }
    fn respond_number(&mut self, mut n: usize) {
        let mut buf = [0; 20];
        let mut start = buf.len();
        loop {
            start -= 1;
            buf[start] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        self.respond(&buf[start..]);
    }
    fn save_cursor(&mut self) {
        self.screen.saved = self.cursor();
        self.saved_style = self.style;
    }
    fn restore_cursor(&mut self) {
        self.screen.x = self.screen.saved.0.min(self.cols() - 1);
        self.screen.y = self.screen.saved.1.min(self.rows() - 1);
        self.screen.wrap_pending = false;
        self.style = self.saved_style;
    }
    fn alternate(&mut self, enabled: bool, save: bool) {
        if enabled && self.alternate.is_none() {
            if save {
                self.save_cursor();
            }
            let mut screen = Screen::new(self.cols(), self.rows());
            screen.history_enabled = false;
            self.alternate = Some((
                core::mem::replace(&mut self.screen, screen),
                self.saved_style,
            ));
        } else if !enabled {
            if let Some((screen, style)) = self.alternate.take() {
                self.screen = screen;
                self.saved_style = style;
                if save {
                    self.restore_cursor();
                }
            }
        }
    }
    fn param(&self, index: usize, default: usize) -> usize {
        let value = if index < self.count {
            self.params[index]
        } else {
            0
        };
        if value == 0 {
            default
        } else {
            value
        }
    }
}
