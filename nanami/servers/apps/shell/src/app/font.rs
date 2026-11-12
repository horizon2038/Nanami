#[cfg(test)]
use fontdue::{Font, FontSettings};
use nanami_runtime::Word;

use super::{COLS, FONT_W};

// MIT License
// Copyright (c) 2018 Source Foundry Authors
#[cfg(test)]
const FONT_BYTES: &[u8] = include_bytes!("../../../honoka/assets/fonts/hack.ttf");
#[cfg(test)]
const FONT_SIZE: f32 = 12.0;
#[cfg(test)]
const FONT_BASELINE: i32 = 10;
const FIRST: usize = 32;
const COUNT: usize = 96;
const MAX_GLYPH_W: usize = 20;
const MAX_GLYPH_H: usize = 20;
#[cfg(test)]
static GLYPH_SAMPLES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
pub const DEFAULT_TEXT_COLOR: u32 = 0x00e8_e0cf;

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

#[cfg(test)]
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

// Preserve the padded drawing layout while expanding only at compile time.
const fn shell_bitmap(pixels: &[u8], width: usize, height: usize) -> [u8; MAX_GLYPH_W * MAX_GLYPH_H] {
    assert!(width <= MAX_GLYPH_W && height <= MAX_GLYPH_H);
    assert!(pixels.len() == width * height);
    let mut bitmap = [0; MAX_GLYPH_W * MAX_GLYPH_H];
    let mut y = 0;
    while y < height {
        let mut x = 0;
        while x < width {
            bitmap[y * MAX_GLYPH_W + x] = pixels[y * width + x];
            x += 1;
        }
        y += 1;
    }
    bitmap
}

include!("shell_glyphs.rs");

