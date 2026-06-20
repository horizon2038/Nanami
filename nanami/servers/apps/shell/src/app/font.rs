use alloc::{
    alloc::{alloc, handle_alloc_error, Layout},
    boxed::Box,
};

use fontdue::{Font, FontSettings};
use libnanami::Word;

use super::{COLS, CONTENT_HEIGHT, FONT_W};

// MIT License
// Copyright (c) 2018 Source Foundry Authors
const FONT_BYTES: &[u8] = include_bytes!("../../../honoka/assets/fonts/hack.ttf");
const FONT_SIZE: f32 = 12.0;
const FONT_BASELINE: i32 = 10;
const ENABLE_FONTDUE: bool = true;
const FIRST: usize = 32;
const COUNT: usize = 96;
const MAX_GLYPH_W: usize = 20;
const MAX_GLYPH_H: usize = 20;
const TEXT_COLOR: u32 = 0x00e8_e0cf;

#[derive(Clone, Copy)]
struct CachedGlyph {
    cached: bool,
    width: usize,
    height: usize,
    advance: usize,
    x_offset: i32,
    y_offset: i32,
    bitmap: [u8; MAX_GLYPH_W * MAX_GLYPH_H],
}

impl CachedGlyph {
    const EMPTY: Self = Self {
        cached: false,
        width: 0,
        height: 0,
        advance: FONT_W,
        x_offset: 0,
        y_offset: 0,
        bitmap: [0; MAX_GLYPH_W * MAX_GLYPH_H],
    };
}

pub struct TextRenderer {
    glyphs: *mut CachedGlyph,
    use_fontdue: bool,
}

impl TextRenderer {
    pub fn new() -> Self {
        log_heap_stats("[shell] font init begin");
        let glyphs = allocate_glyph_cache();
        let use_fontdue = if !ENABLE_FONTDUE {
            libnanami::println!("[shell] fontdue disabled; using bitmap fallback");
            false
        } else if FONT_BYTES.is_empty() {
            libnanami::println!("[shell] font missing; using bitmap fallback");
            false
        } else if let Ok(font) = Font::from_bytes(FONT_BYTES, font_settings()) {
            libnanami::println!("[shell] fontdue ready bytes={:#x}", FONT_BYTES.len());
            let font = Box::leak(Box::new(font));
            prerasterize_glyph_cache(glyphs, font);
            true
        } else {
            libnanami::println!("[shell] fontdue parse failed; using bitmap fallback");
            false
        };
        log_heap_stats("[shell] font init end");
        Self {
            glyphs,
            use_fontdue,
        }
    }

    pub fn draw_text(&mut self, vaddr: Word, fb_width: usize, y: usize, text: &[u8; COLS]) {
        let mut x = 0usize;
        let mut i = 0usize;
        while i < COLS {
            let ch = text[i];
            if ch != 0 {
                if self.use_fontdue {
                    self.draw_cached_glyph(vaddr, fb_width, x, y, ch);
                } else {
                    draw_bitmap_char(vaddr, fb_width, x, y, ch, TEXT_COLOR);
                }
            }
            x += FONT_W;
            i += 1;
        }
    }

    fn draw_cached_glyph(&mut self, vaddr: Word, fb_width: usize, x: usize, y: usize, ch: u8) {
        if !(FIRST as u8..(FIRST + COUNT) as u8).contains(&ch) {
            return;
        }
        let index = (ch as usize) - FIRST;
        let glyph = unsafe { *self.glyphs.add(index) };
        if !glyph.cached {
            return;
        }
        let base_x = x as i32 + glyph.x_offset;
        let base_y = y as i32 + glyph.y_offset;
        let mut gy = 0usize;
        while gy < glyph.height {
            let mut gx = 0usize;
            while gx < glyph.width {
                let alpha = glyph.bitmap[gy * MAX_GLYPH_W + gx];
                if alpha != 0 {
                    let px = base_x + gx as i32;
                    let py = base_y + gy as i32;
                    if px >= 0
                        && py >= 0
                        && (px as usize) < fb_width
                        && (py as usize) < CONTENT_HEIGHT
                    {
                        let background = get_pixel(vaddr, fb_width, px as usize, py as usize);
                        let color = blend_over(background, TEXT_COLOR, alpha);
                        put_pixel(vaddr, fb_width, px as usize, py as usize, color);
                    }
                }
                gx += 1;
            }
            gy += 1;
        }
    }
}

fn font_settings() -> FontSettings {
    FontSettings {
        scale: FONT_SIZE,
        load_substitutions: false,
        ..FontSettings::default()
    }
}

fn log_heap_stats(prefix: &str) {
    let (used, remaining, total) = libnanami::heap::heap_stats();
    libnanami::println!(
        "{} heap-used={:#x} heap-rem={:#x} heap-total={:#x}",
        prefix,
        used,
        remaining,
        total
    );
}

fn allocate_glyph_cache() -> *mut CachedGlyph {
    let layout = Layout::array::<CachedGlyph>(COUNT).unwrap();
    let ptr = unsafe { alloc(layout) as *mut CachedGlyph };
    if ptr.is_null() {
        handle_alloc_error(layout);
    }
    let mut i = 0usize;
    while i < COUNT {
        unsafe {
            ptr.add(i).write(CachedGlyph::EMPTY);
        }
        i += 1;
    }
    ptr
}

