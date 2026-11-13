use super::*;

impl Alpha {
    pub(super) fn handle_process_compare_exchange(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<Word, CapabilityError> {
        use core::sync::atomic::{AtomicU32, Ordering};
        let pid = request.arg0;
        let address = request.arg1;
        if address == 0
            || address & 3 != 0
            || request.arg2 > u32::MAX as Word
            || request.arg3 > u32::MAX as Word
        {
            return Err(CapabilityError::InvalidArgument);
        }
        let target = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if request.identifier == 0
            || (request.identifier != pid && target.reaper_pid != request.identifier)
        {
            return Err(CapabilityError::PermissionDenied);
        }
        let page = align_down(address, PAGE_SIZE);
        let frame = self.process_frame_for_write(pid, page)?;
        let mapped = self.map_process_transfer_frame(pid, frame, None)?;
        // Four-byte alignment guarantees that the atomic cannot cross a page.
        let atomic = unsafe { &*((mapped + address - page) as *const AtomicU32) };
        let previous = match atomic.compare_exchange(
            request.arg2 as u32,
            request.arg3 as u32,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(previous) | Err(previous) => previous,
        };
        Ok(previous as Word)
    }

    pub(super) fn handle_page_alloc_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let page_count = request.arg0;
        if page_count == 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let heap_base = self.map_process_heap_pages(pid, page_count)?;

        info!(
            "[mem] granted pid={:>3} pages={:>4} va=[{:#018x}..{:#018x})",
            pid,
            page_count,
            heap_base,
            heap_base + page_count * PAGE_SIZE,
        );
        Ok(heap_base)
    }

    pub(super) fn handle_heap_alloc_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let requested_size = request.arg0;
        if requested_size == 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let mapped_size = align_up(requested_size, PAGE_SIZE);
        let page_count = mapped_size / PAGE_SIZE;
        let heap_base = self.map_process_heap_pages(pid, page_count)?;
        let guard_base =
            self.processes
                .reserve_process_virtual_gap(pid, USER_HEAP_GUARD_PAGES, PAGE_SIZE)?;

