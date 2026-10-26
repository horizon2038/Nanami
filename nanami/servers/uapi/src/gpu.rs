//! GPU-independent synchronous 2D protocol. The provider validates all handles
//! and rectangles; clients never submit GPU addresses or native commands.
use crate::Word;
pub const GPU_SERVICE: &str = "gpu-service";
pub const ABI_VERSION: Word = 1;
pub const GET_INFO: Word = 0x6001;
pub const CREATE: Word = 0x6002;
pub const DESTROY: Word = 0x6003;
pub const FILL: Word = 0x6004;
pub const COPY: Word = 0x6005;
pub const PRESENT: Word = 0x6006;
pub const CAP_FILL: Word = 1;
pub const CAP_COPY: Word = 2;
pub const CAP_PRESENT: Word = 4;
pub const CAP_ALL: Word = CAP_FILL | CAP_COPY | CAP_PRESENT;
pub const FORMAT_XRGB8888: Word = 1;
pub const MAX_DIMENSION: Word = 8192;
pub const MAX_PITCH: Word = 32704;
pub const MAX_BUFFER_BYTES: Word = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub width: Word,
    pub height: Word,
    pub stride: Word,
    pub bytes: Word,
}
impl Layout {
    pub fn new(width: Word, height: Word, stride: Word) -> Option<Self> {
        let minimum = width.checked_mul(4)?.checked_add(63)? & !63;
        let stride = if stride == 0 { minimum } else { stride };
        let bytes = stride.checked_mul(height)?.checked_add(4095)? & !4095;
        if width == 0
            || height == 0
            || width > MAX_DIMENSION
            || height > MAX_DIMENSION
            || stride < minimum
            || stride > MAX_PITCH
            || stride & 63 != 0
            || bytes > MAX_BUFFER_BYTES
        {
            return None;
        }
        Some(Self {
            width,
            height,
            stride,
            bytes,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: Word,
    pub y: Word,
    pub width: Word,
    pub height: Word,
}
impl Rect {
    pub fn decode(layout: Layout, position: Word, size: Word) -> Option<Self> {
        let rect = Self {
            x: position & 0xffff_ffff,
            y: position >> 32,
            width: size & 0xffff_ffff,
            height: size >> 32,
        };
        rect.inside(layout).then_some(rect)
    }
    pub fn inside(self, layout: Layout) -> bool {
        self.width != 0
            && self.height != 0
            && self.x < layout.width
            && self.y < layout.height
            && self.width <= layout.width - self.x
            && self.height <= layout.height - self.y
    }
    pub fn position(self) -> Word {
        self.x | (self.y << 32)
    }
    pub fn size(self) -> Word {
        self.width | (self.height << 32)
    }
    pub fn overlaps(self, other: Self) -> bool {
        self.x < other.x + other.width
            && other.x < self.x + self.width
            && self.y < other.y + other.height
            && other.y < self.y + self.height
    }
}

/// All operations complete and make their writes CPU-visible before replying.
/// `accelerated` is a subset of `supported`, not a vendor/device identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub supported: Word,
    pub accelerated: Word,
}
impl Info {
    pub fn words(self) -> (Word, Word) {
        (
            ABI_VERSION | (self.supported << 16),
            self.accelerated | (FORMAT_XRGB8888 << 16),
        )
    }
    pub fn decode(a: Word, b: Word) -> Option<Self> {
        let info = Self {
            supported: a >> 16,
            accelerated: b & 0xffff,
        };
        if a & 0xffff != ABI_VERSION
            || b >> 16 != FORMAT_XRGB8888
            || info.supported & !CAP_ALL != 0
            || info.accelerated & !info.supported != 0
        {
            return None;
        }
        Some(info)
    }
}
