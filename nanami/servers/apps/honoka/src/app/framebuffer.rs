use core::{
    cell::Cell,
    sync::atomic::{fence, Ordering},
};
use nanami_client::gpu;
use nanami_runtime::Word;

#[path = "cursor.rs"]
mod cursor;

#[derive(Clone, Copy)]
pub struct ScreenInfo {
    pub width: usize,
    pub height: usize,
    pub stride_bytes: usize,
    pub bits_per_pixel: usize,
    pub red_position: usize,
    pub red_size: usize,
    pub green_position: usize,
    pub green_size: usize,
    pub blue_position: usize,
    pub blue_size: usize,
}

#[derive(Clone, Copy)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub const EMPTY: Self = Self {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    };

    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn is_empty(self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    pub fn inflate(self, amount: i32) -> Self {
        Self {
            x: self.x.saturating_sub(amount),
            y: self.y.saturating_sub(amount),
            width: self.width.saturating_add(amount.saturating_mul(2)),
            height: self.height.saturating_add(amount.saturating_mul(2)),
        }
    }
}

pub struct Framebuffer {
    display_port: Word,
    vaddr: Word,
    bytes: Word,
    screen: ScreenInfo,
    merged_present: bool,
    gpu: Option<gpu::Surface>,
    gpu_failed: Cell<bool>,
}

impl Framebuffer {
    pub fn new(
        display_port: Word,
        vaddr: Word,
        bytes: Word,
        screen: ScreenInfo,
    ) -> Result<Self, nanami_runtime::NanamiError> {
        if display_port == 0
            || vaddr == 0
            || bytes == 0
            || screen.bits_per_pixel != 32
            || screen.stride_bytes == 0
        {
            return Err(nanami_runtime::NanamiError::INVALID_ARGUMENT);
        }
        Ok(Self {
            display_port,
            vaddr,
            bytes,
            screen,
            merged_present: false,
            gpu: None,
            gpu_failed: Cell::new(false),
        })
    }

    pub fn try_gpu(
        display_port: Word,
        screen: ScreenInfo,
    ) -> Result<Option<Self>, nanami_runtime::RequestError> {
        if (
            screen.red_position,
            screen.red_size,
            screen.green_position,
            screen.green_size,
            screen.blue_position,
            screen.blue_size,
        ) != (16, 8, 8, 8, 0, 8)
        {
            return Ok(None);
        }
        let info = gpu::info(display_port)?;
        if info.supported & gpu::CAP_PRESENT == 0 {
            return Ok(None);
        }
        let layout = gpu::Layout::new(screen.width, screen.height, screen.stride_bytes)
            .ok_or(nanami_runtime::RequestError::Unsupported)?;
        let surface = gpu::create(display_port, layout, info.accelerated)?;
        Ok(Some(Self {
            display_port,
            vaddr: surface.address,
            bytes: layout.bytes,
            screen,
            merged_present: false,
            gpu: Some(surface),
            gpu_failed: Cell::new(false),
        }))
    }

    pub fn new_gpu_canvas(&self) -> Option<Self> {
        let parent = self.gpu.as_ref()?;
        if parent.accelerated & gpu::CAP_COPY == 0 || self.gpu_failed.get() {
            return None;
        }
        Self::try_gpu(self.display_port, self.screen).ok().flatten()
    }

    pub fn healthy(&self) -> bool {
        !self.gpu_failed.get()
    }

    fn gpu_result(
        &self,
        result: Result<(), nanami_runtime::RequestError>,
    ) -> Result<(), nanami_runtime::RequestError> {
        if result.is_err() {
            self.gpu_failed.set(true);
        }
        result
    }

    pub fn copy_rect_from(&self, source: &Self, rect: Rect) {
        if self.gpu_failed.get() || !source.healthy() {
            return;
        }
        if rect.is_empty() || rect.x < 0 || rect.y < 0 {
            return;
        }
        let from = gpu::Rect {
            x: rect.x as Word,
            y: rect.y as Word,
            width: rect.width as Word,
            height: rect.height as Word,
        };
        if let (Some(source_surface), Some(target)) = (&source.gpu, &self.gpu) {
            if source_surface.port == target.port
                && target.accelerated & gpu::CAP_COPY != 0
                && from.width * from.height >= 4096
            {
                if self
                    .gpu_result(gpu::copy(source_surface, target, from, from.x, from.y))
                    .is_err()
                {
                    // A timed-out copy may still read its source as well as
                    // write its destination. Neither mapping can be reused.
                    source.gpu_failed.set(true);
                }
                return;
            }
        }
        self.blit_bgra32_from_alpha(
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            source.vaddr,
            source.bytes,
            source.screen.stride_bytes / 4,
            from.x,
            from.y,
            u8::MAX,
        );
    }

