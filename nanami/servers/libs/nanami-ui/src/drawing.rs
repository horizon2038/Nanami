use crate::{ACCENT, CELL, CHROME, INK, RULE, SELECTED};
use alloc::vec::Vec;
use fontdue::{FontSettings, Metrics};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: usize,
    pub h: usize,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: usize, h: usize) -> Self {
        Self { x, y, w, h }
    }
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x
            && y >= self.y
            && x < self.x.saturating_add(self.w as i32)
            && y < self.y.saturating_add(self.h as i32)
    }
    pub fn intersect(self, other: Self) -> Self {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self
            .x
            .saturating_add(self.w as i32)
            .min(other.x.saturating_add(other.w as i32));
        let bottom = self
            .y
            .saturating_add(self.h as i32)
            .min(other.y.saturating_add(other.h as i32));
        Self::new(
            x,
            y,
            right.saturating_sub(x).max(0) as usize,
            bottom.saturating_sub(y).max(0) as usize,
        )
    }
}

pub struct Canvas<'a> {
    pub pixels: &'a mut [u32],
    pub width: usize,
    pub height: usize,
    pub clip: Rect,
}

impl<'a> Canvas<'a> {
    pub fn new(pixels: &'a mut [u32], width: usize, height: usize) -> Self {
        Self {
            pixels,
            width,
            height,
            clip: Rect::new(0, 0, width, height),
        }
    }
    pub fn pixel(&mut self, x: i32, y: i32, color: u32) {
        if !self.clip.contains(x, y)
            || x < 0
            || y < 0
            || x as usize >= self.width
            || y as usize >= self.height
        {
            return;
        }
        if let Some(pixel) = self.pixels.get_mut(y as usize * self.width + x as usize) {
            *pixel = color & 0xffffff;
        }
    }
    pub fn rect(&mut self, rect: Rect, color: u32) {
        let area = rect
            .intersect(self.clip)
            .intersect(Rect::new(0, 0, self.width, self.height));
        for y in area.y..area.y + area.h as i32 {
            let start = y as usize * self.width + area.x as usize;
            let end = start + area.w;
            if end <= self.pixels.len() {
                self.pixels[start..end].fill(color & 0xffffff);
            }
        }
    }
    pub fn border(&mut self, area: Rect, color: u32) {
        if area.w == 0 || area.h == 0 {
            return;
        }
        self.rect(Rect::new(area.x, area.y, area.w, 1), color);
        self.rect(
            Rect::new(area.x, area.y + area.h as i32 - 1, area.w, 1),
            color,
        );
        self.rect(Rect::new(area.x, area.y, 1, area.h), color);
        self.rect(
            Rect::new(area.x + area.w as i32 - 1, area.y, 1, area.h),
            color,
        );
    }
    pub fn line(&mut self, mut x: i32, mut y: i32, end_x: i32, end_y: i32, color: u32) {
        let dx = (end_x - x).abs();
        let dy = -(end_y - y).abs();
        let sx = if x < end_x { 1 } else { -1 };
        let sy = if y < end_y { 1 } else { -1 };
        let mut error = dx + dy;
        loop {
            self.pixel(x, y, color);
            if x == end_x && y == end_y {
                break;
            }
            let twice = error * 2;
            if twice >= dy {
                error += dy;
                x += sx;
            }
            if twice <= dx {
                error += dx;
                y += sy;
            }
        }
    }
    pub fn blend(&mut self, x: i32, y: i32, color: u32, alpha: u8) {
        if !self.clip.contains(x, y)
            || x < 0
            || y < 0
            || x as usize >= self.width
            || y as usize >= self.height
        {
            return;
        }
        if let Some(pixel) = self.pixels.get_mut(y as usize * self.width + x as usize) {
            let mut result = 0;
            for shift in [0, 8, 16] {
                result |= ((((color >> shift) & 255) * alpha as u32
                    + ((*pixel >> shift) & 255) * (255 - alpha as u32)
                    + 127)
                    / 255)
                    << shift;
            }
            *pixel = result;
        }
    }
    pub fn button(&mut self, font: &Font, area: Rect, label: &str, active: bool) {
        self.rect(area, if active { SELECTED } else { CHROME });
        if active {
            self.border(area, ACCENT);
        }
        self.text(
            font,
            area.x + 7,
            area.y + 6,
            label,
            INK,
            area.w.saturating_sub(12),
        );
    }
    pub fn text(&mut self, font: &Font, x: i32, y: i32, text: &str, color: u32, width: usize) {
        for (i, character) in text.chars().take(width / CELL).enumerate() {
            font.draw(self, x + (i * CELL) as i32, y, character, color);
        }
    }
    pub fn field(&mut self, font: &Font, area: Rect, field: &crate::Field) {
        self.rect(area, 0xffffff);
        self.border(area, RULE);
        let cells = area.w.saturating_sub(12) / CELL;
        if cells == 0 {
            return;
        }
        let cursor = field.text[..field.cursor].chars().count();
        let scroll = (cursor + 1).saturating_sub(cells);
        for (i, c) in field.text.chars().skip(scroll).take(cells).enumerate() {
            let x = area.x + 5 + (i * CELL) as i32;
            if field.selected {
                self.rect(Rect::new(x, area.y + 3, CELL, 22), SELECTED);
            }
            font.draw(self, x, area.y + 6, c, INK);
        }
        if !field.selected {
            self.rect(
                Rect::new(
                    area.x + 5 + ((cursor - scroll) * CELL) as i32,
                    area.y + 6,
                    1,
                    17,
                ),
                ACCENT,
            );
        }
    }
}

struct Glyph {
    metrics: Metrics,
    pixels: Vec<u8>,
}
pub struct Font {
    glyphs: Vec<Glyph>,
}
impl Font {
    pub fn new() -> Self {
        let font = fontdue::Font::from_bytes(
            include_bytes!("../../../apps/honoka/assets/fonts/hack.ttf") as &[u8],
            FontSettings::default(),
        )
        .expect("bundled Hack font");
        let glyphs = (32u8..127)
            .map(|c| {
                let (metrics, pixels) = font.rasterize(c as char, 14.0);
                Glyph { metrics, pixels }
            })
            .collect();
        Self { glyphs }
    }
    fn draw(&self, canvas: &mut Canvas<'_>, x: i32, y: i32, character: char, color: u32) {
        let index = if character.is_ascii() && !character.is_control() {
            character as usize - 32
        } else {
            b'?' as usize - 32
        };
        let glyph = &self.glyphs[index];
        let metrics = glyph.metrics;
        let left = x + metrics.xmin;
        let top = y + 15 - metrics.height as i32 - metrics.ymin;
        for row in 0..metrics.height {
            for column in 0..metrics.width {
                canvas.blend(
                    left + column as i32,
                    top + row as i32,
                    color,
                    glyph.pixels[row * metrics.width + column],
                );
            }
        }
    }
}
