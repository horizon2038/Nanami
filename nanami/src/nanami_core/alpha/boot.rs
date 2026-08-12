use super::*;

impl Alpha {
    pub(super) fn prepare_alpha_heap(
        init_info: &InitInfo,
        memory: &mut MemoryManager,
        root_radix: usize,
        bootstrap_generic: CapabilityDescriptor,
        alpha_address_space: CapabilityDescriptor,
        alpha_vm_space: &mut crate::nanami_core::vm_space::BootstrapVmSpace,
    ) -> Result<usize, CapabilityError> {
        let mut selected: Option<(usize, usize, usize, usize)> = None;
        let needed_bytes = ALPHA_HEAP_PAGES * PAGE_SIZE;
        let count = init_info.generic_list_count as usize;

        for i in 0..count {
            let g = init_info.generic_list[i];
            if g.is_device || g.size_radix < 12 {
                continue;
            }
            let size_bytes = 1usize << g.size_radix;
            if size_bytes < needed_bytes {
                continue;
            }
            let desc = make_generic_descriptor(root_radix, i);
            if desc == bootstrap_generic {
                continue;
            }
            let consumed = memory.initial_generic_consumed_bytes_for_public(i);
            let Some(split_start) =
                align_up_checked((g.address as usize).saturating_add(consumed), PAGE_SIZE)
            else {
                continue;
            };
            let end = (g.address as usize).saturating_add(size_bytes);
            let Some(heap_end) = split_start.checked_add(needed_bytes) else {
                continue;
            };
            if heap_end > end {
                continue;
            }
            let Some(base_page) = memory.physical_page_index_from_address(split_start) else {
                continue;
            };
            if memory
                .physical_frame_descriptor_from_index(base_page + ALPHA_HEAP_PAGES - 1)
                .is_none()
            {
                continue;
            }
            let usable_bytes = end - split_start;

            match selected {
                None => selected = Some((i, split_start, base_page, usable_bytes)),
                Some((_, _, _, best_size)) if usable_bytes < best_size => {
                    selected = Some((i, split_start, base_page, usable_bytes))
                }
                _ => {}
            }
        }

        let (generic_idx, base_address, base_page, usable_bytes) =
            selected.ok_or(CapabilityError::InvalidArgument)?;
        info!(
            "heap generic idx={:>3} addr={:#018x} usable={:#x} pages={:>4}",
            generic_idx, base_address, usable_bytes, ALPHA_HEAP_PAGES
        );

        let mut i = 0usize;
        while i < ALPHA_HEAP_PAGES {
            if let Err(e) = memory.ensure_alpha_frame_at_physical_index(base_page + i) {
                info!(
                    "[heap.err] ensure frame idx={:>3} page={} frame_index={} err={:?}",
                    generic_idx,
                    i,
                    base_page + i,
                    e
                );
                return Err(e);
            }
            i += 1;
        }

        let mut j = 0usize;
        while j < ALPHA_HEAP_PAGES {
            let frame = memory
                .physical_frame_descriptor_from_index(base_page + j)
                .ok_or(CapabilityError::InvalidArgument)?;
            let va = ALPHA_HEAP_BASE + j * PAGE_SIZE;
            if let Err(e) = memory.map_frame(alpha_address_space, frame, va, alpha_vm_space) {
                info!(
                    "[heap.err] map idx={:>3} page={} va={:#018x} frame={:#018x} err={:?}",
                    generic_idx, j, va, frame, e
                );
                return Err(e);
            }
            unsafe {
                ptr::write_bytes(va as *mut u8, 0, PAGE_SIZE);
            }
            j += 1;
        }

        Ok(base_address)
    }

    pub(super) fn prepare_runtime_stack(&mut self) -> Result<usize, CapabilityError> {
        let stack_node =
            make_root_slot_descriptor(self.root.root_radix, ALPHA_RUNTIME_STACK_NODE_SLOT);
        match arch::generic::convert(
            self.root.bootstrap_generic,
            CapabilityType::Node,
            ALPHA_RUNTIME_STACK_NODE_RADIX as Word,
            1,
            self.root.root_descriptor,
            ALPHA_RUNTIME_STACK_NODE_SLOT as Word,
        ) {
            Ok(()) | Err(CapabilityError::InvalidArgument) => {}
            Err(e) => return Err(e),
        }

        let _ = self.memory.allocate_process_frames(
            stack_node,
            ALPHA_RUNTIME_STACK_NODE_RADIX,
            0,
            ALPHA_RUNTIME_STACK_PAGES,
        )?;

        let memory = &mut self.memory;
        let processes = &mut self.processes;
        let alpha_as = processes.alpha_entry().address_space;
        let vm_space = processes.alpha_vm_space_mut();
        let mut i = 0usize;
        while i < ALPHA_RUNTIME_STACK_PAGES {
            let frame = make_child_slot_descriptor(stack_node, ALPHA_RUNTIME_STACK_NODE_RADIX, i);
            let va = ALPHA_RUNTIME_STACK_BASE + i * PAGE_SIZE;
            memory.map_frame(alpha_as, frame, va, vm_space)?;
            unsafe {
                ptr::write_bytes(va as *mut u8, 0, PAGE_SIZE);
            }
            i += 1;
        }

        Ok((ALPHA_RUNTIME_STACK_BASE + ALPHA_RUNTIME_STACK_PAGES * PAGE_SIZE - 16) & !0xFusize)
    }
}
