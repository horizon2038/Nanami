//! Gen9 native scanout in an inherited, linear XRGB8888 display mode.
//! No link/PLL changes and no writes to live firmware GGTT mappings.
use nanami_runtime::{RequestError, Word};

pub const PAGE_BYTES: Word = 4096;
const PIPE_STEP: Word = 0x1000;
const PLANE_CONTROL: Word = 0x70180;
const PLANE_STRIDE: Word = 0x70188;
const PLANE_POSITION: Word = 0x7018c;
const PLANE_SIZE: Word = 0x70190;
const PLANE_SURFACE: Word = 0x7019c;
const PLANE_OFFSET: Word = 0x701a4;
const PLANE_LIVE: Word = 0x701ac;
const PIPE_SOURCE: Word = 0x6001c;
const PIPE_FRAME: Word = 0x70040;
const PLANE_ENABLE: u32 = 1 << 31;
const FORMAT_MASK: u32 = 15 << 24;
const FORMAT_XRGB8888: u32 = 4 << 24;
const UNSUPPORTED_LAYOUT: u32 =
    (3 << 21) | (1 << 20) | (1 << 15) | (7 << 10) | (1 << 9) | (1 << 8) | (3 << 4) | 3;
const FBC_CONTROL: Word = 0x43208;
const PSR_CONTROL: Word = 0x64800;
const DC_STATE_ENABLE: Word = 0x45504;
const ENABLE: u32 = 1 << 31;
const MAX_FRAME_BYTES: Word = 64 * 1024 * 1024;
pub const GGTT_FLUSH_CONTROL: Word = 0x101008;

pub trait Registers {
    fn read(&self, offset: Word) -> u32;
    fn write(&mut self, offset: Word, value: u32);
    fn pte(&self, index: Word) -> u64;
    fn write_pte(&mut self, index: Word, value: u64);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    pub pipe: Word,
    pub width: Word,
    pub height: Word,
    pub stride: Word,
    pub bytes: Word,
    pub original_surface: u32,
}

pub fn discover(registers: &impl Registers, aperture_bytes: Word) -> Result<Mode, RequestError> {
    // Active compression/self-refresh has additional ownership and power
    // sequencing requirements. Leave such firmware configurations untouched.
    if registers.read(FBC_CONTROL) & ENABLE != 0
        || registers.read(PSR_CONTROL) & ENABLE != 0
        || registers.read(DC_STATE_ENABLE) & 0x1f != 0
    {
        return Err(RequestError::Unsupported);
    }
    let mut result = None;
    for pipe in 0..3 {
        let step = pipe * PIPE_STEP;
        let control = registers.read(PLANE_CONTROL + step);
        if control & PLANE_ENABLE == 0 {
            continue;
        }
        if result.is_some()
            || control & FORMAT_MASK != FORMAT_XRGB8888
            || control & UNSUPPORTED_LAYOUT != 0
        {
            return Err(RequestError::Unsupported);
        }
        for plane in 1..3 {
            if registers.read(PLANE_CONTROL + step + plane * 0x100) & PLANE_ENABLE != 0 {
                return Err(RequestError::Unsupported);
            }
        }
        if registers.read(PLANE_POSITION + step) != 0 || registers.read(PLANE_OFFSET + step) != 0 {
            return Err(RequestError::Unsupported);
        }
        let size = registers.read(PLANE_SIZE + step);
        let width = ((size & 0xffff) + 1) as Word;
        let height = ((size >> 16) + 1) as Word;
        let source = registers.read(PIPE_SOURCE + step);
        if width != ((source >> 16) + 1) as Word || height != ((source & 0xffff) + 1) as Word {
            return Err(RequestError::Unsupported); // Do not silently adopt scaling.
        }
        let stride = (registers.read(PLANE_STRIDE + step) as Word & 0xfff) * 64;
        let bytes = stride.checked_mul(height).ok_or(RequestError::Protocol)?;
        let original_surface = registers.read(PLANE_SURFACE + step);
        if width > 8192
            || height > 4096
            || stride < width * 4
            || bytes == 0
            || bytes > MAX_FRAME_BYTES
            || original_surface & 0xfff != 0
            || (original_surface as Word)
                .checked_add(bytes)
                .is_none_or(|end| end > aperture_bytes)
        {
            return Err(RequestError::Unsupported);
        }
        result = Some(Mode {
            pipe,
            width,
            height,
            stride,
            bytes,
            original_surface,
        });
    }
    result.ok_or(RequestError::Unsupported)
}

