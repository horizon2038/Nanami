use alloc::{collections::VecDeque, vec, vec::Vec};

const HISTORY_ROWS: usize = 128;

pub const DEFAULT_FG: u32 = 0x00e8_e0cf;
pub const DEFAULT_BG: u32 = 0x0010_1418;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub byte: u8,
    pub fg: u32,
    pub bg: u32,
    pub underline: bool,
}

impl Cell {
    pub const BLANK: Self = Self {
        byte: b' ',
        fg: DEFAULT_FG,
        bg: DEFAULT_BG,
        underline: false,
    };
}

pub struct Screen {
    pub history: VecDeque<Vec<Cell>>,
    pub history_enabled: bool,
    pub cells: Vec<Cell>,
    pub cols: usize,
    pub rows: usize,
    pub x: usize,
    pub y: usize,
    pub saved: (usize, usize),
    pub top: usize,
    pub bottom: usize,
    pub wrap_pending: bool,
}

impl Screen {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            history: VecDeque::new(),
            history_enabled: true,
            cells: vec![Cell::BLANK; cols * rows],
            cols,
            rows,
            x: 0,
            y: 0,
            saved: (0, 0),
            top: 0,
            bottom: rows - 1,
            wrap_pending: false,
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let mut next = Self::new(cols, rows);
        // Preserve the cursor's visible rows when the bottom is removed.
        let start = self.y.saturating_sub(rows - 1);
        for row in 0..start {
            self.remember_row(row);
        }
        next.history = core::mem::take(&mut self.history);
        next.history_enabled = self.history_enabled;
        for y in 0..rows.min(self.rows - start) {
            let width = cols.min(self.cols);
            next.cells[y * cols..y * cols + width].copy_from_slice(
                &self.cells[(y + start) * self.cols..(y + start) * self.cols + width],
            );
        }
        next.x = self.x.min(cols - 1);
        next.y = self.y.saturating_sub(start).min(rows - 1);
        next.saved = (
            self.saved.0.min(cols - 1),
            self.saved.1.saturating_sub(start).min(rows - 1),
        );
        *self = next;
    }

    pub fn scroll_up(&mut self, top: usize, bottom: usize, count: usize, blank: Cell) {
        let count = count.min(bottom + 1 - top);
        self.cells.copy_within(
            (top + count) * self.cols..(bottom + 1) * self.cols,
            top * self.cols,
        );
        self.cells[(bottom + 1 - count) * self.cols..(bottom + 1) * self.cols].fill(blank);
    }

    pub fn scroll_down(&mut self, top: usize, bottom: usize, count: usize, blank: Cell) {
        let count = count.min(bottom + 1 - top);
        self.cells.copy_within(
            top * self.cols..(bottom + 1 - count) * self.cols,
            (top + count) * self.cols,
        );
        self.cells[top * self.cols..(top + count) * self.cols].fill(blank);
    }

    pub fn index(&mut self, blank: Cell) {
        if self.y == self.bottom {
            if self.top == 0 && self.bottom == self.rows - 1 {
                self.remember_row(0);
            }
            self.scroll_up(self.top, self.bottom, 1, blank);
        } else {
            self.y = (self.y + 1).min(self.rows - 1);
        }
        self.wrap_pending = false;
    }

    fn remember_row(&mut self, row: usize) {
        if !self.history_enabled {
            return;
        }
        let mut cells = if self.history.len() == HISTORY_ROWS {
            self.history.pop_front().unwrap()
        } else {
            Vec::new()
        };
        cells.clear();
        cells.extend_from_slice(&self.cells[row * self.cols..(row + 1) * self.cols]);
        self.history.push_back(cells);
    }

    pub fn reverse_index(&mut self, blank: Cell) {
        if self.y == self.top {
            self.scroll_down(self.top, self.bottom, 1, blank);
        } else {
            self.y = self.y.saturating_sub(1);
        }
        self.wrap_pending = false;
    }

    pub fn insert_cells(&mut self, n: usize, blank: Cell) {
        let n = n.min(self.cols - self.x);
        let base = self.y * self.cols;
        self.cells
            .copy_within(base + self.x..base + self.cols - n, base + self.x + n);
        self.cells[base + self.x..base + self.x + n].fill(blank);
    }

    pub fn delete_cells(&mut self, n: usize, blank: Cell) {
        let n = n.min(self.cols - self.x);
        let base = self.y * self.cols;
        self.cells
            .copy_within(base + self.x + n..base + self.cols, base + self.x);
        self.cells[base + self.cols - n..base + self.cols].fill(blank);
    }

    pub fn put(&mut self, cell: Cell, blank: Cell, wrap: bool, insert: bool) {
        if self.wrap_pending && wrap {
            self.x = 0;
            self.index(blank);
        }
        if insert {
            self.insert_cells(1, blank);
        }
        self.cells[self.y * self.cols + self.x] = cell;
        if self.x + 1 == self.cols {
            self.wrap_pending = wrap;
        } else {
            self.x += 1;
        }
    }
}
