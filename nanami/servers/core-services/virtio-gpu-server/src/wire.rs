//! Virtio 1.2, sections 4.1 and 5.7. All command fields are little-endian.
use nanami_gpu::{Layout, Rect};
use nanami_runtime::{RequestError, Word};

pub const HEADER_BYTES: usize = 24;
pub const DISPLAY_INFO_BYTES: usize = HEADER_BYTES + 16 * 24;
pub const FENCE: u32 = 1;
pub const GET_DISPLAY_INFO: u32 = 0x0100;
pub const CREATE_2D: u32 = 0x0101;
pub const UNREF: u32 = 0x0102;
pub const SET_SCANOUT: u32 = 0x0103;
pub const FLUSH: u32 = 0x0104;
pub const TRANSFER: u32 = 0x0105;
pub const ATTACH: u32 = 0x0106;
pub const DETACH: u32 = 0x0107;
pub const OK: u32 = 0x1100;
pub const OK_DISPLAY_INFO: u32 = 0x1101;
const FORMAT_BGRX: u32 = 2;

pub fn read32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
pub fn read64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
pub fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
pub fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

pub struct Command {
    bytes: [u8; 64],
    length: usize,
}
impl Command {
    fn new(kind: u32, length: usize) -> Self {
        let mut command = Self {
            bytes: [0; 64],
            length,
        };
        put32(&mut command.bytes, 0, kind);
        command
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
    pub fn display_info() -> Self {
        Self::new(GET_DISPLAY_INFO, HEADER_BYTES)
    }
    pub fn create(id: u32, layout: Layout) -> Self {
        let mut c = Self::new(CREATE_2D, 40);
        put32(&mut c.bytes, 24, id);
        put32(&mut c.bytes, 28, FORMAT_BGRX);
        // 2D resources have implicit width*4 pitch. Include row padding in
        // the resource width; scanout and damage keep the visible width.
        put32(&mut c.bytes, 32, (layout.stride / 4) as u32);
        put32(&mut c.bytes, 36, layout.height as u32);
        c
    }
    pub fn resource(kind: u32, id: u32) -> Self {
        let mut c = Self::new(kind, 32);
        put32(&mut c.bytes, 24, id);
        c
    }
    pub fn attach(id: u32, physical: Word, bytes: Word) -> Self {
        let mut c = Self::new(ATTACH, 48);
        put32(&mut c.bytes, 24, id);
        put32(&mut c.bytes, 28, 1);
        put64(&mut c.bytes, 32, physical as u64);
        put32(&mut c.bytes, 40, bytes as u32);
        c
    }
    fn rectangle(&mut self, rect: Rect) {
        for (offset, value) in [rect.x, rect.y, rect.width, rect.height]
            .into_iter()
            .enumerate()
        {
            put32(&mut self.bytes, 24 + offset * 4, value as u32);
        }
    }
    pub fn transfer(id: u32, layout: Layout, rect: Rect) -> Self {
        let mut c = Self::new(TRANSFER, 56);
        c.rectangle(rect);
        put64(
            &mut c.bytes,
            40,
            (rect.y * layout.stride + rect.x * 4) as u64,
        );
        put32(&mut c.bytes, 48, id);
        c
    }
    pub fn flush(id: u32, rect: Rect) -> Self {
        let mut c = Self::new(FLUSH, 48);
        c.rectangle(rect);
        put32(&mut c.bytes, 40, id);
        c
    }
    pub fn scanout(scanout: u32, id: u32, rect: Rect) -> Self {
        let mut c = Self::new(SET_SCANOUT, 48);
        c.rectangle(rect);
        put32(&mut c.bytes, 40, scanout);
        put32(&mut c.bytes, 44, id);
        c
    }
}

pub fn display_info(bytes: &[u8]) -> Result<(u32, Layout), RequestError> {
    if bytes.len() < DISPLAY_INFO_BYTES {
        return Err(RequestError::Protocol);
    }
    for scanout in 0..16 {
        let offset = HEADER_BYTES + scanout * 24;
        if read32(bytes, offset + 16) == 0 {
            continue;
        }
        if let Some(layout) = Layout::new(
            read32(bytes, offset + 8) as Word,
            read32(bytes, offset + 12) as Word,
            0,
        ) {
            return Ok((scanout as u32, layout));
        }
    }
    Err(RequestError::Unsupported)
}
