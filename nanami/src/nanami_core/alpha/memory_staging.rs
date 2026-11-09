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
        let buffer = unsafe { core::slice::from_raw_parts_mut(out, size_bytes) };
        self.copy_process_memory_into(source_pid, source_va, buffer)?;
        Ok(buffer)
    }

    pub(super) fn copy_process_memory_into(
        &mut self,
        source_pid: usize,
        source_va: usize,
        buffer: &mut [u8],
    ) -> Result<(), CapabilityError> {
        let size_bytes = buffer.len();
        source_va.checked_add(size_bytes).ok_or(CapabilityError::InvalidArgument)?;
        let out = buffer.as_mut_ptr();
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
            // Read immediately into independent static storage. A ready pool
            // alias needs no second MAP; misses retain the separate copy window.
            let mapped = match self.cached_initialization_frame(frame) {
                Some(mapped) => mapped,
                None => self.map_process_copy_frame(source_pid, frame, None)?,
            };
            unsafe {
                ptr::copy_nonoverlapping(
                    (mapped + src_offset) as *const u8,
                    out.add(copied),
                    chunk,
                );
            }
            copied += chunk;
        }
        Ok(())
    }
}
