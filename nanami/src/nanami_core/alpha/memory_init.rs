use super::*;

// Only Alpha-owned physical RAM Frame capabilities, never child aliases. Their
// lifetime is the physical pool's lifetime, so process revoke/reap cannot leave
// a dangling mapping. This caches mappings, NOT page contents: every allocation
// is still zeroed or filled before it is exposed to its new owner.
const BASE: usize = 0x6700_0000;
const PAGES: usize = 1024;

#[derive(Clone, Copy)]
struct Mapping {
    frame: CapabilityDescriptor,
    ready: bool,
}

pub(super) struct InitializationWindow {
    mappings: [Option<Mapping>; PAGES],
}

impl InitializationWindow {
    pub(super) const fn new() -> Self {
        Self {
            mappings: [None; PAGES],
        }
    }
}

impl Alpha {
    pub(super) fn map_initialization_page(
        &mut self,
        physical_page: usize,
    ) -> Result<usize, CapabilityError> {
        let frame = self
            .memory
            .physical_frame_descriptor_from_index(physical_page)
            .ok_or(CapabilityError::InvalidArgument)?;
        let index = physical_page % PAGES;
        let va = BASE + index * PAGE_SIZE;
        if let Some(mapping) = self.initialization_window.mappings[index] {
            if mapping.ready && mapping.frame == frame {
                return Ok(va);
            }
            self.initialization_window.mappings[index]
                .as_mut()
                .unwrap()
                .ready = false;
            // Preserve failed cleanup in the cache; never overwrite a live PTE.
            self.unmap_alpha_frame(mapping.frame, va)?;
            self.initialization_window.mappings[index] = None;
        }
        // A failed MAP may have installed a PTE. Retain it for cleanup, but
        // never reuse the address as a hit until MAP has completed successfully.
        self.initialization_window.mappings[index] = Some(Mapping {
            frame,
            ready: false,
        });
        self.map_alpha_temporary_frame(frame, va)?;
        self.initialization_window.mappings[index]
            .as_mut()
            .unwrap()
            .ready = true;
        Ok(va)
    }

}
