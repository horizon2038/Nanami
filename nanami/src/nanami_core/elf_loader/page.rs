//! Ordered ELF page construction with a caller-owned source transfer.
use super::{CapabilityError, ElfImage};

pub fn fill_image_page(
    elf: &ElfImage,
    image_bytes: usize,
    page_va: usize,
    output: &mut [u8],
    mut copy: impl FnMut(usize, &mut [u8]) -> Result<(), CapabilityError>,
) -> Result<(), CapabilityError> {
    let mut initialized = false;
    let result = (|| {
        let page_end = page_va.checked_add(output.len())
            .ok_or(CapabilityError::InvalidArgument)?;
        for seg in &elf.segments[..elf.segment_count] {
            let memory_end = seg.virtual_address.checked_add(seg.memory_size)
                .ok_or(CapabilityError::InvalidArgument)?;
            if seg.memory_size == 0 || page_va >= memory_end || page_end <= seg.virtual_address {
                continue;
            }
            let file_end = seg.virtual_address.checked_add(seg.file_size)
                .ok_or(CapabilityError::InvalidArgument)?;
            let first = page_va.max(seg.virtual_address);
            let last = page_end.min(file_end);
            if first >= last { continue; }
            let offset = seg.offset.checked_add(first - seg.virtual_address)
                .ok_or(CapabilityError::InvalidArgument)?;
            let bytes = last - first;
            if offset.checked_add(bytes).is_none_or(|end| end > image_bytes) {
                return Err(CapabilityError::InvalidArgument);
            }
            if !initialized {
                if first != page_va || last != page_end { output.fill(0); }
                initialized = true;
            }
            copy(offset, &mut output[first-page_va..last-page_va])?;
        }
        Ok(page_end)
    })();
    // Match the original fill rule, including the prefix retained on failure.
    if !initialized { output.fill(0); }
    let page_end = result?;
    if let Some(ipc) = elf.ipc_buffer_start {
        let slot = ipc + (nun::TLS_BASE_OFFSET as usize) * nun::BYTE_BITS;
        if slot >= page_va && slot + core::mem::size_of::<nun::Word>() <= page_end {
            let offset = slot - page_va;
            output[offset..offset+core::mem::size_of::<nun::Word>()]
                .copy_from_slice(&(ipc as nun::Word).to_ne_bytes());
        }
    }
    Ok(())
}
