//! Private VM copies, preserving sparse backing and per-page protection.
use super::*;

impl Alpha {
    pub(super) fn clone_process_vm(
        &mut self,
        source_pid: usize,
        destination_pid: usize,
    ) -> Result<(), CapabilityError> {
        let vm = self
            .processes
            .vm_space_mut(source_pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if vm.has_uncertain_mappings(0, usize::MAX) {
            return Err(CapabilityError::Fatal);
        }
        let pages = vm.backed_pages(0, usize::MAX);
        let fresh = self
            .processes
            .vm_space_mut(destination_pid)
            .is_some_and(|vm| vm.backed_pages(0, usize::MAX).is_empty());
        if !fresh {
            return self.clone_backed_pages(source_pid, 0, destination_pid, 0, pages);
        }
        let destination = self
            .processes
            .memory_entry(destination_pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        // A raw private child is suspended and initially empty. Reserve each
        // backed run once, but copy every page into independent storage. Never
        // include sparse holes, including unbacked PROT_NONE reservations.
        for run in pages.chunk_by(|a, b| a.0.checked_add(PAGE_SIZE) == Some(b.0)) {
            let base = run[0].0;
            self.map_process_initialized_pages(
                destination_pid,
                Some(base),
                run.len(),
                Some(PageSource {
                    pid: source_pid,
                    address: base,
                    bytes: run.len() * PAGE_SIZE,
                }),
            )?;
            // Publish no runnable child until all final permissions succeed.
            // Failed construction/protection remains owned by spawn rollback.
            for &(va, permissions) in run {
                let frame = self.process_frame_for_page(destination_pid, va)?;
                self.protect_process_page(
                    destination_pid,
                    destination.address_space,
                    va,
                    frame,
                    permissions,
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn clone_backed_memory(
        &mut self,
        source_pid: usize,
        base_va: usize,
        destination_pid: usize,
        target_va: usize,
        size_bytes: usize,
    ) -> Result<(), CapabilityError> {
        let end = base_va
            .checked_add(size_bytes)
            .ok_or(CapabilityError::InvalidArgument)?;
        target_va
            .checked_add(size_bytes)
            .ok_or(CapabilityError::InvalidArgument)?;
        if (base_va | target_va | size_bytes) & (PAGE_SIZE - 1) != 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let vm = self
            .processes
            .vm_space_mut(source_pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if vm.has_uncertain_mappings(base_va, end) {
            return Err(CapabilityError::Fatal);
        }
        let mut pages = vm.backed_pages(base_va, end);
        if self.processes.memory_owner_pid(source_pid)
            == self.processes.memory_owner_pid(destination_pid)
            && target_va > base_va
        {
            pages.reverse();
        }
        self.clone_backed_pages(source_pid, base_va, destination_pid, target_va, pages)
    }

    fn clone_backed_pages(
        &mut self,
        source_pid: usize,
        base_va: usize,
        destination_pid: usize,
        target_va: usize,
        pages: Vec<(usize, usize)>,
    ) -> Result<(), CapabilityError> {
        let destination = self
            .processes
            .memory_entry(destination_pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        // Sparse reservations stay sparse, including PROT_NONE. Work scales with
        // populated pages, not the size of a reserved virtual address range.
        for (va, permissions) in pages {
            let target = target_va + va - base_va;
            if self
                .processes
                .vm_space_mut(destination_pid)
                .and_then(|vm| vm.find_frame(target))
                .is_none()
            {
                self.map_process_initialized_pages(
                    destination_pid,
                    Some(target),
                    1,
                    Some(PageSource {
                        pid: source_pid,
                        address: va,
                        bytes: PAGE_SIZE,
                    }),
                )?;
            } else {
                self.copy_process_memory(source_pid, va, destination_pid, target, PAGE_SIZE)?;
            }
            let frame = self.process_frame_for_page(destination_pid, target)?;
            self.protect_process_page(
                destination_pid,
                destination.address_space,
                target,
                frame,
                permissions,
            )?;
        }
        Ok(())
    }
}
