//! Bounded request-local copies from current process Frames.
use super::*;

impl Alpha {
    pub(super) fn read_process_memory_into_static_buffer(
        &mut self,
        source_pid: usize,
        source_va: usize,
        size_bytes: usize,
    ) -> Result<&'static [u8], CapabilityError> {
        if size_bytes > PROCESS_SPAWN_MEMORY_MAX_BYTES {
            return Err(CapabilityError::InvalidArgument);
        }
        source_va
            .checked_add(size_bytes)
            .ok_or(CapabilityError::InvalidArgument)?;
        let out = core::ptr::addr_of_mut!(PROCESS_MEMORY_IMAGE_BUFFER) as *mut u8;
        let mut copied = 0usize;
        while copied < size_bytes {
            let src = source_va + copied;
            let src_page = align_down(src, PAGE_SIZE);
            let src_offset = src - src_page;
            let chunk = min_usize(size_bytes - copied, PAGE_SIZE - src_offset);
            // Resolve the current Frame on every page. The existing bounded
            // alias cache owns mapping lifetime; physical-allocation snapshots
            // are neither authority nor necessary to read process memory.
            let frame = self.process_frame_for_page(source_pid, src_page)?;
            let mapped = self.map_process_copy_frame(source_pid, frame, None)?;
            unsafe {
                ptr::copy_nonoverlapping(
                    (mapped + src_offset) as *const u8,
                    out.add(copied),
                    chunk,
                );
            }
            copied += chunk;
        }
        Ok(unsafe { core::slice::from_raw_parts(out as *const u8, size_bytes) })
    }
}
