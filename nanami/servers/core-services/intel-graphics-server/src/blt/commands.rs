//! Gen9 linear XRGB8888 BLT packets. Addresses are provider-owned PPGTT VAs.
use nanami_runtime::{RequestError, Word};
use nanami_uapi::gpu::{Layout, Rect};
const TYPE_BLT: u32 = 2 << 29;
const WRITE_RGBA: u32 = 3 << 20;
const DEPTH_32: u32 = 3 << 24;
pub const MI_ARB_ENABLE: u32 = (0x08 << 23) | 1;
const MI_FLUSH_DW: u32 = (0x26 << 23) | 2;
const POST_SYNC_DWORD: u32 = 1 << 14;
const INVALIDATE_TLB: u32 = 1 << 18;
const USE_GGTT: u32 = 1 << 2;

fn address(address: Word, layout: Layout) -> Result<u32, RequestError> {
    if address & 4095 != 0
        || address
            .checked_add(layout.bytes)
            .is_none_or(|end| end > (1usize << 32))
    {
        return Err(RequestError::InvalidArgument);
    }
    Ok(address as u32)
}
pub fn fill(gpu: Word, layout: Layout, rect: Rect, color: u32) -> Result<[u32; 7], RequestError> {
    if !rect.inside(layout)
        || Layout::new(layout.width, layout.height, layout.stride) != Some(layout)
    {
        return Err(RequestError::InvalidArgument);
    }
    Ok([
        TYPE_BLT | (0x50 << 22) | WRITE_RGBA | 5,
        DEPTH_32 | (0xf0 << 16) | layout.stride as u32,
        (rect.x | (rect.y << 16)) as u32,
        ((rect.x + rect.width) | ((rect.y + rect.height) << 16)) as u32,
        address(gpu, layout)?,
        0,
        color,
    ])
}
pub fn copy(
    source: Word,
    source_layout: Layout,
    target: Word,
    target_layout: Layout,
    rect: Rect,
    x: Word,
    y: Word,
) -> Result<[u32; 10], RequestError> {
    let to = Rect {
        x,
        y,
        width: rect.width,
        height: rect.height,
    };
    if !rect.inside(source_layout)
        || !to.inside(target_layout)
        || Layout::new(
            source_layout.width,
            source_layout.height,
            source_layout.stride,
        ) != Some(source_layout)
        || Layout::new(
            target_layout.width,
            target_layout.height,
            target_layout.stride,
        ) != Some(target_layout)
        || (source == target && rect.overlaps(to))
    {
        return Err(RequestError::InvalidArgument);
    }
    Ok([
        TYPE_BLT | (0x53 << 22) | WRITE_RGBA | 8,
        DEPTH_32 | (0xcc << 16) | target_layout.stride as u32,
        (x | (y << 16)) as u32,
        ((x + rect.width) | ((y + rect.height) << 16)) as u32,
        address(target, target_layout)?,
        0,
        (rect.x | (rect.y << 16)) as u32,
        source_layout.stride as u32,
        address(source, source_layout)?,
        0,
    ])
}
pub fn flush(fence: Word, value: u32, invalidate: bool) -> [u32; 4] {
    assert!(fence & 63 == 0 && fence < (1usize << 32));
    let mut command = MI_FLUSH_DW | POST_SYNC_DWORD;
    if invalidate {
        command |= INVALIDATE_TLB;
    }
    [command, fence as u32 | USE_GGTT, 0, value]
}
