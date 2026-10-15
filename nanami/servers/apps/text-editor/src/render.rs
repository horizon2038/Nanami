use crate::{
    app::{Editor, Field, Mode},
    document::advance_column,
    layout::*,
};
use alloc::{
    collections::{BTreeMap, BTreeSet},
    format,
    vec::Vec,
};
use fontdue::{Font, FontSettings, Metrics};

const FONT_BYTES: &[u8] = include_bytes!("../../honoka/assets/fonts/hack.ttf");
const PAPER: u32 = 0xfbfaf7;
const INK: u32 = 0x302e2b;
const MUTED: u32 = 0x77736c;
const CHROME: u32 = 0xefede7;
const RULE: u32 = 0xd8d4ca;
const ACCENT: u32 = 0x8b683a;
const SELECTION: u32 = 0xe6d6b9;
const ERROR: u32 = 0x9d342b;

struct Glyph {
    metrics: Metrics,
    pixels: Vec<u8>,
}

pub struct Renderer {
    glyphs: BTreeMap<char, Glyph>,
    missing: BTreeSet<char>,
}

impl Renderer {
    pub fn new() -> Self {
        let font =
            Font::from_bytes(FONT_BYTES, FontSettings::default()).expect("bundled Hack font");
        let mut glyphs = BTreeMap::new();
        for byte in 32u8..127 {
            let (metrics, pixels) = font.rasterize(byte as char, 14.0);
            glyphs.insert(byte as char, Glyph { metrics, pixels });
        }
        // The relatively large parsed font is not retained by this small app.
        Self {
            glyphs,
            missing: BTreeSet::new(),
        }
    }

    fn prepare(&mut self, editor: &Editor) {
        let mut needed = BTreeSet::new();
        let mut check = |c: char| {
            if !c.is_control() && !self.glyphs.contains_key(&c) && !self.missing.contains(&c) {
                if needed.len() + self.glyphs.len() + self.missing.len() < 512 {
                    needed.insert(c);
                }
            }
        };
        let doc = &editor.document;
        for line in editor.top..(editor.top + editor.layout.rows).min(doc.line_count()) {
            let mut at = doc.line_start(line);
            let mut column = 0;
            while at < doc.line_end(line) && column <= editor.left + editor.layout.columns {
                let c = doc.character(at);
                if column >= editor.left {
                    check(c);
                }
                column = advance_column(column, c);
                at = doc.next(at);
            }
        }
        if let Some(path) = &editor.path {
            for c in path.chars() {
                check(c);
            }
        }
        match &editor.mode {
            Mode::Path { field, .. } | Mode::Find(field) => {
                for c in field.text.chars() {
                    check(c);
                }
            }
            _ => {}
        }
        if !needed.is_empty() {
            let font =
                Font::from_bytes(FONT_BYTES, FontSettings::default()).expect("bundled Hack font");
            for c in needed {
                if font.lookup_glyph_index(c) == 0 {
                    self.missing.insert(c);
                } else {
                    let (metrics, pixels) = font.rasterize(c, 14.0);
                    self.glyphs.insert(c, Glyph { metrics, pixels });
                }
            }
        }
    }

