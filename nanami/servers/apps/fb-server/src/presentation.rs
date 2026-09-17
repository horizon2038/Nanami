use super::*;

pub(super) fn present_shared_framebuffer(
    display: &mut DisplayState,
    owner_pid: Word,
    position: Word,
    size: Word,
) -> Result<(Word, usize), RequestError> {
    let shared = display.shared;
    if owner_pid == 0 || shared.owner_pid != owner_pid || shared.local_vaddr == 0 {
        return Err(RequestError::Status(
            libnanami::OS_RESPONSE_PERMISSION_DENIED,
        ));
    }

    let x = (position & 0xffff_ffff) as usize;
    let y = ((position >> 32) & 0xffff_ffff) as usize;
    let width = (size & 0xffff_ffff) as usize;
    let height = ((size >> 32) & 0xffff_ffff) as usize;
    if width == 0 || height == 0 || x >= display.screen.width || y >= display.screen.height {
        return Err(RequestError::InvalidArgument);
    }

    let width = width.min(display.screen.width - x);
    let height = height.min(display.screen.height - y);
    let row_bytes = width.saturating_mul(4);
    let stride = display.screen.stride;
    let first = y.saturating_mul(stride).saturating_add(x.saturating_mul(4));
    let end = first
        .checked_add((height - 1).saturating_mul(stride))
        .and_then(|last| last.checked_add(row_bytes))
        .ok_or(RequestError::InvalidArgument)?;
    if end > shared.bytes
        || end > display.screen.framebuffer_bytes
        || end > stride.saturating_mul(display.screen.height)
    {
        return Err(RequestError::InvalidArgument);
    }
    fence(Ordering::Acquire);
    let mut written = 0;
    if x == 0 && row_bytes == stride {
        written = copy_to_hardware(display, first, row_bytes * height);
    } else {
        for row in 0..height {
            written += copy_to_hardware(display, first + row * stride, row_bytes);
        }
    }
    fence(Ordering::Release);
    // Preserve PRESENT's pixel count: it acknowledges the requested region,
    // even if that region was already on screen.
    Ok((width * height, written))
}

fn copy_to_hardware(display: &mut DisplayState, offset: usize, bytes: usize) -> usize {
    // Honoka owns this RAM and waits for our synchronous reply before changing
    // it again. Only fb-server writes the hardware framebuffer after startup.
    let source = unsafe {
        core::slice::from_raw_parts((display.shared.local_vaddr + offset) as *const u8, bytes)
    };
    let destination = display.hardware_vaddr + offset;
    let write = |start: usize, pixels: &[u8]| unsafe {
        core::ptr::copy_nonoverlapping(
            pixels.as_ptr(),
            (destination + start) as *mut u8,
            pixels.len(),
        );
    };
    if let Some(shadow) = &mut display.shadow {
        shadow.copy(offset, source, write)
    } else {
        // Cache allocation failure must not prevent the desktop from working.
        write(0, source);
        bytes
    }
}
