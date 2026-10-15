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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_scroll_recycles_only_the_new_row_without_moving_other_cells() {
        let mut screen = Screen::new(89, 33);
        for row in 0..screen.rows {
            screen.row_mut(row).fill(Cell {
                byte: row as u8,
                ..Cell::BLANK
            });
        }
        let backing = screen.cells.clone();
        let pointer = screen.cells.as_ptr();
        screen.scroll_up(0, 32, 1, Cell::BLANK);
        assert_eq!(screen.cells.as_ptr(), pointer);
        assert_eq!(&screen.cells[89..], &backing[89..]);
        assert!(screen.row(32).iter().all(|cell| *cell == Cell::BLANK));
        screen.scroll_down(0, 32, 1, Cell::BLANK);
        assert_eq!(&screen.cells[89..], &backing[89..]);
    }
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
    first_row: usize,
    damage: Vec<(usize, usize)>,
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
            first_row: 0,
            damage: vec![(0, cols); rows],
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
            next.row_mut(y)[..width].copy_from_slice(&self.row(y + start)[..width]);
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
        if count == 0 {
            return;
        }
        if top == 0 && bottom == self.rows - 1 {
            self.first_row = (self.first_row + count) % self.rows;
        } else {
            for row in top..bottom + 1 - count {
                self.copy_row(row + count, row);
            }
        }
        for row in bottom + 1 - count..=bottom {
            self.row_mut(row).fill(blank);
        }
        self.mark_rows(top, bottom);
    }

    pub fn scroll_down(&mut self, top: usize, bottom: usize, count: usize, blank: Cell) {
        let count = count.min(bottom + 1 - top);
        if count == 0 {
            return;
        }
        if top == 0 && bottom == self.rows - 1 {
            self.first_row = (self.first_row + self.rows - count) % self.rows;
        } else {
            for row in (top + count..=bottom).rev() {
                self.copy_row(row - count, row);
            }
        }
        for row in top..top + count {
            self.row_mut(row).fill(blank);
        }
        self.mark_rows(top, bottom);
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
        cells.extend_from_slice(self.row(row));
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
        let (x, cols) = (self.x, self.cols);
        let row = self.row_mut(self.y);
        row.copy_within(x..cols - n, x + n);
        row[x..x + n].fill(blank);
        self.mark(self.y, x, cols);
    }

    pub fn delete_cells(&mut self, n: usize, blank: Cell) {
        let n = n.min(self.cols - self.x);
        let (x, cols) = (self.x, self.cols);
        let row = self.row_mut(self.y);
        row.copy_within(x + n..cols, x);
        row[cols - n..].fill(blank);
        self.mark(self.y, x, cols);
    }

    pub fn put(&mut self, cell: Cell, blank: Cell, wrap: bool, insert: bool) {
        if self.wrap_pending && wrap {
            self.x = 0;
            self.index(blank);
        }
        if insert {
            self.insert_cells(1, blank);
        }
        let x = self.x;
        self.row_mut(self.y)[x] = cell;
        self.mark(self.y, x, x + 1);
        if self.x + 1 == self.cols {
            self.wrap_pending = wrap;
        } else {
            self.x += 1;
        }
    }

    fn row_offset(&self, row: usize) -> usize {
        assert!(row < self.rows);
        ((self.first_row + row) % self.rows) * self.cols
    }

    pub fn row(&self, row: usize) -> &[Cell] {
        let base = self.row_offset(row);
        &self.cells[base..base + self.cols]
    }

    fn row_mut(&mut self, row: usize) -> &mut [Cell] {
        let base = self.row_offset(row);
        &mut self.cells[base..base + self.cols]
    }

    fn copy_row(&mut self, source: usize, destination: usize) {
        let source = self.row_offset(source);
        let destination = self.row_offset(destination);
        self.cells
            .copy_within(source..source + self.cols, destination);
    }

    pub fn mark(&mut self, row: usize, start: usize, end: usize) {
        if start >= end || row >= self.rows {
            return;
        }
        let damage = &mut self.damage[row];
        if damage.0 == damage.1 {
            *damage = (start, end);
        } else {
            damage.0 = damage.0.min(start);
            damage.1 = damage.1.max(end);
        }
    }

    pub fn mark_rows(&mut self, top: usize, bottom: usize) {
        self.damage[top..=bottom].fill((0, self.cols));
    }

    pub fn take_damage(&mut self, row: usize) -> Option<(usize, usize)> {
        let span = core::mem::replace(&mut self.damage[row], (0, 0));
        (span.0 != span.1).then_some(span)
    }

    pub fn erase_line(&mut self, row: usize, start: usize, end: usize, blank: Cell) {
        self.row_mut(row)[start..end].fill(blank);
        self.mark(row, start, end);
    }

    pub fn erase_display(&mut self, mode: usize, blank: Cell) {
        match mode {
            0 => {
                self.erase_line(self.y, self.x, self.cols, blank);
                for row in self.y + 1..self.rows {
                    self.erase_line(row, 0, self.cols, blank);
                }
            }
            1 => {
                for row in 0..self.y {
                    self.erase_line(row, 0, self.cols, blank);
                }
                self.erase_line(self.y, 0, self.x + 1, blank);
            }
            2 => {
                self.cells.fill(blank);
                self.mark_rows(0, self.rows - 1);
            }
            3 => self.history.clear(),
            _ => {}
        }
    }
}
