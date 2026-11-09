use super::*;

#[cfg(test)]
static PROTECTION_PAGE_VISITS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
pub(super) fn take_protection_page_visits() -> usize {
    PROTECTION_PAGE_VISITS.swap(0, core::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
static PROTECTION_VM_RESOLUTIONS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
pub(super) fn take_protection_vm_resolutions() -> usize {
    PROTECTION_VM_RESOLUTIONS.swap(0, core::sync::atomic::Ordering::Relaxed)
}

impl Alpha {
    pub(super) fn handle_mapping_protect_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let pid = request.arg0;
        let base = request.arg1;
        let bytes = request.arg2;
        let permissions = request.arg3;
        if request.identifier == 0
            || pid == 0
            || base & (PAGE_SIZE - 1) != 0
            || bytes == 0
            || permissions & !7 != 0
        {
            return Err(CapabilityError::InvalidArgument);
        }
        let entry = self
            .processes
            .find_entry_by_pid(pid)
            .filter(|entry| !entry.exited)
            .ok_or(CapabilityError::InvalidArgument)?;
        if request.identifier != pid && entry.reaper_pid != request.identifier {
            return Err(CapabilityError::PermissionDenied);
        }
        let entry = self
            .processes
            .memory_context(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        let end = base
            .checked_add(bytes)
            .and_then(|end| end.checked_add(PAGE_SIZE - 1))
            .ok_or(CapabilityError::InvalidArgument)?
            & !(PAGE_SIZE - 1);
        let unchanged = self.processes.vm_space_mut(pid).is_some_and(|vm| {
            if permissions == 0 {
                // A backed zero-rights range is a subset of this proof. On
                // failure it cannot succeed by checking backing as well.
                vm.confirmed_zero_permissions(base, end)
            } else {
                (permissions & 2 == 0 || !vm.private_range_shared(base, end))
                    && vm.backed_range_with_permissions(base, end, permissions)
            }
        });
        if unchanged {
            // Preserve copy-window cleanup and its failure even when no user
            // PTE changes. Unbacked zero-rights pages also need no transition.
            // Avoid the rollback Vec and repeated page lookups.
            self.invalidate_process_copy_mappings(pid)?;
            return Ok(());
        }
        let mut pages = Vec::new();
        // Large fully backed ranges need no materialization. Keep one stable
        // metadata borrow for the ordered rollback snapshot, rather than
        // finding the VM, Frame and permissions again for every page.
        let snapshot = end - base >= 16 * PAGE_SIZE
            && self.processes.vm_space_mut(pid).is_some_and(|vm| {
                if !vm.fully_backed_aligned_range(base, end) { return false; }
                pages.reserve_exact((end - base) / PAGE_SIZE);
                vm.visit_backed_metadata(base, end, |va, frame, rights, uncertain| {
                    // Point-wise protection ignores non-page-aligned keys.
                    if va & (PAGE_SIZE - 1) == 0 {
                        pages.push((va, Some(frame), if uncertain { 0 } else { rights }));
                    }
                });
                true
            });
        if !snapshot {
            for va in (base..end).step_by(PAGE_SIZE) {
                #[cfg(test)]
                PROTECTION_PAGE_VISITS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                let frame = self
                    .processes
                    .vm_space_mut(pid)
                    .and_then(|vm| vm.find_frame(va));
                let frame = if let Some(frame) = frame {
                    frame
                } else if permissions == 0 {
                    // A never-populated PROT_NONE reservation has no backing yet.
                    continue;
                } else if let Some((index, _, _)) = self.processes.find_lazy_mapping_plan(pid, va) {
                    if matches!(self.processes.lazy_mapping_kind(index),
                        Some(crate::nanami_core::process::ProcessLazyMappingKind::Zero))
                    {
                        if end - base > PAGE_SIZE && self.processes.vm_space_mut(pid).is_some_and(|vm|
                            !vm.mapping_is_uncertain(va) && !vm.private_page_shared(va))
                        {
                            // Range protection need not populate untouched zero
                            // reservations. Record their rights in the same rollback
                            // transaction as backed pages. Single-page demand-fault
                            // requests still run the initialized constructor below.
                            let old = self.processes.vm_space_mut(pid)
                                .ok_or(CapabilityError::InvalidArgument)?.permissions(va);
                            pages.push((va, None, old));
                            continue;
                        }
                        // Demand stack faults may have live VM peers. The private
                        // constructor fills zeroes before publishing final rights,
                        // and accounts for failed/uncertain construction itself.
                        // Do not publish an uninitialized lazy page, even briefly.
                        let reserved = self.processes.take_fresh_stack_frame_slot(pid, va, PROCESS_FRAME_TOTAL_PAGES);
                        self.map_process_initialized_pages_with_stack_slot(pid, Some(va), 1, None, permissions, reserved)?;
                        self.processes.vm_space_mut(pid)
                            .and_then(|vm| vm.find_frame(va)).ok_or(CapabilityError::Fatal)?
                    } else {
                        self.process_frame_for_page(pid, va)?
                    }
                } else {
                    self.map_process_heap_pages_at(pid, va, 1)?;
                    let frame = self
                        .processes
                        .vm_space_mut(pid)
                        .and_then(|vm| vm.find_frame(va))
                        .ok_or(CapabilityError::Fatal)?;
                    // Materializing a reservation must not grant access if a later
                    // allocation or protection update fails.
                    self.protect_process_page(pid, entry.address_space, va, frame, 0)?;
                    frame
                };
                let old = self
                    .processes
                    .vm_space_mut(pid)
                    .ok_or(CapabilityError::InvalidArgument)?
                    .permissions(va);
                pages.push((va, Some(frame), old));
            }
        }
        self.invalidate_process_copy_mappings(pid)?;
        // The empty preflight previously completed without resolving a VM.
        if pages.is_empty() { return Ok(()); }
        #[cfg(test)]
        PROTECTION_VM_RESOLUTIONS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        // Kernel MAP/UNMAP calls cannot dispatch another Alpha process event.
        // Keep this current VM borrow for both forward updates and rollback.
        let vm = self.processes.vm_space_mut(pid).ok_or(CapabilityError::InvalidArgument)?;
        for (index, &(va, frame, _)) in pages.iter().enumerate() {
            let Some(frame) = frame else {
                vm.set_permissions(va, permissions);
                continue;
            };
            if let Err(error) =
                Self::protect_vm_page(vm, entry.address_space, va, frame, permissions)
            {
                for &(va, frame, old) in pages[..=index].iter().rev() {
                    let Some(frame) = frame else {
                        vm.set_permissions(va, old);
                        continue;
                    };
                    if Self::protect_vm_page(vm, entry.address_space, va, frame, old)
                        .is_err()
                    {
                        return Err(CapabilityError::Fatal);
                    }
                }
                return Err(error);
            }
        }
        Ok(())
    }

    pub(super) fn protect_process_page(
        &mut self,
        pid: Word,
        address_space: CapabilityDescriptor,
        va: Word,
        frame: CapabilityDescriptor,
        permissions: Word,
    ) -> Result<(), CapabilityError> {
        #[cfg(test)]
        PROTECTION_VM_RESOLUTIONS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let vm = self
            .processes
            .vm_space_mut(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        Self::protect_vm_page(vm, address_space, va, frame, permissions)
    }

    fn protect_vm_page(
        vm: &mut crate::nanami_core::vm_space::VmSpace,
        address_space: CapabilityDescriptor,
        va: Word,
        frame: CapabilityDescriptor,
        permissions: Word,
    ) -> Result<(), CapabilityError> {
        if permissions & 2 != 0 && vm.private_page_shared(va) {
            return Err(CapabilityError::IllegalOperation);
        }
        // Do not accept an equal-permission no-op until both leaf removal and
        // its shootdown are confirmed. Failed cleanup leaves ownership pinned.
        if vm.mapping_is_uncertain(va) {
            arch::address_space::unmap(address_space, frame, va)?;
            vm.confirm_unmapped(va);
        }
        let old = vm.permissions(va);
        if old == permissions {
            return Ok(());
        }
        if old != 0 {
            if let Err(error) = arch::address_space::unmap(address_space, frame, va) {
                vm.mark_mapping_uncertain(va);
                return Err(error);
            }
            vm.set_permissions(va, 0);
        }
        if permissions != 0 {
            let attributes = nun::capability_call::address_space::Attribute::from_bits(permissions)
                .ok_or(CapabilityError::InvalidArgument)?;
            if let Err(error) = arch::address_space::map(address_space, frame, va, attributes) {
                vm.mark_mapping_uncertain(va);
                return Err(error);
            }
            vm.set_permissions(va, permissions);
        }
        Ok(())
    }
}
