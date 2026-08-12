use super::*;

impl Alpha {
    pub(super) fn ensure_process_frame_chunks(
        &mut self,
        pid: usize,
        process_root: CapabilityDescriptor,
        start_slot: usize,
        page_count: usize,
    ) -> Result<(), CapabilityError> {
        if page_count == 0 {
            return Ok(());
        }
        let end_slot = start_slot
            .checked_add(page_count)
            .ok_or(CapabilityError::InvalidArgument)?;
        if end_slot > PROCESS_FRAME_TOTAL_PAGES {
            return Err(CapabilityError::InvalidArgument);
        }

        let frame_directory = process_frame_directory_descriptor(process_root);
        let process_generic = self.process_arena_for_root(process_root)?;
        let mut chunk = start_slot / PROCESS_FRAME_CHUNK_PAGES;
        let last_chunk = (end_slot - 1) / PROCESS_FRAME_CHUNK_PAGES;
        while chunk <= last_chunk {
            if !self.processes.has_frame_chunk(pid, chunk) {
                arch::generic::convert(
                    process_generic,
                    CapabilityType::Node,
                    PROCESS_FRAME_NODE_RADIX as Word,
                    1,
                    frame_directory,
                    chunk as Word,
                )?;
                self.processes.register_frame_chunk(pid, chunk)?;
            }
            chunk += 1;
        }
        Ok(())
    }

    pub(super) fn process_arena_for_root(
        &self,
        process_root: CapabilityDescriptor,
    ) -> Result<CapabilityDescriptor, CapabilityError> {
        let payload_bits = nun::WORD_BITS - nun::BYTE_BITS;
        let slot_shift = payload_bits
            .checked_sub(self.root.root_radix)
            .ok_or(CapabilityError::InvalidArgument)?;
        let slot_mask = (1usize << self.root.root_radix) - 1;
        let root_slot = (process_root >> slot_shift) & slot_mask;
        if make_root_slot_descriptor(self.root.root_radix, root_slot) != process_root {
            return Err(CapabilityError::InvalidArgument);
        }
        self.memory.process_arena_descriptor(root_slot)
    }

    pub(super) fn allocate_process_frames(
        &mut self,
        pid: usize,
        process_root: CapabilityDescriptor,
        start_slot: usize,
        page_count: usize,
    ) -> Result<Vec<(usize, usize)>, CapabilityError> {
        self.ensure_process_frame_chunks(pid, process_root, start_slot, page_count)?;
        let mut allocated = Vec::new();
        let mut done = 0usize;
        while done < page_count {
            let global_slot = start_slot + done;
            let chunk = global_slot / PROCESS_FRAME_CHUNK_PAGES;
            let chunk_offset = global_slot % PROCESS_FRAME_CHUNK_PAGES;
            let chunk_remaining = PROCESS_FRAME_CHUNK_PAGES - chunk_offset;
            let batch = chunk_remaining.min(page_count - done);
            let allocated_pages = self.memory.allocate_process_frames(
                process_frame_chunk_descriptor(process_root, chunk),
                PROCESS_FRAME_NODE_RADIX,
                chunk_offset,
                batch,
            )?;
            for (offset, page) in allocated_pages.iter().enumerate() {
                let slot = global_slot + offset;
                allocated.push((slot, *page));
            }
            done += batch;
        }
        Ok(allocated)
    }

