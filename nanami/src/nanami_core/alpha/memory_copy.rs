use super::*;

// Separate from the image-staging and zero-fill windows. This is a bounded
// working-set cache, not a limit on the number of guest pages or processes.
const COPY_WINDOW_BASE: usize = 0x6600_0000;
const COPY_WINDOW_WAYS: usize = 4;
const COPY_WINDOW_SETS: usize = 512;
pub(super) const COPY_WINDOW_PAGES: usize = COPY_WINDOW_SETS * COPY_WINDOW_WAYS;
const OWNER_BUCKETS: usize = 256;
const POOL_OWNER: usize = 0;
const NO_SLOT: u16 = u16::MAX;
const _: () = assert!(COPY_WINDOW_PAGES <= u16::MAX as usize);

#[derive(Clone, Copy)]
struct CopyMapping {
    pid: usize,
    frame: CapabilityDescriptor,
    ready: bool,
    previous: u16,
    next: u16,
}

pub(super) struct CopyWindow {
    mappings: [Option<CopyMapping>; COPY_WINDOW_PAGES],
    next: [u8; COPY_WINDOW_SETS],
    // Intrusive bucket lists include uncertain MAP/UNMAP entries. Full PID
    // checks resolve collisions; links occupy CopyMapping's existing padding.
    owner_heads: [u16; OWNER_BUCKETS],
}

impl CopyWindow {
    #[cfg(test)]
    pub(super) fn assert_consistent(&self) {
        let mut seen = [false; COPY_WINDOW_PAGES];
        for bucket in 0..OWNER_BUCKETS {
            let mut slot = self.owner_heads[bucket];
            let mut previous = NO_SLOT;
            while slot != NO_SLOT {
                let index = slot as usize;
                assert!(!seen[index]);
                seen[index] = true;
                let mapping = self.mappings[index].unwrap();
                assert_eq!(mapping.pid % OWNER_BUCKETS, bucket);
                assert_eq!(mapping.previous, previous);
                previous = slot;
                slot = mapping.next;
            }
        }
        for (index, mapping) in self.mappings.iter().enumerate() {
            assert_eq!(seen[index], mapping.is_some());
        }
    }

    pub(super) const fn new() -> Self {
        Self {
            mappings: [None; COPY_WINDOW_PAGES],
            next: [0; COPY_WINDOW_SETS],
            owner_heads: [NO_SLOT; OWNER_BUCKETS],
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
    /// Read-only lookup of an already mapped permanent pool Frame. No PID
    /// inference, miss mapping, eviction or use of an uncertain copy slot.
    pub(super) fn cached_pool_copy_frame(&self, frame: CapabilityDescriptor) -> Option<usize> {
        if !self.memory.is_pool_frame_descriptor(frame) { return None; }
        let first = CopyWindow::set(POOL_OWNER, frame) * COPY_WINDOW_WAYS;
        self.copy_window.mappings[first..first + COPY_WINDOW_WAYS].iter()
            .position(|entry| entry.is_some_and(|mapping|
                mapping.ready && mapping.pid == POOL_OWNER && mapping.frame == frame))
            .map(|way| COPY_WINDOW_BASE + (first + way) * PAGE_SIZE)
    }

    // Resolve both transfer Frames before calling. Consume the returned alias
    // before any operation that can change the initialization window. Mapping
    // another transfer alias below changes only CopyWindow on a miss.
    pub(super) fn map_process_transfer_frame(
        &mut self, pid: usize, frame: CapabilityDescriptor, pinned_va: Option<usize>,
    ) -> Result<usize, CapabilityError> {
        // The initialization cache contains only permanent pool caps and
        // validates the complete descriptor and Ready state on every hit.
        if let Some(address) = self.cached_initialization_frame(frame) {
            return Ok(address);
        }
        self.map_process_copy_frame(pid, frame, pinned_va)
    }

    // Call only after resolving the current process mapping and checking the
    // caller's authority. Source/destination share the cache: a poll read and
    // its writeback use the same mappings even though their roles are reversed.
    pub(super) fn map_process_copy_frame(
        &mut self,
        pid: usize,
        frame: CapabilityDescriptor,
        pinned_va: Option<usize>,
    ) -> Result<usize, CapabilityError> {
        if let Some((address, _)) = self.memory.map_private_large_alias(frame, pinned_va)? {
            return Ok(address);
        }
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
        let bucket = pid % OWNER_BUCKETS;
        let next = self.copy_window.owner_heads[bucket];
        self.copy_window.mappings[index] = Some(CopyMapping {
            pid,
            frame,
            ready: false,
            previous: NO_SLOT,
            next,
        });
        if next != NO_SLOT {
            self.copy_window.mappings[next as usize].as_mut().unwrap().previous = index as u16;
        }
        self.copy_window.owner_heads[bucket] = index as u16;
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
            let previous = mapping.previous;
            let next = mapping.next;
            // Unlike disposable staging windows, never ignore an UNMAP error
            // here: the caller may next revoke this cap or recycle its RAM.
            self.unmap_alpha_frame(frame, COPY_WINDOW_BASE + index * PAGE_SIZE)?;
            if previous == NO_SLOT {
                self.copy_window.owner_heads[pid % OWNER_BUCKETS] = next;
            } else {
                self.copy_window.mappings[previous as usize].as_mut().unwrap().next = next;
            }
            if next != NO_SLOT {
                self.copy_window.mappings[next as usize].as_mut().unwrap().previous = previous;
            }
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
        let pid = self.processes.memory_owner_pid(pid);
        let mut slot = self.copy_window.owner_heads[pid % OWNER_BUCKETS];
        if slot == NO_SLOT {
            return Ok(());
        }
        // Snapshot only slot numbers, in a bounded 256-byte bitset. Iterating
        // it preserves the old ascending UNMAP/error-prefix order even though
        // insertion/eviction keep the bucket list in a different order.
        let mut selected = [0u64; COPY_WINDOW_PAGES.div_ceil(64)];
        while slot != NO_SLOT {
            let mapping = self.copy_window.mappings[slot as usize].unwrap();
            if mapping.pid == pid {
                selected[slot as usize / 64] |= 1u64 << (slot as usize % 64);
            }
            slot = mapping.next;
        }
        for (word, mut bits) in selected.into_iter().enumerate() {
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                let index = word * 64 + bit;
                self.unmap_process_copy_slot(index)?;
                bits &= bits - 1;
            }
        }
        Ok(())
    }
}
