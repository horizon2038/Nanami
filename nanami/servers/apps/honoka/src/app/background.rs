use alloc::vec::Vec;

use crate::framebuffer::{Framebuffer, Rect, ScreenInfo};

/// Static desktop pixels, already scaled and converted to the display format.
/// The screen geometry/theme are fixed for the lifetime of the compositor.
pub struct BackgroundCache {
    pixels: Vec<u32>,
    width: usize,
}

impl BackgroundCache {
    // Bound optional memory use; larger screens retain the direct renderer.
    const MAX_BYTES: usize = 32 * 1024 * 1024;

    pub fn new(screen: ScreenInfo, draw: impl FnOnce(&Framebuffer)) -> Option<Self> {
        let count = screen.width.checked_mul(screen.height)?;
        let bytes = count.checked_mul(core::mem::size_of::<u32>())?;
        if count == 0 || bytes > Self::MAX_BYTES {
            return None;
        }
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(count).ok()?;
        pixels.resize(count, 0);
        let canvas = Framebuffer::new(
            1, // Private RAM canvas: never presented to display_service.
            pixels.as_mut_ptr() as usize,
            bytes,
            ScreenInfo {
                stride_bytes: screen.width * core::mem::size_of::<u32>(),
                ..screen
            },
        )
        .ok()?;
        draw(&canvas);
        Some(Self {
            pixels,
            width: screen.width,
        })
    }

    /// The compositor passes screen-clipped damage rectangles. Destination
    /// and source bounds are also checked by the common framebuffer blitter.
    pub fn paint(&self, framebuffer: &Framebuffer, dirty: Rect) {
        framebuffer.blit_bgra32_from_alpha(
            dirty.x,
            dirty.y,
            dirty.width,
            dirty.height,
            self.pixels.as_ptr() as usize,
            self.pixels.len() * core::mem::size_of::<u32>(),
            self.width,
            dirty.x as usize,
            dirty.y as usize,
            u8::MAX,
        );
    }
}
