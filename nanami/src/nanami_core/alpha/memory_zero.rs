//! Fresh owned RAM is always cleared before any process mapping is published.
use super::*;

impl Alpha {
    pub(super) fn zero_physical_pages(
        &mut self, first: usize, pages: usize,
    ) -> Result<(), CapabilityError> {
        first.checked_add(pages).ok_or(CapabilityError::InvalidArgument)?;
        let mut offset = 0;
        while offset < pages {
            // Only the permanent pool Frame is cached. Contents are not a
            // cache hit: even a reused alias must clear every byte again.
            let (address, count) = self.map_initialization_run(first + offset, pages - offset)?;
            unsafe { core::ptr::write_bytes(address as *mut u8, 0, count * PAGE_SIZE); }
            offset += count;
        }
        Ok(())
    }
}
