//! Test-only snapshot of the pre-batched comparison loop (2026-10-31).
//! Keep its tile traversal/merging identical to the production baseline so
//! host and QEMU timing compare the same detection workload and output.
use crate::framebuffer::Rect;
fn merge(pending: &mut Option<Rect>, next: Rect, changed: &mut impl FnMut(Rect)) {
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
pub unsafe fn capture(
    source: usize,
    captured: usize,
    stride: usize,
    width: usize,
    height: usize,
    mut changed: impl FnMut(Rect),
) {
    let mut pending = None;
    for top in (0..height).step_by(16) {
        let bottom = (top + 16).min(height);
        let mut run = None;
        for left in (0..width).step_by(32) {
            let right = (left + 32).min(width);
            let mut dirty = false;
            for row in top..bottom {
                let mut index = row * stride + left;
                let end = row * stride + right;
                while index < end {
                    let address = source + index * 4;
                    let previous = captured + index * 4;
                    if index + 1 < end && (address | previous) & 7 == 0 {
                        let value = core::ptr::read_volatile(address as *const u64);
                        if core::ptr::read(previous as *const u64) != value {
                            core::ptr::write(previous as *mut u64, value);
                            dirty = true;
                        }
                        index += 2;
                    } else {
                        let value = core::ptr::read_volatile(address as *const u32);
                        if core::ptr::read(previous as *const u32) != value {
                            core::ptr::write(previous as *mut u32, value);
                            dirty = true;
                        }
                        index += 1;
                    }
                }
            }
            if dirty {
                run.get_or_insert(left);
            } else if let Some(start) = run.take() {
                merge(
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
            merge(
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
