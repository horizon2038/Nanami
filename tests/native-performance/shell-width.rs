//! Production history storage and renderer, at widths above the initial 89 columns.
#![allow(dead_code)]
extern crate alloc;
extern crate self as nanami_runtime;
pub use std::println;
pub type Word = usize;
const FONT_W: usize = 8;
const FONT_H: usize = 12;
const COLS: usize = terminal::MAX_COLUMNS;
const MAX_ROWS: usize = 128;
pub mod heap {
    pub fn heap_stats() -> (usize, usize, usize) {
        (0, 0, 0)
    }
}
#[path = "../../nanami/servers/apps/shell/src/app/font.rs"]
mod font;
#[path = "../../nanami/servers/apps/shell/src/scrollback.rs"]
mod scrollback;
#[path = "../../nanami/servers/apps/shell/src/terminal/mod.rs"]
mod terminal;

#[test]
fn completed_wide_command_keeps_its_right_hand_text_and_color() {
    let mut terminal = terminal::Terminal::new(120, 3);
    for &byte in b"\x1b[31m".iter().chain([b'W'; 110].iter()) {
        terminal.process_byte(byte);
    }
    let mut line = [0; COLS];
    let mut colors = [0; COLS];
    for (column, cell) in terminal.output_row(0).iter().enumerate() {
        line[column] = cell.byte;
        colors[column] = cell.fg;
    }
    let mut history = scrollback::Scrollback::new();
    assert_eq!(history.push_wrapped(&line, &colors, 120), 0);
    assert_eq!(history.len(), 1);
    assert_eq!(history.line(0)[109], b'W');
    assert_eq!(history.colors(0)[109], 0xcd0000);
    let (width, height) = (120 * FONT_W, FONT_H * 2);
    let background = 0x0010_1418;
    let mut pixels = vec![background; width * height + 2];
    let mut renderer = font::TextRenderer::new();
    renderer.draw_text_colored(
        pixels[1..].as_mut_ptr() as Word,
        width,
        height,
        0,
        history.line(0),
        history.colors(0),
    );
    assert!(pixels[1..1 + width * height]
        .chunks_exact(width)
        .any(|row| row[100 * FONT_W..110 * FONT_W]
            .iter()
            .any(|pixel| *pixel != background)));
    assert_eq!(pixels[0], background);
    assert_eq!(pixels[width * height + 1], background);
}

#[test]
fn builtin_lines_wrap_at_current_width_without_losing_bytes_or_colors() {
    let mut history = scrollback::Scrollback::new();
    let mut line = [0; COLS];
    let mut colors = [0; COLS];
    for column in 0..240 {
        line[column] = b'A' + (column % 26) as u8;
        colors[column] = column as u32;
    }
    history.push_wrapped(&line, &colors, 120);
    assert_eq!(history.len(), 2);
    for row in 0..2 {
        assert_eq!(&history.line(row)[..120], &line[row * 120..(row + 1) * 120]);
        assert_eq!(
            &history.colors(row)[..120],
            &colors[row * 120..(row + 1) * 120]
        );
    }
    history.clear();
    history.push_wrapped(&line, &colors, 30);
    assert_eq!(history.len(), 8);
    assert_eq!(&history.line(7)[..30], &line[210..240]);
    history.clear();
    history.push_wrapped(&[0; COLS], &[0; COLS], 120);
    assert_eq!(history.len(), 1); // a blank logical line still advances a row
}

#[test]
fn wide_history_is_heap_backed_bounded_and_reuses_evicted_rows() {
    assert!(std::mem::size_of::<scrollback::Scrollback>() < 256);
    let mut history = scrollback::Scrollback::new();
    for value in 0..MAX_ROWS {
        history.push([value as u8; COLS], [value as u32; COLS]);
    }
    let oldest = history.line(0).as_ptr();
    assert!(history.push([255; COLS], [255; COLS]));
    assert_eq!(history.len(), MAX_ROWS);
    assert_eq!(history.line(MAX_ROWS - 1).as_ptr(), oldest);
    assert_eq!(history.line(0)[0], 1);
    assert_eq!(history.line(MAX_ROWS - 1)[COLS - 1], 255);
    history.clear();
    history.push([3; COLS], [4; COLS]);
    assert_eq!(history.len(), 1);
}
