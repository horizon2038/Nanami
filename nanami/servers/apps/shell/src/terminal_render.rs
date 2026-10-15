//! Reconstruct a horizontal damage band in the same order as a full redraw.
use crate::{font::TextRenderer, terminal::Terminal, FONT_H, FONT_W};
use nanami_runtime::Word;

pub(super) fn damage_band(
    text: &TextRenderer,
    first: usize,
    end: usize,
    height: usize,
) -> (usize, usize) {
    let (ink_top, ink_bottom) = text.row_ink_bounds();
    let top = (first as i64 * FONT_H as i64 + i64::from(ink_top.min(0))).max(0) as usize;
    let bottom =
        ((end - 1) as i64 * FONT_H as i64 + i64::from(ink_bottom.max(FONT_H as i32))) as usize;
    (top.min(height), bottom.min(height))
}

pub(super) fn paint(
    terminal: &Terminal,
    text: &mut TextRenderer,
    pixels: Word,
    width: usize,
    height: usize,
    top: usize,
    bottom: usize,
) {
    let bottom = bottom.min(height);
    if top >= bottom {
        return;
    }
    fill(pixels, width, 0, top, width, bottom, 0x0010_1418);
    let (ink_top, ink_bottom) = text.row_ink_bounds();
    // A glyph can extend beyond its cell. Include those neighboring source
    // rows, but clip all writes so an unchanged antialiased pixel is not blended
    // twice. Cell backgrounds and glyphs retain their full-redraw paint order.
    let first = ((top as i64 - ink_bottom as i64).div_euclid(FONT_H as i64) + 1).max(0) as usize;
    let end = ((bottom as i64 - 1 - ink_top as i64).div_euclid(FONT_H as i64) + 1).max(0) as usize;
    let end = end.min(terminal.rows()).min(height / FONT_H);
    let cursor = terminal.cursor();
    for row in first..end {
        let y = row * FONT_H;
        for (column, cell) in terminal
            .view_row(row)
            .iter()
            .enumerate()
            .take(width / FONT_W)
        {
            let mut cell = *cell;
            if terminal.cursor_visible && terminal.at_bottom() && cursor == (column, row) {
                core::mem::swap(&mut cell.fg, &mut cell.bg);
            }
            let x = column * FONT_W;
            fill(
                pixels,
                width,
                x,
                y.max(top),
                x + FONT_W,
                (y + FONT_H).min(bottom),
                cell.bg,
            );
            text.draw_cell_in_band(
                pixels,
                width,
                height,
                x,
                y,
                cell.byte,
                cell.fg,
                (top, bottom),
            );
            if cell.underline && y + FONT_H > top && y + FONT_H <= bottom {
                fill(
                    pixels,
                    width,
                    x,
                    y + FONT_H - 1,
                    x + FONT_W,
                    y + FONT_H,
                    cell.fg,
                );
            }
        }
    }
}

fn fill(
    pixels: Word,
    width: usize,
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
    color: u32,
) {
    for y in top..bottom {
        for x in left..right {
            unsafe { core::ptr::write_volatile((pixels as *mut u32).add(y * width + x), color) };
        }
    }
}