const fn shell_ink_bounds() -> (i32, i32) {
    let mut bounds = (0, 12);
    let mut index = 0;
    while index < COUNT {
        let glyph = &SHELL_GLYPHS[index];
        if glyph.cached {
            if glyph.y_offset < bounds.0 {
                bounds.0 = glyph.y_offset;
            }
            let bottom = glyph.y_offset + glyph.height as i32;
            if bottom > bounds.1 {
                bounds.1 = bottom;
            }
        }
        index += 1;
    }
    bounds
}
const SHELL_INK_BOUNDS: (i32, i32) = shell_ink_bounds();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipped_glyphs_match_legacy_pixels_for_all_slots_and_bands() {
        let mut renderer = TextRenderer::new();
        for (width, height) in [(1, 1), (7, 9), (23, 25)] {
            for x in [0, 1, width - 1, width, width + 9] {
                for y in [0, 1, height - 1, height, height + 9] {
                    for band in [(0, height), (0, 1), (1, height), (height / 2, height / 2 + 1), (height, height), (height + 1, usize::MAX), (height, 0)] {
                        for (index, glyph) in SHELL_GLYPHS.iter().enumerate() {
                            let mut pixels: Vec<u32> = (0..width * height + 4).map(|i| (i as u32).wrapping_mul(0x9e3779b9)).collect();
                            let mut expected = pixels.clone();
                            let color = 0x12abef;
                            let bx = x as i32 + glyph.x_offset;
                            let by = y as i32 + glyph.y_offset;
                            for gy in 0..glyph.height {
                                for gx in 0..glyph.width {
                                    let alpha = glyph.bitmap[gy * MAX_GLYPH_W + gx];
                                    let px = bx + gx as i32; let py = by + gy as i32;
                                    if alpha != 0 && px >= 0 && py >= 0 && (px as usize) < width && (py as usize) < height && (py as usize) >= band.0 && (py as usize) < band.1 {
                                        let p = py as usize * width + px as usize;
                                        expected[p] = blend_over(expected[p], color, alpha);
                                    }
                                }
                            }
                            renderer.draw_cached_glyph(pixels.as_mut_ptr() as Word, width, height, x, y, (FIRST + index) as u8, color, band);
                            assert_eq!(pixels, expected, "slot={index} size={width}x{height} x={x} y={y} band={band:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn clipped_glyph_only_samples_visible_band() {
        let mut renderer = TextRenderer::new();
        let mut pixels = [0x123456u32; 80 * 40];
        let glyph = &SHELL_GLYPHS[b'W' as usize - FIRST];
        let y = 10;
        let row = (y as i32 + glyph.y_offset) as usize;
        GLYPH_SAMPLES.store(0, core::sync::atomic::Ordering::Relaxed);
        renderer.draw_cached_glyph(pixels.as_mut_ptr() as Word, 80, 40, 10, y, b'W', 0xabcdef, (row, row + 1));
        assert_eq!(GLYPH_SAMPLES.swap(0, core::sync::atomic::Ordering::Relaxed), glyph.width);
        renderer.draw_cached_glyph(pixels.as_mut_ptr() as Word, 80, 40, 80, y, b'W', 0xabcdef, (0, 40));
        assert_eq!(GLYPH_SAMPLES.swap(0, core::sync::atomic::Ordering::Relaxed), 0);
        std::println!("SHELL-GLYPH-CLIP old_samples={} band_samples={} offscreen_samples=0", glyph.width * glyph.height, glyph.width);
    }

    #[test]
    fn static_glyphs_match_live_fontdue_all_metrics_and_bitmap_bytes() {
        let font = Font::from_bytes(FONT_BYTES, font_settings()).unwrap();
        for (index, actual) in SHELL_GLYPHS.iter().enumerate() {
            let expected = rasterize_glyph(&font, (FIRST + index) as u8);
            assert_eq!((actual.cached, actual.width, actual.height, actual.advance,
                actual.x_offset, actual.y_offset),
                (expected.cached, expected.width, expected.height, expected.advance,
                expected.x_offset, expected.y_offset), "slot={index}");
            assert_eq!(actual.bitmap, expected.bitmap, "slot={index}");
        }
        let renderer = TextRenderer::new();
        let expected_bounds = SHELL_GLYPHS.iter().filter(|glyph| glyph.cached)
            .fold((0i32, 12i32), |bounds, glyph| (
                bounds.0.min(glyph.y_offset),
                bounds.1.max(glyph.y_offset + glyph.height as i32),
            ));
        assert_eq!(renderer.row_ink_bounds(), expected_bounds);
        std::println!("SHELL-STATIC-FONT slots={} cache-bytes={} row-ink={expected_bounds:?}",
            COUNT, core::mem::size_of_val(&SHELL_GLYPHS));
    }
}

pub struct TextRenderer {
    glyphs: &'static [CachedGlyph; COUNT],
    use_fontdue: bool,
    row_ink_bounds: (i32, i32),
}

impl TextRenderer {
    pub fn new() -> Self {
        log_heap_stats("[shell] font init begin");
        log_heap_stats("[shell] font init end");
        Self {
            glyphs: &SHELL_GLYPHS,
            use_fontdue: true,
            row_ink_bounds: SHELL_INK_BOUNDS,
        }
    }

    pub fn draw_text_colored(
        &mut self,
        vaddr: Word,
        fb_width: usize,
        fb_height: usize,
        y: usize,
        text: &[u8; COLS],
        colors: &[u32; COLS],
    ) {
        let mut x = 0usize;
        let mut i = 0usize;
        while i < COLS.min(fb_width / FONT_W) {
            let ch = text[i];
            if ch != 0 {
                if self.use_fontdue {
                    self.draw_cached_glyph(
                        vaddr,
                        fb_width,
                        fb_height,
                        x,
                        y,
                        ch,
                        colors[i],
                        (0, fb_height),
                    );
                } else {
                    draw_bitmap_char(vaddr, fb_width, x, y, ch, colors[i]);
                }
            }
            x += FONT_W;
            i += 1;
        }
    }

    fn draw_cached_glyph(
        &mut self,
        vaddr: Word,
        fb_width: usize,
        fb_height: usize,
        x: usize,
        y: usize,
        ch: u8,
        text_color: u32,
        band: (usize, usize),
    ) {
        if !(FIRST as u8..(FIRST + COUNT) as u8).contains(&ch) {
            return;
        }
        let index = (ch as usize) - FIRST;
        let glyph = &self.glyphs[index];
        if !glyph.cached {
            return;
        }
        let base_x = x as i32 + glyph.x_offset;
        let base_y = y as i32 + glyph.y_offset;
        // Only nonnegative, representable i32 pixel coordinates can pass the
        // old bounds checks. Wide differences also handle offscreen origins
        // and usize-sized framebuffer/band limits without narrowing overflow.
        let span = |base: i32, length: usize, start: usize, end: usize| {
            let limit = i32::MAX as usize + 1;
            ((start.min(limit) as i64 - base as i64).clamp(0, length as i64) as usize,
             (end.min(limit) as i64 - base as i64).clamp(0, length as i64) as usize)
        };
        let (left, right) = span(base_x, glyph.width, 0, fb_width);
        let (top, bottom) = span(base_y, glyph.height, band.0, fb_height.min(band.1));
        let mut gy = top;
        while gy < bottom {
            let py = (base_y as i64 + gy as i64) as usize;
            let mut gx = left;
            while gx < right {
                let alpha = glyph.bitmap[gy * MAX_GLYPH_W + gx];
                #[cfg(test)]
                GLYPH_SAMPLES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if alpha != 0 {
                    let px = (base_x as i64 + gx as i64) as usize;
                    let background = get_pixel(vaddr, fb_width, px, py);
                    let color = blend_over(background, text_color, alpha);
                    put_pixel(vaddr, fb_width, px, py, color);
                }
                gx += 1;
            }
            gy += 1;
        }
    }

    pub fn draw_cell(
        &mut self,
        vaddr: Word,
        width: usize,
        height: usize,
        x: usize,
        y: usize,
        ch: u8,
        color: u32,
    ) {
        self.draw_cell_in_band(vaddr, width, height, x, y, ch, color, (0, height));
    }

    pub fn row_ink_bounds(&self) -> (i32, i32) {
        self.row_ink_bounds
    }

    pub fn space_is_blank(&self) -> bool {
        if !self.use_fontdue { return true; } // Bitmap fallback has no space ink.
        let space = &self.glyphs[b' ' as usize - FIRST];
        !space.cached || space.width == 0 || space.height == 0
    }

    pub fn draw_cell_in_band(
        &mut self,
        vaddr: Word,
        width: usize,
        height: usize,
        x: usize,
        y: usize,
        ch: u8,
        color: u32,
        band: (usize, usize),
    ) {
        if self.use_fontdue {
            self.draw_cached_glyph(vaddr, width, height, x, y, ch, color, band);
        } else if x + FONT_W <= width && y + 12 <= height {
            draw_bitmap_char_in_band(vaddr, width, x, y, ch, color, band);
        }
    }
}

#[cfg(test)]
fn font_settings() -> FontSettings {
    FontSettings {
        scale: FONT_SIZE,
        load_substitutions: false,
        ..FontSettings::default()
    }
}

fn log_heap_stats(prefix: &str) {
    let (used, remaining, total) = nanami_runtime::heap::heap_stats();
    nanami_runtime::println!(
        "{} heap-used={:#x} heap-rem={:#x} heap-total={:#x}",
        prefix,
        used,
        remaining,
        total
    );
}

#[cfg(test)]
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
    draw_bitmap_char_in_band(vaddr, fb_width, x, y, ch, color, (0, usize::MAX));
}

fn draw_bitmap_char_in_band(
    vaddr: Word,
    fb_width: usize,
    x: usize,
    y: usize,
    ch: u8,
    color: u32,
    band: (usize, usize),
) {
    let glyph = glyph5x7(ch);
    let mut gy = 0usize;
    while gy < 7 {
        let row = glyph[gy];
        let mut gx = 0usize;
        while gx < 5 {
            if ((row >> (4 - gx)) & 1) != 0 && y + gy + 2 >= band.0 && y + gy + 2 < band.1 {
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

#[cfg(test)]
fn ceil_positive(value: f32) -> i32 {
    let truncated = value as i32;
    if value > truncated as f32 {
        truncated + 1
    } else {
        truncated
    }
}
