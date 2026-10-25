//! Additive, read-only display discovery. Monitor modes are EDID advertisements,
//! not a promise that the driver/link can program every advertised timing.
use crate::Word;
pub const CONNECTOR_HDMI: u8 = 1;
pub const CONNECTOR_DVI: u8 = 2;
pub const CONNECTOR_DP: u8 = 3;
pub const CONNECTOR_EDP: u8 = 4;
pub const CONNECTOR_ACTIVE: u8 = 1;
pub const CONNECTOR_EDID_VALID: u8 = 2;
pub const CONNECTOR_EDID_TRUNCATED: u8 = 4;
pub const EDID_MAX_BYTES: usize = 256;
pub const MODE_END: Word = Word::MAX;
pub const MODE_HSYNC_POSITIVE: u32 = 1;
pub const MODE_VSYNC_POSITIVE: u32 = 2;
pub const MODE_PREFERRED: u32 = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConnectorInfo {
    /// Provider-local, nonzero output ID. Zero on the wire terminates iteration.
    pub id: u8,
    pub kind: u8,
    pub pipe: u8,
    pub flags: u8,
    pub width: u16,
    pub height: u16,
    pub edid_bytes: u16,
    pub mode_count: u16,
}
impl ConnectorInfo {
    pub fn words(self) -> (Word, Word) {
        (
            self.id as Word
                | ((self.kind as Word) << 8)
                | ((self.pipe as Word) << 16)
                | ((self.flags as Word) << 24)
                | ((self.width as Word) << 32)
                | ((self.height as Word) << 48),
            self.edid_bytes as Word | ((self.mode_count as Word) << 16),
        )
    }
    pub fn from_words(a: Word, b: Word) -> Option<Self> {
        let result = Self {
            id: a as u8,
            kind: (a >> 8) as u8,
            pipe: (a >> 16) as u8,
            flags: (a >> 24) as u8,
            width: (a >> 32) as u16,
            height: (a >> 48) as u16,
            edid_bytes: b as u16,
            mode_count: (b >> 16) as u16,
        };
        if result.id == 0
            || !(CONNECTOR_HDMI..=CONNECTOR_EDP).contains(&result.kind)
            || result.flags & !(CONNECTOR_ACTIVE | CONNECTOR_EDID_VALID | CONNECTOR_EDID_TRUNCATED)
                != 0
            || !matches!(result.edid_bytes, 0 | 128 | 256)
            || (result.edid_bytes != 0) != (result.flags & CONNECTOR_EDID_VALID != 0)
            || (result.edid_bytes == 0
                && (result.mode_count != 0 || result.flags & CONNECTOR_EDID_TRUNCATED != 0))
            || (result.flags & CONNECTOR_ACTIVE != 0 && (result.width == 0 || result.height == 0))
            || b >> 32 != 0
        {
            return None;
        }
        Some(result)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timing {
    pub clock_khz: u32,
    pub flags: u32,
    pub hactive: u16,
    pub hsync_start: u16,
    pub hsync_end: u16,
    pub htotal: u16,
    pub vactive: u16,
    pub vsync_start: u16,
    pub vsync_end: u16,
    pub vtotal: u16,
}
impl Timing {
    pub fn valid(self) -> bool {
        self.clock_khz != 0
            && self.flags & !(MODE_HSYNC_POSITIVE | MODE_VSYNC_POSITIVE | MODE_PREFERRED) == 0
            && self.hactive != 0
            && self.hactive < self.hsync_start
            && self.hsync_start < self.hsync_end
            && self.hsync_end <= self.htotal
            && self.vactive != 0
            && self.vactive < self.vsync_start
            && self.vsync_start < self.vsync_end
            && self.vsync_end <= self.vtotal
    }
    pub fn words(self) -> [Word; 3] {
        [
            self.clock_khz as Word | ((self.flags as Word) << 32),
            self.hactive as Word
                | ((self.hsync_start as Word) << 16)
                | ((self.hsync_end as Word) << 32)
                | ((self.htotal as Word) << 48),
            self.vactive as Word
                | ((self.vsync_start as Word) << 16)
                | ((self.vsync_end as Word) << 32)
                | ((self.vtotal as Word) << 48),
        ]
    }
    pub fn from_words(words: [Word; 3]) -> Option<Self> {
        let [a, h, v] = words;
        let result = Self {
            clock_khz: a as u32,
            flags: (a >> 32) as u32,
            hactive: h as u16,
            hsync_start: (h >> 16) as u16,
            hsync_end: (h >> 32) as u16,
            htotal: (h >> 48) as u16,
            vactive: v as u16,
            vsync_start: (v >> 16) as u16,
            vsync_end: (v >> 32) as u16,
            vtotal: (v >> 48) as u16,
        };
        result.valid().then_some(result)
    }
}
