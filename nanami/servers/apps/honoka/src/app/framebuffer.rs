use libnanami::Word;

use crate::constants::CURSOR_SIZE;

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
    vaddr: Word,
    bytes: Word,
    screen: ScreenInfo,
}

impl Framebuffer {
    pub fn new(
        vaddr: Word,
        bytes: Word,
        screen: ScreenInfo,
    ) -> Result<Self, libnanami::NanamiError> {
        if vaddr == 0 || bytes == 0 || screen.bits_per_pixel != 32 || screen.stride_bytes == 0 {
            return Err(libnanami::NanamiError::INVALID_ARGUMENT);
        }
        Ok(Self {
            vaddr,
            bytes,
            screen,
        })
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

    pub fn color(&self, r8: u8, g8: u8, b8: u8) -> u32 {
        let r = scale_channel(r8, self.screen.red_size) << self.screen.red_position;
        let g = scale_channel(g8, self.screen.green_size) << self.screen.green_position;
        let b = scale_channel(b8, self.screen.blue_size) << self.screen.blue_position;
        (r | g | b) as u32
    }

    pub fn fill_rect(&self, x: i32, y: i32, width: i32, height: i32, color: u32) {
        if width <= 0 || height <= 0 {
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

        let mut py = y0;
        while py < y1 {
            let offset = (py as usize)
                .saturating_mul(self.screen.stride_bytes)
                .saturating_add((x0 as usize).saturating_mul(4));
            if offset.saturating_add(row_pixels.saturating_mul(4)) <= self.bytes as usize {
                let row_base = (self.vaddr as usize).saturating_add(offset);
                let mut i = 0usize;
                while i < row_pixels {
                    unsafe {
                        core::ptr::write_volatile(
                            row_base.saturating_add(i.saturating_mul(4)) as *mut u32,
                            color,
                        );
                    }
                    i += 1;
                }
            }
            py += 1;
        }
    }

    pub fn fill_rect_clip(&self, rect: Rect, color: u32) {
        self.fill_rect(rect.x, rect.y, rect.width, rect.height, color);
    }

    pub fn blit_bgra32_from(
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
    ) {
        if width <= 0 || height <= 0 || src_vaddr == 0 || src_bytes == 0 || src_stride_pixels == 0 {
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
            let mut x = 0usize;
            while x < width {
                unsafe {
                    let pixel = core::ptr::read_volatile(
                        src_base.saturating_add(x.saturating_mul(4)) as *const u32,
                    );
                    core::ptr::write_volatile(
                        dst_base.saturating_add(x.saturating_mul(4)) as *mut u32,
                        pixel,
                    );
                }
                x += 1;
            }
            y += 1;
        }
    }

    pub fn draw_cursor(&self, cx: i32, cy: i32, color: u32, shadow: u32) {
        let mut y = 0i32;
        while y < CURSOR_SIZE {
            let mut x = 0i32;
            while x <= y / 2 {
                self.put_pixel(cx + x + 1, cy + y + 1, shadow);
                self.put_pixel(cx + x, cy + y, color);
                x += 1;
            }
            y += 1;
        }

        let mut d = 0i32;
        while d < CURSOR_SIZE {
            self.put_pixel(cx + d + 1, cy + d + 1, shadow);
            self.put_pixel(cx + d, cy + d, color);
            d += 1;
        }
    }

    pub fn put_pixel(&self, x: i32, y: i32, color: u32) {
        if x < 0 || y < 0 {
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
            core::ptr::write_volatile(ptr, color);
        }
    }
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
