use super::*;

// Separate from the image-staging and zero-fill windows. This is a bounded
// working-set cache, not a limit on the number of guest pages or processes.
const COPY_WINDOW_BASE: usize = 0x6600_0000;
const COPY_WINDOW_PAGES: usize = 16;

#[derive(Clone, Copy)]
struct CopyMapping {
    pid: usize,
    frame: CapabilityDescriptor,
    ready: bool,
}

pub(super) struct CopyWindow {
    mappings: [Option<CopyMapping>; COPY_WINDOW_PAGES],
    next: usize,
}

impl CopyWindow {
    pub(super) const fn new() -> Self {
        Self {
            mappings: [None; COPY_WINDOW_PAGES],
            next: 0,
        }
    }
}

impl Alpha {
    // Call only after resolving the current process mapping and checking the
    // caller's authority. Source/destination share the cache: a poll read and
    // its writeback use the same mappings even though their roles are reversed.
    pub(super) fn map_process_copy_frame(
        &mut self,
        pid: usize,
        frame: CapabilityDescriptor,
        pinned_va: Option<usize>,
    ) -> Result<usize, CapabilityError> {
        if let Some(index) =
            self.copy_window.mappings.iter().position(|entry| {
                entry.is_some_and(|m| m.ready && m.pid == pid && m.frame == frame)
            })
        {
            return Ok(COPY_WINDOW_BASE + index * PAGE_SIZE);
        }

        let mut index = self
            .copy_window
            .mappings
            .iter()
            .position(|m| m.is_none_or(|m| !m.ready))
            .unwrap_or(self.copy_window.next);
        if pinned_va == Some(COPY_WINDOW_BASE + index * PAGE_SIZE) {
            index = (index + 1) % COPY_WINDOW_PAGES;
        }
        self.unmap_process_copy_slot(index)?;
        let va = COPY_WINDOW_BASE + index * PAGE_SIZE;
        // MAP can fail after installing a PTE. Retain the descriptor for
        // cleanup, but do not allow a cache hit until MAP succeeds.
        self.copy_window.mappings[index] = Some(CopyMapping {
            pid,
            frame,
            ready: false,
        });
        self.map_alpha_temporary_frame(frame, va)?;
        self.copy_window.mappings[index].as_mut().unwrap().ready = true;
        self.copy_window.next = (index + 1) % COPY_WINDOW_PAGES;
        Ok(va)
    }

    fn unmap_process_copy_slot(&mut self, index: usize) -> Result<(), CapabilityError> {
        if let Some(mapping) = self.copy_window.mappings[index].as_mut() {
            mapping.ready = false;
            let frame = mapping.frame;
            // Unlike disposable staging windows, never ignore an UNMAP error
            // here: the caller may next revoke this cap or recycle its RAM.
            self.unmap_alpha_frame(frame, COPY_WINDOW_BASE + index * PAGE_SIZE)?;
            self.copy_window.mappings[index] = None;
        }
        Ok(())
    }

    // Must precede unmap, exec, cap removal/revocation, or physical-page reuse.
    // Failure leaves the mapping tracked and must stop the destructive action.
    pub(super) fn invalidate_process_copy_mappings(
        &mut self,
        pid: usize,
    ) -> Result<(), CapabilityError> {
        for index in 0..COPY_WINDOW_PAGES {
            if self.copy_window.mappings[index].is_some_and(|m| m.pid == pid) {
                self.unmap_process_copy_slot(index)?;
            }
        }
        Ok(())
    }
}