    pub fn with_merged_present(mut self, enabled: bool) -> Self {
        self.merged_present = enabled;
        self
    }

    pub fn merged_present(&self) -> bool {
        self.merged_present
    }

    pub fn screen(&self) -> ScreenInfo {
        self.screen
    }

    pub fn vaddr(&self) -> Word {
        self.vaddr
    }

    pub fn bytes(&self) -> Word {
        self.bytes
    }

    pub fn present(&self, rect: Rect) -> Result<(), nanami_runtime::RequestError> {
        if self.gpu_failed.get() {
            return Err(nanami_runtime::RequestError::Transport);
        }
        if rect.is_empty() {
            return Ok(());
        }
        let x = clamp_i32(rect.x, 0, self.screen.width as i32) as Word;
        let y = clamp_i32(rect.y, 0, self.screen.height as i32) as Word;
        let x1 = clamp_i32(
            rect.x.saturating_add(rect.width),
            0,
            self.screen.width as i32,
        ) as Word;
        let y1 = clamp_i32(
            rect.y.saturating_add(rect.height),
            0,
            self.screen.height as i32,
        ) as Word;
        if x1 <= x || y1 <= y {
            return Ok(());
        }
        fence(Ordering::Release);
        if let Some(surface) = &self.gpu {
            return self.gpu_result(gpu::present(
                surface,
                gpu::Rect {
                    x,
                    y,
                    width: x1 - x,
                    height: y1 - y,
                },
            ));
        }
        nanami_client::gfx::display_service_present(self.display_port, x, y, x1 - x, y1 - y)
    }

    pub fn color(&self, r8: u8, g8: u8, b8: u8) -> u32 {
        let r = scale_channel(r8, self.screen.red_size) << self.screen.red_position;
        let g = scale_channel(g8, self.screen.green_size) << self.screen.green_position;
        let b = scale_channel(b8, self.screen.blue_size) << self.screen.blue_position;
        (r | g | b) as u32
    }

    pub fn fill_rect(&self, x: i32, y: i32, width: i32, height: i32, color: u32) {
        if self.gpu_failed.get() || width <= 0 || height <= 0 {
            return;
        }

        let x0 = clamp_i32(x, 0, self.screen.width as i32);
        let y0 = clamp_i32(y, 0, self.screen.height as i32);
        let x1 = clamp_i32(x.saturating_add(width), 0, self.screen.width as i32);
        let y1 = clamp_i32(y.saturating_add(height), 0, self.screen.height as i32);

        let row_pixels = x1.saturating_sub(x0) as usize;
        if row_pixels == 0 {
            return;
        }

        if let Some(surface) = &self.gpu {
            let rect = gpu::Rect {
                x: x0 as Word,
                y: y0 as Word,
                width: row_pixels,
                height: (y1 - y0) as Word,
            };
            if rect.height != 0
                && rect.width * rect.height >= 4096
                && surface.accelerated & gpu::CAP_FILL != 0
            {
                let _ = self.gpu_result(gpu::fill(surface, rect, color));
                return;
            }
        }

        let mut py = y0;
        while py < y1 {
            let offset = (py as usize)
                .saturating_mul(self.screen.stride_bytes)
                .saturating_add((x0 as usize).saturating_mul(4));
            if offset.saturating_add(row_pixels.saturating_mul(4)) <= self.bytes as usize {
                let row_base = (self.vaddr as usize).saturating_add(offset);
                // This is compositor-owned RAM, not the hardware aperture.
                // Publication to fb-server is ordered by present().
                unsafe {
                    core::slice::from_raw_parts_mut(row_base as *mut u32, row_pixels).fill(color);
                }
            }
            py += 1;
        }
    }

    pub fn fill_rect_clip(&self, rect: Rect, color: u32) {
        self.fill_rect(rect.x, rect.y, rect.width, rect.height, color);
    }

