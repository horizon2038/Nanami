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

// Independent full-paint ordering, clipped to the requested band. Start with
// arbitrary prior pixels so missing clears and out-of-band stores are visible.
fn reference_band(
    terminal: &terminal::Terminal, text: &mut font::TextRenderer,
    pixels: &mut [u32], width: usize, height: usize, top: usize, bottom: usize,
) {
    let bottom = bottom.min(height);
    if top >= bottom {
        return;
    }
    pixels[top * width..bottom * width].fill(0x0010_1418);
    for row in 0..terminal.rows().min(height / FONT_H) {
        let y = row * FONT_H;
        for (column, cell) in terminal.view_row(row).iter().enumerate().take(width / FONT_W) {
            let mut cell = *cell;
            if terminal.cursor_visible && terminal.at_bottom() && terminal.cursor() == (column, row) {
                std::mem::swap(&mut cell.fg, &mut cell.bg);
            }
            let x = column * FONT_W;
            for py in y.max(top)..(y + FONT_H).min(bottom) {
                pixels[py * width + x..py * width + x + FONT_W].fill(cell.bg);
            }
            text.draw_cell_in_band(pixels.as_mut_ptr() as Word, width, height,
                x, y, cell.byte, cell.fg, (top, bottom));
            if cell.underline && y + FONT_H > top && y + FONT_H <= bottom {
                let offset = (y + FONT_H - 1) * width + x;
                pixels[offset..offset + FONT_W].fill(cell.fg);
            }
        }
    }
}

#[test]
fn arbitrary_bands_match_full_order_with_margins_clamped_rows_and_prior_pixels() {
    let mut text = font::TextRenderer::new();
    let mut cases = 0;
    for (width, height, cols, rows) in [
        (713usize, 397usize, 89, 33), (83, 63, 4, 2), (17, 37, 9, 8),
        (7, 11, 1, 1), (9, 13, 1, 1), (0, 25, 3, 4),
        (49, 0, 3, 4), (33, 3100, 3, 300),
    ] {
        let mut terminal = terminal::Terminal::new(cols, rows);
        for round in 0..3 {
            for byte in b"\x1b[31;44;4m gypj_|~W\x1b[0m\r\n\x1b[32;45mABCDE\r\n" {
                terminal.process_byte(*byte);
            }
            if round == 1 {
                terminal.scroll(2);
            }
            let bands = [(0, height), (0, 1), (1, 12), (11, 13), (12, 25),
                (13, 26), (height.saturating_sub(1), height + 8),
                (height, height + 1), (25, 3), (0, 0)];
            for (top, bottom) in bands {
                let mut actual: Vec<u32> = (0..width * height + 2)
                    .map(|i| (i as u32).wrapping_mul(123457) ^ 0xa579317f).collect();
                let mut expected = actual.clone();
                render::paint(&terminal, &mut text, actual[1..].as_mut_ptr() as Word,
                    width, height, top, bottom);
                reference_band(&terminal, &mut text, &mut expected[1..1 + width * height],
                    width, height, top, bottom);
                assert_eq!(actual, expected, "{width}x{height} band={top}..{bottom} round={round}");
                cases += 1;
            }
        }
    }
    println!("SHELL-BACKGROUND oracle-cases={cases}");
}

#[test]
fn background_fill_work_for_full_and_partial_bands() {
    let terminal = terminal::Terminal::new(89, 33);
    let mut text = font::TextRenderer::new();
    let mut pixels = vec![0; 712 * 396];
    for (top, bottom) in [(0, 396), (24, 37)] {
        render::take_fill_stores();
        render::paint(&terminal, &mut text, pixels.as_mut_ptr() as Word,
            712, 396, top, bottom);
        let stores = render::take_fill_stores();
        println!("SHELL-BACKGROUND band={top}..{bottom} fill-stores={stores}");
        assert_eq!(stores, 712 * (bottom - top));
    }
}

#[test]
fn blank_rows_use_one_background_span_per_row_and_no_cell_draws() {
    let mut terminal = terminal::Terminal::new(89, 33);
    terminal.cursor_visible = false;
    let mut text = font::TextRenderer::new();
    assert!(text.space_is_blank());
    let mut pixels = vec![0xdeadbeefu32; 712 * 396];
    render::take_paint_calls();
    render::take_fill_stores();
    render::paint(&terminal,&mut text,pixels.as_mut_ptr() as Word,712,396,0,396);
    assert_eq!(render::take_paint_calls(),(33,0));
    assert_eq!(render::take_fill_stores(),712*396);
    assert!(pixels.iter().all(|pixel| *pixel==0x0010_1418));
}

#[test]
fn blank_spans_keep_background_changes_cursor_scrollback_overhang_and_band_guards() {
    let mut terminal = terminal::Terminal::new(20, 5);
    let mut text = font::TextRenderer::new();
    for _ in 0..8 { for &byte in b"g  \x1b[41m   \x1b[42;4m  \x1b[0mW  \r\n" { terminal.process_byte(byte); } }
    for &byte in b"\x1b[2;7H" { terminal.process_byte(byte); }
    let (width,height)=(163,62);
    for cursor in [false,true] { for scroll in [0,2] {
        terminal.cursor_visible=cursor;
        terminal.scroll(scroll);
        for (top,bottom) in [(0,height),(1,11),(12,13),(15,31),(height-1,height),(8,8)] {
            let mut expected=vec![0x3210_7654;width*height];
            let mut actual=vec![0xdead_beef;width*height+2];
            actual[1..1+width*height].copy_from_slice(&expected);
            reference_band(&terminal,&mut text,&mut expected,width,height,top,bottom);
            render::paint(&terminal,&mut text,actual[1..].as_mut_ptr() as Word,width,height,top,bottom);
            assert!(actual[1..1+width*height]==expected,"cursor={cursor} scroll={scroll} band={top}..{bottom}");
            assert_eq!(actual[0],0xdead_beef);
            assert_eq!(actual[width*height+1],0xdead_beef);
        }
    }}
}

#[test]
fn underlined_spaces_keep_the_original_background_glyph_and_line_order() {
    let mut terminal=terminal::Terminal::new(10,2);
    terminal.cursor_visible=false;
    for &byte in b"\x1b[4m        " { terminal.process_byte(byte); }
    let mut text=font::TextRenderer::new();
    let mut pixels=vec![0;80*24];
    render::take_paint_calls();
    render::paint(&terminal,&mut text,pixels.as_mut_ptr() as Word,80,24,0,24);
    assert_eq!(render::take_paint_calls(),(18,8));
    let mut expected=vec![0;80*24];
    reference_band(&terminal,&mut text,&mut expected,80,24,0,24);
    assert!(pixels==expected);
}
