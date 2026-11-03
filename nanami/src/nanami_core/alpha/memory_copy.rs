use super::*;

// Separate from the image-staging and zero-fill windows. This is a bounded
// working-set cache, not a limit on the number of guest pages or processes.
const COPY_WINDOW_BASE: usize = 0x6600_0000;
const COPY_WINDOW_WAYS: usize = 4;
const COPY_WINDOW_SETS: usize = 512;
pub(super) const COPY_WINDOW_PAGES: usize = COPY_WINDOW_SETS * COPY_WINDOW_WAYS;
const OWNER_BUCKETS: usize = 256;
const POOL_OWNER: usize = 0;
const _: () = assert!(COPY_WINDOW_PAGES <= u16::MAX as usize);

#[derive(Clone, Copy)]
struct CopyMapping {
    pid: usize,
    frame: CapabilityDescriptor,
    ready: bool,
}

pub(super) struct CopyWindow {
    mappings: [Option<CopyMapping>; COPY_WINDOW_PAGES],
    next: [u8; COPY_WINDOW_SETS],
    // Bucket collisions only cause an extra invalidation scan. Failed MAP and
    // UNMAP entries count too: zero must prove that no alias belongs to a PID.
    owner_counts: [u16; OWNER_BUCKETS],
}

impl CopyWindow {
    pub(super) const fn new() -> Self {
        Self {
            mappings: [None; COPY_WINDOW_PAGES],
            next: [0; COPY_WINDOW_SETS],
            owner_counts: [0; OWNER_BUCKETS],
        }
    }

    fn set(pid: usize, frame: CapabilityDescriptor) -> usize {
        let mut hash = (frame as u64).wrapping_add((pid as u64).wrapping_mul(0x9e3779b97f4a7c15));
        hash ^= hash >> 30;
        hash = hash.wrapping_mul(0xbf58476d1ce4e5b9);
        hash ^= hash >> 27;
        hash = hash.wrapping_mul(0x94d049bb133111eb);
        hash ^= hash >> 31;
        (hash as usize) & (COPY_WINDOW_SETS - 1)
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
        // Owned/shared RAM buffers may retain permanent Alpha pool Frames.
        // Ordinary private RAM, MMIO and framebuffer mappings retain child
        // aliases and the existing PID-scoped cleanup behavior.
        let pid = if self.memory.is_pool_frame_descriptor(frame) {
            POOL_OWNER
        } else {
            self.processes.memory_owner_pid(pid)
        };
        let set = CopyWindow::set(pid, frame);
        let first = set * COPY_WINDOW_WAYS;
        let entries = &self.copy_window.mappings[first..first + COPY_WINDOW_WAYS];
        if let Some(way) = entries
            .iter()
            .position(|entry| entry.is_some_and(|m| m.ready && m.pid == pid && m.frame == frame))
        {
            return Ok(COPY_WINDOW_BASE + (first + way) * PAGE_SIZE);
        }

        let mut way = entries
            .iter()
            .position(|m| m.is_none_or(|m| !m.ready))
            .unwrap_or(self.copy_window.next[set] as usize);
        if pinned_va == Some(COPY_WINDOW_BASE + (first + way) * PAGE_SIZE) {
            way = (way + 1) % COPY_WINDOW_WAYS;
        }
        let index = first + way;
        self.unmap_process_copy_slot(index)?;
        let va = COPY_WINDOW_BASE + index * PAGE_SIZE;
        // MAP can fail after installing a PTE. Retain the descriptor for
        // cleanup, but do not allow a cache hit until MAP succeeds.
        self.copy_window.mappings[index] = Some(CopyMapping {
            pid,
            frame,
            ready: false,
        });
        self.copy_window.owner_counts[pid % OWNER_BUCKETS] += 1;
        self.map_alpha_temporary_frame(frame, va)?;
        self.copy_window.mappings[index].as_mut().unwrap().ready = true;
        self.copy_window.next[set] = ((way + 1) % COPY_WINDOW_WAYS) as u8;
        Ok(va)
    }

    fn unmap_process_copy_slot(&mut self, index: usize) -> Result<(), CapabilityError> {
        if let Some(mapping) = self.copy_window.mappings[index].as_mut() {
            mapping.ready = false;
            let frame = mapping.frame;
            let pid = mapping.pid;
            // Unlike disposable staging windows, never ignore an UNMAP error
            // here: the caller may next revoke this cap or recycle its RAM.
            self.unmap_alpha_frame(frame, COPY_WINDOW_BASE + index * PAGE_SIZE)?;
            self.copy_window.mappings[index] = None;
            self.copy_window.owner_counts[pid % OWNER_BUCKETS] -= 1;
        }
        Ok(())
    }

    // Must precede unmap, exec, cap removal/revocation, or physical-page reuse.
    // Failure leaves the mapping tracked and must stop the destructive action.
    pub(super) fn invalidate_process_copy_mappings(
        &mut self,
        pid: usize,
    ) -> Result<(), CapabilityError> {
        let pid = self.processes.memory_owner_pid(pid);
        if self.copy_window.owner_counts[pid % OWNER_BUCKETS] == 0 {
            return Ok(());
        }
        for index in 0..COPY_WINDOW_PAGES {
            if self.copy_window.mappings[index].is_some_and(|m| m.pid == pid) {
                self.unmap_process_copy_slot(index)?;
            }
        }
        Ok(())
    }
}
