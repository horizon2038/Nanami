use super::{COLS, MAX_ROWS};

pub struct Scrollback {
    rows: [[u8; COLS]; MAX_ROWS],
    colors: [[u32; COLS]; MAX_ROWS],
    head: usize,
    len: usize,
}

impl Scrollback {
    pub const fn new() -> Self {
        Self {
            rows: [[0; COLS]; MAX_ROWS],
            colors: [[0; COLS]; MAX_ROWS],
            head: 0,
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    pub fn pop(&mut self) {
        self.len = self.len.saturating_sub(1);
    }

    pub fn line(&self, index: usize) -> &[u8; COLS] {
        &self.rows[self.slot(index)]
    }

    pub fn colors(&self, index: usize) -> &[u32; COLS] {
        &self.colors[self.slot(index)]
    }

    pub fn replace(&mut self, index: usize, line: [u8; COLS], colors: [u32; COLS]) {
        let slot = self.slot(index);
        self.rows[slot] = line;
        self.colors[slot] = colors;
    }

    /// Append a line, returning whether the oldest line was evicted.
    pub fn push(&mut self, line: [u8; COLS], colors: [u32; COLS]) -> bool {
        let full = self.len == MAX_ROWS;
        if full {
            self.head = (self.head + 1) % MAX_ROWS;
        } else {
            self.len += 1;
        }
        self.replace(self.len - 1, line, colors);
        full
    }

    fn slot(&self, index: usize) -> usize {
        assert!(index < self.len);
        (self.head + index) % MAX_ROWS
    }
}
