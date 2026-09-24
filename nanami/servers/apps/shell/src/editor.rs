//! The shell command line; independent of terminal escape sequences.
pub struct LineEditor<const N: usize, const H: usize> {
    pub bytes: [u8; N],
    pub len: usize,
    pub cursor: usize,
    history: [[u8; N]; H],
    lengths: [usize; H],
    count: usize,
    head: usize,
    selected: usize,
    draft: [u8; N],
    draft_len: usize,
    draft_cursor: usize,
}

impl<const N: usize, const H: usize> LineEditor<N, H> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
            cursor: 0,
            history: [[0; N]; H],
            lengths: [0; H],
            count: 0,
            head: 0,
            selected: 0,
            draft: [0; N],
            draft_len: 0,
            draft_cursor: 0,
        }
    }

    pub fn insert(&mut self, byte: u8) {
        if self.len == N {
            return;
        }
        self.bytes
            .copy_within(self.cursor..self.len, self.cursor + 1);
        self.bytes[self.cursor] = byte;
        self.len += 1;
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor != 0 {
            self.cursor -= 1;
            self.delete();
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.len {
            self.bytes
                .copy_within(self.cursor + 1..self.len, self.cursor);
            self.len -= 1;
            self.bytes[self.len] = 0;
        }
    }

    pub fn clear(&mut self) {
        self.bytes.fill(0);
        self.len = 0;
        self.cursor = 0;
        self.selected = self.count;
    }

    pub fn remember(&mut self) {
        if H != 0 && self.len != 0 {
            let last = (self.head + H - 1) % H;
            if self.count == 0 || self.history[last][..self.lengths[last]] != self.bytes[..self.len]
            {
                self.history[self.head] = self.bytes;
                self.lengths[self.head] = self.len;
                self.head = (self.head + 1) % H;
                self.count = (self.count + 1).min(H);
            }
        }
        self.selected = self.count;
    }

    pub fn previous(&mut self) {
        if self.selected == 0 {
            return;
        }
        if self.selected == self.count {
            self.draft = self.bytes;
            self.draft_len = self.len;
            self.draft_cursor = self.cursor;
        }
        self.selected -= 1;
        self.load();
    }

    pub fn next(&mut self) {
        if self.selected == self.count {
            return;
        }
        self.selected += 1;
        if self.selected == self.count {
            self.bytes = self.draft;
            self.len = self.draft_len;
            self.cursor = self.draft_cursor;
        } else {
            self.load();
        }
    }

    fn load(&mut self) {
        let slot = (self.head + H - self.count + self.selected) % H;
        self.bytes = self.history[slot];
        self.len = self.lengths[slot];
        self.cursor = self.len;
    }

    pub fn visible_start(&self, columns: usize) -> usize {
        self.cursor.saturating_sub(columns.saturating_sub(1))
    }
}
