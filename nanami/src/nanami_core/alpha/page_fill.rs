//! Fill every byte before use, without zeroing bytes immediately overwritten.
use super::*;

pub(super) fn fill_lazy_page(
    kind: &ProcessLazyMappingKind,
    page_va: usize,
    temp_va: usize,
) -> Result<(), CapabilityError> {
    let ProcessLazyMappingKind::Image { image, elf } = kind else {
        unsafe { ptr::write_bytes(temp_va as *mut u8, 0, PAGE_SIZE); }
        return Ok(());
    };

    let output = unsafe { core::slice::from_raw_parts_mut(temp_va as *mut u8, PAGE_SIZE) };
    fill_image_page(elf, image.len(), page_va, output, |offset, destination| {
        destination.copy_from_slice(&image[offset..offset + destination.len()]);
        Ok(())
    })
}