    pub(super) fn spawn_process_from_elf(
        &mut self,
        image_name: &str,
        image_bytes: &'static [u8],
        reaper_pid: usize,
        resolver_port: Option<CapabilityDescriptor>,
        auto_resume: bool,
        priority_override: Option<Word>,
    ) -> Result<usize, CapabilityError> {
        info!("[proc] parse elf: {}", image_name);
        let elf = parse_elf64(image_bytes)?;
        info!(
            "[proc] elf entry={:#018x} segments={:>2}",
            elf.entry_point, elf.segment_count
        );

        let (pid, process_root_slot) = self.processes.alloc_process_slot()?;
        macro_rules! spawn_try {
            ($expr:expr) => {
                match $expr {
                    Ok(value) => value,
                    Err(error) => {
                        self.cleanup_failed_spawn(pid, process_root_slot);
                        return Err(error);
                    }
                }
            };
        }
        let child_root = make_root_slot_descriptor(self.root.root_radix, process_root_slot);
        let child_pcb =
            make_child_slot_descriptor(child_root, PROCESS_ROOT_RADIX, PROCESS_SLOT_PCB);
        let child_os_port =
            make_child_slot_descriptor(child_root, PROCESS_ROOT_RADIX, PROCESS_SLOT_OS_PORT);
        let child_address_space =
            make_child_slot_descriptor(child_root, PROCESS_ROOT_RADIX, PROCESS_SLOT_ADDRESS_SPACE);
        let child_notification = make_child_slot_descriptor(
            child_root,
            PROCESS_ROOT_RADIX,
            PROCESS_SLOT_NOTIFICATION_PORT,
        );
        let process_generic = spawn_try!(self.memory.ensure_process_arena(process_root_slot));

        info!(
            "[proc] create child root slot={:>3} desc={:#018x}",
            process_root_slot, child_root
        );
        spawn_try!(arch::generic::convert(
            process_generic,
            CapabilityType::Node,
            PROCESS_ROOT_RADIX as Word,
            1,
            self.root.root_descriptor,
            process_root_slot as Word,
        ));

        info!("[proc] populate child root");
        spawn_try!(arch::generic::convert(
            process_generic,
            CapabilityType::ProcessControlBlock,
            14,
            1,
            child_root,
            PROCESS_SLOT_PCB as Word,
        ));
        spawn_try!(arch::node::copy(
            child_root,
            PROCESS_SLOT_OS_PORT as Word,
            self.processes.alpha_entry().os_port,
        ));
        spawn_try!(arch::generic::convert(
            process_generic,
            CapabilityType::AddressSpace,
            0,
            1,
            child_root,
            PROCESS_SLOT_ADDRESS_SPACE as Word,
        ));
        spawn_try!(arch::generic::convert(
            process_generic,
            CapabilityType::Node,
            PAGE_TABLE_NODE_RADIX as Word,
            1,
            child_root,
            PROCESS_SLOT_L3_NODE as Word,
        ));
        spawn_try!(arch::generic::convert(
            process_generic,
            CapabilityType::Node,
            PAGE_TABLE_NODE_RADIX as Word,
            1,
            child_root,
            PROCESS_SLOT_L2_NODE as Word,
        ));
        spawn_try!(arch::generic::convert(
            process_generic,
            CapabilityType::Node,
            PAGE_TABLE_NODE_RADIX as Word,
            1,
            child_root,
            PROCESS_SLOT_L1_NODE as Word,
        ));
        spawn_try!(arch::generic::convert(
            process_generic,
            CapabilityType::Node,
            PROCESS_FRAME_DIRECTORY_RADIX as Word,
            1,
            child_root,
            PROCESS_SLOT_FRAME_NODE as Word,
        ));
        spawn_try!(arch::generic::convert(
            process_generic,
            CapabilityType::IpcPort,
            0,
            1,
            child_root,
            PROCESS_SLOT_SERVICE_PORT as Word,
        ));
        spawn_try!(arch::generic::convert(
            process_generic,
            CapabilityType::NotificationPort,
            0,
            1,
            child_root,
            PROCESS_SLOT_NOTIFICATION_PORT as Word,
        ));
        let _ = arch::notification_port::identify(child_notification, 0);
        let _ = arch::ipc_port::identify(child_os_port, pid as Word);

        let mut image_base = usize::MAX;
        let mut image_end = 0usize;
        let mut i = 0usize;
        while i < elf.segment_count {
            let seg = elf.segments[i];
            if seg.memory_size != 0 {
                image_base = image_base.min(align_down(seg.virtual_address, PAGE_SIZE));
                image_end =
                    image_end.max(align_up(seg.virtual_address + seg.memory_size, PAGE_SIZE));
            }
            i += 1;
        }
        if image_base == usize::MAX || image_end <= image_base {
            return Err(CapabilityError::InvalidArgument);
        }
        let image_pages = (image_end - image_base) / PAGE_SIZE;
        let total_frames = image_pages + USER_STACK_PAGES;
        let stack_top = USER_STACK_BASE + USER_STACK_PAGES * PAGE_SIZE;
        // Native runtime reads argc, argv[0], and envp[0] from the initial stack.
        // Keep these zero words inside the mapped stack even when argc == 0.
        let initial_stack_pointer = stack_top - 32;
        let heap_base = align_up(image_end.max(USER_ANONYMOUS_MAP_BASE), PAGE_SIZE);
        let raw_fault_process = elf.ipc_buffer_start.is_none() && resolver_port.is_some();
        let (has_ipc_buffer, ipc_buffer_va, ipc_buffer_frame_slot, ipc_buffer_tls_base) =
            match elf.ipc_buffer_start {
                Some(va) => {
                    if va < image_base || va >= image_end || (va & (PAGE_SIZE - 1)) != 0 {
                        error!(
                        "[proc.err] invalid __ipc_buffer_start={:#018x} image=[{:#018x}..{:#018x})",
                        va, image_base, image_end
                    );
                        return Err(CapabilityError::InvalidArgument);
                    }
                    let frame_slot = (va - image_base) / PAGE_SIZE;
                    spawn_try!(self.ensure_process_frame_chunks(pid, child_root, frame_slot, 1));
                    (
                        true,
                        va,
                        frame_slot,
                        va + (nun::TLS_BASE_OFFSET as usize) * nun::BYTE_BITS,
                    )
                }
                None if raw_fault_process => {
                    info!("[proc] raw fault-handler ELF without Nanami IPC buffer");
                    (false, 0, 0, 0)
                }
                None => {
                    error!("[proc.err] missing required symbol __ipc_buffer_start");
                    return Err(CapabilityError::InvalidArgument);
                }
            };

        info!(
            "[proc] lazy map plan image=[{:#018x}..{:#018x}) image_pages={:>3} stack_pages={:>3} ipc={:#018x} temp={:#018x}",
            image_base,
            image_end,
            image_pages,
            USER_STACK_PAGES,
            ipc_buffer_va,
            TEMP_MAP_BASE + pid * TEMP_MAP_STRIDE
        );

        spawn_try!(self.processes.ensure_vm_space_for_pid(pid));
        spawn_try!(self.processes.register_lazy_mapping(
            pid,
            image_base,
            image_pages,
            0,
            ProcessLazyMappingKind::Image {
                image: image_bytes,
                elf,
            },
        ));
        spawn_try!(self.processes.register_lazy_mapping(
            pid,
            USER_STACK_BASE,
            USER_STACK_PAGES,
            image_pages,
            ProcessLazyMappingKind::Zero,
        ));
        info!("[proc] lazy vm tracker ready pid={:>3}", pid);

        let mut image_page = 0usize;
        while image_page < image_pages {
            let image_va = image_base + image_page * PAGE_SIZE;
            spawn_try!(self.materialize_lazy_page(pid, child_root, child_address_space, image_va));
            image_page += 1;
        }
        self.processes.drop_image_lazy_mappings_for_pid(pid);
        let ipc_buffer_frame = if has_ipc_buffer {
            process_frame_descriptor(child_root, ipc_buffer_frame_slot)
        } else {
            0
        };
        let mut stack_page = 0usize;
        while stack_page < USER_STACK_PAGES {
            let stack_va = USER_STACK_BASE + stack_page * PAGE_SIZE;
            spawn_try!(self.materialize_lazy_page(pid, child_root, child_address_space, stack_va));
            stack_page += 1;
        }

        let config = nun::capability_call::process_control_block::ConfigurationInfo::new(
            true,           // address_space
            true,           // root_node
            has_ipc_buffer, // frame_ipc_buffer
            true,           // notification_port
            true,           // ipc_port_resolver
            true,           // instruction_pointer
            true,           // stack_pointer
            has_ipc_buffer, // thread_local_base
            true,           // priority
            false,          // affinity
        );

        info!(
            "[proc] configure pcb={:#018x} root={:#018x} as={:#018x} ip={:#018x} sp={:#018x}",
            child_pcb, child_root, child_address_space, elf.entry_point, initial_stack_pointer
        );
        let priority = priority_override.unwrap_or_else(|| process_priority_for_image(image_name));
        let resolver_port = if let Some(external_resolver) = resolver_port {
            spawn_try!(arch::node::copy(
                child_root,
                PROCESS_SLOT_FAULT_RESOLVER as Word,
                external_resolver,
            ));
            let child_resolver = make_child_slot_descriptor(
                child_root,
                PROCESS_ROOT_RADIX,
                PROCESS_SLOT_FAULT_RESOLVER,
            );
            spawn_try!(arch::ipc_port::identify(child_resolver, pid as Word));
            child_resolver
        } else {
            child_os_port
        };
        spawn_try!(arch::process_control_block::configure(
            child_pcb,
            config,
            child_address_space,
            child_root,
            ipc_buffer_frame,
            child_notification,
            resolver_port,
            elf.entry_point,
            initial_stack_pointer,
            ipc_buffer_tls_base,
            priority,
            0,
        ));

        spawn_try!(self.processes.install_process(
            pid,
            reaper_pid,
            process_root_slot,
            child_root,
            child_pcb,
            child_address_space,
            child_os_port,
            pid as Word,
            total_frames,
            heap_base,
            USER_HEAP_LIMIT,
        ));
        if auto_resume {
            spawn_try!(arch::process_control_block::resume(child_pcb));
        }
        info!(
            "[proc] child {} image={} pid={:>3} priority={:>2} root={:#018x} entry={:#018x}",
            if auto_resume { "resumed" } else { "prepared" },
            image_name,
            pid,
            priority,
            child_root,
            elf.entry_point
        );
        Ok(pid)
    }

