use super::{
    write_u32, write_u64, Word, LINUX_FB_FIX_SCREENINFO_BYTES, LINUX_FB_VAR_SCREENINFO_BYTES,
};
use crate::common::framebuffer_size::FramebufferSize;

pub(super) fn write_fb_fix_screeninfo(base: Word, size: FramebufferSize) {
    unsafe {
        ::core::ptr::write_bytes(base as *mut u8, 0, LINUX_FB_FIX_SCREENINFO_BYTES as usize);
        let id = b"Nanami Honoka fb";
        ::core::ptr::copy_nonoverlapping(id.as_ptr(), base as *mut u8, id.len());
        write_u64(base + 16, 0);
        write_u32(base + 24, size.bytes() as u32);
        write_u32(base + 28, 0);
        write_u32(base + 32, 0);
        write_u32(base + 36, 2);
        write_u32(base + 48, size.stride() as u32);
    }
}

pub(super) fn write_fb_var_screeninfo(base: Word, size: FramebufferSize) {
    unsafe {
        ::core::ptr::write_bytes(base as *mut u8, 0, LINUX_FB_VAR_SCREENINFO_BYTES as usize);
        write_u32(base, size.width as u32);
        write_u32(base + 4, size.height as u32);
        write_u32(base + 8, size.width as u32);
        write_u32(base + 12, size.height as u32);
        write_u32(base + 24, 32);
        write_u32(base + 32, 16);
        write_u32(base + 36, 8);
        write_u32(base + 44, 8);
        write_u32(base + 48, 8);
        write_u32(base + 56, 0);
        write_u32(base + 60, 8);
        write_u32(base + 68, 24);
        write_u32(base + 72, 8);
    }
}