fn prerasterize_glyph_cache(glyphs: *mut CachedGlyph, font: &Font) {
    let mut i = 0usize;
    while i < COUNT {
        let ch = (FIRST + i) as u8;
        let glyph = rasterize_glyph(font, ch);
        unsafe {
            glyphs.add(i).write(glyph);
        }
        i += 1;
    }
}

fn rasterize_glyph(font: &Font, ch: u8) -> CachedGlyph {
    let (metrics, bitmap) = font.rasterize(ch as char, FONT_SIZE);
    let mut glyph = CachedGlyph::EMPTY;
    glyph.cached = true;
    glyph.width = metrics.width.min(MAX_GLYPH_W);
    glyph.height = metrics.height.min(MAX_GLYPH_H);
    glyph.advance = ceil_positive(metrics.advance_width).max(1) as usize;
    glyph.x_offset = metrics.xmin;
    glyph.y_offset = FONT_BASELINE - metrics.ymin - glyph.height as i32;

    let mut y = 0usize;
    while y < glyph.height {
        let mut x = 0usize;
        while x < glyph.width {
            glyph.bitmap[y * MAX_GLYPH_W + x] = bitmap[y * metrics.width + x];
            x += 1;
        }
        y += 1;
    }
    glyph
}

fn blend_over(background: u32, foreground: u32, alpha: u8) -> u32 {
    let a = alpha as u32;
    let inv = 255u32.saturating_sub(a);
    let fr = (foreground >> 16) & 0xff;
    let fg = (foreground >> 8) & 0xff;
    let fb = foreground & 0xff;
    let br = (background >> 16) & 0xff;
    let bg = (background >> 8) & 0xff;
    let bb = background & 0xff;
    let r = (fr * a + br * inv) / 255;
    let g = (fg * a + bg * inv) / 255;
    let b = (fb * a + bb * inv) / 255;
    (r << 16) | (g << 8) | b
}

fn draw_bitmap_char(vaddr: Word, fb_width: usize, x: usize, y: usize, ch: u8, color: u32) {
    let glyph = glyph5x7(ch);
    let mut gy = 0usize;
    while gy < 7 {
        let row = glyph[gy];
        let mut gx = 0usize;
        while gx < 5 {
            if ((row >> (4 - gx)) & 1) != 0 {
                put_pixel(vaddr, fb_width, x + gx + 1, y + gy + 2, color);
            }
            gx += 1;
        }
        gy += 1;
    }
}

fn glyph5x7(ch: u8) -> [u8; 7] {
    match ch {
        b'0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
        b'1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
        b'2' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f],
        b'3' => [0x1e, 0x01, 0x01, 0x0e, 0x01, 0x01, 0x1e],
        b'4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
        b'5' => [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
        b'6' => [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        b'7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        b'8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
        b'9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c],
        b'a' | b'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        b'b' | b'B' => [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e],
        b'c' | b'C' => [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e],
        b'd' | b'D' => [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e],
        b'e' | b'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f],
        b'f' | b'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        b'g' | b'G' => [0x0e, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0f],
        b'h' | b'H' => [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        b'i' | b'I' => [0x0e, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e],
        b'j' | b'J' => [0x07, 0x02, 0x02, 0x02, 0x12, 0x12, 0x0c],
        b'k' | b'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        b'l' | b'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f],
        b'm' | b'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        b'n' | b'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        b'o' | b'O' => [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        b'p' | b'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        b'q' | b'Q' => [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d],
        b'r' | b'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        b's' | b'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e],
        b't' | b'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        b'u' | b'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        b'v' | b'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04],
        b'w' | b'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0a],
        b'x' | b'X' => [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11],
        b'y' | b'Y' => [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04],
        b'z' | b'Z' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f],
        b'>' => [0x10, 0x08, 0x04, 0x02, 0x04, 0x08, 0x10],
        b'_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1f],
        b'-' => [0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00],
        b'.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x0c],
        b':' => [0x00, 0x0c, 0x0c, 0x00, 0x0c, 0x0c, 0x00],
        b'/' => [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10],
        b' ' => [0, 0, 0, 0, 0, 0, 0],
        _ => [0x1f, 0x11, 0x02, 0x04, 0x04, 0x00, 0x04],
    }
}

fn put_pixel(vaddr: Word, fb_width: usize, x: usize, y: usize, color: u32) {
    let index = y.saturating_mul(fb_width).saturating_add(x);
    unsafe {
        core::ptr::write_volatile((vaddr + (index * 4) as Word) as *mut u32, color);
    }
}

fn get_pixel(vaddr: Word, fb_width: usize, x: usize, y: usize) -> u32 {
    let index = y.saturating_mul(fb_width).saturating_add(x);
    unsafe { core::ptr::read_volatile((vaddr + (index * 4) as Word) as *const u32) }
}

fn ceil_positive(value: f32) -> i32 {
    let truncated = value as i32;
    if value > truncated as f32 {
        truncated + 1
    } else {
        truncated
    }
}
