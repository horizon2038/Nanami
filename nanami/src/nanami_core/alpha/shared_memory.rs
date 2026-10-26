use super::*;
use crate::nanami_core::process::{ProcessEntry, SharedMemoryReservation};

struct SharedMapping {
    entry: ProcessEntry,
    reservation: SharedMemoryReservation,
    copied: usize,
    mapped: usize,
}

impl Alpha {
    pub(super) fn handle_mapping_share_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        let pid = request.identifier;
        let peer_pid = request.arg0;
        let source_va = request.arg1;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        if peer_pid == 0
            || peer_pid == pid
            || source_va == 0
            || source_va & (PAGE_SIZE - 1) != 0
            || request.arg2 == 0
            || request.arg3 & (PAGE_SIZE - 1) != 0
        {
            return Err(CapabilityError::InvalidArgument);
        }
        let pages = request
            .arg2
            .checked_add(PAGE_SIZE - 1)
            .ok_or(CapabilityError::InvalidArgument)?
            / PAGE_SIZE;
        let allocation = self
            .processes
            .find_active_physical_allocation_reference(pid, source_va, pages)
            .ok_or(CapabilityError::PermissionDenied)?;
        let source = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if source.exited
            || self
                .processes
                .find_entry_by_pid(peer_pid)
                .is_none_or(|peer| peer.exited)
        {
            return Err(CapabilityError::InvalidArgument);
        }
        let source = self
            .processes
            .memory_entry(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        let vm = self
            .processes
            .vm_space_mut(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        for page in 0..pages {
            if vm.find_frame(source_va + page * PAGE_SIZE)
                != Some(process_frame_descriptor(
                    source.root_node,
                    allocation.start_slot + page,
                ))
            {
                return Err(CapabilityError::IllegalOperation);
            }
        }
        let mut peer = if request.arg3 == 0 {
            self.reserve_shared_mapping(peer_pid, pages)?
        } else {
            let entry = self
                .processes
                .find_entry_by_pid(peer_pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            if entry.reaper_pid != pid {
                return Err(CapabilityError::PermissionDenied);
            }
            let entry = self
                .processes
                .memory_entry(peer_pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            let end = request
                .arg3
                .checked_add(
                    pages
                        .checked_mul(PAGE_SIZE)
                        .ok_or(CapabilityError::InvalidArgument)?,
                )
                .ok_or(CapabilityError::InvalidArgument)?;
            for va in (request.arg3..end).step_by(PAGE_SIZE) {
                if self.processes.find_lazy_mapping(peer_pid, va).is_some()
                    || self
                        .processes
                        .vm_space_mut(peer_pid)
                        .and_then(|vm| vm.find_frame(va))
                        .is_some()
                {
                    return Err(CapabilityError::IllegalOperation);
                }
            }
            let reservation = self.processes.reserve_shared_memory_at(
                peer_pid,
                pages,
                PROCESS_FRAME_TOTAL_PAGES,
                request.arg3,
            )?;
            SharedMapping {
                entry,
                reservation,
                copied: 0,
                mapped: 0,
            }
        };
        if let Err(error) = self.processes.register_physical_allocation(
            peer_pid,
            peer.reservation.base_va,
            peer.reservation.start_slot,
            allocation.base_page,
            pages,
        ) {
            self.processes
                .recycle_shared_memory(peer_pid, peer.reservation);
            return Err(error);
        }
        let result = (|| {
            self.ensure_process_frame_chunks(
                peer_pid,
                peer.entry.root_node,
                peer.reservation.start_slot,
                pages,
            )?;
            for page in 0..pages {
                let slot = peer.reservation.start_slot + page;
                arch::node::copy(
                    process_frame_chunk_descriptor(
                        peer.entry.root_node,
                        slot / PROCESS_FRAME_CHUNK_PAGES,
                    ),
                    (slot % PROCESS_FRAME_CHUNK_PAGES) as Word,
                    process_frame_descriptor(source.root_node, allocation.start_slot + page),
                )?;
                peer.copied += 1;
            }
            let vm = self
                .processes
                .vm_space_mut(peer_pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            for page in 0..pages {
                let va = peer.reservation.base_va + page * PAGE_SIZE;
                if vm.find_frame(va).is_some() {
                    return Err(CapabilityError::IllegalOperation);
                }
                peer.mapped += 1;
                self.memory.map_frame_strict(
                    peer.entry.address_space,
                    process_frame_descriptor(
                        peer.entry.root_node,
                        peer.reservation.start_slot + page,
                    ),
                    va,
                    vm,
                )?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            if let Err(cleanup) = self.remove_shared_mapping(&mut peer) {
                error!(
                    "[shm.err] share rollback pid={} va={:#x} err={:?}",
                    peer_pid, peer.reservation.base_va, cleanup
                );
            }
            return Err(error);
        }
        Ok((source_va, peer.reservation.base_va))
    }

    pub(super) fn handle_shared_memory_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        let pid = request.identifier;
        let peer_pid = request.arg0;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        if peer_pid == pid || request.arg1 == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let bytes = request
            .arg1
            .checked_add(PAGE_SIZE - 1)
            .ok_or(CapabilityError::InvalidArgument)?
            & !(PAGE_SIZE - 1);
        let pages = bytes / PAGE_SIZE;
        let caller = self.reserve_shared_mapping(pid, pages)?;
        let mut mappings = Vec::with_capacity(2);
        mappings.push(caller);
        if peer_pid != 0 {
            match self.reserve_shared_mapping(peer_pid, pages) {
                Ok(peer) => mappings.push(peer),
                Err(error) => {
                    self.processes
                        .recycle_shared_memory(pid, mappings[0].reservation);
                    return Err(error);
                }
            }
        }
        let base_page = match self.memory.allocate_physical_any(bytes) {
            Ok(page) => page,
            Err(error) => {
                for mapping in &mappings {
                    self.processes
                        .recycle_shared_memory(mapping.entry.pid, mapping.reservation);
                }
                return Err(error);
            }
        };
        // Keep a physical reference for each side until all its copies have
        // been removed, including when installation needs to be rolled back.
        for mapping in &mappings {
            let r = mapping.reservation;
            self.processes.register_physical_allocation(
                mapping.entry.pid,
                r.base_va,
                r.start_slot,
                base_page,
                pages,
            )?;
        }
        if let Err(error) = self.install_shared_memory(&mut mappings, base_page) {
            for mapping in &mut mappings {
                if let Err(cleanup_error) = self.remove_shared_mapping(mapping) {
                    // Fail closed: retain the reference and reservation when
                    // unmapping/removing a cap fails. Never recycle live RAM.
                    error!(
                        "[shm.err] rollback pid={} va={:#x} err={:?}",
                        mapping.entry.pid, mapping.reservation.base_va, cleanup_error
                    );
                }
            }
            return Err(error);
        }
        let caller_va = mappings[0].reservation.base_va;
        let peer_va = mappings
            .get(1)
            .map_or(0, |mapping| mapping.reservation.base_va);
        info!("[shm] granted pid={:>3}<->pid={:>3} size={:#x} paddr={:#018x} local={:#018x} peer={:#018x}",
            pid, peer_pid, bytes, base_page * PAGE_SIZE, caller_va, peer_va);
        Ok((caller_va, peer_va))
    }

    fn reserve_shared_mapping(
        &mut self,
        pid: usize,
        pages: usize,
    ) -> Result<SharedMapping, CapabilityError> {
        let entry = self
            .processes
            .memory_entry(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        let reservation =
            self.processes
                .reserve_shared_memory(pid, pages, PROCESS_FRAME_TOTAL_PAGES)?;
        Ok(SharedMapping {
            entry,
            reservation,
            copied: 0,
            mapped: 0,
        })
    }

    fn install_shared_memory(
        &mut self,
        mappings: &mut [SharedMapping],
        base_page: usize,
    ) -> Result<(), CapabilityError> {
        let pages = mappings[0].reservation.page_count;
        for mapping in mappings.iter() {
            self.ensure_process_frame_chunks(
                mapping.entry.pid,
                mapping.entry.root_node,
                mapping.reservation.start_slot,
                pages,
            )?;
        }
        for page in 0..pages {
            // Materialize each physical frame once, then copy to both peers.
            self.memory
                .ensure_alpha_frame_at_physical_index(base_page + page)?;
            let source = self
                .memory
                .physical_frame_descriptor_from_index(base_page + page)
                .ok_or(CapabilityError::InvalidArgument)?;
            for mapping in mappings.iter_mut() {
                let slot = mapping.reservation.start_slot + page;
                arch::node::copy(
                    process_frame_chunk_descriptor(
                        mapping.entry.root_node,
                        slot / PROCESS_FRAME_CHUNK_PAGES,
                    ),
                    (slot % PROCESS_FRAME_CHUNK_PAGES) as Word,
                    source,
                )?;
                mapping.copied += 1;
            }
        }
        // Recycled physical pages must not disclose a previous owner's data.
        self.zero_process_frames(
            mappings[0].entry.root_node,
            mappings[0].reservation.start_slot,
            pages,
        )?;
        for mapping in mappings.iter_mut() {
            let vm = self
                .processes
                .vm_space_mut(mapping.entry.pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            for page in 0..pages {
                let frame = process_frame_descriptor(
                    mapping.entry.root_node,
                    mapping.reservation.start_slot + page,
                );
                let va = mapping.reservation.base_va + page * PAGE_SIZE;
                // A fixed-address mapping can be ahead of the heap watermark.
                // Reject overlap before marking the page as ours for rollback.
                if vm.find_frame(va).is_some() {
                    return Err(CapabilityError::IllegalOperation);
                }
                // A failed map may have installed its PTE before failing a TLB
                // update; rollback must attempt to unmap this page as well.
                mapping.mapped += 1;
                self.memory
                    .map_frame_strict(mapping.entry.address_space, frame, va, vm)?;
            }
        }
        Ok(())
    }

    pub(super) fn release_shared_memory(
        &mut self,
        entry: ProcessEntry,
        reservation: SharedMemoryReservation,
    ) -> Result<(), CapabilityError> {
        self.remove_shared_mapping(&mut SharedMapping {
            entry,
            reservation,
            copied: reservation.page_count,
            mapped: reservation.page_count,
        })
    }

    fn remove_shared_mapping(
        &mut self,
        mapping: &mut SharedMapping,
    ) -> Result<(), CapabilityError> {
        let r = mapping.reservation;
        let entry = mapping.entry;
        self.invalidate_process_copy_mappings(entry.pid)?;
        while mapping.mapped != 0 {
            let page = mapping.mapped - 1;
            let va = r.base_va + page * PAGE_SIZE;
            let frame = process_frame_descriptor(entry.root_node, r.start_slot + page);
            if self
                .processes
                .vm_space_mut(entry.pid)
                .is_none_or(|vm| vm.permissions(va) != 0)
            {
                arch::address_space::unmap(entry.address_space, frame, va)?;
            }
            if let Some(vm) = self.processes.vm_space_mut(entry.pid) {
                vm.forget_frame(va);
            }
            mapping.mapped -= 1;
        }
        while mapping.copied != 0 {
            let slot = r.start_slot + mapping.copied - 1;
            // Remove this copy only, not the Alpha source or the peer's copy.
            arch::node::remove(
                process_frame_chunk_descriptor(entry.root_node, slot / PROCESS_FRAME_CHUNK_PAGES),
                (slot % PROCESS_FRAME_CHUNK_PAGES) as Word,
            )?;
            mapping.copied -= 1;
        }
        let (allocation, last) = self.processes.release_physical_allocation_reference(
            entry.pid,
            r.base_va,
            r.page_count,
        )?;
        if last {
            self.memory
                .free_physical(allocation.base_page * PAGE_SIZE, r.page_count * PAGE_SIZE)?;
        }
        self.processes.recycle_shared_memory(entry.pid, r);
        Ok(())
    }
}
