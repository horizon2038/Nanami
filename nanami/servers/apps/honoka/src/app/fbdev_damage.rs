//! Exact damage detection for unmodified fbdev clients without damage queues.
//! The compositor draws this captured image, never rereads a concurrently
//! changing source after comparing it (which could otherwise miss an update).
use crate::framebuffer::Rect;

const TILE_WIDTH: usize = 32;
const TILE_HEIGHT: usize = 16;

#[cfg(test)]
static RESULT_TILE_CHECKS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static SAMPLED_PIXELS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static COMPARED_PIXELS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
pub fn take_row_pixels() -> (usize, usize) {
    use core::sync::atomic::Ordering::Relaxed;
    (SAMPLED_PIXELS.swap(0, Relaxed), COMPARED_PIXELS.swap(0, Relaxed))
}
#[cfg(test)]
pub fn take_result_tile_checks() -> usize {
    RESULT_TILE_CHECKS.swap(0, core::sync::atomic::Ordering::Relaxed)
}

#[inline]
unsafe fn capture_scalar_row<const KNOWN_DIRTY: bool>(source: usize, captured: usize, pixels: usize) -> bool {
    let mut dirty = false;
    for index in 0..pixels {
        let value = core::ptr::read_volatile((source + index * 4) as *const u32);
        let output = (captured + index * 4) as *mut u32;
        if KNOWN_DIRTY || core::ptr::read(output) != value {
            core::ptr::write(output, value);
            dirty = true;
        }
    }
    dirty
}

/// Capture a row span; known-dirty spans need no old-image comparison.
/// All writes use the sampled values, not another read of the live producer.
unsafe fn capture_row<const KNOWN_DIRTY: bool>(source: usize, captured: usize, pixels: usize) -> bool {
    #[cfg(test)]
    {
        SAMPLED_PIXELS.fetch_add(pixels, core::sync::atomic::Ordering::Relaxed);
        if !KNOWN_DIRTY {
            COMPARED_PIXELS.fetch_add(pixels, core::sync::atomic::Ordering::Relaxed);
        }
    }
    let mut index = 0;
    let mut dirty = false;
    if (source | captured) & 7 != 0 {
        if pixels == 0 || source & 7 != 4 || captured & 7 != 4 {
            return capture_scalar_row::<KNOWN_DIRTY>(source, captured, pixels);
        }
        // Odd pixel strides put both rows four bytes past alignment. Peel
        // exactly one sampled pixel; the remaining pair accesses are aligned.
        dirty = capture_scalar_row::<KNOWN_DIRTY>(source, captured, 1);
        index = 1;
    }
    {
        while index + 8 <= pixels {
            let input = (source + index * 4) as *const u64;
            let output = (captured + index * 4) as *mut u64;
            let sample = [
                core::ptr::read_volatile(input),
                core::ptr::read_volatile(input.add(1)),
                core::ptr::read_volatile(input.add(2)),
                core::ptr::read_volatile(input.add(3)),
            ];
            let changed = KNOWN_DIRTY || ((core::ptr::read(output) ^ sample[0])
                | (core::ptr::read(output.add(1)) ^ sample[1])
                | (core::ptr::read(output.add(2)) ^ sample[2])
                | (core::ptr::read(output.add(3)) ^ sample[3])) != 0;
            if changed {
                core::ptr::write(output as *mut [u64; 4], sample);
                dirty = true;
            }
            index += 8;
        }
    }
    // Bitwise OR ensures the tail is sampled even after an earlier difference.
    dirty | capture_scalar_row::<KNOWN_DIRTY>(source + index * 4, captured + index * 4, pixels - index)
}

unsafe fn capture_tile_row<const PARTIAL: bool>(
    source: usize, captured: usize, pixels: usize, tiles: usize, mut dirty: u64,
) -> u64 {
    for tile in 0..tiles {
        let left = tile * TILE_WIDTH;
        let width = (pixels - left).min(TILE_WIDTH);
        let offset = left * 4;
        if PARTIAL && dirty & (1u64 << tile) != 0 {
            // This tile already contributes the same damage rectangle. Keep
            // all live samples, without comparing its previously captured row.
            capture_row::<true>(source + offset, captured + offset, width);
        } else {
            dirty |= (capture_row::<false>(source + offset, captured + offset, width) as u64) << tile;
        }
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
        // Keep producer/capture reads contiguous across each row. A bounded
        // mask retains tile results without allocating or changing run order.
        const BLOCK_TILES: usize = u64::BITS as usize;
        for block in (0..width).step_by(TILE_WIDTH * BLOCK_TILES) {
            let end = (block + TILE_WIDTH * BLOCK_TILES).min(width);
            let tiles = (end - block).div_ceil(TILE_WIDTH);
            let full_mask = u64::MAX >> (BLOCK_TILES - tiles);
            let mut dirty_tiles = 0u64;
            for row in top..bottom {
                let offset = (row * stride + block) * 4;
                if dirty_tiles == full_mask {
                    // Every tile in this block already needs a redraw. Keep
                    // sampling/storing all remaining rows, without comparing
                    // the old capture or rediscovering the same dirty bits.
                    capture_row::<true>(source + offset, captured + offset, end - block);
                    continue;
                }
                dirty_tiles = if dirty_tiles == 0 {
                    // The common clean scan needs no per-tile dirty-bit test.
                    capture_tile_row::<false>(source + offset, captured + offset, end - block, tiles, 0)
                } else {
                    capture_tile_row::<true>(source + offset, captured + offset, end - block, tiles, dirty_tiles)
                };
            }
            // Every sampled tile is dirty: extend the current horizontal run
            // in one step. tiles is 1..=64, so the shift is always in range.
            if dirty_tiles == full_mask {
                run.get_or_insert(block);
                continue;
            }
            // A clean block can only terminate an incoming run at its first
            // tile. If no run is pending, there are no tile results to visit.
            let result_tiles = if dirty_tiles == 0 { usize::from(run.is_some()) } else { tiles };
            let mut tile = 0;
            while tile < result_tiles {
                #[cfg(test)]
                RESULT_TILE_CHECKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                // One result step covers a complete clean/dirty bit run.
                // tile < result_tiles <= 64 keeps the shift below 64.
                let remaining = dirty_tiles >> tile;
                let dirty = remaining & 1 != 0;
                let span = if dirty { remaining.trailing_ones() } else { remaining.trailing_zeros() };
                let left = block + tile * TILE_WIDTH;
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
                tile += (span as usize).min(result_tiles - tile);
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
