pub const CELL: usize = 9;
pub const LINE: usize = 22;
pub const TOOLBAR: usize = 34;
pub const PATH_HEIGHT: usize = 30;
pub const STATUS: usize = 26;
pub const EDIT_TOP: usize = TOOLBAR + PATH_HEIGHT + 8;
pub const SCROLLBAR: usize = 12;
pub const BUTTONS: [(&str, usize); 7] = [
    ("New", 54),
    ("Open...", 86),
    ("Save", 62),
    ("Save as...", 110),
    ("Undo", 62),
    ("Redo", 62),
    ("Find...", 86),
];

#[derive(Clone, Copy)]
pub struct Layout {
    pub width: usize,
    pub height: usize,
    pub text_x: usize,
    pub bottom: usize,
    pub rows: usize,
    pub columns: usize,
}

impl Layout {
    pub fn new(width: usize, height: usize, lines: usize) -> Self {
        let digits = lines.max(1).ilog10() as usize + 1;
        let text_x = (digits.max(3) + 2) * CELL + 14;
        let bottom = height.saturating_sub(STATUS);
        Self {
            width,
            height,
            text_x,
            bottom,
            rows: bottom.saturating_sub(EDIT_TOP) / LINE,
            columns: width.saturating_sub(text_x + SCROLLBAR + 8) / CELL,
        }
    }

    pub fn toolbar_button(&self, x: usize, y: usize) -> Option<usize> {
        if y >= TOOLBAR {
            return None;
        }
        let mut left = 8;
        for (i, (_, width)) in BUTTONS.iter().enumerate() {
            if left + width > self.width {
                break;
            }
            if x >= left && x < left + width {
                return Some(i);
            }
            left += width;
        }
        None
    }
}