pub fn frame(registers: &impl Registers, mode: Mode) -> u32 {
    registers.read(PIPE_FRAME + mode.pipe * PIPE_STEP)
}

pub fn flip(registers: &mut impl Registers, mode: Mode, surface: Word) -> Result<(), RequestError> {
    if surface & (PAGE_BYTES - 1) != 0 || surface > u32::MAX as Word {
        return Err(RequestError::InvalidArgument);
    }
    // Only the base changes: format, stride, clocks, links and watermarks stay
    // unchanged. PLANE_SURF arms the hardware's vblank-buffered update.
    registers.write(PLANE_SURFACE + mode.pipe * PIPE_STEP, surface as u32);
    let _ = registers.read(PLANE_SURFACE + mode.pipe * PIPE_STEP);
    Ok(())
}

pub fn latched(registers: &impl Registers, mode: Mode, surface: Word) -> bool {
    registers.read(PLANE_LIVE + mode.pipe * PIPE_STEP) & !0xfff == surface as u32
}

/// Reserve only invalid PTEs. A firmware-filled GGTT is not free memory.
pub fn free_span(
    registers: &impl Registers,
    entries: Word,
    pages: Word,
) -> Result<Word, RequestError> {
    if pages == 0 || pages > entries {
        return Err(RequestError::Unsupported);
    }
    let mut count = 0;
    for index in 1..entries {
        if registers.pte(index) == 0 {
            count += 1;
        } else {
            count = 0;
        }
        if count == pages {
            return Ok(index + 1 - pages);
        }
    }
    Err(RequestError::Unsupported)
}

pub fn bind(
    registers: &mut impl Registers,
    start: Word,
    physical: Word,
    pages: Word,
) -> Result<(), RequestError> {
    if pages == 0
        || physical & (PAGE_BYTES - 1) != 0
        || pages
            .checked_mul(PAGE_BYTES)
            .and_then(|bytes| physical.checked_add(bytes))
            .is_none_or(|end| end > (1usize << 39))
    {
        return Err(RequestError::Unsupported);
    }
    for index in 0..pages {
        if registers.pte(start + index) != 0 {
            return Err(RequestError::Protocol);
        }
    }
    for index in 0..pages {
        let pte = (physical + index * PAGE_BYTES) as u64 | 1;
        registers.write_pte(start + index, pte);
        if registers.pte(start + index) != pte {
            unbind(registers, start, index + 1);
            return Err(RequestError::Protocol);
        }
    }
    registers.write(GGTT_FLUSH_CONTROL, 1);
    let _ = registers.read(GGTT_FLUSH_CONTROL);
    Ok(())
}

pub fn unbind(registers: &mut impl Registers, start: Word, pages: Word) {
    for index in 0..pages {
        registers.write_pte(start + index, 0);
    }
    registers.write(GGTT_FLUSH_CONTROL, 1);
    let _ = registers.read(GGTT_FLUSH_CONTROL);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: Word,
    pub y: Word,
    pub width: Word,
    pub height: Word,
}

impl Rect {
    pub fn checked(mode: Mode, position: Word, size: Word) -> Result<Self, RequestError> {
        let (x, y) = (position & 0xffff_ffff, position >> 32);
        let (width, height) = (size & 0xffff_ffff, size >> 32);
        if x >= mode.width || y >= mode.height || width == 0 || height == 0 {
            return Err(RequestError::InvalidArgument);
        }
        Ok(Self {
            x,
            y,
            width: width.min(mode.width - x),
            height: height.min(mode.height - y),
        })
    }

    pub fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Self {
            x,
            y,
            width: (self.x + self.width).max(other.x + other.width) - x,
            height: (self.y + self.height).max(other.y + other.height) - y,
        }
    }
}
