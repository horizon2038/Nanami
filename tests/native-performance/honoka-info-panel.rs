//! Pixel tests with Honoka's real font rasterizer, framebuffer and panel renderer.
#![allow(dead_code)]
extern crate alloc;
extern crate self as nanami_client;
extern crate self as nanami_runtime;
pub use std::println;
pub type Word = usize;
pub struct NanamiError;
impl NanamiError {
    pub const INVALID_ARGUMENT: Self = Self;
}
#[derive(Debug)]
pub enum RequestError {
    InvalidArgument,
    Unsupported,
    Protocol,
    Transport,
    Status(Word),
}
pub const OS_RESPONSE_OK: Word = 0;
#[path = "../../nanami/servers/libs/nanami-client/src/gpu.rs"]
pub mod gpu;
fn call_port(
    _: Word,
    _: Word,
    _: Word,
    _: Word,
    _: Word,
    _: Word,
    _: u8,
) -> Result<(Word, Word, Word), RequestError> {
    panic!("software panel must not issue GPU IPC");
}
pub fn request_mapping_release(_: Word, _: Word) -> Result<(), RequestError> {
    panic!("software panel must not release GPU memory");
}
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

fn legacy_wrap(field: &str, width: i32, mut measure: impl FnMut(&[u8]) -> i32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (end, ch) in field.char_indices() {
        if end > start && measure(field[start..end + ch.len_utf8()].as_bytes()) > width {
            lines.push(String::from(&field[start..end]));
            start = end;
        }
    }
    lines.push(String::from(&field[start..]));
    lines
}

#[test]
fn linear_wrapping_matches_previous_layout_for_real_font_and_fallback_advances() {
    let text = font::TextRenderer::new();
    for field in fields().into_iter().chain([
        String::new(),
        String::from("a"),
        String::from("Aé日本語 B\t🙂XYZ"),
        "wide and narrow chars Wi.".repeat(40),
    ]) {
        for width in [1, 7, 8, 17, 100, 1024, i32::MAX] {
            for fallback in [false, true] {
                let measure = |bytes: &[u8]| {
                    if fallback {
                        bytes.len() as i32 * 7
                    } else {
                        text.text_width(bytes)
                    }
                };
                let expected = legacy_wrap(&field, width, measure);
                let mut actual = Vec::new();
                info_panel::wrap_line(&field, width, measure, &mut actual);
                assert_eq!(actual, expected, "width={width} fallback={fallback}");
            }
        }
    }
}

#[test]
fn wrapping_measures_each_byte_once_instead_of_remeasuring_prefixes() {
    let field = "a".repeat(1024);
    let mut previous_bytes = 0;
    let expected = legacy_wrap(&field, i32::MAX, |bytes| {
        previous_bytes += bytes.len();
        bytes.len() as i32
    });
    let mut measured_bytes = 0;
    let mut actual = Vec::new();
    info_panel::wrap_line(
        &field,
        i32::MAX,
        |bytes| {
            measured_bytes += bytes.len();
            bytes.len() as i32
        },
        &mut actual,
    );
    assert_eq!(actual, expected);
    assert_eq!(measured_bytes, 1024);
    assert_eq!(previous_bytes, (1024 * 1025 / 2) - 1);
}