        info!(
            "[heap] granted pid={:>3} bytes={:#x} mapped={:#x} va=[{:#018x}..{:#018x}) guard=[{:#018x}..{:#018x})",
            pid,
            requested_size,
            mapped_size,
            heap_base,
            heap_base + mapped_size,
            guard_base,
            guard_base + USER_HEAP_GUARD_PAGES * PAGE_SIZE,
        );
        Ok((heap_base, mapped_size))
    }

    pub(super) fn handle_dma_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let size_bytes = request.arg0;
        if size_bytes == 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let mapped_size = align_up(size_bytes, PAGE_SIZE);
        let page_count = mapped_size / PAGE_SIZE;
        let (root_node, address_space, base_va, start_slot) = self.processes.reserve_process_heap(
            pid,
            page_count,
            PAGE_SIZE,
            PROCESS_FRAME_TOTAL_PAGES,
        )?;
        let base_page = self.memory.allocate_physical_any(mapped_size)?;
        let base_paddr = base_page * PAGE_SIZE;
        self.processes
            .register_physical_allocation(pid, base_va, start_slot, base_page, page_count)?;

        self.ensure_process_frame_chunks(pid, root_node, start_slot, page_count)?;
        let mut i = 0usize;
        while i < page_count {
            let frame_index = base_page + i;
            self.memory.copy_alpha_frame_to_process_node(
                frame_index,
                process_frame_chunk_descriptor(
                    root_node,
                    (start_slot + i) / PROCESS_FRAME_CHUNK_PAGES,
                ),
                PROCESS_FRAME_NODE_RADIX,
                (start_slot + i) % PROCESS_FRAME_CHUNK_PAGES,
            )?;
            i += 1;
        }

        let memory = &mut self.memory;
        let processes = &mut self.processes;
        let vm = processes
            .vm_space_mut(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        let mut j = 0usize;
        while j < page_count {
            let frame = process_frame_descriptor(root_node, start_slot + j);
            let va = base_va + j * PAGE_SIZE;
            memory.map_frame(address_space, frame, va, vm)?;
            j += 1;
        }

        info!(
            "[dma] granted pid={:>3} size={:#x} paddr={:#018x} vaddr={:#018x}",
            pid, mapped_size, base_paddr, base_va
        );
        Ok((base_paddr, base_va))
    }

    pub(super) fn handle_initial_framebuffer_information_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }

        match request.arg0 {
            FRAMEBUFFER_INFORMATION_REGION => Ok((
                self.initial_framebuffer.address,
                self.initial_framebuffer.size_bytes,
            )),
            FRAMEBUFFER_INFORMATION_GEOMETRY => Ok((
                self.initial_framebuffer.width,
                self.initial_framebuffer.height,
            )),
            FRAMEBUFFER_INFORMATION_FORMAT => Ok((
                self.initial_framebuffer.stride,
                self.initial_framebuffer.bits_per_pixel,
            )),
            FRAMEBUFFER_INFORMATION_COLOR_AND_ID => Ok((
                self.initial_framebuffer.display_id,
                pack_framebuffer_color_information(
                    self.initial_framebuffer.red_position,
                    self.initial_framebuffer.red_size,
                    self.initial_framebuffer.green_position,
                    self.initial_framebuffer.green_size,
                    self.initial_framebuffer.blue_position,
                    self.initial_framebuffer.blue_size,
                ),
            )),
            _ => Err(CapabilityError::InvalidArgument),
        }
    }

    pub(super) fn validate_process_memory_access(
        &self,
        caller_pid: usize,
        target_pid: usize,
    ) -> Result<(), CapabilityError> {
        let target = self
            .processes
            .find_entry_by_pid(target_pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if caller_pid != target_pid && target.reaper_pid != caller_pid {
            return Err(CapabilityError::PermissionDenied);
        }
        Ok(())
    }

    pub(super) fn handle_process_memory_clone_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let caller_pid = request.identifier;
        let source_pid = request.arg0;
        let destination_pid = request.arg1;
        let base_va = request.arg2;
        let size_bytes = request.arg3;
        if caller_pid == 0
            || source_pid == 0
            || destination_pid == 0
            || base_va == 0
            || size_bytes == 0
        {
            return Err(CapabilityError::InvalidArgument);
        }
        self.validate_process_memory_access(caller_pid, source_pid)?;
        self.validate_process_memory_access(caller_pid, destination_pid)?;
        self.clone_backed_memory(source_pid, base_va, destination_pid, base_va, size_bytes)
    }

    pub(super) fn handle_process_memory_copy_within_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let caller_pid = request.identifier;
        let target_pid = request.arg0;
        let source_va = request.arg1;
        let destination_va = request.arg2;
        let size_bytes = request.arg3;
        if caller_pid == 0
            || target_pid == 0
            || source_va == 0
            || destination_va == 0
            || size_bytes == 0
        {
            return Err(CapabilityError::InvalidArgument);
        }
        self.validate_process_memory_access(caller_pid, target_pid)?;
        if (source_va | destination_va | size_bytes) & (PAGE_SIZE - 1) == 0 {
            return self.clone_backed_memory(
                target_pid,
                source_va,
                target_pid,
                destination_va,
                size_bytes,
            );
        }
        self.copy_process_memory(
            target_pid,
            source_va,
            target_pid,
            destination_va,
            size_bytes,
        )
    }

    pub(super) fn handle_process_map_anonymous_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        let caller_pid = request.identifier;
        let target_pid = request.arg0;
        let size_bytes = request.arg1;
        let requested_base = request.arg2;
        let protected = request.code == nanami_uapi::os::OS_REQUEST_PROCESS_MAP_INITIALIZED_PROTECTED;
        // A zero base uses the same automatic heap reservation as anonymous
        // mmap. The descriptor is still mandatory for final-rights requests.
        if protected && request.arg3 == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        if caller_pid == 0 || target_pid == 0 || size_bytes == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let target = self
            .processes
            .find_entry_by_pid(target_pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if target.reaper_pid != caller_pid && target_pid != caller_pid {
            return Err(CapabilityError::PermissionDenied);
        }

        let supervised = target.capabilities_sealed && target.reaper_pid == caller_pid
            && target_pid != caller_pid && !target.exited && !target.resource_quarantined;

        let mapped_size = size_bytes
            .checked_add(PAGE_SIZE - 1)
            .ok_or(CapabilityError::InvalidArgument)?
            & !(PAGE_SIZE - 1);
        let page_count = mapped_size / PAGE_SIZE;
        let base = if request.arg3 != 0 {
            let descriptor = self.read_process_memory_into_static_buffer(
                caller_pid,
                request.arg3,
                (if protected { 3 } else { nanami_uapi::os::OS_MAP_INITIALIZER_WORDS }) * core::mem::size_of::<Word>(),
            )?;
            let source = unsafe { ptr::read_unaligned(descriptor.as_ptr() as *const [Word; 2]) };
            let permissions = if protected {
                unsafe { ptr::read_unaligned(descriptor.as_ptr().add(2 * core::mem::size_of::<Word>()) as *const Word) }
            } else {
                7
            };
            let source = Some(PageSource { pid: caller_pid, address: source[0], bytes: source[1] });
            if protected && permissions & nanami_uapi::os::OS_MAP_INITIALIZER_COW_ZERO != 0 {
                let rights = permissions & !nanami_uapi::os::OS_MAP_INITIALIZER_COW_ZERO;
                if !supervised || !matches!(rights, 3 | 7) || source.is_none_or(|s| s.address != 0 || s.bytes != 0)
                    || self.processes.find_entry_by_pid(caller_pid).is_none_or(|entry|
                        entry.capabilities_sealed || entry.exited || entry.resource_quarantined)
                { return Err(CapabilityError::PermissionDenied); }
                self.map_process_anonymous_zero_cow(target_pid, (requested_base != 0).then_some(requested_base), page_count, rights)?
            } else if protected && permissions & nanami_uapi::os::OS_MAP_INITIALIZER_DEFER != 0 {
                if !supervised || requested_base == 0
                    || permissions != (1 | nanami_uapi::os::OS_MAP_INITIALIZER_DEFER)
                    || self.processes.find_entry_by_pid(caller_pid).is_none_or(|entry|
                        entry.capabilities_sealed || entry.exited || entry.resource_quarantined)
                { return Err(CapabilityError::PermissionDenied); }
                let descriptor = self.read_process_memory_into_static_buffer(caller_pid,
                    request.arg3, 4 * core::mem::size_of::<Word>())?;
                let eager = unsafe { ptr::read_unaligned(descriptor.as_ptr()
                    .add(3 * core::mem::size_of::<Word>()) as *const Word) };
                self.map_process_initialized_pages_deferred(target_pid, requested_base,
                    page_count, source, eager)?
            } else { self.map_process_initialized_pages_protected(
                target_pid,
                (requested_base != 0).then_some(requested_base),
                page_count,
                source,
                permissions,
            )? }
        } else if requested_base == 0 {
            self.map_process_heap_pages(target_pid, page_count)
                .map_err(|e| {
                    info!(
                        "[proc.map.err] caller={:>3} target={:>3} bytes={:#x} pages={} err={:?}",
                        caller_pid, target_pid, size_bytes, page_count, e
                    );
                    e
                })?
        } else {
            self.map_process_heap_pages_at(target_pid, requested_base, page_count)
                .map_err(|e| {
                    info!(
                        "[proc.map.err] caller={:>3} target={:>3} fixed={:#018x} bytes={:#x} pages={} err={:?}",
                        caller_pid, target_pid, requested_base, size_bytes, page_count, e
                    );
                    e
                })?
        };
        info!(
            "[proc.map] granted caller={:>3} target={:>3} bytes={:#x} mapped={:#x} va=[{:#018x}..{:#018x})",
            caller_pid,
            target_pid,
            size_bytes,
            mapped_size,
            base,
            base + mapped_size
        );
        Ok((base, mapped_size))
    }

    pub(super) fn map_process_page_into_alpha(
        &mut self,
        pid: usize,
        page_va: usize,
        temp_va: usize,
    ) -> Result<(CapabilityDescriptor, usize), CapabilityError> {
        if page_va & (PAGE_SIZE - 1) != 0 || temp_va & (PAGE_SIZE - 1) != 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let frame = self.process_frame_for_page(pid, page_va)?;
        self.map_alpha_temporary_frame(frame, temp_va)?;
        Ok((frame, temp_va))
    }

    pub(super) fn process_frame_for_page(
        &mut self,
        pid: usize,
        page_va: usize,
    ) -> Result<CapabilityDescriptor, CapabilityError> {
        self.process_frame_for_access(pid, page_va, false)
    }

    pub(super) fn process_frame_for_write(
        &mut self,
        pid: usize,
        page_va: usize,
    ) -> Result<CapabilityDescriptor, CapabilityError> {
        self.process_frame_for_access(pid, page_va, true)
    }

    fn process_frame_for_access(
        &mut self,
        pid: usize,
        page_va: usize,
        writing: bool,
    ) -> Result<CapabilityDescriptor, CapabilityError> {
        if page_va & (PAGE_SIZE - 1) != 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        if writing && self.processes.immutable_source_overlapping_range(pid, page_va, PAGE_SIZE) {
            return Err(CapabilityError::PermissionDenied);
        }
        let frame = match self
            .processes
            .vm_space_mut(pid)
            .map(|vm| {
                if writing && vm.private_page_shared(page_va) {
                    return Err(CapabilityError::IllegalOperation);
                }
                Ok(vm.find_frame(page_va))
            })
            .transpose()?
            .flatten()
        {
            Some(frame) => frame,
            None => {
                let entry = self
                    .processes
                    .memory_context(pid)
                    .ok_or(CapabilityError::InvalidArgument)?;
                self.materialize_lazy_page(pid, entry.root_node, entry.address_space, page_va)?;
                self.processes
                    .vm_space_mut(pid)
                    .and_then(|vm| vm.find_frame(page_va))
                    .ok_or(CapabilityError::InvalidArgument)?
            }
        };
        Ok(frame)
    }

    pub(super) fn map_alpha_temporary_frame(
        &mut self,
        frame: CapabilityDescriptor,
        temp_va: usize,
    ) -> Result<(), CapabilityError> {
        let alpha_as = self.processes.alpha_entry().address_space;
        if let Some(old_frame) = self.processes.alpha_vm_space_mut().find_frame(temp_va) {
            match arch::address_space::unmap(alpha_as, old_frame, temp_va) {
                Ok(()) => {
                    #[cfg(not(test))]
                    super::memory_init::record_mapping(false, false);
                    self.processes.alpha_vm_space_mut().forget_frame(temp_va);
                }
                Err(CapabilityError::IllegalOperation) => {
                    self.processes.alpha_vm_space_mut().forget_frame(temp_va);
                }
                Err(error) => return Err(error),
            }
        }
        let vm = self.processes.alpha_vm_space_mut();
        self.memory.map_frame_strict(alpha_as, frame, temp_va, vm)?;
        #[cfg(not(test))]
        super::memory_init::record_mapping(false, true);
        Ok(())
    }

    pub(super) fn unmap_alpha_temporary_frame(
        &mut self,
        frame: CapabilityDescriptor,
        temp_va: usize,
    ) -> Result<(), CapabilityError> {
        match self.unmap_alpha_frame(frame, temp_va) {
            Err(CapabilityError::IllegalOperation) => {
                self.processes.alpha_vm_space_mut().forget_frame(temp_va);
                Ok(())
            }
            result => result,
        }
    }

    pub(super) fn unmap_alpha_frame(
        &mut self,
        frame: CapabilityDescriptor,
        virtual_address: usize,
    ) -> Result<(), CapabilityError> {
        let alpha_as = self.processes.alpha_entry().address_space;
        arch::address_space::unmap(alpha_as, frame, virtual_address)?;
        #[cfg(not(test))]
        super::memory_init::record_mapping(false, false);
        self.processes
            .alpha_vm_space_mut()
            .forget_frame(virtual_address);
        Ok(())
    }
}