    pub(super) fn spawn_initramfs_image(
        &mut self,
        image_name: &str,
        reaper_pid: usize,
        resolver_port: Option<CapabilityDescriptor>,
        auto_resume: bool,
        priority_override: Option<Word>,
    ) -> Result<usize, CapabilityError> {
        let mut spawned_pid = None;
        cpio::for_each_newc_entry(INITRAMFS_IMAGE, |entry| {
            if spawned_pid.is_some() || !initramfs_image_is_auto_spawn_candidate(entry.name) {
                return Ok(());
            }
            if initramfs_image_name_matches(entry.name, image_name) {
                spawned_pid = Some(self.spawn_process_from_elf(
                    entry.name,
                    entry.data,
                    reaper_pid,
                    resolver_port,
                    auto_resume,
                    priority_override,
                )?);
            }
            Ok(())
        })?;
        spawned_pid.ok_or(CapabilityError::InvalidArgument)
    }

    pub(super) fn cleanup_failed_spawn(&mut self, pid: usize, root_slot: usize) {
        let _ = arch::node::revoke(self.root.root_descriptor, root_slot as Word);
        let _ = arch::node::remove(self.root.root_descriptor, root_slot as Word);

        let physical_allocations = self.processes.releasable_physical_allocations_for_pid(pid);
        for allocation in physical_allocations.iter() {
            let _ = self.memory.free_physical(
                allocation.base_page * PAGE_SIZE,
                allocation.page_count * PAGE_SIZE,
            );
        }
        self.free_deferred_process_allocations(pid, None);

        if self.processes.find_entry_by_pid(pid).is_some() {
            let _ = self.processes.mark_exited(pid, 0, 1);
            let _ = self.processes.reap_process(pid, true);
        } else {
            self.processes.discard_process_artifacts(pid, root_slot);
        }
        let _ = self.memory.reset_process_arena(root_slot);
    }

