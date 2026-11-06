//! Checked sparse/extent mapping release with a single ownership snapshot.
use super::*;

impl Alpha {
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
        let allow_holes = request.arg3 == nanami_uapi::os::OS_MAPPING_RELEASE_ALLOW_HOLES;
        if request.arg3 != 0 && !allow_holes {
            return Err(CapabilityError::InvalidArgument);
        }
        if size_bytes == 0 || (base_va & (PAGE_SIZE - 1)) != 0 {
            info!(
                "[map.err] release invalid args caller={:>3} target={:>3} va={:#018x} bytes={:#x}",
                caller_pid, target_pid, base_va, size_bytes
            );
            return Err(CapabilityError::InvalidArgument);
        }

        let mapped_size = size_bytes
            .checked_add(PAGE_SIZE - 1)
            .ok_or(CapabilityError::InvalidArgument)?
            & !(PAGE_SIZE - 1);
        let end_va = base_va
            .checked_add(mapped_size)
            .ok_or(CapabilityError::InvalidArgument)?;
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
        let entry = self
            .processes
            .memory_entry(target_pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if let Some(reservation) = self
            .processes
            .shared_memory_reservation(target_pid, base_va, page_count)
        {
            return self.release_shared_memory(entry, reservation);
        }
        // A RAM reservation remains one releasable object after its backing
        // becomes page-counted. Partial/cross-object releases must not route
        // through private-page retirement and leave a broken live reservation.
        if self
            .processes
            .shared_memory_overlapping_range(target_pid, base_va, mapped_size)
            .is_some()
        {
            return Err(CapabilityError::InvalidArgument);
        }
        // Shared-object release never consumes this private-extent lookup.
        let exact_allocation = self
            .processes
            .find_active_physical_allocation_reference(target_pid, base_va, page_count);

        // Validate the complete range before changing any PTE or allocation reference.
        let mut allocations = Vec::new();
        if exact_allocation.is_none() {
            allocations = self
                .processes
                .physical_pages_in_range(target_pid, base_va, end_va);
            if !allow_holes && allocations.len() != page_count {
                return Err(CapabilityError::InvalidArgument);
            }
            // Sparse holes need no per-page walk. Every backed leaf, including
            // retained PROT_NONE backing, must have a validated reference.
            if allow_holes
                && self.processes.vm_space_mut(target_pid).is_some_and(|vm| {
                    !vm.all_backed_pages(base_va, end_va, |va| {
                        allocations
                            .binary_search_by_key(&va, |allocation| allocation.base_va)
                            .is_ok()
                    })
                })
            {
                return Err(CapabilityError::InvalidArgument);
            }
        }
        self.invalidate_process_copy_mappings(target_pid)?;
        let spans = if let Some(ref allocation) = exact_allocation {
            core::slice::from_ref(allocation)
        } else {
            allocations.as_slice()
        };
        for allocation in spans {
            for page in 0..allocation.page_count {
                let va = allocation.base_va + page * PAGE_SIZE;
                let slot = allocation.start_slot + page;
                let frame = process_frame_descriptor(entry.root_node, slot);
                let present = self
                    .processes
                    .vm_space_mut(target_pid)
                    .is_some_and(|vm| vm.frame_may_be_present(va));
                if let Err(e) = if present {
                    arch::address_space::unmap(entry.address_space, frame, va)
                } else {
                    Ok(())
                } {
                    if let Some(vm) = self.processes.vm_space_mut(target_pid) {
                        vm.mark_mapping_uncertain(va);
                    }
                    info!(
                        "[map.err] unmap target={:>3} va={:#018x} frame={:#018x} slot={} err={:?}",
                        target_pid, va, frame, slot, e
                    );
                    return Err(e);
                }
                if let Some(vm) = self.processes.vm_space_mut(target_pid) {
                    vm.forget_frame(va);
                }
            }
        }

        if exact_allocation.is_some() {
            self.processes
                .defer_active_physical_allocation(target_pid, base_va, page_count)?;
        } else {
            // Fresh page extents are registered in increasing VA order; release
            // their newest references first instead of repeatedly walking past
            // the rest of the same extent in the ownership list.
            for allocation in allocations.into_iter().rev() {
                self.processes.defer_active_physical_allocation(
                    target_pid,
                    allocation.base_va,
                    1,
                )?;
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
}
