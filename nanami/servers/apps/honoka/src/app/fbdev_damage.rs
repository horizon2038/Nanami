//! Exact damage detection for unmodified fbdev clients without damage queues.
//! The compositor draws this captured image, never rereads a concurrently
//! changing source after comparing it (which could otherwise miss an update).
use crate::framebuffer::Rect;

const TILE_WIDTH: usize = 32;
const TILE_HEIGHT: usize = 16;

/// Compare one tile row without a per-pair alignment test or per-pixel branch.
/// All writes use the sampled values, not another read of the live producer.
unsafe fn capture_row(source: usize, captured: usize, pixels: usize) -> bool {
    let mut index = 0;
    let mut dirty = false;
    if (source | captured) & 7 == 0 {
        while index + 8 <= pixels {
            let input = (source + index * 4) as *const u64;
            let output = (captured + index * 4) as *mut u64;
            let sample = [
                core::ptr::read_volatile(input),
                core::ptr::read_volatile(input.add(1)),
                core::ptr::read_volatile(input.add(2)),
                core::ptr::read_volatile(input.add(3)),
            ];
            let difference = (core::ptr::read(output) ^ sample[0])
                | (core::ptr::read(output.add(1)) ^ sample[1])
                | (core::ptr::read(output.add(2)) ^ sample[2])
                | (core::ptr::read(output.add(3)) ^ sample[3]);
            if difference != 0 {
                core::ptr::write(output as *mut [u64; 4], sample);
                dirty = true;
            }
            index += 8;
        }
    }
    // Includes unaligned rows, short widths and the final partial group.
    while index < pixels {
        let value = core::ptr::read_volatile((source + index * 4) as *const u32);
        let output = (captured + index * 4) as *mut u32;
        if core::ptr::read(output) != value {
            core::ptr::write(output, value);
            dirty = true;
        }
        index += 1;
    }
    dirty
}

fn merge_vertical(pending: &mut Option<Rect>, next: Rect, changed: &mut impl FnMut(Rect)) {
    if let Some(previous) = pending {
        if previous.x == next.x
            && previous.width == next.width
            && previous.y + previous.height == next.y
        {
            previous.height += next.height;
            return;
        }
        changed(*previous);
    }
    *pending = Some(next);
}

/// Both buffers contain stride*height u32 pixels, do not overlap, and remain
/// mapped throughout the call. Only Honoka writes the captured buffer.
pub unsafe fn capture(
    source: usize,
    captured: usize,
    stride: usize,
    width: usize,
    height: usize,
    mut changed: impl FnMut(Rect),
) {
    let mut pending = None;
    for top in (0..height).step_by(TILE_HEIGHT) {
        let bottom = (top + TILE_HEIGHT).min(height);
        let mut run = None;
        for left in (0..width).step_by(TILE_WIDTH) {
            let right = (left + TILE_WIDTH).min(width);
            let mut dirty = false;
            for row in top..bottom {
                let offset = (row * stride + left) * 4;
                dirty |= capture_row(source + offset, captured + offset, right - left);
            }
            if dirty {
                run.get_or_insert(left);
            } else if let Some(start) = run.take() {
                merge_vertical(
                    &mut pending,
                    Rect::new(
                        start as i32,
                        top as i32,
                        (left - start) as i32,
                        (bottom - top) as i32,
                    ),
                    &mut changed,
                );
            }
        }
        if let Some(start) = run {
            merge_vertical(
                &mut pending,
                Rect::new(
                    start as i32,
                    top as i32,
                    (width - start) as i32,
                    (bottom - top) as i32,
                ),
                &mut changed,
            );
        }
    }
    if let Some(rect) = pending {
        changed(rect);
    }
}
