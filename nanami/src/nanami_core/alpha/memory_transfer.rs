//! Request-local process-memory transfers; mapping lifetime stays in the alias caches.
use super::*;

impl Alpha {
    pub(super) fn copy_process_memory(
        &mut self,
        source_pid: usize,
        source_va: usize,
        destination_pid: usize,
        destination_va: usize,
        size_bytes: usize,
    ) -> Result<(), CapabilityError> {
        if source_pid == destination_pid && source_va == destination_va {
            return Ok(());
        }
        let source_end = source_va
            .checked_add(size_bytes)
            .ok_or(CapabilityError::InvalidArgument)?;
        let _ = destination_va
            .checked_add(size_bytes)
            .ok_or(CapabilityError::InvalidArgument)?;
        let copy_backward = source_pid == destination_pid
            && destination_va > source_va
            && destination_va < source_end;
        // Only this synchronous transfer owns these cursors. The accessors
        // below can populate an absent page, but never replace an existing
        // Frame or change its private-write guard. No reentrant dispatch or
        // destructive VM operation occurs between chunks of the same page.
        // Alias placement is still revalidated below after BOTH accessors.
        let mut source_frame: Option<(usize, CapabilityDescriptor)> = None;
        let mut destination_frame: Option<(usize, CapabilityDescriptor)> = None;
        let mut copied = 0usize;
        while copied < size_bytes {
            let (src, dst, chunk) = if copy_backward {
                let remaining = size_bytes - copied;
                let src_end = source_va + remaining;
                let dst_end = destination_va + remaining;
                let src_page_tail = ((src_end - 1) & (PAGE_SIZE - 1)) + 1;
                let dst_page_tail = ((dst_end - 1) & (PAGE_SIZE - 1)) + 1;
                let chunk = min_usize(remaining, min_usize(src_page_tail, dst_page_tail));
                (src_end - chunk, dst_end - chunk, chunk)
            } else {
                let src = source_va + copied;
                let dst = destination_va + copied;
                let src_offset = src & (PAGE_SIZE - 1);
                let dst_offset = dst & (PAGE_SIZE - 1);
                let chunk = min_usize(
                    size_bytes - copied,
                    min_usize(PAGE_SIZE - src_offset, PAGE_SIZE - dst_offset),
                );
                (src, dst, chunk)
            };
            let src_page = align_down(src, PAGE_SIZE);
            let dst_page = align_down(dst, PAGE_SIZE);
            let src_offset = src - src_page;
            let dst_offset = dst - dst_page;
            let src_frame = match source_frame {
                Some((page, frame)) if page == src_page => frame,
                _ => {
                    let frame = self.process_frame_for_page(source_pid, src_page)?;
                    source_frame = Some((src_page, frame));
                    frame
                }
            };
            let dst_frame = match destination_frame {
                Some((page, frame)) if page == dst_page => frame,
                _ => {
                    let frame = self.process_frame_for_write(destination_pid, dst_page)?;
                    destination_frame = Some((dst_page, frame));
                    frame
                }
            };
            let src_temp_va = self.map_process_transfer_frame(source_pid, src_frame, None)?;

            if src_frame == dst_frame {
                unsafe {
                    ptr::copy(
                        (src_temp_va + src_offset) as *const u8,
                        (src_temp_va + dst_offset) as *mut u8,
                        chunk,
                    );
                }
            } else {
                let dst_temp_va =
                    self.map_process_transfer_frame(destination_pid, dst_frame, Some(src_temp_va))?;
                unsafe {
                    ptr::copy_nonoverlapping(
                        (src_temp_va + src_offset) as *const u8,
                        (dst_temp_va + dst_offset) as *mut u8,
                        chunk,
                    );
                }
            }
            copied += chunk;
        }

        Ok(())
    }

}
