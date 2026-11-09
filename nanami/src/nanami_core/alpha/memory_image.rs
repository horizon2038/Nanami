//! Keep ready source aliases warm throughout prepared ELF construction.
use super::*;
use super::memory_pages::{REUSE_INITIALIZATION_SOURCE, SWAP_INITIALIZATION_ROLE};

pub(super) struct ImageSourceAliases {
    slots: [u64; memory_init::PAGES.div_ceil(64)],
}

impl ImageSourceAliases {
    fn protects(&self, physical: usize) -> bool {
        let slot = physical % memory_init::PAGES;
        self.slots[slot / 64] & (1u64 << (slot % 64)) != 0
    }
}

impl Alpha {
    pub(super) fn prepare_image_initialization_source(
        &self, source: Option<PageSource>,
    ) -> Option<ImageSourceAliases> {
        if !REUSE_INITIALIZATION_SOURCE || !SWAP_INITIALIZATION_ROLE { return None; }
        if option_env!("NANAMI_INIT_SOURCE_REUSE_DIAG") == Some("1") && source.is_none() {
            use core::sync::atomic::{AtomicBool, Ordering};
            static REPORTED: AtomicBool = AtomicBool::new(false);
            if !REPORTED.swap(true, Ordering::Relaxed) {
                crate::force_info!("[image.source] source-absent");
            }
        }
        let source = source?;
        let end = source.address.checked_add(source.bytes)?;
        if source.bytes == 0 { return None; }
        let vm = self.processes.vm_space(source.pid)?;
        let mut aliases = ImageSourceAliases { slots: [0; memory_init::PAGES.div_ceil(64)] };
        vm.visit_backed_metadata(align_down(source.address, PAGE_SIZE), end, |va, frame, _, _| {
            if va & (PAGE_SIZE - 1) != 0 { return; }
            if let Some(slot) = self.cached_initialization_slot(frame) {
                aliases.slots[slot / 64] |= 1u64 << (slot % 64);
            }
        });
        let any = aliases.slots.iter().any(|word| *word != 0);
        if option_env!("NANAMI_INIT_SOURCE_REUSE_DIAG") == Some("1") {
            use core::sync::atomic::{AtomicUsize, Ordering};
            static SEEN: AtomicUsize = AtomicUsize::new(0);
            let bit = if any { 2 } else { 1 };
            if SEEN.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
                let count: u32 = aliases.slots.iter().map(|word| word.count_ones()).sum();
                crate::force_info!("[image.source] source-present owner={} ready-slots={}", source.pid, count);
            }
        }
        if !any { return None; }
        Some(aliases)
    }

    pub(super) fn initialize_exec_image_page_content(
        &mut self, physical: usize, kind: &ProcessLazyMappingKind, page_va: usize,
        source: Option<PageSource>, aliases: Option<&ImageSourceAliases>,
    ) -> Result<(), CapabilityError> {
        let Some(source) = source else {
            let temp = self.map_initialization_page(physical)?;
            return fill_lazy_page(kind, page_va, temp);
        };
        let ProcessLazyMappingKind::Image { elf, .. } = kind else {
            self.map_initialization_page(physical)?;
            return Err(CapabilityError::InvalidArgument);
        };
        // Placement only, never read authority. Also preserve source aliases
        // that will be read by a later page in this same construction.
        let destination = if aliases.is_some_and(|aliases| aliases.protects(physical)) {
            let frame = self.memory.physical_frame_descriptor_from_index(physical)
                .ok_or(CapabilityError::InvalidArgument)?;
            let destination = self.map_process_copy_frame(0, frame, None)?;
            if option_env!("NANAMI_INIT_SOURCE_REUSE_DIAG") == Some("1") {
                use core::sync::atomic::{AtomicBool, Ordering};
                static REPORTED: AtomicBool = AtomicBool::new(false);
                if !REPORTED.swap(true, Ordering::Relaxed) {
                    crate::force_info!("[image.source] preserved-ready-source-alias owner={}", source.pid);
                }
            }
            destination
        } else {
            self.map_initialization_page(physical)?
        };
        let output = unsafe { core::slice::from_raw_parts_mut(destination as *mut u8, PAGE_SIZE) };
        fill_image_page(elf, source.bytes, page_va, output, |offset, output| {
            let address = source.address.checked_add(offset).ok_or(CapabilityError::InvalidArgument)?;
            self.copy_prepared_image_source(source.pid, address, output, destination)
        })
    }

    fn copy_prepared_image_source(
        &mut self, pid: usize, source: usize, output: &mut [u8], destination: usize,
    ) -> Result<(), CapabilityError> {
        source.checked_add(output.len()).ok_or(CapabilityError::InvalidArgument)?;
        let mut copied = 0;
        while copied < output.len() {
            let address = source + copied;
            let page = align_down(address, PAGE_SIZE);
            let within = address - page;
            let bytes = (output.len() - copied).min(PAGE_SIZE - within);
            // Preserve ordinary staging's current-Frame/direct-pool rule.
            let frame = self.process_frame_for_page(pid, page)?;
            let mapped = match self.cached_initialization_frame(frame) {
                Some(mapped) => mapped,
                None => self.map_process_copy_frame(pid, frame, Some(destination))?,
            };
            unsafe {
                ptr::copy_nonoverlapping((mapped + within) as *const u8,
                    output.as_mut_ptr().add(copied), bytes);
            }
            copied += bytes;
        }
        Ok(())
    }
}
