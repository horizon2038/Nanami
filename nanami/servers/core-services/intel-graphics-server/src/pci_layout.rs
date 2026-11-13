//! Read the inherited Gen9 BAR layout without sizing writes to a live GPU.
//! Gen9 GTTMMADR is fixed at 16 MiB; MSAC[4:0] describes GMADR's size.
use nanami_runtime::{RequestError, Word};

pub struct Layout {
    pub registers: Word,
    pub aperture: Word,
    pub aperture_bytes: Word,
}

pub fn read(
    mut config: impl FnMut(Word, Word) -> Result<u32, RequestError>,
) -> Result<Layout, RequestError> {
    if config(4, 2)? & 2 == 0 {
        return Err(RequestError::Unsupported); // There is no live memory-decode owner to inherit.
    }
    let registers = bar(&mut config, 0x10)?;
    let aperture = bar(&mut config, 0x18)?;
    let size = config(0x62, 1)? as Word & 0x1f;
    if !matches!(size, 0 | 1 | 3 | 7 | 15 | 31) {
        return Err(RequestError::Unsupported);
    }
    let aperture_bytes = (size + 1) * 128 * 1024 * 1024;
    if registers & (16 * 1024 * 1024 - 1) != 0
        || aperture & (aperture_bytes - 1) != 0
        || registers
            .checked_add(16 * 1024 * 1024)
            .is_none_or(|end| end > 1usize << 39)
        || aperture
            .checked_add(aperture_bytes)
            .is_none_or(|end| end > 1usize << 39)
    {
        return Err(RequestError::Unsupported);
    }
    Ok(Layout {
        registers,
        aperture,
        aperture_bytes,
    })
}

fn bar(
    config: &mut impl FnMut(Word, Word) -> Result<u32, RequestError>,
    offset: Word,
) -> Result<Word, RequestError> {
    let low = config(offset, 4)?;
    if low & 7 != 4 {
        return Err(RequestError::Unsupported); // Gen9 uses 64-bit memory BARs, not I/O.
    }
    let high = config(offset + 4, 4)?;
    let base = (low as Word & !15) | ((high as Word) << 32);
    if base == 0 {
        return Err(RequestError::Unsupported);
    }
    Ok(base)
}
