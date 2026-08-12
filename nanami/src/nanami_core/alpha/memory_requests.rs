use super::*;

impl Alpha {
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

    pub(super) fn map_process_heap_pages(
        &mut self,
        pid: usize,
        page_count: usize,
    ) -> Result<usize, CapabilityError> {
        if pid == 0 || page_count == 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let (root_node, address_space, heap_base, start_slot) = self
            .processes
            .reserve_process_heap(pid, page_count, PAGE_SIZE, PROCESS_FRAME_TOTAL_PAGES)?;
        let allocated_frames =
            self.allocate_process_frames(pid, root_node, start_slot, page_count)?;
        for (slot, page) in allocated_frames {
            let va = heap_base + (slot - start_slot) * PAGE_SIZE;
            self.processes
                .register_physical_allocation(pid, va, slot, page, 1)?;
        }
        self.zero_process_frames(root_node, start_slot, page_count)?;

        let memory = &mut self.memory;
        let processes = &mut self.processes;
        let mut i = 0usize;
        while i < page_count {
            let frame = process_frame_descriptor(root_node, start_slot + i);
            let va = heap_base + i * PAGE_SIZE;
            let vm = processes
                .vm_space_mut(pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            memory.map_frame(address_space, frame, va, vm)?;
            i += 1;
        }

        Ok(heap_base)
    }

    pub(super) fn map_process_heap_pages_at(
        &mut self,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) -> Result<usize, CapabilityError> {
        if pid == 0 || base_va == 0 || page_count == 0 || (base_va & (PAGE_SIZE - 1)) != 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let (root_node, address_space, start_slot) = self.processes.reserve_process_heap_at(
            pid,
            base_va,
            page_count,
            PAGE_SIZE,
            PROCESS_FRAME_TOTAL_PAGES,
        )?;
        let reuse_allocation = self
            .processes
            .take_deferred_physical_allocation(pid, base_va, page_count);
        let allocated_frames = if let Some(allocation) = reuse_allocation {
            self.ensure_process_frame_chunks(pid, root_node, start_slot, page_count)?;
            let mut frames = Vec::new();
            let mut i = 0usize;
            while i < page_count {
                let frame_index = allocation.base_page + i;
                self.memory
                    .ensure_alpha_frame_at_physical_index(frame_index)?;
                let source_frame = self
                    .memory
                    .physical_frame_descriptor_from_index(frame_index)
                    .ok_or(CapabilityError::InvalidArgument)?;
                arch::node::copy(
                    process_frame_chunk_descriptor(
                        root_node,
                        (start_slot + i) / PROCESS_FRAME_CHUNK_PAGES,
                    ),
                    ((start_slot + i) % PROCESS_FRAME_CHUNK_PAGES) as Word,
                    source_frame,
                )?;
                frames.push((start_slot + i, frame_index));
                i += 1;
            }
            frames
        } else {
            self.allocate_process_frames(pid, root_node, start_slot, page_count)?
        };
        for (slot, page) in allocated_frames {
            let va = base_va + (slot - start_slot) * PAGE_SIZE;
            self.processes
                .register_physical_allocation(pid, va, slot, page, 1)?;
        }
        self.zero_process_frames(root_node, start_slot, page_count)?;

        let memory = &mut self.memory;
        let processes = &mut self.processes;
        let mut i = 0usize;
        while i < page_count {
            let frame = process_frame_descriptor(root_node, start_slot + i);
            let va = base_va + i * PAGE_SIZE;
            let vm = processes
                .vm_space_mut(pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            memory.map_frame_strict(address_space, frame, va, vm)?;
            i += 1;
        }

        Ok(base_va)
    }

    pub(super) fn zero_process_frames(
        &mut self,
        process_root: CapabilityDescriptor,
        start_slot: usize,
        page_count: usize,
    ) -> Result<(), CapabilityError> {
        let mut i = 0usize;
        while i < page_count {
            let frame = process_frame_descriptor(process_root, start_slot + i);
            let temp_va = PROCESS_ZERO_TEMP_BASE + i * PAGE_SIZE;
            self.map_alpha_temporary_frame(frame, temp_va)?;
            unsafe {
                ptr::write_bytes(temp_va as *mut u8, 0, PAGE_SIZE);
            }
            if let Err(e) = self.unmap_alpha_temporary_frame(frame, temp_va) {
                error!(
                    "[proc.err] unmap zero temp frame={:#018x} temp={:#018x} err={:?}",
                    frame, temp_va, e
                );
                return Err(e);
            }
            i += 1;
        }
        Ok(())
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
        let mut j = 0usize;
        while j < page_count {
            let frame = process_frame_descriptor(root_node, start_slot + j);
            let va = base_va + j * PAGE_SIZE;
            let vm = processes
                .vm_space_mut(pid)
                .ok_or(CapabilityError::InvalidArgument)?;
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

    pub(super) fn handle_shared_memory_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }

        let peer_pid = request.arg0;
        let size_bytes = request.arg1;
        if peer_pid == 0 || peer_pid == pid || size_bytes == 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let mapped_size = align_up(size_bytes, PAGE_SIZE);
        let page_count = mapped_size / PAGE_SIZE;
        if page_count == 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        if self.processes.find_entry_by_pid(pid).is_none()
            || self.processes.find_entry_by_pid(peer_pid).is_none()
        {
            return Err(CapabilityError::InvalidArgument);
        }

        let (caller_root, caller_as, caller_va, caller_start_slot) = self
            .processes
            .reserve_process_heap(pid, page_count, PAGE_SIZE, PROCESS_FRAME_TOTAL_PAGES)?;
        let (peer_root, peer_as, peer_va, peer_start_slot) = self.processes.reserve_process_heap(
            peer_pid,
            page_count,
            PAGE_SIZE,
            PROCESS_FRAME_TOTAL_PAGES,
        )?;

        let base_page = self.memory.allocate_physical_any(mapped_size)?;
        let base_paddr = base_page * PAGE_SIZE;
        self.processes.register_physical_allocation(
            pid,
            caller_va,
            caller_start_slot,
            base_page,
            page_count,
        )?;
        self.processes.register_physical_allocation(
            peer_pid,
            peer_va,
            peer_start_slot,
            base_page,
            page_count,
        )?;

        self.ensure_process_frame_chunks(pid, caller_root, caller_start_slot, page_count)?;
        self.ensure_process_frame_chunks(peer_pid, peer_root, peer_start_slot, page_count)?;

        let mut i = 0usize;
        while i < page_count {
            let frame_index = base_page + i;
            // Convert Generic->Frame only once per physical frame, then fan-out copy to both processes.
            // Calling ensure twice for the same frame causes kernel-side "out of memory" noise
            // because each 4KiB generic is single-shot allocatable.
            self.memory
                .ensure_alpha_frame_at_physical_index(frame_index)?;
            let source_frame = self
                .memory
                .physical_frame_descriptor_from_index(frame_index)
                .ok_or(CapabilityError::InvalidArgument)?;
            arch::node::copy(
                process_frame_chunk_descriptor(
                    caller_root,
                    (caller_start_slot + i) / PROCESS_FRAME_CHUNK_PAGES,
                ),
                ((caller_start_slot + i) % PROCESS_FRAME_CHUNK_PAGES) as Word,
                source_frame,
            )?;
            arch::node::copy(
                process_frame_chunk_descriptor(
                    peer_root,
                    (peer_start_slot + i) / PROCESS_FRAME_CHUNK_PAGES,
                ),
                ((peer_start_slot + i) % PROCESS_FRAME_CHUNK_PAGES) as Word,
                source_frame,
            )?;
            i += 1;
        }

        let memory = &mut self.memory;
        let processes = &mut self.processes;
        let mut j = 0usize;
        while j < page_count {
            let caller_frame = process_frame_descriptor(caller_root, caller_start_slot + j);
            let caller_page_va = caller_va + j * PAGE_SIZE;
            let caller_vm = processes
                .vm_space_mut(pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            memory.map_frame(caller_as, caller_frame, caller_page_va, caller_vm)?;

            let peer_frame = process_frame_descriptor(peer_root, peer_start_slot + j);
            let peer_page_va = peer_va + j * PAGE_SIZE;
            let peer_vm = processes
                .vm_space_mut(peer_pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            memory.map_frame(peer_as, peer_frame, peer_page_va, peer_vm)?;
            j += 1;
        }

        info!(
            "[shm] granted pid={:>3}<->pid={:>3} size={:#x} paddr={:#018x} local={:#018x} peer={:#018x}",
            pid, peer_pid, mapped_size, base_paddr, caller_va, peer_va
        );
        Ok((caller_va, peer_va))
    }

    pub(super) fn handle_mapping_release_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let caller_pid = request.identifier;
        if caller_pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let target_pid = if request.arg2 == 0 {
            caller_pid
        } else {
            request.arg2
        };
        let base_va = request.arg0;
        let size_bytes = request.arg1;
        if size_bytes == 0 || (base_va & (PAGE_SIZE - 1)) != 0 {
            info!(
                "[map.err] release invalid args caller={:>3} target={:>3} va={:#018x} bytes={:#x}",
                caller_pid, target_pid, base_va, size_bytes
            );
            return Err(CapabilityError::InvalidArgument);
        }

        let mapped_size = align_up(size_bytes, PAGE_SIZE);
        let page_count = mapped_size / PAGE_SIZE;
        let entry = self
            .processes
            .find_entry_by_pid(target_pid)
            .ok_or_else(|| {
                info!("[map.err] release unknown target={:>3}", target_pid);
                CapabilityError::InvalidArgument
            })?;
        if caller_pid != target_pid && entry.reaper_pid != caller_pid {
            info!(
                "[map.err] release denied caller={:>3} target={:>3} reaper={:>3}",
                caller_pid, target_pid, entry.reaper_pid
            );
            return Err(CapabilityError::PermissionDenied);
        }
        let exact_allocation = self
            .processes
            .find_active_physical_allocation_reference(target_pid, base_va, page_count);

        let mut i = 0usize;
        while i < page_count {
            let va = base_va + i * PAGE_SIZE;
            let allocation = if let Some(allocation) = exact_allocation {
                allocation
            } else {
                self.processes
                    .find_active_physical_allocation_reference(target_pid, va, 1)
                    .ok_or_else(|| {
                        info!(
                            "[map.err] release lookup failed caller={:>3} target={:>3} va={:#018x} bytes={:#x} page={}/{}",
                            caller_pid, target_pid, va, mapped_size, i, page_count
                        );
                        CapabilityError::InvalidArgument
                    })?
            };
            let slot = allocation.start_slot + if allocation.page_count == 1 { 0 } else { i };
            let frame = process_frame_descriptor(entry.root_node, slot);
            if let Err(e) = arch::address_space::unmap(entry.address_space, frame, va) {
                info!(
                    "[map.err] unmap target={:>3} va={:#018x} frame={:#018x} slot={} err={:?}",
                    target_pid, va, frame, slot, e
                );
                return Err(e);
            }
            if let Some(vm) = self.processes.vm_space_mut(target_pid) {
                vm.forget_frame(va);
            }
            i += 1;
        }

        if exact_allocation.is_some() {
            let (allocation, _) = self
                .processes
                .release_physical_allocation_reference(target_pid, base_va, page_count)?;
            self.processes
                .defer_physical_allocation_for_pid(target_pid, allocation);
        } else {
            let mut page = 0usize;
            while page < page_count {
                let va = base_va + page * PAGE_SIZE;
                let (allocation, _) = self
                    .processes
                    .release_physical_allocation_reference(target_pid, va, 1)?;
                self.processes
                    .defer_physical_allocation_for_pid(target_pid, allocation);
                page += 1;
            }
        }
        info!(
            "[map] released caller={:>3} target={:>3} va=[{:#018x}..{:#018x}) pages={}",
            caller_pid,
            target_pid,
            base_va,
            base_va + mapped_size,
            page_count
        );
        Ok(())
    }

    pub(super) fn handle_process_memory_copy_request(
        &mut self,
        request: OsRequestEvent,
        write_to_target: bool,
    ) -> Result<(), CapabilityError> {
        let caller_pid = request.identifier;
        let target_pid = request.arg0;
        let target_va = request.arg1;
        let caller_va = request.arg2;
        let size_bytes = request.arg3;
        if caller_pid == 0 || target_pid == 0 || target_va == 0 || caller_va == 0 || size_bytes == 0
        {
            return Err(CapabilityError::InvalidArgument);
        }

        let target = self
            .processes
            .find_entry_by_pid(target_pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if target.reaper_pid != caller_pid && target_pid != caller_pid {
            return Err(CapabilityError::PermissionDenied);
        }
        if self.processes.find_entry_by_pid(caller_pid).is_none() {
            return Err(CapabilityError::InvalidArgument);
        }

        let (source_pid, source_va, destination_pid, destination_va) = if write_to_target {
            (caller_pid, caller_va, target_pid, target_va)
        } else {
            (target_pid, target_va, caller_pid, caller_va)
        };
        self.copy_process_memory(
            source_pid,
            source_va,
            destination_pid,
            destination_va,
            size_bytes,
        )
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
        self.copy_process_memory(source_pid, base_va, destination_pid, base_va, size_bytes)
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

        let mapped_size = align_up(size_bytes, PAGE_SIZE);
        let page_count = mapped_size / PAGE_SIZE;
        let base = if requested_base == 0 {
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

    pub(super) fn copy_process_memory(
        &mut self,
        source_pid: usize,
        source_va: usize,
        destination_pid: usize,
        destination_va: usize,
        size_bytes: usize,
    ) -> Result<(), CapabilityError> {
        if source_pid == destination_pid && source_va == destination_va {
            return Ok(());
        }
        let source_end = source_va
            .checked_add(size_bytes)
            .ok_or(CapabilityError::InvalidArgument)?;
        let _ = destination_va
            .checked_add(size_bytes)
            .ok_or(CapabilityError::InvalidArgument)?;
        let copy_backward = source_pid == destination_pid
            && destination_va > source_va
            && destination_va < source_end;
        let src_temp_va = PROCESS_COPY_TEMP_BASE;
        let dst_temp_va = PROCESS_COPY_TEMP_BASE + PAGE_SIZE;
        let mut copied = 0usize;
        while copied < size_bytes {
            let (src, dst, chunk) = if copy_backward {
                let remaining = size_bytes - copied;
                let src_end = source_va + remaining;
                let dst_end = destination_va + remaining;
                let src_page_tail = ((src_end - 1) & (PAGE_SIZE - 1)) + 1;
                let dst_page_tail = ((dst_end - 1) & (PAGE_SIZE - 1)) + 1;
                let chunk = min_usize(remaining, min_usize(src_page_tail, dst_page_tail));
                (src_end - chunk, dst_end - chunk, chunk)
            } else {
                let src = source_va + copied;
                let dst = destination_va + copied;
                let src_offset = src & (PAGE_SIZE - 1);
                let dst_offset = dst & (PAGE_SIZE - 1);
                let chunk = min_usize(
                    size_bytes - copied,
                    min_usize(PAGE_SIZE - src_offset, PAGE_SIZE - dst_offset),
                );
                (src, dst, chunk)
            };
            let src_page = align_down(src, PAGE_SIZE);
            let dst_page = align_down(dst, PAGE_SIZE);
            let src_offset = src - src_page;
            let dst_offset = dst - dst_page;
            let src_frame = self.process_frame_for_page(source_pid, src_page)?;
            let dst_frame = self.process_frame_for_page(destination_pid, dst_page)?;
            self.map_alpha_temporary_frame(src_frame, src_temp_va)?;

            if src_frame == dst_frame {
                let bounce = core::ptr::addr_of_mut!(PROCESS_COPY_BOUNCE_BUFFER) as *mut u8;
                unsafe {
                    ptr::copy_nonoverlapping(
                        (src_temp_va + src_offset) as *const u8,
                        bounce,
                        chunk,
                    );
                    ptr::copy_nonoverlapping(bounce, (src_temp_va + dst_offset) as *mut u8, chunk);
                }
                self.unmap_alpha_temporary_frame(src_frame, src_temp_va)?;
            } else {
                if let Err(error) = self.map_alpha_temporary_frame(dst_frame, dst_temp_va) {
                    let _ = self.unmap_alpha_temporary_frame(src_frame, src_temp_va);
                    return Err(error);
                }
                unsafe {
                    ptr::copy_nonoverlapping(
                        (src_temp_va + src_offset) as *const u8,
                        (dst_temp_va + dst_offset) as *mut u8,
                        chunk,
                    );
                }
                let src_unmap = self.unmap_alpha_temporary_frame(src_frame, src_temp_va);
                let dst_unmap = self.unmap_alpha_temporary_frame(dst_frame, dst_temp_va);
                src_unmap?;
                dst_unmap?;
            }
            copied += chunk;
        }

        Ok(())
    }

    pub(super) fn read_process_memory_into_static_buffer(
        &mut self,
        source_pid: usize,
        source_va: usize,
        size_bytes: usize,
    ) -> Result<&'static [u8], CapabilityError> {
        if size_bytes > PROCESS_SPAWN_MEMORY_MAX_BYTES {
            return Err(CapabilityError::InvalidArgument);
        }
        let out = core::ptr::addr_of_mut!(PROCESS_MEMORY_IMAGE_BUFFER) as *mut u8;
        let mut copied = 0usize;
        while copied < size_bytes {
            let src = source_va
                .checked_add(copied)
                .ok_or(CapabilityError::InvalidArgument)?;
            let src_page = align_down(src, PAGE_SIZE);
            let src_offset = src - src_page;
            let chunk = min_usize(size_bytes - copied, PAGE_SIZE - src_offset);
            let temp_offset = (copied / PAGE_SIZE)
                .checked_mul(PAGE_SIZE)
                .ok_or(CapabilityError::InvalidArgument)?;
            if temp_offset + PAGE_SIZE > PROCESS_COPY_TEMP_WINDOW_SIZE {
                return Err(CapabilityError::InvalidArgument);
            }
            let temp_va = PROCESS_COPY_TEMP_BASE + temp_offset;
            let (src_frame, src_temp) =
                self.map_process_page_into_alpha(source_pid, src_page, temp_va)?;

            unsafe {
                ptr::copy_nonoverlapping(
                    (src_temp + src_offset) as *const u8,
                    out.add(copied),
                    chunk,
                );
            }

            if let Err(e) = self.unmap_alpha_temporary_frame(src_frame, src_temp) {
                info!(
                    "[proc.copy.err] unmap spawn-memory temp={:#018x} frame={:#018x} err={:?}",
                    src_temp, src_frame, e
                );
                return Err(e);
            }
            copied += chunk;
        }
        Ok(unsafe { core::slice::from_raw_parts(out as *const u8, size_bytes) })
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
        if page_va & (PAGE_SIZE - 1) != 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let entry = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;

        let frame = match self
            .processes
            .vm_space_mut(pid)
            .and_then(|vm| vm.find_frame(page_va))
        {
            Some(frame) => frame,
            None => {
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
                Ok(()) | Err(CapabilityError::IllegalOperation) => {
                    self.processes.alpha_vm_space_mut().forget_frame(temp_va);
                }
                Err(error) => return Err(error),
            }
        }
        let vm = self.processes.alpha_vm_space_mut();
        self.memory.map_frame_strict(alpha_as, frame, temp_va, vm)
    }

    pub(super) fn unmap_alpha_temporary_frame(
        &mut self,
        frame: CapabilityDescriptor,
        temp_va: usize,
    ) -> Result<(), CapabilityError> {
        let alpha_as = self.processes.alpha_entry().address_space;
        match arch::address_space::unmap(alpha_as, frame, temp_va) {
            Ok(()) | Err(CapabilityError::IllegalOperation) => {
                self.processes.alpha_vm_space_mut().forget_frame(temp_va);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}