    pub fn fill_rect_alpha(&self, x: i32, y: i32, width: i32, height: i32, color: u32, alpha: u8) {
        if alpha == u8::MAX {
            self.fill_rect(x, y, width, height, color);
            return;
        }
        if alpha == 0 || width <= 0 || height <= 0 {
            return;
        }

        let x0 = clamp_i32(x, 0, self.screen.width as i32);
        let y0 = clamp_i32(y, 0, self.screen.height as i32);
        let x1 = clamp_i32(x.saturating_add(width), 0, self.screen.width as i32);
        let y1 = clamp_i32(y.saturating_add(height), 0, self.screen.height as i32);
        let mut py = y0;
        while py < y1 {
            let mut px = x0;
            while px < x1 {
                self.blend_pixel(px, py, color, alpha);
                px += 1;
            }
            py += 1;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn blit_bgra32_from_alpha(
        &self,
        dst_x: i32,
        dst_y: i32,
        width: i32,
        height: i32,
        src_vaddr: Word,
        src_bytes: Word,
        src_stride_pixels: usize,
        src_x: usize,
        src_y: usize,
        alpha: u8,
    ) {
        if self.gpu_failed.get()
            || width <= 0
            || height <= 0
            || src_vaddr == 0
            || src_bytes == 0
            || src_stride_pixels == 0
        {
            return;
        }
        if alpha == 0 {
            return;
        }

        let row_bytes = (width as usize).saturating_mul(4);
        if src_x.saturating_add(width as usize) > src_stride_pixels {
            return;
        }
        if dst_x < 0 || dst_y < 0 {
            return;
        }
        let dst_x = dst_x as usize;
        let dst_y = dst_y as usize;
        let width = width as usize;
        let height = height as usize;
        if dst_x.saturating_add(width) > self.screen.width
            || dst_y.saturating_add(height) > self.screen.height
        {
            return;
        }

        let mut y = 0usize;
        while y < height {
            let sy = src_y.saturating_add(y);
            let row_offset = sy
                .saturating_mul(src_stride_pixels)
                .saturating_add(src_x)
                .saturating_mul(4);
            if row_offset.saturating_add(row_bytes) > src_bytes as usize {
                return;
            }
            let dst_offset = dst_y
                .saturating_add(y)
                .saturating_mul(self.screen.stride_bytes)
                .saturating_add(dst_x.saturating_mul(4));
            if dst_offset.saturating_add(row_bytes) > self.bytes as usize {
                return;
            }

            let src_base = (src_vaddr as usize).saturating_add(row_offset);
            let dst_base = (self.vaddr as usize).saturating_add(dst_offset);
            if alpha == u8::MAX {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        src_base as *const u8,
                        dst_base as *mut u8,
                        row_bytes,
                    );
                }
                y += 1;
                continue;
            }
            let mut x = 0usize;
            while x < width {
                unsafe {
                    let pixel =
                        core::ptr::read(src_base.saturating_add(x.saturating_mul(4)) as *const u32);
                    let dst_ptr = dst_base.saturating_add(x.saturating_mul(4)) as *mut u32;
                    let background = core::ptr::read(dst_ptr);
                    core::ptr::write(dst_ptr, self.blend_color(background, pixel, alpha));
                }
                x += 1;
            }
            y += 1;
        }
    }

    pub fn draw_cursor(&self, cx: i32, cy: i32, color: u32, outline: u32, clip: Rect) {
        cursor::draw(self, cx, cy, color, outline, clip);
    }

    pub fn put_pixel(&self, x: i32, y: i32, color: u32) {
        if self.gpu_failed.get() || x < 0 || y < 0 {
            return;
        }
        let x = x as usize;
        let y = y as usize;
        if x >= self.screen.width || y >= self.screen.height {
            return;
        }

        let offset = y
            .saturating_mul(self.screen.stride_bytes)
            .saturating_add(x.saturating_mul(4));
        if offset.saturating_add(4) > self.bytes as usize {
            return;
        }

        // Safety: offset is bounds-checked against the framebuffer mapping size.
        let ptr = (self.vaddr as usize).saturating_add(offset) as *mut u32;
        unsafe {
            ptr.write(color);
        }
    }

