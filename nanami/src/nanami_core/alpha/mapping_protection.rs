use super::*;

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
            .memory_entry(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        let end = base
            .checked_add(bytes)
            .and_then(|end| end.checked_add(PAGE_SIZE - 1))
            .ok_or(CapabilityError::InvalidArgument)?
            & !(PAGE_SIZE - 1);
        let mut pages = Vec::new();
        for va in (base..end).step_by(PAGE_SIZE) {
            let frame = self
                .processes
                .vm_space_mut(pid)
                .and_then(|vm| vm.find_frame(va));
            let frame = if let Some(frame) = frame {
                frame
            } else if permissions == 0 {
                // A never-populated PROT_NONE reservation has no backing yet.
                continue;
            } else if self.processes.find_lazy_mapping(pid, va).is_some() {
                self.process_frame_for_page(pid, va)?
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
            pages.push((va, frame, old));
        }
        self.invalidate_process_copy_mappings(pid)?;
        for (index, &(va, frame, _)) in pages.iter().enumerate() {
            if let Err(error) =
                self.protect_process_page(pid, entry.address_space, va, frame, permissions)
            {
                for &(va, frame, old) in pages[..=index].iter().rev() {
                    if self
                        .protect_process_page(pid, entry.address_space, va, frame, old)
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
        let vm = self
            .processes
            .vm_space_mut(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
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