    pub(super) fn free_deferred_process_allocations(
        &mut self,
        pid: usize,
        process_root: Option<CapabilityDescriptor>,
    ) {
        let allocations = self
            .processes
            .releasable_deferred_physical_allocations_for_pid(pid);
        for allocation in allocations.iter() {
            if let Some(root_node) = process_root {
                let mut i = 0usize;
                while i < allocation.page_count {
                    let slot = allocation.start_slot + i;
                    let _ = arch::node::remove(
                        process_frame_chunk_descriptor(root_node, slot / PROCESS_FRAME_CHUNK_PAGES),
                        (slot % PROCESS_FRAME_CHUNK_PAGES) as Word,
                    );
                    i += 1;
                }
            }
            let _ = self.memory.free_physical(
                allocation.base_page * PAGE_SIZE,
                allocation.page_count * PAGE_SIZE,
            );
        }
        self.processes
            .drop_deferred_physical_allocations_for_pid(pid);
    }

    pub(super) fn try_handle_demand_page_fault(
        &mut self,
        fault: KernelFaultEvent,
    ) -> Result<(), CapabilityError> {
        if fault.identifier == 0 || fault.architecture_fault_code & 1 != 0 {
            return Err(CapabilityError::IllegalOperation);
        }

        let page_va = align_down(fault.fault_address, PAGE_SIZE);
        let entry = self
            .processes
            .find_entry_by_pid(fault.identifier)
            .ok_or(CapabilityError::InvalidArgument)?;
        let _ = self.materialize_lazy_page(
            fault.identifier,
            entry.root_node,
            entry.address_space,
            page_va,
        )?;
        debug!(
            "[fault] demand page pid={:>3} va={:#018x}",
            fault.identifier, page_va
        );
        Ok(())
    }

    pub(super) fn materialize_lazy_page(
        &mut self,
        pid: usize,
        process_root: CapabilityDescriptor,
        address_space: CapabilityDescriptor,
        page_va: usize,
    ) -> Result<CapabilityDescriptor, CapabilityError> {
        if page_va & (PAGE_SIZE - 1) != 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        if let Some(frame) = self
            .processes
            .vm_space_mut(pid)
            .and_then(|vm| vm.find_frame(page_va))
        {
            return Ok(frame);
        }
        let mapping = self
            .processes
            .find_lazy_mapping(pid, page_va)
            .ok_or(CapabilityError::InvalidArgument)?;
        let page_offset = (page_va - mapping.base_va) / PAGE_SIZE;
        let frame_slot = mapping.start_slot + page_offset;
        let allocated = self.allocate_process_frames(pid, process_root, frame_slot, 1)?;
        let base_page = allocated
            .first()
            .map(|(_, page)| *page)
            .ok_or(CapabilityError::InvalidArgument)?;
        self.processes
            .register_physical_allocation(pid, page_va, frame_slot, base_page, 1)?;

        let frame = process_frame_descriptor(process_root, frame_slot);
        {
            let vm = self
                .processes
                .vm_space_mut(pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            self.memory.map_frame(address_space, frame, page_va, vm)?;
        }

        let temp_va = TEMP_MAP_BASE + pid * TEMP_MAP_STRIDE + frame_slot * PAGE_SIZE;
        self.map_alpha_temporary_frame(frame, temp_va)?;

        fill_lazy_page(mapping.kind, page_va, temp_va)?;
        if let Err(e) = self.unmap_alpha_temporary_frame(frame, temp_va) {
            error!(
                "[proc.err] unmap lazy temp pid={:>3} va={:#018x} temp={:#018x} frame={:#018x} err={:?}",
                pid, page_va, temp_va, frame, e
            );
            return Err(e);
        }
        Ok(frame)
    }

    pub(super) fn handle_exit_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }

