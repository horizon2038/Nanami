use super::*;

impl Alpha {
    pub(super) fn prepare_alpha_heap(
        environment: &AlphaEnvironment,
        memory: &mut MemoryManager,
        bootstrap_generic: CapabilityDescriptor,
        alpha_address_space: CapabilityDescriptor,
        alpha_vm_space: &mut crate::nanami_core::vm_space::BootstrapVmSpace,
    ) -> Result<(usize, usize), CapabilityError> {
        let mut selected: Option<(usize, usize, usize, usize, bool)> = None;
        let count = environment.resource_count;

        // Reserve the permanent runtime stack before converting the remaining
        // RAM to 4K frames. Keep the original separate-stack path when no single
        // resource can hold both; do not require a larger firmware RAM extent.
        for reserved_pages in [ALPHA_HEAP_PAGES + ALPHA_RUNTIME_STACK_PAGES, ALPHA_HEAP_PAGES] {
            let needed_bytes = reserved_pages * PAGE_SIZE;
            for i in 0..count {
                let g = environment.resources[i];
                if g.is_device() || g.size_radix < 12 {
                    continue;
                }
                let size_bytes = 1usize << g.size_radix;
                if size_bytes < needed_bytes {
                    continue;
                }
                if g.capability == bootstrap_generic {
                    continue;
                }
                let consumed = memory.initial_generic_consumed_bytes_for_public(i);
                let Some(split_start) =
                    align_up_checked((g.address as usize).saturating_add(consumed), PAGE_SIZE)
                else {
                    continue;
                };
                let end = (g.address as usize).saturating_add(size_bytes);
                let split_start = align_up_checked(split_start, 1 << 21)
                    .filter(|aligned| MemoryManager::large_bootstrap_heap_eligible(*aligned, reserved_pages)
                        && aligned.checked_add(needed_bytes).is_some_and(|heap_end| heap_end <= end))
                    .unwrap_or(split_start);
                let Some(heap_end) = split_start.checked_add(needed_bytes) else {
                    continue;
                };
                if heap_end > end {
                    continue;
                }
                let usable_bytes = end - split_start;
                let large = MemoryManager::large_bootstrap_heap_eligible(split_start, reserved_pages);

                match selected {
                    None => selected = Some((i, split_start, reserved_pages, usable_bytes, large)),
                    Some((_, _, _, best_size, best_large))
                        if (large && !best_large) || (large == best_large && usable_bytes < best_size) => {
                        selected = Some((i, split_start, reserved_pages, usable_bytes, large))
                    }
                    _ => {}
                }
            }
            if selected.is_some() { break; }
        }

        let (generic_idx, base_address, reserved_pages, usable_bytes, _) =
            selected.ok_or(CapabilityError::InvalidArgument)?;
        info!(
            "heap generic idx={:>3} addr={:#018x} usable={:#x} pages={:>4}",
            generic_idx, base_address, usable_bytes, ALPHA_HEAP_PAGES
        );

        let frames = memory.prepare_bootstrap_heap_frames(generic_idx, base_address, reserved_pages, true)?;
        let heap = frames.slice_pages(0, ALPHA_HEAP_PAGES).ok_or(CapabilityError::InvalidArgument)?;

        let mut j = 0usize;
        while j < heap.leaves() {
            let frame = heap.frame(j)
                .ok_or(CapabilityError::InvalidArgument)?;
            let va = ALPHA_HEAP_BASE + j * heap.leaf_bytes();
            if let Err(e) = memory.map_bootstrap_heap_frame(alpha_address_space, frame, va,
                heap.leaf_bits(), alpha_vm_space) {
                info!(
                    "[heap.err] map idx={:>3} page={} va={:#018x} frame={:#018x} err={:?}",
                    generic_idx, j, va, frame, e
                );
                return Err(e);
            }
            unsafe {
                ptr::write_bytes(va as *mut u8, 0, heap.leaf_bytes());
            }
            j += 1;
        }
        nun::println!("[alpha.heap] leaf-bytes={} leaves={} bytes={}",
            heap.leaf_bytes(), heap.leaves(), ALPHA_HEAP_PAGES * PAGE_SIZE);

        Ok((frames.physical_base(), reserved_pages * PAGE_SIZE))
    }

    pub(super) fn prepare_runtime_stack(&mut self) -> Result<usize, CapabilityError> {
        if let Some(frames) = self.memory.bootstrap_stack_frames(ALPHA_HEAP_PAGES, ALPHA_RUNTIME_STACK_PAGES) {
            let alpha_as = self.processes.alpha_entry().address_space;
            let vm = self.processes.alpha_vm_space_mut();
            for i in 0..frames.leaves() {
                let frame = frames.frame(i).ok_or(CapabilityError::InvalidArgument)?;
                let va = ALPHA_RUNTIME_STACK_BASE + i * frames.leaf_bytes();
                self.memory.map_bootstrap_heap_frame(alpha_as, frame, va, frames.leaf_bits(), vm)?;
                unsafe { ptr::write_bytes(va as *mut u8, 0, frames.leaf_bytes()); }
            }
            nun::println!("[alpha.stack] leaf-bytes={} leaves={} bytes={}",
                frames.leaf_bytes(), frames.leaves(), ALPHA_RUNTIME_STACK_PAGES * PAGE_SIZE);
            return Ok((ALPHA_RUNTIME_STACK_BASE + ALPHA_RUNTIME_STACK_PAGES * PAGE_SIZE - 16) & !0xFusize);
        }
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
