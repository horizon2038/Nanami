use super::*;
use crate::nanami_core::process::{ProcessEntry, SharedMemoryReservation};
use crate::nanami_core::vm_space::{FRAME_WORD_BYTES, FRAME_WORD_PAGES};

struct SharedMapping {
    entry: ProcessEntry,
    reservation: SharedMemoryReservation,
    copied: usize,
    mapped: usize,
}

impl Alpha {
    /// A supervisor already has checked read authority. A view additionally
    /// pins one ordinary RAM allocation through the permanent Alpha pool Frame.
    /// Only the mapped VA is returned to the trusted native supervisor.
    /// It deliberately does not follow later changes in the source VM.
    pub(super) fn handle_process_read_view_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        self.handle_process_page_view_request(request, false)
    }

    pub(super) fn handle_process_page_view_request(
        &mut self,
        request: OsRequestEvent,
        writable: bool,
    ) -> Result<(), CapabilityError> {
        let caller = request.identifier;
        let source_pid = request.arg0;
        let source_va = request.arg1;
        let local_va = request.arg2;
        if caller == 0
            || source_pid == caller
            || source_va == 0
            || local_va == 0
            || (source_va | local_va) & (PAGE_SIZE - 1) != 0
            || request.arg3 != 0
        {
            return Err(CapabilityError::InvalidArgument);
        }
        let source = self
            .processes
            .find_entry_by_pid(source_pid)
            .filter(|entry| !entry.exited)
            .ok_or(CapabilityError::InvalidArgument)?;
        if source.reaper_pid != caller || !source.capabilities_sealed {
            return Err(CapabilityError::PermissionDenied);
        }
        let caller_entry = self
            .processes
            .find_entry_by_pid(caller)
            .filter(|entry| !entry.exited)
            .ok_or(CapabilityError::InvalidArgument)?;
        if caller_entry.capabilities_sealed {
            return Err(CapabilityError::PermissionDenied);
        }
        // Exact one-page private ownership excludes MMIO and shared-buffer
        // reservations. Do not invent suballocations inside larger extents.
        let allocation = self
            .processes
            .find_active_physical_allocation_reference(source_pid, source_va, 1)
            .filter(|_| {
                self.processes
                    .shared_memory_reservation(source_pid, source_va, 1)
                    .is_none()
            })
            .ok_or(CapabilityError::PermissionDenied)?;
        let owner = self
            .processes
            .memory_entry(source_pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        self.memory
            .ensure_alpha_frame_at_physical_index(allocation.base_page)?;
        let frame = self
            .memory
            .physical_frame_descriptor_from_index(allocation.base_page)
            .ok_or(CapabilityError::InvalidArgument)?;
        let vm = self
            .processes
            .vm_space_mut(source_pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        let child = process_frame_descriptor(owner.root_node, allocation.start_slot);
        let permissions = if writable { 3 } else { 1 };
        if vm.permissions(source_va) & permissions != permissions
            || (writable && vm.private_page_shared(source_va))
            || !vm
                .find_frame(source_va)
                .is_some_and(|current| current == frame || current == child)
        {
            return Err(CapabilityError::PermissionDenied);
        }
        let mut mapping = self.reserve_shared_mapping_at(caller, 1, local_va)?;
        if let Err(error) = self.processes.register_pool_allocation(
            caller,
            local_va,
            mapping.reservation.start_slot,
            allocation.base_page,
            1,
        ) {
            self.processes
                .recycle_shared_memory(caller, mapping.reservation);
            return Err(error);
        }
        let result = (|| {
            self.process_root_slot(mapping.entry.root_node)?;
            let vm = self
                .processes
                .vm_space_mut(caller)
                .ok_or(CapabilityError::InvalidArgument)?;
            self.memory
                .ensure_page_tables(mapping.entry.address_space, local_va, vm)?;
            // Track a possibly installed PTE before the fallible MAP. Normal
            // release can then retry cleanup even after an ambiguous failure.
            vm.record_frame(local_va, frame)
                .map_err(|_| CapabilityError::Fatal)?;
            vm.set_permissions(local_va, permissions);
            mapping.mapped = 1;
            arch::address_space::map(
                mapping.entry.address_space,
                frame,
                local_va,
                nun::capability_call::address_space::Attribute::from_bits(permissions)
                    .ok_or(CapabilityError::InvalidArgument)?,
            )
        })();
        if let Err(error) = result {
            if let Err(cleanup) = self.remove_shared_mapping(&mut mapping) {
                error!(
                    "[view.err] rollback pid={} va={:#x} err={:?}",
                    caller, local_va, cleanup
                );
                return Err(CapabilityError::Fatal);
            }
            return Err(error);
        }
        #[cfg(not(test))]
        if option_env!("NANAMI_SHARED_POOL_DIAG") == Some("1") {
            use core::sync::atomic::{AtomicUsize, Ordering};
            static VIEWS: AtomicUsize = AtomicUsize::new(0);
            if VIEWS.fetch_add(1, Ordering::Relaxed) < 16 {
                crate::force_info!("[view.pool] writable={} copied={}", writable, mapping.copied);
            }
        }
        Ok(())
    }

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
        let end = source_va
            .checked_add(pages * PAGE_SIZE)
            .ok_or(CapabilityError::InvalidArgument)?;
        if self.processes.immutable_source_overlapping_range(pid, source_va, end - source_va) {
            return Err(CapabilityError::PermissionDenied);
        }
        if self
            .processes
            .vm_space_mut(pid)
            .is_some_and(|vm| vm.private_range_shared(source_va, end))
        {
            // Exporting a writable alias also requires explicit privatization.
            return Err(CapabilityError::IllegalOperation);
        }
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
            let expected = self
                .memory
                .physical_frame_descriptor_from_index(allocation.base_page + page)
                .ok_or(CapabilityError::InvalidArgument)?;
            // Both descriptors name the registered physical object; ordinary
            // private RAM still records its existing child capability.
            let child = process_frame_descriptor(source.root_node, allocation.start_slot + page);
            let current = vm.find_frame(source_va + page * PAGE_SIZE);
            if current != Some(expected) && current != Some(child) {
                return Err(CapabilityError::IllegalOperation);
            }
        }
        let mut peer = if request.arg3 == 0 {
            self.reserve_shared_mapping(peer_pid, pages, false)?
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
            if self.processes.has_lazy_mapping_at_pages(peer_pid, request.arg3, end)
                || self.processes.vm_space_mut(peer_pid)
                    .is_some_and(|vm| vm.has_backed_aligned_pages(request.arg3, end))
            {
                return Err(CapabilityError::IllegalOperation);
            }
            let reservation = self.processes.reserve_owned_pool_memory_at(
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
        if let Err(error) = self.processes.register_pool_allocation(
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
            // The PTEs use permanent pool Frames. The returned API contains
            // only VAs; retain RAM through this peer's physical reference,
            // without creating unused child Frame capabilities or their nodes.
            self.process_root_slot(peer.entry.root_node)?;
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
                // Bulk sharing has the same known span as fresh shared RAM.
                // Prepare optional index storage without publishing a Frame.
                if page == 0 || (va >> 12) & 63 == 0 { vm.prepare_frame_word(va, pages - page); }
                if let Err(error) = self.memory.map_frame_strict(
                    peer.entry.address_space,
                    self.memory
                        .physical_frame_descriptor_from_index(allocation.base_page + page)
                        .ok_or(CapabilityError::InvalidArgument)?,
                    va,
                    vm,
                ) {
                    vm.mark_mapping_uncertain(va);
                    return Err(error);
                }
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
        #[cfg(not(test))]
        if option_env!("NANAMI_SHARED_POOL_DIAG") == Some("1") {
            use core::sync::atomic::{AtomicUsize, Ordering};
            static SHARES: AtomicUsize = AtomicUsize::new(0);
            if SHARES.fetch_add(1, Ordering::Relaxed) < 16 {
                crate::force_info!("[share.pool] pages={} fixed={} copied={}", pages, request.arg3 != 0, peer.copied);
            }
        }
        Ok((source_va, peer.reservation.base_va))
    }

    pub(super) fn handle_shared_memory_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        self.create_shared_memory(request, false)
    }

    pub(super) fn handle_memory_source_request(&mut self, request: OsRequestEvent)
        -> Result<(usize, usize), CapabilityError> {
        if request.arg0 != 0 || request.arg2 == 0 || request.arg3 == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        self.processes.find_entry_by_pid(request.identifier)
            .filter(|entry| !entry.exited && !entry.resource_quarantined && !entry.capabilities_sealed)
            .ok_or(CapabilityError::PermissionDenied)?;
        self.create_shared_memory(request, true)
    }

    fn create_shared_memory(&mut self, request: OsRequestEvent, source_only: bool)
        -> Result<(usize, usize), CapabilityError> {
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
        // Nonzero initializer arguments create owned, readonly RAM. Never
        // permit a writable peer alias of an immutable snapshot constructor.
        let initializer = if request.arg2 != 0 || request.arg3 != 0 {
            if peer_pid != 0 { return Err(CapabilityError::InvalidArgument); }
            let source = PageSource { pid, address: request.arg2, bytes: request.arg3 };
            self.validate_owned_memory_initializer(source, bytes)?;
            Some(source)
        } else { None };
        // MAP already uses the permanent pool Frame. Separate physical
        // references pin each side; per-process Frame copies are unnecessary.
        let mut caller = self.reserve_shared_mapping(pid, pages, false)?;
        if source_only {
            caller.reservation = self.processes.mark_immutable_memory_source(pid, caller.reservation)?;
        }
        let mut mappings = Vec::with_capacity(2);
        mappings.push(caller);
        if peer_pid != 0 {
            match self.reserve_shared_mapping(peer_pid, pages, false) {
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
            if r.owns_frame_caps() {
                self.processes.register_physical_allocation(mapping.entry.pid, r.base_va, r.start_slot, base_page, pages)?;
            } else {
                self.processes.register_pool_allocation(mapping.entry.pid, r.base_va, r.start_slot, base_page, pages)?;
            }
        }
        if let Err(error) = self.install_shared_memory(&mut mappings, base_page, initializer) {
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
        #[cfg(not(test))]
        if option_env!("NANAMI_SHARED_POOL_DIAG") == Some("1") && peer_pid != 0 {
            use core::sync::atomic::{AtomicUsize, Ordering};
            static BUFFERS: AtomicUsize = AtomicUsize::new(0);
            if BUFFERS.fetch_add(1, Ordering::Relaxed) < 16 {
                let copies: usize = mappings.iter().map(|mapping| mapping.copied).sum();
                crate::force_info!("[shm.pool] pages={} owners={} copied={}", pages, mappings.len(), copies);
            }
        }
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
        owns_frame_caps: bool,
    ) -> Result<SharedMapping, CapabilityError> {
        let entry = self
            .processes
            .memory_entry(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        let reservation = if owns_frame_caps {
            self.processes.reserve_shared_memory(pid, pages, PROCESS_FRAME_TOTAL_PAGES)?
        } else {
            self.processes.reserve_owned_pool_memory(pid, pages, PROCESS_FRAME_TOTAL_PAGES)?
        };
        Ok(SharedMapping {
            entry,
            reservation,
            copied: 0,
            mapped: 0,
        })
    }

    fn reserve_shared_mapping_at(
        &mut self,
        pid: usize,
        pages: usize,
        base: usize,
    ) -> Result<SharedMapping, CapabilityError> {
        let entry = self
            .processes
            .memory_entry(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        let end = pages
            .checked_mul(PAGE_SIZE)
            .and_then(|bytes| base.checked_add(bytes))
            .ok_or(CapabilityError::InvalidArgument)?;
        for va in (base..end).step_by(PAGE_SIZE) {
            if self.processes.find_lazy_mapping(pid, va).is_some()
                || self
                    .processes
                    .vm_space_mut(pid)
                    .and_then(|vm| vm.find_frame(va))
                    .is_some()
            {
                return Err(CapabilityError::IllegalOperation);
            }
        }
        let reservation =
            self.processes
                .reserve_owned_pool_memory_at(pid, pages, PROCESS_FRAME_TOTAL_PAGES, base)?;
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
        initializer: Option<PageSource>,
    ) -> Result<(), CapabilityError> {
        let pages = mappings[0].reservation.page_count;
        for mapping in mappings.iter() {
            if mapping.reservation.owns_frame_caps() { self.ensure_process_frame_chunks(
                mapping.entry.pid,
                mapping.entry.root_node,
                mapping.reservation.start_slot,
                pages,
            )?; } else { self.process_root_slot(mapping.entry.root_node)?; }
        }
        let owns_caps = mappings.iter().any(|mapping| mapping.reservation.owns_frame_caps());
        // Every side already owns its reference, but no PTE has been published.
        // A zero-filled shared buffer can use the same temporary huge Frame as
        // a private snapshot; demotion finishes before exporting real 4K Frames.
        let initialized = if !owns_caps && (initializer.is_none() || mappings.len() == 1) {
            self.initialize_owned_with_large_frames(base_page, pages, initializer)?
        } else { false };
        if owns_caps {
            let frame_hint = self.memory.allocation_frame_hint(base_page, pages);
            for page in 0..pages {
                self.memory.ensure_alpha_frame_for_allocation(base_page + page, frame_hint)?;
                let source = self.memory.physical_frame_descriptor_from_index(base_page + page)
                    .ok_or(CapabilityError::InvalidArgument)?;
                for mapping in mappings.iter_mut().filter(|mapping| mapping.reservation.owns_frame_caps()) {
                    let slot = mapping.reservation.start_slot + page;
                    arch::node::copy(
                        process_frame_chunk_descriptor(mapping.entry.root_node, slot / PROCESS_FRAME_CHUNK_PAGES),
                        (slot % PROCESS_FRAME_CHUNK_PAGES) as Word,
                        source,
                    )?;
                    mapping.copied += 1;
                }
            }
        } else if !initialized {
            self.memory.ensure_alpha_frames_for_allocation(base_page, pages)?;
        }
        // Recycled physical pages must not disclose a previous owner's data.
        if !initialized {
            if let Some(source) = initializer {
                self.initialize_owned_physical_pages(base_page, pages, source)?;
            } else {
                self.zero_physical_pages(base_page, pages)?;
            }
        }
        let attributes = nun::capability_call::address_space::Attribute::from_bits(
            if initializer.is_some() { 1 } else { 7 },
        ).ok_or(CapabilityError::InvalidArgument)?;
        for mapping in mappings.iter_mut() {
            let vm = self
                .processes
                .vm_space_mut(mapping.entry.pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            while mapping.mapped < pages {
                let page = mapping.mapped;
                let va = mapping.reservation.base_va + page * PAGE_SIZE;
                if mapping.reservation.immutable_source()
                    && va & (FRAME_WORD_BYTES - 1) == 0
                    && pages - page >= FRAME_WORD_PAGES
                {
                    let mut frames = [0; FRAME_WORD_PAGES];
                    self.memory.read_ready_pool_frames(base_page + page, &mut frames)?;
                    vm.record_immutable_source_frame_word(va, &frames)
                        .map_err(|_| CapabilityError::IllegalOperation)?;
                    mapping.mapped += FRAME_WORD_PAGES;
                    continue;
                }
                let frame = self
                    .memory
                    .physical_frame_descriptor_from_index(base_page + page)
                    .ok_or(CapabilityError::InvalidArgument)?;
                // A fixed-address mapping can be ahead of the heap watermark.
                // Reject overlap before marking the page as ours for rollback.
                if vm.find_frame(va).is_some()
                    || (mapping.reservation.immutable_source() && vm.mapping_is_uncertain(va))
                {
                    return Err(CapabilityError::IllegalOperation);
                }
                // A failed map may have installed its PTE before failing a TLB
                // update; rollback must attempt to unmap this page as well.
                mapping.mapped += 1;
                if page == 0 || (va >> 12) & 63 == 0 { vm.prepare_frame_word(va, pages - page); }
                if mapping.reservation.immutable_source() {
                    // This VA is a backing identity, not a native pointer.
                    // Record ownership/absence for ordinary release and make
                    // privileged writes fail even before a guest alias exists.
                    vm.record_frame(va, frame).map_err(|_| CapabilityError::Fatal)?;
                    vm.set_permissions(va, 0);
                    vm.mark_private_page_shared(va, true);
                    // Frame metadata alone must not imply a leaf table exists.
                    vm.invalidate_page_table_region(va);
                    continue;
                }
                if let Err(error) = self.memory.map_frame_strict_with_attribute(mapping.entry.address_space, frame, va, attributes, vm) {
                    vm.mark_mapping_uncertain(va);
                    return Err(error);
                }
                // Memory records Frame identity; this transaction owns logical
                // rights. Cache admission must see the same readonly contract
                // as the successfully installed leaf, before returning RAM.
                if initializer.is_some() { vm.set_permissions(va, 1); }
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
            copied: if reservation.owns_frame_caps() { reservation.page_count } else { 0 },
            mapped: reservation.page_count,
        })
    }

    fn remove_shared_mapping(
        &mut self,
        mapping: &mut SharedMapping,
    ) -> Result<(), CapabilityError> {
        let r = mapping.reservation;
        let entry = mapping.entry;
        // Ordinary whole extents, including the one-page view-release hot path,
        // retain their original accounting. Only explicitly canonicalized RAM
        // owns a complete set of single-page references instead of one extent.
        let canonical_pages = if r.has_page_references() {
            let end = r
                .page_count
                .checked_mul(PAGE_SIZE)
                .and_then(|bytes| r.base_va.checked_add(bytes))
                .ok_or(CapabilityError::InvalidArgument)?;
            let pages = self
                .processes
                .physical_pages_in_range(entry.pid, r.base_va, end);
            if pages.len() != r.page_count
                || pages.iter().enumerate().any(|(offset, page)| {
                    page.base_va != r.base_va + offset * PAGE_SIZE
                        || page.start_slot != r.start_slot + offset
                        || page.owns_frame_cap != r.owns_frame_caps()
                        || self.processes.physical_reference_count(*page) == 0
                })
            {
                // Validate before any UNMAP or capability removal. A damaged
                // owner set is quarantined, not mistaken for already freed RAM.
                return Err(CapabilityError::IllegalOperation);
            }
            Some((pages, end))
        } else {
            None
        };
        let pool_extent = if !r.owns_frame_caps() && !r.has_page_references() {
            Some(self.processes.find_active_physical_allocation_reference(entry.pid, r.base_va, r.page_count)
                .filter(|allocation| !allocation.owns_frame_cap && allocation.start_slot == r.start_slot)
                .ok_or(CapabilityError::IllegalOperation)?)
        } else { None };
        self.invalidate_process_copy_mappings(entry.pid)?;
        // Synchronous UNMAP calls cannot dispatch another Alpha process event;
        // the VM identity stays fixed throughout this cleanup. Empty mappings
        // still require copy invalidation, but need no VM lookup.
        let mut vm = if mapping.mapped == 0 {
            None
        } else {
            self.processes.vm_space_mut(entry.pid)
        };
        while mapping.mapped != 0 {
            let page = mapping.mapped - 1;
            let va = r.base_va + page * PAGE_SIZE;
            if vm
                .as_ref()
                .is_none_or(|vm| vm.frame_may_be_present(va))
            {
                let frame = if r.owns_frame_caps() {
                    process_frame_descriptor(entry.root_node, r.start_slot + page)
                } else {
                    let physical = if let Some((pages, _)) = &canonical_pages { pages[page].base_page }
                        else { pool_extent.ok_or(CapabilityError::Fatal)?.base_page + page };
                    self.memory.physical_frame_descriptor_from_index(physical)
                        .ok_or(CapabilityError::Fatal)?
                };
                if let Err(error) = arch::address_space::unmap(entry.address_space, frame, va) {
                    if let Some(vm) = vm.as_mut() {
                        vm.mark_mapping_uncertain(va);
                    }
                    return Err(error);
                }
            }
            if let Some(vm) = vm.as_mut() {
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
        if let Some((pages, end)) = canonical_pages {
            let memory = &mut self.memory;
            let retired = self.processes.try_retire_physical_pages(
                entry.pid, r.base_va, end, &pages, |page, last| {
                    if last { memory.free_physical(page.base_page * PAGE_SIZE, PAGE_SIZE) }
                    else { Ok(()) }
                },
            )?;
            if !retired {
                for page in pages.into_iter().rev() {
                    if self.processes.physical_reference_count(page) == 1 {
                        // Keep the reference until the allocator confirms release.
                        self.memory
                            .free_physical(page.base_page * PAGE_SIZE, PAGE_SIZE)?;
                    }
                    self.processes
                        .release_physical_allocation_reference(entry.pid, page.base_va, 1)?;
                }
            }
        } else {
            let memory = &mut self.memory;
            self.processes.retire_active_physical_allocation(entry.pid, r.base_va, r.page_count, |allocation, last| {
                if last { memory.free_physical(allocation.base_page * PAGE_SIZE, r.page_count * PAGE_SIZE) }
                else { Ok(()) }
            })?;
        }
        self.processes.recycle_shared_memory(entry.pid, r);
        Ok(())
    }
}
