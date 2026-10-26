use alloc::vec::Vec;

use crate::framebuffer::{Framebuffer, Rect};

/// Static desktop pixels, already scaled and converted to the display format.
/// The screen geometry/theme are fixed for the lifetime of the compositor.
pub struct BackgroundCache {
    canvas: Framebuffer,
    _pixels: Option<Vec<u32>>,
}

impl BackgroundCache {
    // Bound optional memory use; larger screens retain the direct renderer.
    const MAX_BYTES: usize = 32 * 1024 * 1024;

    pub fn new(framebuffer: &Framebuffer, draw: impl FnOnce(&Framebuffer)) -> Option<Self> {
        let screen = framebuffer.screen();
        let count = screen.width.checked_mul(screen.height)?;
        let bytes = count.checked_mul(core::mem::size_of::<u32>())?;
        if count == 0 || bytes > Self::MAX_BYTES {
            return None;
        }
        if let Some(canvas) = framebuffer.new_gpu_canvas() {
            draw(&canvas);
            if !canvas.healthy() {
                return None;
            }
            return Some(Self {
                canvas,
                _pixels: None,
            });
        }
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(count).ok()?;
        pixels.resize(count, 0);
        let canvas = Framebuffer::new(
            1, // Private RAM canvas: never presented to display_service.
            pixels.as_mut_ptr() as usize,
            bytes,
            crate::framebuffer::ScreenInfo {
                stride_bytes: screen.width * core::mem::size_of::<u32>(),
                ..screen
            },
        )
        .ok()?;
        draw(&canvas);
        Some(Self {
            canvas,
            _pixels: Some(pixels),
        })
    }

    /// The compositor passes screen-clipped damage rectangles. Destination
    /// and source bounds are also checked by the common framebuffer blitter.
    pub fn paint(&self, framebuffer: &Framebuffer, dirty: Rect) {
        framebuffer.copy_rect_from(&self.canvas, dirty);
    }
}