    pub fn draw(&mut self, pixels: &mut [u32], editor: &Editor) {
        self.prepare(editor);
        let layout = editor.layout;
        let mut canvas = Canvas {
            pixels,
            width: layout.width,
            height: layout.height,
        };
        canvas.rect(0, 0, layout.width, layout.height, PAPER);
        canvas.rect(0, 0, layout.width, TOOLBAR, CHROME);
        canvas.rect(0, TOOLBAR - 1, layout.width, 1, RULE);
        let mut x = 8;
        for (label, width) in BUTTONS {
            if x + width > layout.width {
                break;
            }
            self.text(&mut canvas, x + 8, 7, label, INK, x + width);
            x += width;
        }
        match &editor.mode {
            Mode::Path { save, field } => {
                self.field(
                    &mut canvas,
                    if *save { "Save as" } else { "Open" },
                    field,
                    if *save { "Save" } else { "Open" },
                );
            }
            Mode::Find(field) => {
                self.field(&mut canvas, "Find", field, "Next");
            }
            _ => {
                let mark = if editor.document.dirty() { "* " } else { "" };
                self.text(
                    &mut canvas,
                    16,
                    TOOLBAR + 6,
                    &format!("{}{}", mark, editor.path.as_deref().unwrap_or("Untitled")),
                    MUTED,
                    layout.width.saturating_sub(12),
                );
            }
        }
        canvas.rect(0, TOOLBAR + PATH_HEIGHT - 1, layout.width, 1, RULE);
        let doc = &editor.document;
        let (start, end) = doc.selection();
        canvas.rect(
            0,
            EDIT_TOP - 8,
            layout.text_x.saturating_sub(8),
            layout.bottom.saturating_sub(EDIT_TOP - 8),
            CHROME,
        );
        for row in 0..layout.rows {
            let line = editor.top + row;
            if line >= doc.line_count() {
                break;
            }
            let y = EDIT_TOP + row * LINE;
            let number = format!("{}", line + 1);
            self.text(
                &mut canvas,
                layout.text_x.saturating_sub(18 + number.len() * CELL),
                y + 2,
                &number,
                MUTED,
                layout.text_x.saturating_sub(10),
            );
            let mut at = doc.line_start(line);
            let mut column = 0;
            while at < doc.line_end(line) && column < editor.left + layout.columns {
                let c = doc.character(at);
                let next_column = advance_column(column, c);
                for cell in column.max(editor.left)..next_column.min(editor.left + layout.columns) {
                    let x = layout.text_x + (cell - editor.left) * CELL;
                    if at >= start && at < end {
                        canvas.rect(x, y, CELL, LINE, SELECTION);
                    }
                    if cell == column && c != '\t' {
                        self.character(&mut canvas, x, y + 2, c, INK, x + CELL);
                    }
                }
                at = doc.next(at);
                column = next_column;
            }
            // Show selected line breaks, including the empty line before a newline.
            if doc.line_end(line) >= start
                && doc.line_end(line) < end
                && column >= editor.left
                && column < editor.left + layout.columns
            {
                canvas.rect(
                    layout.text_x + (column - editor.left) * CELL,
                    y,
                    CELL,
                    LINE,
                    SELECTION,
                );
            }
        }
        if matches!(editor.mode, Mode::Edit) {
            let line = doc.line_for(doc.cursor);
            let column = doc.column(doc.cursor);
            if line >= editor.top
                && line < editor.top + layout.rows
                && column >= editor.left
                && column < editor.left + layout.columns
            {
                canvas.rect(
                    layout.text_x + (column - editor.left) * CELL,
                    EDIT_TOP + (line - editor.top) * LINE + 2,
                    2,
                    17,
                    ACCENT,
                );
            }
        }
        let track = layout.rows * LINE;
        if track > 0 && doc.line_count() > layout.rows {
            let x = layout.width.saturating_sub(SCROLLBAR);
            canvas.rect(x, EDIT_TOP, SCROLLBAR, track, CHROME);
            let thumb = (track * layout.rows / doc.line_count()).max(18).min(track);
            let y = EDIT_TOP
                + (track - thumb) * editor.top
                    / doc.line_count().saturating_sub(layout.rows).max(1);
            canvas.rect(x + 3, y, 5, thumb, 0xb6b0a4);
        }
        canvas.rect(0, layout.bottom, layout.width, STATUS, CHROME);
        canvas.rect(0, layout.bottom, layout.width, 1, RULE);
        let position = format!(
            "Ln {}, Col {}   UTF-8 / {}",
            doc.line_for(doc.cursor) + 1,
            doc.column(doc.cursor) + 1,
            if doc.crlf { "CRLF" } else { "LF" }
        );
        self.text(
            &mut canvas,
            12,
            layout.bottom + 5,
            &position,
            MUTED,
            layout.width,
        );
        let status = if doc.dirty() && !editor.error {
            "Modified"
        } else {
            &editor.status
        };
        let available = layout.width.saturating_sub(position.len() * CELL + 36) / CELL;
        let length = status.chars().count().min(available);
        let status_x = layout.width.saturating_sub(length * CELL + 12);
        self.text(
            &mut canvas,
            status_x,
            layout.bottom + 5,
            status,
            if editor.error { ERROR } else { MUTED },
            layout.width.saturating_sub(8),
        );

        if matches!(editor.mode, Mode::Confirm(_) | Mode::Overwrite(_)) {
            let (left, top, width) = editor.dialog_bounds();
            canvas.rect(left, top, width, 126, RULE);
            canvas.rect(left + 1, top + 1, width.saturating_sub(2), 124, PAPER);
            let message = if matches!(editor.mode, Mode::Confirm(_)) {
                "Save changes before continuing?"
            } else {
                "Replace the existing file?"
            };
            self.text(
                &mut canvas,
                left + 16,
                top + 16,
                message,
                INK,
                left + width.saturating_sub(12),
            );
            self.text(
                &mut canvas,
                left + 16,
                top + 46,
                "Esc cancels; your text stays here.",
                MUTED,
                left + width.saturating_sub(12),
            );
            let choices: &[&str] = if matches!(editor.mode, Mode::Confirm(_)) {
                &["Save [Enter]", "Discard [D]", "Cancel [Esc]"]
            } else {
                &["Replace [Enter]", "Cancel [Esc]"]
            };
            for (i, label) in choices.iter().enumerate() {
                let x = left + i * width / choices.len();
                canvas.rect(
                    x + 6,
                    top + 82,
                    (width / choices.len()).saturating_sub(12),
                    32,
                    CHROME,
                );
                self.text(
                    &mut canvas,
                    x + 12,
                    top + 89,
                    label,
                    if i == 0 { ACCENT } else { INK },
                    left + (i + 1) * width / choices.len(),
                );
            }
        }
    }