    pub fn blend_pixel(&self, x: i32, y: i32, color: u32, alpha: u8) {
        if alpha == u8::MAX {
            self.put_pixel(x, y, color);
            return;
        }
        if alpha == 0 {
            return;
        }
        let Some(ptr) = self.pixel_ptr(x, y) else {
            return;
        };
        unsafe {
            let background = ptr.read();
            ptr.write(self.blend_color(background, color, alpha));
        }
    }

    fn pixel_ptr(&self, x: i32, y: i32) -> Option<*mut u32> {
        if self.gpu_failed.get() || x < 0 || y < 0 {
            return None;
        }
        let x = x as usize;
        let y = y as usize;
        if x >= self.screen.width || y >= self.screen.height {
            return None;
        }
        let offset = y
            .saturating_mul(self.screen.stride_bytes)
            .saturating_add(x.saturating_mul(4));
        if offset.saturating_add(4) > self.bytes as usize {
            return None;
        }
        Some((self.vaddr as usize).saturating_add(offset) as *mut u32)
    }

    fn blend_color(&self, background: u32, foreground: u32, alpha: u8) -> u32 {
        let red_mask = channel_mask(self.screen.red_size, self.screen.red_position);
        let green_mask = channel_mask(self.screen.green_size, self.screen.green_position);
        let blue_mask = channel_mask(self.screen.blue_size, self.screen.blue_position);
        let rgb_mask = red_mask | green_mask | blue_mask;
        (background & !rgb_mask)
            | blend_channel(
                background,
                foreground,
                red_mask,
                self.screen.red_position,
                alpha,
            )
            | blend_channel(
                background,
                foreground,
                green_mask,
                self.screen.green_position,
                alpha,
            )
            | blend_channel(
                background,
                foreground,
                blue_mask,
                self.screen.blue_position,
                alpha,
            )
    }
}

fn channel_mask(size: usize, position: usize) -> u32 {
    if size == 0 || position >= u32::BITS as usize {
        return 0;
    }
    let size = size.min(u32::BITS as usize - position);
    let bits = if size == u32::BITS as usize {
        u32::MAX
    } else {
        (1u32 << size) - 1
    };
    bits << position
}

fn blend_channel(background: u32, foreground: u32, mask: u32, position: usize, alpha: u8) -> u32 {
    if mask == 0 {
        return 0;
    }
    let background = ((background & mask) >> position) as u64;
    let foreground = ((foreground & mask) >> position) as u64;
    let alpha = alpha as u64;
    let value = (foreground * alpha + background * (255 - alpha) + 127) / 255;
    ((value as u32) << position) & mask
}

pub fn parse_screen_info(detail0: Word, detail1: Word) -> ScreenInfo {
    let bits_per_pixel = ((detail0 >> 16) & 0xffff) as usize;
    let width = ((detail0 >> 32) & 0xffff) as usize;
    let height = ((detail0 >> 48) & 0xffff) as usize;
    let stride = (detail1 & 0xffff_ffff) as usize;
    let bytes_per_pixel = if bits_per_pixel == 0 {
        0
    } else {
        (bits_per_pixel + 7) / 8
    };
    let stride_bytes = if bytes_per_pixel != 0 && stride >= width.saturating_mul(bytes_per_pixel) {
        stride
    } else {
        stride.saturating_mul(bytes_per_pixel)
    };

    ScreenInfo {
        width,
        height,
        stride_bytes,
        bits_per_pixel,
        red_position: ((detail1 >> 32) & 0x1f) as usize,
        red_size: ((detail1 >> 37) & 0x1f) as usize,
        green_position: ((detail1 >> 42) & 0x1f) as usize,
        green_size: ((detail1 >> 47) & 0x1f) as usize,
        blue_position: ((detail1 >> 52) & 0x1f) as usize,
        blue_size: ((detail1 >> 57) & 0x1f) as usize,
    }
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        if !self.gpu_failed.get() {
            if let Some(surface) = &self.gpu {
                let _ = gpu::destroy(surface);
            }
        }
    }
}

pub fn clamp_i32(value: i32, min: i32, max: i32) -> i32 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

fn scale_channel(value: u8, bits: usize) -> usize {
    if bits == 0 {
        return 0;
    }
    if bits >= 8 {
        return (value as usize) << (bits - 8);
    }
    ((value as usize) * ((1usize << bits) - 1) + 127) / 255
}
