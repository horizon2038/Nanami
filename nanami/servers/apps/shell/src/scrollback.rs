use super::{COLS, MAX_ROWS};
use alloc::{boxed::Box, vec::Vec};

struct Row {
    text: [u8; COLS],
    colors: [u32; COLS],
}

pub struct Scrollback {
    // Allocate rows on demand and reuse them after eviction/pop/clear. Keeping
    // maximum-width history inline would exceed Shell's 256 KiB entry stack.
    rows: Vec<Box<Row>>,
    head: usize,
    len: usize,
}

impl Scrollback {
    pub const fn new() -> Self {
        Self {
            rows: Vec::new(),
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
        &self.rows[self.slot(index)].text
    }

    pub fn colors(&self, index: usize) -> &[u32; COLS] {
        &self.rows[self.slot(index)].colors
    }

    pub fn replace(&mut self, index: usize, line: [u8; COLS], colors: [u32; COLS]) {
        let slot = self.slot(index);
        self.rows[slot].text = line;
        self.rows[slot].colors = colors;
    }

    /// Append a line, returning whether the oldest line was evicted.
    pub fn push(&mut self, line: [u8; COLS], colors: [u32; COLS]) -> bool {
        let full = self.len == MAX_ROWS;
        if full {
            self.head = (self.head + 1) % MAX_ROWS;
        } else {
            self.len += 1;
        }
        let slot = self.slot(self.len - 1);
        if slot == self.rows.len() {
            self.rows.push(Box::new(Row { text: line, colors }));
            return full;
        }
        self.replace(self.len - 1, line, colors);
        full
    }

    /// Format a logical output line using the current window width. Zero bytes
    /// are padding, while spaces and color attributes are preserved verbatim.
    pub fn push_wrapped(
        &mut self,
        line: &[u8; COLS],
        colors: &[u32; COLS],
        columns: usize,
    ) -> usize {
        let columns = columns.clamp(1, COLS);
        let bytes = line
            .iter()
            .rposition(|byte| *byte != 0)
            .map_or(0, |last| last + 1);
        let mut evicted = 0;
        for offset in (0..bytes.max(1)).step_by(columns) {
            let count = (bytes - offset).min(columns);
            let mut text = [0; COLS];
            let mut attributes = [0; COLS];
            text[..count].copy_from_slice(&line[offset..offset + count]);
            attributes[..count].copy_from_slice(&colors[offset..offset + count]);
            evicted += usize::from(self.push(text, attributes));
        }
        evicted
    }

    fn slot(&self, index: usize) -> usize {
        assert!(index < self.len);
        (self.head + index) % MAX_ROWS
    }
}
