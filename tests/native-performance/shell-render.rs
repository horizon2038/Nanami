//! Pixel equivalence between production damage rendering and the old full pass.
#![allow(dead_code)]
extern crate alloc;
extern crate self as nanami_runtime;
pub use std::println;
pub type Word = usize;
const COLS: usize = 89;
const FONT_W: usize = 8;
const FONT_H: usize = 12;
pub mod heap {
    pub fn heap_stats() -> (usize, usize, usize) {
        (0, 0, 0)
    }
}
#[path = "../../nanami/servers/apps/shell/src/app/font.rs"]
mod font;
#[path = "../../nanami/servers/apps/shell/src/terminal_render.rs"]
mod render;
#[path = "../../nanami/servers/apps/shell/src/terminal/mod.rs"]
mod terminal;

fn legacy(
    terminal: &terminal::Terminal,
    text: &mut font::TextRenderer,
    pixels: &mut [u32],
    width: usize,
    height: usize,
) {
    pixels.fill(0x0010_1418);
    for y in 0..terminal.rows().min(height / FONT_H) {
        for (x, cell) in terminal.view_row(y).iter().enumerate().take(width / FONT_W) {
            let mut cell = *cell;
            if terminal.cursor_visible && terminal.at_bottom() && terminal.cursor() == (x, y) {
                std::mem::swap(&mut cell.fg, &mut cell.bg);
            }
            for py in y * FONT_H..(y + 1) * FONT_H {
                pixels[py * width + x * FONT_W..py * width + (x + 1) * FONT_W].fill(cell.bg);
            }
            text.draw_cell(
                pixels.as_mut_ptr() as Word,
                width,
                height,
                x * FONT_W,
                y * FONT_H,
                cell.byte,
                cell.fg,
            );
            if cell.underline {
                let start = ((y + 1) * FONT_H - 1) * width + x * FONT_W;
                pixels[start..start + FONT_W].fill(cell.fg);
            }
        }
    }
}

fn damage(
    terminal: &mut terminal::Terminal,
    text: &mut font::TextRenderer,
    pixels: &mut [u32],
    width: usize,
    height: usize,
) -> usize {
    let mut row = 0;
    let mut pixel_budget = 0;
    while row < terminal.rows() {
        if terminal.take_damage(row).is_none() {
            row += 1;
            continue;
        }
        let first = row;
        row += 1;
        while row < terminal.rows() && terminal.take_damage(row).is_some() {
            row += 1;
        }
        let (top, bottom) = render::damage_band(text, first, row, height);
        pixel_budget += (bottom - top) * width;
        render::paint(
            terminal,
            text,
            pixels.as_mut_ptr() as Word,
            width,
            height,
            top,
            bottom,
        );
    }
    pixel_budget
}

#[test]
fn damage_pixels_match_full_redraw_including_glyph_overhang_cursor_and_scrollback() {
    let mut text = font::TextRenderer::new();
    let operations: &[&[u8]] = &[
        b"\r\n",
        b"gypj_|~W",
        b"\x1b[31;44;4m",
        b"\x1b[0m",
        b"\x1b[2J",
        b"\x1b[H",
        b"\x1b[2;3H",
        b"\x1b[999;999H",
        b"\x1b[2K",
        b"\x1b[1J",
        b"\x1b[?25l",
        b"\x1b[?25h",
        b"\x1b[?1049h",
        b"\x1b[?1049l",
        b"\x1b[6n",
        b"\x1b[2;4r",
        b"\x1b[r",
        b"\x1b[3S",
        b"\x1bM",
        b"\x1b[2@",
        b"\x1b[P",
        b"\x1b[1K",
        b"\x1b[1X",
    ];
    let ascii: Vec<u8> = (32..=127).collect();
    for (width, height) in [(713, 397), (83, 63), (16, 24), (9, 13)] {
        let mut terminal = terminal::Terminal::new(width / FONT_W, height / FONT_H);
        let mut actual = vec![0xaabbccdd; width * height + 2];
        let mut expected = vec![0; width * height];
        // Surface creation/resize is a mandatory full repaint, including margins.
        render::paint(
            &terminal,
            &mut text,
            actual[1..].as_mut_ptr() as Word,
            width,
            height,
            0,
            height,
        );
        for row in 0..terminal.rows() {
            terminal.take_damage(row);
        }
        let mut seed = 391u64;
        for step in 0..800 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            if step % 47 == 0 {
                terminal.scroll((seed >> 15) as i16 % 10);
            } else {
                let bytes = if step % 17 == 0 {
                    &ascii[..]
                } else {
                    operations[seed as usize % operations.len()]
                };
                for &byte in bytes {
                    terminal.process_byte(byte);
                }
            }
            damage(
                &mut terminal,
                &mut text,
                &mut actual[1..1 + width * height],
                width,
                height,
            );
            legacy(&terminal, &mut text, &mut expected, width, height);
            assert_eq!(
                &actual[1..1 + width * height],
                &expected,
                "{width}x{height} step={step}"
            );
            assert_eq!(actual[0], 0xaabbccdd);
            assert_eq!(actual[width * height + 1], 0xaabbccdd);
        }
    }
}

#[test]
fn one_character_reconstructs_a_row_band_not_the_whole_window() {
    let mut terminal = terminal::Terminal::new(89, 33);
    let mut text = font::TextRenderer::new();
    let mut pixels = vec![0; 712 * 396];
    damage(&mut terminal, &mut text, &mut pixels, 712, 396);
    terminal.process_byte(b'a');
    let pixels_reconstructed = damage(&mut terminal, &mut text, &mut pixels, 712, 396);
    assert!(pixels_reconstructed < 712 * 396 / 10);
    for &byte in b"\x1b[6n" {
        terminal.process_byte(byte);
    }
    assert_eq!(damage(&mut terminal, &mut text, &mut pixels, 712, 396), 0);
}
