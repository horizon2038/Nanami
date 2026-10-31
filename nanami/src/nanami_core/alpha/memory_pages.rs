//! Private RAM construction: initialize each byte once before publishing pages.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct PageSource {
    pub pid: usize,
    pub address: usize,
    pub bytes: usize,
}

impl Alpha {
    pub(super) fn map_process_heap_pages(
        &mut self,
        pid: usize,
        page_count: usize,
    ) -> Result<usize, CapabilityError> {
        self.map_process_initialized_pages(pid, None, page_count, None)
    }

    pub(super) fn map_process_heap_pages_at(
        &mut self,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) -> Result<usize, CapabilityError> {
        self.map_process_initialized_pages(pid, Some(base_va), page_count, None)
    }

    pub(super) fn map_process_initialized_pages(
        &mut self,
        pid: usize,
        requested_base: Option<usize>,
        page_count: usize,
        source: Option<PageSource>,
    ) -> Result<usize, CapabilityError> {
        let bytes = page_count
            .checked_mul(PAGE_SIZE)
            .ok_or(CapabilityError::InvalidArgument)?;
        if pid == 0 || page_count == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        if let Some(source) = source {
            if source.bytes > bytes || (source.bytes != 0 && source.address == 0) {
                return Err(CapabilityError::InvalidArgument);
            }
            let end = source
                .address
                .checked_add(source.bytes)
                .ok_or(CapabilityError::InvalidArgument)?;
            // Validate the source before allocating destination backing.
            if source.bytes != 0 {
                for va in (align_down(source.address, PAGE_SIZE)..end).step_by(PAGE_SIZE) {
                    self.process_frame_for_page(source.pid, va)?;
                }
            }
        }
        let (root_node, address_space, base_va, start_slot) = if let Some(base) = requested_base {
            let end = base
                .checked_add(bytes)
                .ok_or(CapabilityError::InvalidArgument)?;
            if self
                .processes
                .vm_space_mut(pid)
                .ok_or(CapabilityError::InvalidArgument)?
                .backed_pages(base, end)
                .first()
                .is_some()
            {
                return Err(CapabilityError::InvalidArgument);
            }
            let (root, space, slot) = self.processes.reserve_process_heap_at(
                pid,
                base,
                page_count,
                PAGE_SIZE,
                PROCESS_FRAME_TOTAL_PAGES,
            )?;
            (root, space, base, slot)
        } else {
            self.processes.reserve_process_heap(
                pid,
                page_count,
                PAGE_SIZE,
                PROCESS_FRAME_TOTAL_PAGES,
            )?
        };
        self.ensure_process_frame_chunks(pid, root_node, start_slot, page_count)?;
        let reuse_allocation = self
            .processes
            .take_deferred_physical_allocation(pid, base_va, page_count);
        if let Some(allocation) = reuse_allocation {
            // Transfer ownership of the entire reused extent before any
            // fallible cap/mapping operation, including its unused suffix.
            for i in 0..page_count {
                self.processes.register_physical_allocation(
                    pid,
                    base_va + i * PAGE_SIZE,
                    start_slot + i,
                    allocation.base_page + i,
                    1,
                )?;
            }
        }
        let result = (|| {
            for i in 0..page_count {
                let slot = start_slot + i;
                let va = base_va + i * PAGE_SIZE;
                let node =
                    process_frame_chunk_descriptor(root_node, slot / PROCESS_FRAME_CHUNK_PAGES);
                let within = slot % PROCESS_FRAME_CHUNK_PAGES;
                let page = if let Some(allocation) = reuse_allocation {
                    let page = allocation.base_page + i;
                    self.memory.ensure_alpha_frame_at_physical_index(page)?;
                    let frame = self
                        .memory
                        .physical_frame_descriptor_from_index(page)
                        .ok_or(CapabilityError::InvalidArgument)?;
                    arch::node::copy(node, within, frame)?;
                    page
                } else {
                    let page = self.memory.allocate_process_frame(
                        node,
                        PROCESS_FRAME_NODE_RADIX,
                        within,
                    )?;
                    self.processes
                        .register_physical_allocation(pid, va, slot, page, 1)?;
                    page
                };
                let destination = self.map_initialization_page(page)?;
                self.initialize_process_page(destination, source, i * PAGE_SIZE)?;
                let frame = process_frame_descriptor(root_node, slot);
                let vm = self
                    .processes
                    .vm_space_mut(pid)
                    .ok_or(CapabilityError::InvalidArgument)?;
                if let Err(error) = self.memory.map_frame_strict(address_space, frame, va, vm) {
                    // A failed MAP may have installed a leaf. Keep its Frame
                    // tracked so rollback must confirm UNMAP before reuse.
                    vm.record_frame(va, frame)
                        .map_err(|_| CapabilityError::Fatal)?;
                    return Err(error);
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            // Registered but not yet published pages are also owned here.
            // Release tolerates holes; every allocated page remains accounted.
            self.handle_mapping_release_request(OsRequestEvent {
                identifier: pid,
                code: nanami_uapi::os::OS_REQUEST_MAPPING_RELEASE,
                arg0: base_va,
                arg1: bytes,
                arg2: pid,
                arg3: nanami_uapi::os::OS_MAPPING_RELEASE_ALLOW_HOLES,
            })?;
            return Err(error);
        }
        Ok(base_va)
    }

    fn initialize_process_page(
        &mut self,
        destination: usize,
        source: Option<PageSource>,
        offset: usize,
    ) -> Result<(), CapabilityError> {
        let mut copied = 0;
        if let Some(source) = source {
            let bytes = PAGE_SIZE.min(source.bytes.saturating_sub(offset));
            while copied < bytes {
                let address = source.address + offset + copied;
                let page = align_down(address, PAGE_SIZE);
                let within = address - page;
                let chunk = (bytes - copied).min(PAGE_SIZE - within);
                let frame = self.process_frame_for_page(source.pid, page)?;
                // A different window keeps source mapping churn from evicting
                // the destination initialization mapping.
                let mapped = self.map_process_copy_frame(source.pid, frame, None)?;
                unsafe {
                    ptr::copy_nonoverlapping(
                        (mapped + within) as *const u8,
                        (destination + copied) as *mut u8,
                        chunk,
                    );
                }
                copied += chunk;
            }
        }
        unsafe {
            ptr::write_bytes((destination + copied) as *mut u8, 0, PAGE_SIZE - copied);
        }
        Ok(())
    }
}
