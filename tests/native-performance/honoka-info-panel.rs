//! Pixel tests with Honoka's real font rasterizer, framebuffer and panel renderer.
#![allow(dead_code)]
extern crate alloc;
extern crate self as libnanami;
extern crate self as nanami_services;
pub use std::println;
pub type Word = usize;
pub struct NanamiError;
impl NanamiError {
    pub const INVALID_ARGUMENT: Self = Self;
}
pub struct RequestError;
pub mod heap {
    pub fn heap_stats() -> (usize, usize, usize) {
        (0, 0, 0)
    }
}
pub mod gfx {
    pub fn display_service_present(
        _: usize,
        _: usize,
        _: usize,
        _: usize,
        _: usize,
    ) -> Result<(), super::RequestError> {
        panic!("panel must not issue IPC");
    }
}
#[path = "../../nanami/servers/apps/honoka/src/app/constants.rs"]
mod constants;
#[path = "../../nanami/servers/apps/honoka/src/app/font.rs"]
mod font;
#[path = "../../nanami/servers/apps/honoka/src/app/framebuffer.rs"]
mod framebuffer;
#[path = "../../nanami/servers/apps/honoka/src/app/info_panel.rs"]
mod info_panel;
use framebuffer::{Framebuffer, Rect, ScreenInfo};

const WALLPAPER: u32 = 0x123456;
fn canvas(width: usize, height: usize) -> (Framebuffer, Vec<u32>) {
    let mut pixels = vec![WALLPAPER; width * height];
    let screen = ScreenInfo {
        width,
        height,
        stride_bytes: width * 4,
        bits_per_pixel: 32,
        red_position: 16,
        red_size: 8,
        green_position: 8,
        green_size: 8,
        blue_position: 0,
        blue_size: 8,
    };
    let fb = Framebuffer::new(1, pixels.as_mut_ptr() as usize, pixels.len() * 4, screen)
        .ok()
        .unwrap();
    (fb, pixels)
}
fn fields() -> [String; 4] {
    [
        "Kernel Version: A9N v0.3.4-smp+x86-64-Release-20260916",
        "Nanami Version: 0.1.0",
        "Architecture: x86_64",
        "Platform: pc99",
    ]
    .map(String::from)
}

#[test]
fn right_aligned_panel_is_opaque_white_with_black_text_below_clock() {
    let (fb, pixels) = canvas(800, 600);
    let text = font::TextRenderer::new();
    let panel = info_panel::InfoPanel::new(fb.screen(), &text, fields());
    panel.draw(&fb, &text, Rect::new(0, 0, 800, 600));
    let painted: Vec<_> = pixels
        .iter()
        .enumerate()
        .filter(|(_, p)| **p != WALLPAPER)
        .collect();
    assert_eq!(painted.iter().map(|(i, _)| i % 800).max(), Some(791));
    assert_eq!(painted.iter().map(|(i, _)| i / 800).min(), Some(38));
    assert!(painted.iter().any(|(_, p)| **p == 0xffffff));
    assert!(painted.iter().any(|(_, p)| **p == 0));
    for (_, &pixel) in painted {
        assert_eq!(pixel & 255, (pixel >> 8) & 255);
        assert_eq!(pixel & 255, (pixel >> 16) & 255);
    }
    assert!(pixels[..800 * constants::MENU_BAR_HEIGHT as usize]
        .iter()
        .all(|&p| p == WALLPAPER));
}

#[test]
fn tiled_redraw_matches_full_redraw_on_wide_narrow_and_tiny_screens() {
    let text = font::TextRenderer::new();
    for (width, height) in [(1920, 1080), (320, 240), (96, 600), (16, 16)] {
        let (fb, expected) = canvas(width, height);
        let (tiled, actual) = canvas(width, height);
        let panel = info_panel::InfoPanel::new(fb.screen(), &text, fields());
        panel.draw(&fb, &text, Rect::new(0, 0, width as i32, height as i32));
        for y in (0..height).step_by(17) {
            for x in (0..width).step_by(23) {
                panel.draw(&tiled, &text, Rect::new(x as i32, y as i32, 23, 17));
            }
        }
        assert_eq!(actual, expected, "{width}x{height}");
        assert_eq!(pixels_below_panel(&actual, width, height), WALLPAPER);
    }
}

fn pixels_below_panel(pixels: &[u32], width: usize, height: usize) -> u32 {
    pixels[width * height - 1]
}

#[test]
fn clipping_never_touches_pixels_outside_damage() {
    let (fb, pixels) = canvas(800, 600);
    let text = font::TextRenderer::new();
    let panel = info_panel::InfoPanel::new(fb.screen(), &text, fields());
    let dirty = Rect::new(700, 50, 21, 27);
    panel.draw(&fb, &text, dirty);
    assert!(pixels.iter().any(|&p| p != WALLPAPER));
    for (i, &pixel) in pixels.iter().enumerate() {
        if !(700..721).contains(&(i % 800)) || !(50..77).contains(&(i / 800)) {
            assert_eq!(pixel, WALLPAPER);
        }
    }
}
