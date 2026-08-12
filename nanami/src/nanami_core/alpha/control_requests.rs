use super::*;

impl Alpha {
    pub(super) fn handle_nanami_control_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let property = decode_control_text(request.arg0, request.arg1);
        let value = decode_control_text(request.arg2, request.arg3);
        if bytes_equal(&property, b"os.log") {
            if bytes_equal(&value, b"enable") {
                crate::nanami_utils::log::set_info_enabled(true);
                info!("[control] os.log enabled");
                return Ok(());
            }
            if bytes_equal(&value, b"disable") {
                info!("[control] os.log disabled");
                crate::nanami_utils::log::set_info_enabled(false);
                return Ok(());
            }
        }
        Err(CapabilityError::InvalidArgument)
    }

    pub(super) fn handle_nanami_info_request(
        &self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        if request.identifier == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        match request.arg0 {
            NANAMI_INFO_MEMORY => {
                let info = self.memory.physical_memory_info()?;
                Ok((
                    info.total_pages.saturating_mul(PAGE_SIZE),
                    info.free_pages.saturating_mul(PAGE_SIZE),
                ))
            }
            NANAMI_INFO_PROCESS => {
                let info = self.processes.statistics();
                Ok((info.running, info.exited))
            }
            _ => Err(CapabilityError::InvalidArgument),
        }
    }

    pub(super) fn handle_irq_control_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }

        let irq_number = request.arg0;
        let notification_slot = request.arg1;
        let interrupt_slot = request.arg2;

        validate_process_device_slot(notification_slot)?;
        validate_process_device_slot(interrupt_slot)?;

        let process_entry = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        let requested_notification_descriptor = make_child_slot_descriptor(
            process_entry.root_node,
            PROCESS_ROOT_RADIX,
            notification_slot,
        );
        let default_notification_descriptor = make_child_slot_descriptor(
            process_entry.root_node,
            PROCESS_ROOT_RADIX,
            PROCESS_SLOT_NOTIFICATION_PORT,
        );

        self.processes.assign_irq_to_pid(pid, irq_number)?;

        if notification_slot != PROCESS_SLOT_NOTIFICATION_PORT as Word {
            match arch::node::copy(
                process_entry.root_node,
                notification_slot,
                default_notification_descriptor,
            ) {
                Ok(()) => {}
                Err(_) => {
                    // Reusing the same notification slot for multiple IRQ registrations
                    // is valid. The user-visible slot must keep a stable notification object;
                    // IRQ-specific identifiers are assigned only to the per-IRQ alias below.
                }
            }
        }

        match arch::interrupt_region::make_port(
            self.interrupt_region,
            irq_number,
            process_entry.root_node,
            interrupt_slot,
        ) {
            Ok(()) => {}
            Err(e) => {
                let _ = self.processes.unassign_irq_from_pid(pid, irq_number);
                return Err(e);
            }
        }

        let interrupt_descriptor =
            make_child_slot_descriptor(process_entry.root_node, PROCESS_ROOT_RADIX, interrupt_slot);

        let irq_identifier = irq_notification_identifier(irq_number)?;
        let alias_slot =
            select_irq_notification_alias_slot(irq_number, notification_slot, interrupt_slot);
        let alias_descriptor =
            make_child_slot_descriptor(process_entry.root_node, PROCESS_ROOT_RADIX, alias_slot);

        if alias_slot == notification_slot || alias_slot == interrupt_slot {
            let _ = self.processes.unassign_irq_from_pid(pid, irq_number);
            return Err(CapabilityError::InvalidArgument);
        }

        // Notification identifiers are slot-local. Binding multiple IRQs through the same
        // process-visible slot would overwrite that slot's identifier on every registration.
        // Bind each interrupt through an alias slot that points at the same notification object,
        // while userland continues to wait on `notification_slot`.
        arch::node::copy(
            process_entry.root_node,
            alias_slot,
            requested_notification_descriptor,
        )?;
        let _ = arch::notification_port::identify(alias_descriptor, irq_identifier);
        if let Err(e) = arch::interrupt_port::bind(interrupt_descriptor, alias_descriptor) {
            let _ = self.processes.unassign_irq_from_pid(pid, irq_number);
            return Err(e);
        }

        info!(
            "[irq] granted pid={:>3} irq={:>3} notification_slot={:>3} alias_slot={:>3} interrupt_slot={:>3}",
            pid, irq_number, notification_slot, alias_slot, interrupt_slot
        );

        Ok(())
    }

    pub(super) fn handle_notification_port_create_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let notification_slot = request.arg0;
        let identifier = request.arg1;
        validate_process_device_slot(notification_slot)?;

        let process_entry = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;

        arch::generic::convert(
            self.process_arena_for_root(process_entry.root_node)?,
            CapabilityType::NotificationPort,
            0,
            1,
            process_entry.root_node,
            notification_slot,
        )?;

        let notification_descriptor = make_child_slot_descriptor(
            process_entry.root_node,
            PROCESS_ROOT_RADIX,
            notification_slot,
        );
        let _ = arch::notification_port::identify(notification_descriptor, identifier);

        Ok(())
    }

    pub(super) fn handle_notification_port_copy_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let caller_pid = request.identifier;
        if caller_pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }

        let source_pid = request.arg0;
        let source_notification_slot = request.arg1;
        let destination_slot = request.arg2;
        let identifier = request.arg3;
        validate_process_device_slot(source_notification_slot)?;
        validate_process_device_slot(destination_slot)?;

        let caller_entry = self
            .processes
            .find_entry_by_pid(caller_pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        let source_entry = self
            .processes
            .find_entry_by_pid(source_pid)
            .ok_or(CapabilityError::InvalidArgument)?;

        let source_notification_descriptor = make_child_slot_descriptor(
            source_entry.root_node,
            PROCESS_ROOT_RADIX,
            source_notification_slot,
        );

        let _ = arch::node::remove(caller_entry.root_node, destination_slot as Word);
        arch::node::copy(
            caller_entry.root_node,
            destination_slot,
            source_notification_descriptor,
        )?;

        let destination_descriptor = make_child_slot_descriptor(
            caller_entry.root_node,
            PROCESS_ROOT_RADIX,
            destination_slot,
        );
        let _ = arch::notification_port::identify(destination_descriptor, identifier);

        Ok(())
    }

    pub(super) fn handle_service_connect_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }

        let process_entry = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;

        let destination_slot = request.arg0;
        validate_process_device_slot(destination_slot)?;
        let (raw_name, raw_len) = decode_service_name_24(request.arg1, request.arg2, request.arg3)
            .ok_or(CapabilityError::InvalidArgument)?;
        let service_name = core::str::from_utf8(&raw_name[..raw_len])
            .map_err(|_| CapabilityError::InvalidArgument)?;

        let (service_port, service_pid) = self
            .communication
            .resolve_service_with_owner(service_name)
            .ok_or(CapabilityError::InvalidArgument)?;

        arch::node::copy(process_entry.root_node, destination_slot, service_port)?;
        let destination_descriptor = make_child_slot_descriptor(
            process_entry.root_node,
            PROCESS_ROOT_RADIX,
            destination_slot,
        );
        arch::ipc_port::identify(destination_descriptor, pid as Word)?;

        info!(
            "[svc] connect name={} pid={:>3} dst_slot={:>3} src_port={:#018x}",
            service_name, pid, destination_slot, service_port
        );

        Ok(service_pid)
    }

    pub(super) fn handle_shared_framebuffer_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }

        let peer_pid = request.arg0;
        let physical_address = request.arg1;
        let size_bytes = request.arg2;
        if peer_pid == 0 || peer_pid == pid || physical_address == 0 || size_bytes == 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let framebuffer_start = self.initial_framebuffer.address;
        let framebuffer_end = framebuffer_start.saturating_add(self.initial_framebuffer.size_bytes);
        let request_end = physical_address.saturating_add(size_bytes);
        if physical_address < framebuffer_start || request_end > framebuffer_end {
            return Err(CapabilityError::PermissionDenied);
        }

        if self.processes.find_entry_by_pid(pid).is_none()
            || self.processes.find_entry_by_pid(peer_pid).is_none()
        {
            return Err(CapabilityError::InvalidArgument);
        }

        let page_base = physical_address & !(PAGE_SIZE - 1);
        let offset = physical_address - page_base;
        let mapped_size = align_up(offset.saturating_add(size_bytes), PAGE_SIZE);
        let page_count = mapped_size / PAGE_SIZE;
        if page_count == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        info!(
            "[fb-shm] request pid={:>3}->pid={:>3} paddr={:#018x} size={:#x} mapped={:#x} pages={}",
            pid, peer_pid, physical_address, size_bytes, mapped_size, page_count
        );

        // The caller is fb-server, which already mapped the physical framebuffer via MMIO.
        // Only the peer compositor needs a new mapping here; mapping the caller again
        // consumes thousands of process frame slots at 1920x1080.
        let (peer_root, peer_as, peer_base_va, peer_start_slot) = match self
            .processes
            .reserve_process_heap(peer_pid, page_count, PAGE_SIZE, PROCESS_FRAME_TOTAL_PAGES)
        {
            Ok(v) => v,
            Err(e) => {
                match self.processes.find_entry_by_pid(peer_pid) {
                    Some(entry) => {
                        info!(
                            "[fb-shm.err] reserve peer pid={:>3} pages={} next_slot={} max_slots={} heap_next={:#018x} heap_limit={:#018x} err={:?}",
                            peer_pid,
                            page_count,
                            entry.next_frame_slot,
                            PROCESS_FRAME_TOTAL_PAGES,
                            entry.user_heap_next_va,
                            entry.user_heap_limit_va,
                            e
                        );
                    }
                    None => {
                        info!(
                            "[fb-shm.err] reserve peer pid={:>3} missing err={:?}",
                            peer_pid, e
                        );
                    }
                }
                return Err(e);
            }
        };

        let (converted_base_index, skip_pages, converted_page_count) = match self
            .memory
            .ensure_alpha_frames_for_range_from_initial_generic(page_base, mapped_size, true)
        {
            Ok(v) => v,
            Err(e) => {
                info!(
                    "[fb-shm.err] ensure frames paddr={:#018x} mapped={:#x} pages={} err={:?}",
                    page_base, mapped_size, page_count, e
                );
                return Err(e);
            }
        };
        if converted_page_count != page_count {
            info!(
                "[fb-shm.err] converted count mismatch expected={} actual={} base_index={} skip={}",
                page_count, converted_page_count, converted_base_index, skip_pages
            );
            return Err(CapabilityError::InvalidArgument);
        }

        self.ensure_process_frame_chunks(peer_pid, peer_root, peer_start_slot, page_count)?;

        let mut i = 0usize;
        while i < page_count {
            let source_frame = self
                .memory
                .physical_frame_descriptor_from_index(converted_base_index + skip_pages + i)
                .ok_or(CapabilityError::InvalidArgument)?;
            let dst_node = process_frame_chunk_descriptor(
                peer_root,
                (peer_start_slot + i) / PROCESS_FRAME_CHUNK_PAGES,
            );
            let dst_slot = (peer_start_slot + i) % PROCESS_FRAME_CHUNK_PAGES;
            if let Err(e) = arch::node::copy(dst_node, dst_slot as Word, source_frame) {
                info!(
                    "[fb-shm.err] copy frame i={} dst_slot={} src={:#018x} peer_node={:#018x} err={:?}",
                    i,
                    dst_slot,
                    source_frame,
                    dst_node,
                    e
                );
                return Err(e);
            }
            i += 1;
        }

        let memory = &mut self.memory;
        let processes = &mut self.processes;
        let mut j = 0usize;
        while j < page_count {
            let peer_frame = process_frame_descriptor(peer_root, peer_start_slot + j);
            let peer_page_va = peer_base_va + j * PAGE_SIZE;
            let peer_vm = processes
                .vm_space_mut(peer_pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            if let Err(e) = memory.map_frame(peer_as, peer_frame, peer_page_va, peer_vm) {
                info!(
                    "[fb-shm.err] map frame j={} va={:#018x} frame={:#018x} as={:#018x} err={:?}",
                    j, peer_page_va, peer_frame, peer_as, e
                );
                return Err(e);
            }
            j += 1;
        }

        let peer_va = peer_base_va.saturating_add(offset);
        info!(
            "[fb-shm] granted pid={:>3}->pid={:>3} size={:#x} paddr={:#018x} peer={:#018x}",
            pid, peer_pid, mapped_size, page_base, peer_va
        );
        Ok((0, peer_va))
    }

    pub(super) fn handle_mmio_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let physical_address = request.arg0;
        let size_bytes = request.arg1;
        if size_bytes == 0 || (physical_address & (PAGE_SIZE - 1)) != 0 {
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
        let base_page = self
            .memory
            .allocate_physical_at(physical_address, mapped_size, true)?;
        let base_paddr = base_page * PAGE_SIZE;
        if base_paddr != physical_address {
            return Err(CapabilityError::InvalidArgument);
        }
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

        self.ensure_process_frame_chunks(pid, root_node, start_slot, page_count)?;
        let mut i = 0usize;
        while i < page_count {
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
            "[mmio] granted pid={:>3} size={:#x} paddr={:#018x} vaddr={:#018x}",
            pid, mapped_size, physical_address, base_va
        );
        Ok((physical_address, base_va))
    }

    pub(super) fn handle_io_port_control_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }

        let range_min = request.arg0;
        let range_max = request.arg1;
        let io_port_slot = request.arg2;

        if range_min > range_max {
            return Err(CapabilityError::InvalidArgument);
        }

        validate_process_device_slot(io_port_slot)?;

        let process_entry = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;

        io_port_mint(
            self.root_io_port,
            range_min,
            range_max,
            process_entry.root_node,
            io_port_slot,
        )?;

        self.processes
            .add_io_range_to_pid(pid, range_min, range_max)?;

        info!(
            "[io] granted pid={:>3} range=[{:#018x}..={:#018x}] slot={:>3}",
            pid, range_min, range_max, io_port_slot
        );

        Ok(())
    }

    pub(super) fn handle_service_register_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }

        let process_entry = self.processes.find_entry_by_pid(pid).ok_or_else(|| {
            error!("[svc.err] register unknown pid={:>3}", pid);
            CapabilityError::InvalidArgument
        })?;

        let (raw_name, raw_len) = decode_service_name_24(request.arg0, request.arg1, request.arg2)
            .ok_or_else(|| {
                error!(
                    "[svc.err] register decode failed pid={:>3} args=[{:#018x},{:#018x},{:#018x}] slot={:#x}",
                    pid, request.arg0, request.arg1, request.arg2, request.arg3
                );
                CapabilityError::InvalidArgument
            })?;
        let service_name = core::str::from_utf8(&raw_name[..raw_len]).map_err(|_| {
            error!(
                "[svc.err] register non-utf8 pid={:>3} len={} args=[{:#018x},{:#018x},{:#018x}]",
                pid, raw_len, request.arg0, request.arg1, request.arg2
            );
            CapabilityError::InvalidArgument
        })?;
        let service_slot = request.arg3;
        validate_process_device_slot(service_slot).map_err(|e| {
            error!(
                "[svc.err] register invalid slot pid={:>3} name={} slot={:#x}",
                pid, service_name, service_slot
            );
            e
        })?;
        let service_port =
            make_child_slot_descriptor(process_entry.root_node, PROCESS_ROOT_RADIX, service_slot);

        self.communication
            .register_service(pid, service_name, service_port)?;

        info!(
            "[svc] registered name={} pid={:>3} port={:#018x}",
            service_name, pid, service_port
        );

        Ok(pid)
    }

    pub(super) fn handle_service_list_request(
        &self,
        request: OsRequestEvent,
    ) -> Option<(usize, usize)> {
        self.communication.service_info_by_ordinal(request.arg0)
    }
}
