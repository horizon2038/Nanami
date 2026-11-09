//! Sparse, request-local ELF staging. Unloaded bytes must never be consumed.
use super::{CapabilityError, ElfImage, ElfReader};

pub struct StagedImage<'a, F> {
    buffer: &'a mut [u8],
    loaded: [u64; 64],
    page_size: usize,
    copy: F,
}

impl<'a, F> StagedImage<'a, F>
where F: FnMut(usize, &mut [u8]) -> Result<(), CapabilityError> {
    pub fn new(buffer: &'a mut [u8], page_size: usize, copy: F) -> Result<Self, CapabilityError> {
        if page_size == 0 || buffer.len().div_ceil(page_size) > 4096 {
            return Err(CapabilityError::InvalidArgument);
        }
        Ok(Self { buffer, loaded: [0; 64], page_size, copy })
    }

    /// Populate every byte used by the ordinary ordered LOAD overlay. Complete
    /// readonly pages will be installed from the immutable source, not buffer.
    pub fn stage_private_pages(&mut self, elf: &ElfImage) -> Result<(), CapabilityError> {
        for segment in &elf.segments[..elf.segment_count] {
            let mut page = segment.virtual_address / self.page_size * self.page_size;
            let end = segment.virtual_address.checked_add(segment.file_size)
                .ok_or(CapabilityError::InvalidArgument)?;
            while page < end {
                let page_end = page.checked_add(self.page_size)
                    .ok_or(CapabilityError::InvalidArgument)?;
                if elf.snapshot_page(page, self.page_size).is_none() {
                    let first = page.max(segment.virtual_address);
                    let last = page_end.min(end);
                    let offset = segment.offset.checked_add(first - segment.virtual_address)
                        .ok_or(CapabilityError::InvalidArgument)?;
                    self.read(offset, last - first)?;
                }
                page = page_end;
            }
        }
        Ok(())
    }

    pub fn finish(self) -> &'a [u8] { self.buffer }
}

impl<F> ElfReader for StagedImage<'_, F>
where F: FnMut(usize, &mut [u8]) -> Result<(), CapabilityError> {
    fn len(&self) -> usize { self.buffer.len() }

    fn read(&mut self, offset: usize, length: usize) -> Result<&[u8], CapabilityError> {
        let end = offset.checked_add(length).ok_or(CapabilityError::InvalidArgument)?;
        if end > self.buffer.len() { return Err(CapabilityError::InvalidArgument); }
        if length != 0 {
            let first = offset / self.page_size;
            let last = (end - 1) / self.page_size;
            for page in first..=last {
                let mask = 1u64 << (page % 64);
                if self.loaded[page / 64] & mask != 0 { continue; }
                let start = page * self.page_size;
                let limit = (start + self.page_size).min(self.buffer.len());
                (self.copy)(start, &mut self.buffer[start..limit])?;
                // Publish only a successful complete page. A failed request
                // cannot leak stale bytes from this or the preceding image.
                self.loaded[page / 64] |= mask;
            }
        }
        Ok(&self.buffer[offset..end])
    }
}
