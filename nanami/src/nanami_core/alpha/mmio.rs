use super::*;

impl Alpha {
    pub(super) fn handle_mmio_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        let physical_address = request.arg0;
        let size_bytes = request.arg1;
        let mut stage = "validate";
        let mut page = 0usize;
        let result = (|| {
            let pid = request.identifier;
            if pid == 0 {
                return Err(CapabilityError::PermissionDenied);
            }
            if size_bytes == 0 || (physical_address & (PAGE_SIZE - 1)) != 0 {
                return Err(CapabilityError::InvalidArgument);
            }

            let mapped_size = align_up(size_bytes, PAGE_SIZE);
            let page_count = mapped_size / PAGE_SIZE;
            stage = "reserve-virtual";
            let (root_node, address_space, base_va, start_slot) = self
                .processes
                .reserve_process_heap(pid, page_count, PAGE_SIZE, PROCESS_FRAME_TOTAL_PAGES)?;
            stage = "allocate-physical";
            let base_page =
                self.memory
                    .allocate_physical_at(physical_address, mapped_size, true)?;
            let base_paddr = base_page * PAGE_SIZE;
            if base_paddr != physical_address {
                return Err(CapabilityError::InvalidArgument);
            }
            stage = "convert-frames";
            let (converted_base_index, skip_pages, converted_page_count) = self
                .memory
                .ensure_alpha_frames_for_range_from_initial_generic(
                    physical_address,
                    mapped_size,
                    true,
                )?;
            if converted_page_count != page_count {
                return Err(CapabilityError::InvalidArgument);
            }

            stage = "frame-slots";
            self.ensure_process_frame_chunks(pid, root_node, start_slot, page_count)?;
            stage = "copy-frames";
            let mut i = 0usize;
            while i < page_count {
                page = i;
                let source_frame = self
                    .memory
                    .physical_frame_descriptor_from_index(converted_base_index + skip_pages + i)
                    .ok_or(CapabilityError::InvalidArgument)?;
                arch::node::copy(
                    process_frame_chunk_descriptor(
                        root_node,
                        (start_slot + i) / PROCESS_FRAME_CHUNK_PAGES,
                    ),
                    ((start_slot + i) % PROCESS_FRAME_CHUNK_PAGES) as Word,
                    source_frame,
                )?;
                i += 1;
            }

            stage = "map-frames";
            let memory = &mut self.memory;
            let processes = &mut self.processes;
            let mut j = 0usize;
            while j < page_count {
                page = j;
                let frame = process_frame_descriptor(root_node, start_slot + j);
                let va = base_va + j * PAGE_SIZE;
                let vm = processes
                    .vm_space_mut(pid)
                    .ok_or(CapabilityError::InvalidArgument)?;
                memory.map_frame(address_space, frame, va, vm)?;
                j += 1;
            }

            info!(
                "[mmio] granted pid={:>3} size={:#x} paddr={:#018x} vaddr={:#018x}",
                pid, mapped_size, physical_address, base_va
            );
            Ok((physical_address, base_va))
        })();
        if let Err(error) = &result {
            error!(
                "[mmio.err] pid={} paddr={:#x} bytes={:#x} stage={} page={} err={:?}",
                request.identifier, physical_address, size_bytes, stage, page, error
            );
        }
        result
    }
}
