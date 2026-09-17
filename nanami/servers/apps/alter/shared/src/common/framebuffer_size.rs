// Shared by the launcher and personality servers. Sizes are content pixels;
// Honoka adds decorations and may clamp the requested size to the desktop.
pub const ALTER_LAUNCH_FLAG_FB_SIZE: usize = 1 << 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FramebufferSize {
    pub width: usize,
    pub height: usize,
}

impl FramebufferSize {
    pub const DEFAULT: Self = Self {
        width: 800,
        height: 600,
    };

    pub fn new(width: usize, height: usize) -> Option<Self> {
        // Honoka uses signed dimensions; Linux fb_fix_screeninfo uses u32
        // for both line_length and smem_len. Never truncate either ABI.
        if width == 0 || height == 0 || width > i32::MAX as usize || height > i32::MAX as usize {
            return None;
        }
        let bytes = width.checked_mul(4)?.checked_mul(height)?;
        if bytes > u32::MAX as usize {
            return None;
        }
        Some(Self { width, height })
    }

    pub fn parse(text: &[u8]) -> Option<Self> {
        let split = text.iter().position(|&byte| byte == b'x')?;
        Self::new(decimal(&text[..split])?, decimal(&text[split + 1..])?)
    }

    pub fn packed(self) -> usize {
        self.width | (self.height << 32)
    }

    pub fn from_packed(value: usize) -> Option<Self> {
        Self::new(value & 0xffff_ffff, value >> 32)
    }

    pub const fn stride(self) -> usize {
        self.width * 4
    }

    pub const fn bytes(self) -> usize {
        self.stride() * self.height
    }
}

fn decimal(text: &[u8]) -> Option<usize> {
    if text.is_empty() {
        return None;
    }
    text.iter().try_fold(0usize, |value, &byte| {
        if !byte.is_ascii_digit() {
            return None;
        }
        value.checked_mul(10)?.checked_add((byte - b'0') as usize)
    })
}
