#[cfg(test)]
use fontdue::{Font, FontSettings};

use crate::framebuffer::{Framebuffer, Rect};

// MIT License
// Copyright (c) 2018 Source Foundry Authors
#[cfg(test)]
const FONT_BYTES: &[u8] = include_bytes!("../../assets/fonts/hack.ttf");
#[cfg(test)]
const TITLE_FONT_SIZE: f32 = 13.0;
#[cfg(test)]
const TITLE_BASELINE: i32 = 10;
const FALLBACK_GLYPH_WIDTH: i32 = 7;
const FIRST: usize = 32;
const COUNT: usize = 96;
const MAX_GLYPH_W: usize = 24;
const MAX_GLYPH_H: usize = 24;

#[cfg(test)]
static GLYPH_SAMPLES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

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
        advance: FALLBACK_GLYPH_WIDTH as usize,
        x_offset: 0,
        y_offset: 0,
        bitmap: [0; MAX_GLYPH_W * MAX_GLYPH_H],
    };
}

// Expand the compact generated rows at compile time, preserving the renderer's
// padded bitmap layout without any startup parsing, rasterization or allocation.
const fn title_bitmap(pixels: &[u8], width: usize, height: usize) -> [u8; MAX_GLYPH_W * MAX_GLYPH_H] {
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

include!("title_glyphs.rs");

pub struct TextRenderer {
    glyphs: &'static [CachedGlyph; COUNT],
    use_fontdue: bool,
}

impl TextRenderer {
    pub fn new() -> Self {
        Self {
            glyphs: &TITLE_GLYPHS,
            use_fontdue: true,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_title(
        &self,
        framebuffer: &Framebuffer,
        dirty: Rect,
        x: i32,
        y: i32,
        text: &[u8],
        color: u32,
        background: u32,
        opacity: u8,
    ) {
        if !self.use_fontdue {
            draw_fallback_text(framebuffer, dirty, x, y + 1, text, color, opacity);
            return;
        }

        let mut pen_x = x;
        let mut i = 0usize;
        while i < text.len() {
            let ch = text[i];
            if let Some(glyph) = self.cached_glyph(ch) {
                draw_cached_glyph(
                    framebuffer,
                    dirty,
                    pen_x,
                    y,
                    glyph,
                    color,
                    background,
                    opacity,
                );
                pen_x = pen_x.saturating_add(glyph.advance as i32);
            }
            i += 1;
        }
    }

    pub fn text_width(&self, text: &[u8]) -> i32 {
        text.iter().fold(0i32, |width, &ch| {
            let advance = if self.use_fontdue {
                self.cached_glyph(ch)
                    .map_or(0, |glyph| glyph.advance as i32)
            } else {
                FALLBACK_GLYPH_WIDTH
            };
            width.saturating_add(advance)
        })
    }

    fn cached_glyph(&self, ch: u8) -> Option<&CachedGlyph> {
        if !(FIRST as u8..(FIRST + COUNT) as u8).contains(&ch) {
            return None;
        }
        let index = (ch as usize) - FIRST;
        let glyph = &self.glyphs[index];
        if glyph.cached {
            Some(glyph)
        } else {
            None
        }
    }
}

#[cfg(test)]
fn font_settings() -> FontSettings {
    FontSettings {
        scale: TITLE_FONT_SIZE,
        load_substitutions: false,
        ..FontSettings::default()
    }
}

#[cfg(test)]
fn rasterize_glyph(font: &Font, ch: u8) -> CachedGlyph {
    let (metrics, bitmap) = font.rasterize(ch as char, TITLE_FONT_SIZE);
    let mut glyph = CachedGlyph::EMPTY;
    glyph.cached = true;
    glyph.width = metrics.width.min(MAX_GLYPH_W);
    glyph.height = metrics.height.min(MAX_GLYPH_H);
    glyph.advance = ceil_positive(metrics.advance_width).max(1) as usize;
    glyph.x_offset = metrics.xmin;
    glyph.y_offset = TITLE_BASELINE - metrics.ymin - glyph.height as i32;

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

#[allow(clippy::too_many_arguments)]
fn draw_cached_glyph(
    framebuffer: &Framebuffer,
    dirty: Rect,
    x: i32,
    y: i32,
    glyph: &CachedGlyph,
    color: u32,
    background: u32,
    opacity: u8,
) {
    let base_x = x.saturating_add(glyph.x_offset);
    let base_y = y.saturating_add(glyph.y_offset);
    let clip = dirty.x.checked_add(dirty.width).zip(dirty.y.checked_add(dirty.height));
    let (gx_start, gx_end, gy_start, gy_end) = if let Some((right, bottom)) = clip {
        let span = |base: i32, length: usize, start: i32, end: i32| {
            // Wide differences avoid overflow for offscreen glyph origins.
            // A representable exclusive clip end also excludes coordinates
            // which the old saturating pixel additions collapse to i32::MAX.
            ((start as i64 - base as i64).clamp(0, length as i64) as usize,
                (end as i64 - base as i64).clamp(0, length as i64) as usize)
        };
        let (left, right) = span(base_x, glyph.width, dirty.x, right);
        let (top, bottom) = span(base_y, glyph.height, dirty.y, bottom);
        (left, right, top, bottom)
    } else {
        // Retain the original per-pixel behavior for overflowing clip edges.
        (0, glyph.width, 0, glyph.height)
    };
    let mut gy = gy_start;
    while gy < gy_end {
        let mut gx = gx_start;
        while gx < gx_end {
            let alpha = glyph.bitmap[gy * MAX_GLYPH_W + gx];
            #[cfg(test)]
            GLYPH_SAMPLES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if alpha != 0 {
                let px = base_x.saturating_add(gx as i32);
                let py = base_y.saturating_add(gy as i32);
                if clip.is_some() || contains(dirty, px, py) {
                    framebuffer.blend_pixel(px, py, blend_over(background, color, alpha), opacity);
                }
            }
            gx += 1;
        }
        gy += 1;
    }
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

#[cfg(test)]
fn ceil_positive(value: f32) -> i32 {
    let truncated = value as i32;
    if value > truncated as f32 {
        truncated + 1
    } else {
        truncated
    }
}

fn draw_fallback_text(
    framebuffer: &Framebuffer,
    dirty: Rect,
    x: i32,
    y: i32,
    text: &[u8],
    color: u32,
    opacity: u8,
) {
    let mut i = 0usize;
    while i < text.len() {
        draw_glyph5x7(
            framebuffer,
            dirty,
            x + (i as i32) * FALLBACK_GLYPH_WIDTH,
            y,
            text[i],
            color,
            opacity,
        );
        i += 1;
    }
}

fn draw_glyph5x7(
    framebuffer: &Framebuffer,
    dirty: Rect,
    x: i32,
    y: i32,
    ch: u8,
    color: u32,
    opacity: u8,
) {
    let glyph = glyph5x7(ch);
    let mut gy = 0usize;
    while gy < 7 {
        let row = glyph[gy];
        let mut gx = 0usize;
        while gx < 5 {
            if ((row >> (4 - gx)) & 1) != 0 {
                let px = x + gx as i32;
                let py = y + gy as i32;
                if contains(dirty, px, py) {
                    framebuffer.blend_pixel(px, py, color, opacity);
                }
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
        b'-' => [0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00],
        b'_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1f],
        b'.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x0c],
        b' ' => [0, 0, 0, 0, 0, 0, 0],
        _ => [0x1f, 0x01, 0x02, 0x04, 0x04, 0x00, 0x04],
    }
}

fn contains(rect: Rect, x: i32, y: i32) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_cache_matches_fontdue_for_all_slots_and_metrics() {
        let renderer = TextRenderer::new();
        let font = Font::from_bytes(FONT_BYTES, font_settings()).unwrap();
        for index in 0..COUNT {
            let expected = rasterize_glyph(&font, (FIRST + index) as u8);
            let actual = &renderer.glyphs[index];
            assert_eq!((actual.cached, actual.width, actual.height, actual.advance,
                actual.x_offset, actual.y_offset),
                (expected.cached, expected.width, expected.height, expected.advance,
                expected.x_offset, expected.y_offset), "slot={index}");
            assert_eq!(actual.bitmap, expected.bitmap, "slot={index}");
        }
    }
    use alloc::vec;
    use crate::framebuffer::ScreenInfo;
    use core::sync::atomic::Ordering::Relaxed;

    #[test]
    fn cached_glyph_clips_samples_and_matches_legacy_pixel_walk() {
        let mut glyph = CachedGlyph::EMPTY;
        glyph.width = MAX_GLYPH_W;
        glyph.height = MAX_GLYPH_H;
        for (i, alpha) in glyph.bitmap.iter_mut().enumerate() {
            *alpha = (i as u8).wrapping_mul(37);
        }
        let screen = ScreenInfo { width: 36, height: 34, stride_bytes: 39 * 4, bits_per_pixel: 32,
            red_position: 16, red_size: 8, green_position: 8, green_size: 8,
            blue_position: 0, blue_size: 8 };
        for (x, y) in [(5, 4), (-9, -9), (29, 28), (i32::MAX - 3, i32::MAX - 3),
            (i32::MIN, i32::MIN)] {
            for dirty in [Rect::new(0, 0, 36, 34), Rect::new(7, 9, 11, 13),
                Rect::new(0, 0, 1, 1), Rect::EMPTY, Rect::new(3, 3, -2, -1),
                Rect::new(i32::MAX - 2, i32::MAX - 2, 10, 10)] {
                for opacity in [0, 1, 127, 254, 255] {
                    let mut expected = vec![0xa579317fu32; 39 * 34];
                    let mut actual = expected.clone();
                    let output = Framebuffer::new(1, actual.as_mut_ptr() as usize,
                        actual.len() * 4, screen).ok().unwrap();
                    let reference = Framebuffer::new(1, expected.as_mut_ptr() as usize,
                        expected.len() * 4, screen).ok().unwrap();
                    draw_cached_glyph(&output, dirty, x, y, &glyph, 0x12abef, 0xfedc23, opacity);
                    for gy in 0..glyph.height {
                        for gx in 0..glyph.width {
                            let alpha = glyph.bitmap[gy * MAX_GLYPH_W + gx];
                            let px = x.saturating_add(gx as i32);
                            let py = y.saturating_add(gy as i32);
                            if alpha != 0 && contains(dirty, px, py) {
                                reference.blend_pixel(px, py, blend_over(0xfedc23, 0x12abef, alpha), opacity);
                            }
                        }
                    }
                    assert_eq!(actual, expected);
                }
            }
        }
        let mut pixels = vec![0u32; 39 * 34];
        let output = Framebuffer::new(1, pixels.as_mut_ptr() as usize, pixels.len() * 4, screen).ok().unwrap();
        GLYPH_SAMPLES.store(0, Relaxed);
        draw_cached_glyph(&output, Rect::new(7, 9, 1, 1), 5, 4, &glyph, 0xffffff, 0, 255);
        let samples = GLYPH_SAMPLES.swap(0, Relaxed);
        std::println!("GLYPH-DAMAGE-CLIP bitmap=24x24 dirty=1x1 samples={samples}");
        assert_eq!(samples, 1);
    }
}