    fn field(&self, canvas: &mut Canvas<'_>, label: &str, field: &Field, action: &str) {
        let left = (label.len() + 2) * CELL + 12;
        let right = canvas.width.saturating_sub(98);
        self.text(canvas, 12, TOOLBAR + 6, label, MUTED, left);
        canvas.rect(left, TOOLBAR + 3, right.saturating_sub(left), 23, 0xffffff);
        canvas.rect(left, TOOLBAR + 26, right.saturating_sub(left), 1, ACCENT);
        let cells = right.saturating_sub(left + 10) / CELL;
        let cursor_column = field.text[..field.cursor].chars().count();
        let scroll = (cursor_column + 1).saturating_sub(cells);
        for (i, c) in field.text.chars().skip(scroll).take(cells).enumerate() {
            let x = left + 4 + i * CELL;
            if field.selected {
                canvas.rect(x, TOOLBAR + 4, CELL, 21, SELECTION);
            }
            self.character(canvas, x, TOOLBAR + 6, c, INK, right);
        }
        if !field.selected && cells > 0 {
            canvas.rect(
                left + 4 + (cursor_column - scroll) * CELL,
                TOOLBAR + 6,
                1,
                17,
                ACCENT,
            );
        }
        self.text(
            canvas,
            right + 12,
            TOOLBAR + 6,
            action,
            ACCENT,
            canvas.width,
        );
    }

    fn text(
        &self,
        canvas: &mut Canvas<'_>,
        mut x: usize,
        y: usize,
        text: &str,
        color: u32,
        right: usize,
    ) {
        for c in text.chars() {
            if x + CELL > right {
                break;
            }
            self.character(canvas, x, y, c, color, right);
            x += CELL;
        }
    }

    fn character(
        &self,
        canvas: &mut Canvas<'_>,
        x: usize,
        y: usize,
        c: char,
        color: u32,
        right: usize,
    ) {
        if let Some(glyph) = self.glyphs.get(&c) {
            let metrics = glyph.metrics;
            let top = y as isize + 15 - metrics.height as isize - metrics.ymin as isize;
            let left = x as isize + metrics.xmin as isize;
            for row in 0..metrics.height {
                for col in 0..metrics.width {
                    let px = left + col as isize;
                    let py = top + row as isize;
                    if px >= 0 && py >= 0 && px < right as isize {
                        canvas.blend(
                            px as usize,
                            py as usize,
                            color,
                            glyph.pixels[row * metrics.width + col],
                        );
                    }
                }
            }
        } else if !c.is_control() {
            // Missing glyphs are visible; the underlying UTF-8 bytes are never replaced.
            canvas.rect(x + 1, y + 3, 6, 1, MUTED);
            canvas.rect(x + 1, y + 13, 6, 1, MUTED);
            canvas.rect(x + 1, y + 3, 1, 11, MUTED);
            canvas.rect(x + 6, y + 3, 1, 11, MUTED);
        }
    }
}

struct Canvas<'a> {
    pixels: &'a mut [u32],
    width: usize,
    height: usize,
}

impl Canvas<'_> {
    fn rect(&mut self, x: usize, y: usize, width: usize, height: usize, color: u32) {
        for row in y..y.saturating_add(height).min(self.height) {
            let start = row.saturating_mul(self.width).saturating_add(x);
            let end = row
                .saturating_mul(self.width)
                .saturating_add(x.saturating_add(width).min(self.width));
            if start <= end && end <= self.pixels.len() {
                self.pixels[start..end].fill(color);
            }
        }
    }

    fn blend(&mut self, x: usize, y: usize, color: u32, alpha: u8) {
        if x >= self.width || y >= self.height || alpha == 0 {
            return;
        }
        if let Some(pixel) = self
            .pixels
            .get_mut(y.saturating_mul(self.width).saturating_add(x))
        {
            let mut result = 0;
            for shift in [0, 8, 16] {
                let front = (color >> shift) & 255;
                let back = (*pixel >> shift) & 255;
                result |=
                    ((front * alpha as u32 + back * (255 - alpha as u32) + 127) / 255) << shift;
            }
            *pixel = result;
        }
    }
}
