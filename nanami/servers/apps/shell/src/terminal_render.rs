//! Reconstruct a horizontal damage band in the same order as a full redraw.
use crate::{font::TextRenderer, terminal::Terminal, FONT_H, FONT_W};
use nanami_runtime::Word;

#[cfg(test)]
static FILL_STORES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(super) fn take_fill_stores() -> usize {
    FILL_STORES.swap(0, core::sync::atomic::Ordering::Relaxed)
}

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
    // Cell backgrounds below overwrite every covered pixel before its final
    // glyph blend. Clear only margins, still before any glyph can spill into
    // them; retain the existing cell/background/glyph paint order.
    let covered_width = terminal.cols().min(width / FONT_W) * FONT_W;
    let covered_height = terminal.rows().min(height / FONT_H) * FONT_H;
    if covered_width < width {
        fill(pixels, width, covered_width, top, width, bottom.min(covered_height), 0x0010_1418);
    }
    if covered_height < bottom {
        fill(pixels, width, 0, top.max(covered_height), width, bottom, 0x0010_1418);
    }
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
    #[cfg(test)]
    FILL_STORES.fetch_add(
        right.saturating_sub(left) * bottom.saturating_sub(top),
        core::sync::atomic::Ordering::Relaxed,
    );
    for y in top..bottom {
        for x in left..right {
            unsafe { core::ptr::write_volatile((pixels as *mut u32).add(y * width + x), color) };
        }
    }
}