        let process_entry = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if process_entry.pcb == 0 {
            return Err(CapabilityError::InvalidDescriptor);
        }

        arch::process_control_block::suspend(process_entry.pcb)?;
        let is_ok = request.arg0;
        let error_value = request.arg1;
        self.processes.mark_exited(pid, is_ok, error_value)?;
        info!(
            "[proc] exited pid={:>3} pcb={:#018x} is_ok={} error={:#018x}",
            pid, process_entry.pcb, is_ok, error_value
        );
        Ok(())
    }

    pub(super) fn handle_process_spawn_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        if request.identifier == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let (raw_name, raw_len) = decode_service_name_24(request.arg0, request.arg1, request.arg2)
            .ok_or(CapabilityError::InvalidArgument)?;
        let image_name = core::str::from_utf8(&raw_name[..raw_len])
            .map_err(|_| CapabilityError::InvalidArgument)?;

        let pid = self.spawn_initramfs_image(image_name, request.identifier, None, true, None)?;
        info!(
            "[proc] spawned by request caller={:>3} image={} pid={:>3}",
            request.identifier, image_name, pid
        );
        Ok(pid)
    }

    pub(super) fn handle_process_spawn_memory_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        if request.identifier == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let image_vaddr = request.arg0;
        let size_bytes = request.arg1;
        let priority = request.arg2;
        if image_vaddr == 0
            || size_bytes == 0
            || size_bytes > PROCESS_SPAWN_MEMORY_MAX_BYTES
            || self
                .processes
                .find_entry_by_pid(request.identifier)
                .is_none()
        {
            return Err(CapabilityError::InvalidArgument);
        }

        let image_bytes = self.read_process_memory_into_static_buffer(
            request.identifier,
            image_vaddr,
            size_bytes,
        )?;
        let pid = self.spawn_process_from_elf(
            "rootfs-memory.elf",
            image_bytes,
            request.identifier,
            None,
            true,
            Some(priority),
        )?;
        info!(
            "[proc] spawned from memory caller={:>3} pid={:>3} bytes={:#x} priority={:>2}",
            request.identifier, pid, size_bytes, priority
        );
        Ok(pid)
    }

    pub(super) fn handle_process_spawn_memory_suspended_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        if request.identifier == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let caller = self
            .processes
            .find_entry_by_pid(request.identifier)
            .ok_or(CapabilityError::InvalidArgument)?;
        if caller.root_node == 0 {
            return Err(CapabilityError::InvalidDescriptor);
        }
        let image_vaddr = request.arg0;
        let size_bytes = request.arg1;
        let priority = request.arg2;
        let destination_slot = request.arg3;
        if image_vaddr == 0
            || size_bytes == 0
            || size_bytes > PROCESS_SPAWN_MEMORY_MAX_BYTES
            || destination_slot == 0
            || destination_slot >= (1 << PROCESS_ROOT_RADIX)
        {
            return Err(CapabilityError::InvalidArgument);
        }

        let image_bytes = self.read_process_memory_into_static_buffer(
            request.identifier,
            image_vaddr,
            size_bytes,
        )?;
        let pid = self.spawn_process_from_elf(
            "rootfs-memory.elf",
            image_bytes,
            request.identifier,
            None,
            false,
            Some(priority),
        )?;
        let child = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::Fatal)?;
        let _ = arch::node::remove(caller.root_node, destination_slot as Word);
        if let Err(error) = arch::node::copy(caller.root_node, destination_slot as Word, child.pcb)
        {
            self.cleanup_failed_spawn(pid, child.root_slot);
            return Err(error);
        }
        info!(
            "[proc] spawned memory suspended caller={:>3} pid={:>3} pcb_slot={:>3} bytes={:#x} priority={:>2}",
            request.identifier, pid, destination_slot, size_bytes, priority
        );
        Ok(pid)
    }

    pub(super) fn handle_process_spawn_memory_fault_handler_suspended_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        if request.identifier == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let caller = self
            .processes
            .find_entry_by_pid(request.identifier)
            .ok_or(CapabilityError::InvalidArgument)?;
        if caller.root_node == 0 {
            return Err(CapabilityError::InvalidDescriptor);
        }
        let image_vaddr = request.arg0;
        let size_bytes = request.arg1;
        let priority = request.arg2;
        let destination_slot = request.arg3;
        if image_vaddr == 0
            || size_bytes == 0
            || size_bytes > PROCESS_SPAWN_MEMORY_MAX_BYTES
            || destination_slot == 0
            || destination_slot >= (1 << PROCESS_ROOT_RADIX)
        {
            error!(
                "[proc.err] memory fault spawn invalid args caller={:>3} image={:#018x} bytes={:#x} priority={:>2} dst_slot={:>3}",
                request.identifier, image_vaddr, size_bytes, priority, destination_slot
            );
            return Err(CapabilityError::InvalidArgument);
        }

        let resolver_port = make_child_slot_descriptor(
            caller.root_node,
            PROCESS_ROOT_RADIX,
            PROCESS_SLOT_SERVICE_PORT,
        );
        let image_bytes = self.read_process_memory_into_static_buffer(
            request.identifier,
            image_vaddr,
            size_bytes,
        )?;
        let pid = self.spawn_process_from_elf(
            "rootfs-linux.elf",
            image_bytes,
            request.identifier,
            Some(resolver_port),
            false,
            Some(priority),
        )?;
        let child = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::Fatal)?;
        let _ = arch::node::remove(caller.root_node, destination_slot as Word);
        if let Err(error) = arch::node::copy(caller.root_node, destination_slot as Word, child.pcb)
        {
            error!(
                "[proc.err] memory child pcb copy failed caller={:>3} pid={:>3} dst_slot={:>3} pcb={:#018x} err={:?}",
                request.identifier, pid, destination_slot, child.pcb, error
            );
            self.cleanup_failed_spawn(pid, child.root_slot);
            return Err(error);
        }
        info!(
            "[proc] spawned memory with fault-handler caller={:>3} pid={:>3} pcb_slot={:>3} bytes={:#x} priority={:>2}",
            request.identifier, pid, destination_slot, size_bytes, priority
        );
        Ok(pid)
    }

    pub(super) fn handle_process_exec_memory_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        let caller_pid = request.identifier;
        let target_pid = request.arg0;
        let image_vaddr = request.arg1;
        let size_bytes = request.arg2;
        let priority = request.arg3;
        if caller_pid == 0
            || target_pid == 0
            || image_vaddr == 0
            || size_bytes == 0
            || size_bytes > PROCESS_SPAWN_MEMORY_MAX_BYTES
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
        if target.pcb == 0 || target.root_node == 0 || target.address_space == 0 {
            return Err(CapabilityError::InvalidDescriptor);
        }

        let image_bytes =
            self.read_process_memory_into_static_buffer(caller_pid, image_vaddr, size_bytes)?;
        self.replace_process_image_from_memory(target_pid, target, image_bytes, priority)
    }

    pub(super) fn replace_process_image_from_memory(
        &mut self,
        pid: usize,
        target: crate::nanami_core::process::ProcessEntry,
        image_bytes: &'static [u8],
        _priority: Word,
    ) -> Result<usize, CapabilityError> {
        let elf = parse_elf64(image_bytes)?;
        let mut image_base = usize::MAX;
        let mut image_end = 0usize;
        let mut i = 0usize;
        while i < elf.segment_count {
            let seg = elf.segments[i];
            if seg.memory_size != 0 {
                image_base = image_base.min(align_down(seg.virtual_address, PAGE_SIZE));
                image_end =
                    image_end.max(align_up(seg.virtual_address + seg.memory_size, PAGE_SIZE));
            }
            i += 1;
        }
        if image_base == usize::MAX || image_end <= image_base {
            return Err(CapabilityError::InvalidArgument);
        }

        let image_pages = (image_end - image_base) / PAGE_SIZE;
        let total_frames = image_pages + USER_STACK_PAGES;
        let heap_base = align_up(image_end.max(USER_ANONYMOUS_MAP_BASE), PAGE_SIZE);

        self.drop_process_runtime_mappings(pid, target)?;
        self.recreate_process_address_space_for_exec(target)?;
        self.processes.reset_runtime_memory_for_exec(
            pid,
            total_frames,
            heap_base,
            USER_HEAP_LIMIT,
        )?;
        self.processes.ensure_vm_space_for_pid(pid)?;
        self.processes.register_lazy_mapping(
            pid,
            USER_STACK_BASE,
            USER_STACK_PAGES,
            image_pages,
            ProcessLazyMappingKind::Zero,
        )?;

        let image_kind = ProcessLazyMappingKind::Image {
            image: image_bytes,
            elf,
        };
        let mut image_page = 0usize;
        while image_page < image_pages {
            let image_va = image_base + image_page * PAGE_SIZE;
            self.materialize_exec_image_page(
                pid,
                target.root_node,
                target.address_space,
                image_kind,
                image_va,
                image_page,
            )?;
            image_page += 1;
        }
        let mut stack_page = 0usize;
        while stack_page < USER_STACK_PAGES {
            let stack_va = USER_STACK_BASE + stack_page * PAGE_SIZE;
            self.materialize_lazy_page(pid, target.root_node, target.address_space, stack_va)?;
            stack_page += 1;
        }
        info!(
            "[proc] exec memory pid={:>3} image=[{:#018x}..{:#018x}) pages={} entry={:#018x}",
            pid, image_base, image_end, image_pages, elf.entry_point
        );
        Ok(elf.entry_point)
    }

    pub(super) fn materialize_exec_image_page(
        &mut self,
        pid: usize,
        process_root: CapabilityDescriptor,
        address_space: CapabilityDescriptor,
        kind: ProcessLazyMappingKind,
        page_va: usize,
        page_index: usize,
    ) -> Result<(), CapabilityError> {
        let frame_slot = page_index;
        let allocated = self.allocate_process_frames(pid, process_root, frame_slot, 1)?;
        let base_page = allocated
            .first()
            .map(|(_, page)| *page)
            .ok_or(CapabilityError::InvalidArgument)?;
        self.processes
            .register_physical_allocation(pid, page_va, frame_slot, base_page, 1)?;

        let frame = process_frame_descriptor(process_root, frame_slot);
        {
            let vm = self
                .processes
                .vm_space_mut(pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            self.memory
                .map_frame_strict(address_space, frame, page_va, vm)?;
        }

        let temp_va = PROCESS_ZERO_TEMP_BASE + page_index * PAGE_SIZE;
        self.map_alpha_temporary_frame(frame, temp_va)?;
        fill_lazy_page(kind, page_va, temp_va)?;
        if let Err(e) = self.unmap_alpha_temporary_frame(frame, temp_va) {
            error!(
                "[proc.exec.err] unmap image temp pid={:>3} va={:#018x} temp={:#018x} frame={:#018x} err={:?}",
                pid, page_va, temp_va, frame, e
            );
            return Err(e);
        }
        Ok(())
    }

    pub(super) fn recreate_process_address_space_for_exec(
        &mut self,
        target: crate::nanami_core::process::ProcessEntry,
    ) -> Result<(), CapabilityError> {
        arch::node::remove(target.root_node, PROCESS_SLOT_ADDRESS_SPACE as Word)?;
        arch::generic::convert(
            self.process_arena_for_root(target.root_node)?,
            CapabilityType::AddressSpace,
            0,
            1,
            target.root_node,
            PROCESS_SLOT_ADDRESS_SPACE as Word,
        )?;
        let config = nun::capability_call::process_control_block::ConfigurationInfo::new(
            true, false, false, false, false, false, false, false, false, false,
        );
        arch::process_control_block::configure(
            target.pcb,
            config,
            target.address_space,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        )
    }

    pub(super) fn drop_process_runtime_mappings(
        &mut self,
        pid: usize,
        target: crate::nanami_core::process::ProcessEntry,
    ) -> Result<(), CapabilityError> {
        let allocations =
            self.processes
                .reset_runtime_memory_for_exec(pid, 0, 0, USER_HEAP_LIMIT)?;
        for (allocation, is_last_reference) in allocations {
            let mut i = 0usize;
            while i < allocation.page_count {
                let slot = allocation.start_slot + i;
                let frame = process_frame_descriptor(target.root_node, slot);
                let va = allocation.base_va + i * PAGE_SIZE;
                if let Err(e) = arch::address_space::unmap(target.address_space, frame, va) {
                    info!(
                        "[proc.exec.warn] unmap failed pid={:>3} va={:#018x} frame={:#018x} err={:?}",
                        pid, va, frame, e
                    );
                }
                if let Err(e) = arch::node::remove(
                    process_frame_chunk_descriptor(
                        target.root_node,
                        slot / PROCESS_FRAME_CHUNK_PAGES,
                    ),
                    (slot % PROCESS_FRAME_CHUNK_PAGES) as Word,
                ) {
                    info!(
                        "[proc.exec.warn] frame cap remove failed pid={:>3} frame={:#018x} slot={} err={:?}",
                        pid, frame, slot, e
                    );
                }
                i += 1;
            }
            if is_last_reference {
                self.memory.free_physical(
                    allocation.base_page * PAGE_SIZE,
                    allocation.page_count * PAGE_SIZE,
                )?;
            }
        }
        self.free_deferred_process_allocations(pid, Some(target.root_node));
        Ok(())
    }

    pub(super) fn handle_process_spawn_fault_handler_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        self.handle_process_spawn_fault_handler_request_with_resume(request, true)
    }

    pub(super) fn handle_process_spawn_fault_handler_suspended_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        self.handle_process_spawn_fault_handler_request_with_resume(request, false)
    }

    pub(super) fn handle_process_spawn_fault_handler_request_with_resume(
        &mut self,
        request: OsRequestEvent,
        auto_resume: bool,
    ) -> Result<usize, CapabilityError> {
        if request.identifier == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let caller = self
            .processes
            .find_entry_by_pid(request.identifier)
            .ok_or(CapabilityError::InvalidArgument)?;
        if caller.root_node == 0 {
            return Err(CapabilityError::InvalidDescriptor);
        }
        let destination_slot = request.arg3;
        if destination_slot == 0 || destination_slot >= (1 << PROCESS_ROOT_RADIX) {
            return Err(CapabilityError::InvalidArgument);
        }

        let resolver_port = make_child_slot_descriptor(
            caller.root_node,
            PROCESS_ROOT_RADIX,
            PROCESS_SLOT_SERVICE_PORT,
        );
        let (raw_name, raw_len) = decode_service_name_24(request.arg0, request.arg1, request.arg2)
            .ok_or(CapabilityError::InvalidArgument)?;
        let image_name = core::str::from_utf8(&raw_name[..raw_len])
            .map_err(|_| CapabilityError::InvalidArgument)?;

        let pid = self.spawn_initramfs_image(
            image_name,
            request.identifier,
            Some(resolver_port),
            auto_resume,
            None,
        )?;
        let child = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::Fatal)?;
        let _ = arch::node::remove(caller.root_node, destination_slot as Word);
        if let Err(error) = arch::node::copy(caller.root_node, destination_slot as Word, child.pcb)
        {
            error!(
                "[proc.err] spawned child pcb copy failed caller={:>3} image={} pid={:>3} dst_slot={:>3} pcb={:#018x} err={:?}",
                request.identifier, image_name, pid, destination_slot, child.pcb, error
            );
            self.cleanup_failed_spawn(pid, child.root_slot);
            return Err(error);
        }
        info!(
            "[proc] spawned with fault-handler caller={:>3} image={} pid={:>3} pcb_slot={:>3} suspended={}",
            request.identifier, image_name, pid, destination_slot, !auto_resume
        );
        Ok(pid)
    }

    pub(super) fn handle_process_status_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        if request.identifier == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let pid = request.arg0;
        if pid == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let entry = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        validate_process_observer(request.identifier, entry.pid, entry.reaper_pid)?;
        Ok((entry.exited as usize, entry.exit_code))
    }

    pub(super) fn handle_process_alive_request(
        &self,
        request: OsRequestEvent,
    ) -> Result<bool, CapabilityError> {
        if request.identifier == 0 || request.arg0 == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        Ok(self
            .processes
            .find_entry_by_pid(request.arg0)
            .map(|entry| !entry.exited)
            .unwrap_or(false))
    }

    pub(super) fn handle_process_reap_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        if request.identifier == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let pid = request.arg0;
        if pid == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let entry = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        validate_process_observer(request.identifier, entry.pid, entry.reaper_pid)?;
        if !entry.exited {
            return Err(CapabilityError::IllegalOperation);
        }
        arch::node::revoke(self.root.root_descriptor, entry.root_slot as Word)?;
        arch::node::remove(self.root.root_descriptor, entry.root_slot as Word)?;
        let physical_allocations = self.processes.releasable_physical_allocations_for_pid(pid);
        for allocation in physical_allocations.iter() {
            self.memory.free_physical(
                allocation.base_page * PAGE_SIZE,
                allocation.page_count * PAGE_SIZE,
            )?;
        }
        self.free_deferred_process_allocations(pid, None);
        self.processes.drop_physical_allocations_for_pid(pid);
        self.memory.reset_process_arena(entry.root_slot)?;
        self.processes.reap_process(pid, true)?;
        info!(
            "[proc] reaped pid={:>3} root_slot={:>3}",
            pid, entry.root_slot
        );
        Ok(())
    }

    pub(super) fn handle_process_kill_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        if request.identifier == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let pid = request.arg0;
        if pid == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let entry = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        validate_process_observer(request.identifier, entry.pid, entry.reaper_pid)?;
        if entry.exited {
            return Ok(());
        }
        arch::process_control_block::suspend(entry.pcb)?;
        self.processes.mark_exited(pid, 0, request.arg1)?;
        info!(
            "[proc] killed pid={:>3} pcb={:#018x} signal={:#018x}",
            pid, entry.pcb, request.arg1
        );
        Ok(())
    }
}
