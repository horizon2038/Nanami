//! Private RAM construction: initialize each byte once before publishing pages.
use super::*;

pub(super) const REUSE_INITIALIZATION_SOURCE: bool = match option_env!("NANAMI_INIT_SOURCE_REUSE") {
    Some(value) => value.len() == 1 && value.as_bytes()[0] == b'1',
    None => true,
};
pub(super) const SWAP_INITIALIZATION_ROLE: bool = match option_env!("NANAMI_INIT_SOURCE_ROLE_SWAP") {
    Some(value) => value.len() == 1 && value.as_bytes()[0] == b'1',
    None => true,
};

#[derive(Clone, Copy)]
pub(super) struct PageSource {
    pub pid: usize,
    pub address: usize,
    pub bytes: usize,
}

// The unpublished suffix belongs to this synchronous spawn until registered.
pub(super) struct PreinstallPoolBatch {
    pid: usize,
    root: CapabilityDescriptor,
    base: usize,
    first: usize,
    count: usize,
    claimed: usize,
}

// Request-local only: Alpha handles no other process event while constructing
// these pages, so the source memory owner's root cannot change before return.
// Frame identity, permissions, sharing and uncertain mappings remain live
// checks at each use; this snapshot confers no authority by itself.
pub(super) struct InitializationSource {
    pub(super) pid: usize,
    pub(super) root: CapabilityDescriptor,
    pub(super) pages: alloc::vec::Vec<crate::nanami_core::process::ProcessPhysicalAllocation>,
    // Only an index into this immutable, sorted, unambiguous snapshot. Each
    // hit compares the requested VA; all live VM/Frame checks remain outside.
    cursor: core::cell::Cell<usize>,
}

#[cfg(test)]
static SOURCE_REFERENCE_PROBES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(super) fn take_source_reference_probes() -> usize {
    SOURCE_REFERENCE_PROBES.swap(0, core::sync::atomic::Ordering::Relaxed)
}

fn source_reference_key(allocation: &crate::nanami_core::process::ProcessPhysicalAllocation) -> usize {
    #[cfg(test)]
    SOURCE_REFERENCE_PROBES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    allocation.base_va
}

impl InitializationSource {
    pub(super) fn new(
        pid: usize, root: CapabilityDescriptor,
        pages: alloc::vec::Vec<crate::nanami_core::process::ProcessPhysicalAllocation>,
    ) -> Self {
        Self { pid, root, pages, cursor: core::cell::Cell::new(0) }
    }

    #[inline]
    pub(super) fn find_page(&self, page: usize)
        -> Option<&crate::nanami_core::process::ProcessPhysicalAllocation>
    {
        let current = self.cursor.get();
        if let Some(allocation) = self.pages.get(current) {
            if source_reference_key(allocation) == page {
                return Some(allocation);
            }
        }
        if let Some(next) = current.checked_add(1) {
            if let Some(allocation) = self.pages.get(next) {
                if source_reference_key(allocation) == page {
                    self.cursor.set(next);
                    return Some(allocation);
                }
            }
        }
        let index = self.pages.binary_search_by_key(&page, source_reference_key).ok()?;
        self.cursor.set(index);
        Some(&self.pages[index])
    }
}

// Consumed within one output-page construction. Between resolution and copy
// only Alpha's temporary mappings change, never the process VM/source binding.
// No content, physical ownership or alias address is cached here.
struct PreparedSourceFrame {
    pid: usize,
    page: usize,
    frame: CapabilityDescriptor,
}

pub(super) fn prepared_pool_allowed(raw: bool, auto_resume: bool, final_permissions: bool) -> bool {
    cfg!(target_arch = "x86_64") && raw && !auto_resume && final_permissions
}

impl Alpha {
    pub(super) fn prepare_preinstall_pool_batch(
        &mut self, pid: usize, root: CapabilityDescriptor, base: usize, count: usize,
    ) -> Result<Option<PreinstallPoolBatch>, CapabilityError> {
        if count < 2 {
            return Ok(None);
        }
        let bytes = count.checked_mul(PAGE_SIZE).ok_or(CapabilityError::InvalidArgument)?;
        if pid == 0 || self.processes.find_entry_by_pid(pid).is_some()
            || self.processes.memory_owner_pid(pid) != pid
            || base & (PAGE_SIZE - 1) != 0 || count > PROCESS_FRAME_TOTAL_PAGES
            || base.checked_add(bytes).is_none()
        { return Err(CapabilityError::InvalidArgument); }
        self.process_root_slot(root)?;
        match self.memory.allocate_physical_any(bytes) {
            Ok(first) => Ok(Some(PreinstallPoolBatch { pid, root, base, first, count, claimed: 0 })),
            Err(CapabilityError::InvalidArgument) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(super) fn claim_preinstall_pool_frame(
        &mut self, pid: usize, root: CapabilityDescriptor, va: usize, slot: usize,
        batch: &mut PreinstallPoolBatch,
    ) -> Result<(usize, CapabilityDescriptor), CapabilityError> {
        if pid == 0 || self.processes.find_entry_by_pid(pid).is_some()
            || self.processes.memory_owner_pid(pid) != pid
            || pid != batch.pid || root != batch.root || slot != batch.claimed
            || batch.claimed >= batch.count || va != batch.base + batch.claimed * PAGE_SIZE
        { return Err(CapabilityError::InvalidArgument); }
        self.process_root_slot(root)?;
        let physical = batch.first + batch.claimed;
        self.processes.register_pool_alias(pid, va, slot, physical)?;
        batch.claimed += 1;
        self.memory.ensure_alpha_frame_at_physical_index(physical)?;
        let frame = self.memory.physical_frame_descriptor_from_index(physical)
            .ok_or(CapabilityError::Fatal)?;
        Ok((physical, frame))
    }

    pub(super) fn release_preinstall_pool_batch_tail(
        &mut self, batch: PreinstallPoolBatch,
    ) -> Result<(), CapabilityError> {
        if batch.claimed < batch.count {
            let address = (batch.first + batch.claimed) * PAGE_SIZE;
            let bytes = (batch.count - batch.claimed) * PAGE_SIZE;
            if self.memory.free_physical(address, bytes).is_err() {
                // Failed free retains RAM. Preserve ownership for the ordinary
                // failed-spawn cleanup/quarantine, including unpublished pages.
                for i in batch.claimed..batch.count {
                    self.processes.register_pool_alias(
                        batch.pid, batch.base + i * PAGE_SIZE, i, batch.first + i,
                    )?;
                }
            }
        }
        Ok(())
    }

    /// The prepared raw spawn owns a fresh root and has not installed or run
    /// the child yet. No management Frame descriptors are part of that reply.
    pub(super) fn allocate_preinstall_pool_frame(
        &mut self, pid: usize, root: CapabilityDescriptor, va: usize, slot: usize,
    ) -> Result<(usize, CapabilityDescriptor), CapabilityError> {
        if pid == 0 || self.processes.find_entry_by_pid(pid).is_some()
            || self.processes.memory_owner_pid(pid) != pid
            || va & (PAGE_SIZE - 1) != 0 || slot >= PROCESS_FRAME_TOTAL_PAGES
        { return Err(CapabilityError::InvalidArgument); }
        self.process_root_slot(root)?;
        let physical = self.memory.allocate_physical_any(PAGE_SIZE)?;
        self.processes.register_pool_alias(pid, va, slot, physical)?;
        self.memory.ensure_alpha_frame_at_physical_index(physical)?;
        let frame = self.memory.physical_frame_descriptor_from_index(physical)
            .ok_or(CapabilityError::Fatal)?;
        Ok((physical, frame))
    }

    /// Image setup may precede process installation. Only an installed sealed
    /// raw owner can omit management caps; native IPC/pre-install stays legacy.
    pub(super) fn allocate_image_page_frame(
        &mut self, pid: usize, root: CapabilityDescriptor, va: usize, slot: usize,
        raw_image: bool,
    ) -> Result<(usize, CapabilityDescriptor), CapabilityError> {
        let owner = self.processes.memory_owner_pid(pid);
        let pool = raw_image && self.processes.find_entry_by_pid(owner)
            .is_some_and(|entry| entry.capabilities_sealed);
        if pool {
            self.process_root_slot(root)?;
            if self.processes.memory_context(pid).is_none_or(|entry| entry.root_node != root) {
                return Err(CapabilityError::InvalidArgument);
            }
            let physical = self.memory.allocate_physical_any(PAGE_SIZE)?;
            self.processes.register_pool_alias(pid, va, slot, physical)?;
            // Any partial conversion remains represented for fatal exec cleanup.
            self.memory.ensure_alpha_frame_at_physical_index(physical)?;
            let frame = self.memory.physical_frame_descriptor_from_index(physical)
                .ok_or(CapabilityError::Fatal)?;
            Ok((physical, frame))
        } else {
            self.ensure_process_frame_chunks(pid, root, slot, 1)?;
            let physical = self.memory.allocate_process_frame(
                process_frame_chunk_descriptor(root, slot / PROCESS_FRAME_CHUNK_PAGES),
                PROCESS_FRAME_NODE_RADIX, slot % PROCESS_FRAME_CHUNK_PAGES,
            )?;
            self.processes.register_physical_allocation(pid, va, slot, physical, 1)?;
            Ok((physical, process_frame_descriptor(root, slot)))
        }
    }

    pub(super) fn map_process_heap_pages(
        &mut self,
        pid: usize,
        page_count: usize,
    ) -> Result<usize, CapabilityError> {
        self.map_process_initialized_pages(pid, None, page_count, None)
    }

    pub(super) fn map_process_heap_pages_at(
        &mut self,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) -> Result<usize, CapabilityError> {
        self.map_process_initialized_pages(pid, Some(base_va), page_count, None)
    }

    pub(super) fn map_process_initialized_pages(
        &mut self,
        pid: usize,
        requested_base: Option<usize>,
        page_count: usize,
        source: Option<PageSource>,
    ) -> Result<usize, CapabilityError> {
        self.map_process_initialized_pages_protected(pid, requested_base, page_count, source, 7)
    }

    pub(super) fn map_process_initialized_pages_protected(
        &mut self,
        pid: usize,
        requested_base: Option<usize>,
        page_count: usize,
        source: Option<PageSource>,
        permissions: usize,
    ) -> Result<usize, CapabilityError> {
        self.map_process_initialized_pages_with_stack_slot(pid, requested_base, page_count, source, permissions, None)
    }

    pub(super) fn map_process_initialized_pages_with_stack_slot(
        &mut self, pid: usize, requested_base: Option<usize>, page_count: usize,
        source: Option<PageSource>, permissions: usize,
        reserved: Option<crate::nanami_core::process::FreshZeroFrameSlot>,
    ) -> Result<usize, CapabilityError> {
        if permissions == 0 || permissions & !7 != 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let attributes = nun::capability_call::address_space::Attribute::from_bits(permissions)
            .ok_or(CapabilityError::InvalidArgument)?;
        let bytes = page_count
            .checked_mul(PAGE_SIZE)
            .ok_or(CapabilityError::InvalidArgument)?;
        if pid == 0 || page_count == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let owner = self.processes.memory_owner_pid(pid);
        let pool_backing = self.processes.find_entry_by_pid(owner)
            .is_some_and(|entry| entry.capabilities_sealed);
        let mut pool_source_only = true;
        if let Some(source) = source {
            if source.bytes > bytes || (source.bytes != 0 && source.address == 0) {
                return Err(CapabilityError::InvalidArgument);
            }
            let end = source
                .address
                .checked_add(source.bytes)
                .ok_or(CapabilityError::InvalidArgument)?;
            // Validate the source before allocating destination backing.
            if source.bytes != 0 {
                for va in (align_down(source.address, PAGE_SIZE)..end).step_by(PAGE_SIZE) {
                    let frame = self.process_frame_for_page(source.pid, va)?;
                    if REUSE_INITIALIZATION_SOURCE {
                        pool_source_only &= self.memory.is_pool_frame_descriptor(frame);
                    }
                }
            }
        }
        // One bounded ownership walk per request, never a full allocation-list
        // search for each copied page. Failure only disables the optimization.
        // Pool Frames can use their exact ready-alias probe directly. This
        // skips only an optional ownership snapshot; each Frame is resolved
        // again when copied, and misses still use CopyWindow.
        let source_pages = if REUSE_INITIALIZATION_SOURCE && !pool_source_only {
            source.filter(|source| source.bytes != 0).and_then(|source| {
                let root = self.processes.memory_root_node(source.pid)?;
                let pages = self.processes.try_physical_pages_in_range(
                    source.pid, align_down(source.address, PAGE_SIZE), source.address + source.bytes, 2048,
                )?;
                Some(InitializationSource::new(source.pid, root, pages))
            })
        } else {
            None
        };
        let (root_node, address_space, base_va, start_slot) = if let Some(reserved) = reserved {
            let base = requested_base.ok_or(CapabilityError::InvalidArgument)?;
            if page_count != 1 || source.is_some() { return Err(CapabilityError::InvalidArgument); }
            let owner = self.processes.memory_owner_pid(pid);
            let (root, space, slot) = reserved.into_parts(owner, base)
                .ok_or(CapabilityError::InvalidArgument)?;
            let entry = self.processes.memory_context(pid).ok_or(CapabilityError::InvalidArgument)?;
            if entry.root_node != root || entry.address_space != space {
                return Err(CapabilityError::InvalidArgument);
            }
            let vm = self.processes.vm_space_mut(pid).ok_or(CapabilityError::InvalidArgument)?;
            if vm.has_uncertain_mappings(base, base + PAGE_SIZE) { return Err(CapabilityError::Fatal); }
            if vm.has_backed_pages(base, base + PAGE_SIZE) { return Err(CapabilityError::InvalidArgument); }
            (root, space, base, slot)
        } else if let Some(base) = requested_base {
            let end = base
                .checked_add(bytes)
                .ok_or(CapabilityError::InvalidArgument)?;
            let vm = self
                .processes
                .vm_space_mut(pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            if vm.has_uncertain_mappings(base, end) {
                return Err(CapabilityError::Fatal);
            }
            if vm.has_backed_pages(base, end) {
                return Err(CapabilityError::InvalidArgument);
            }
            let (root, space, slot) = self.processes.reserve_process_heap_at(
                pid,
                base,
                page_count,
                PAGE_SIZE,
                PROCESS_FRAME_TOTAL_PAGES,
            )?;
            (root, space, base, slot)
        } else {
            self.processes.reserve_process_heap(
                pid,
                page_count,
                PAGE_SIZE,
                PROCESS_FRAME_TOTAL_PAGES,
            )?
        };
        if pool_backing {
            self.process_root_slot(root_node)?;
        } else {
            self.ensure_process_frame_chunks(pid, root_node, start_slot, page_count)?;
        }
        let reuse_allocation = self
            .processes
            .take_deferred_physical_allocation(pid, base_va, page_count);
        if let Some(allocation) = reuse_allocation {
            // Transfer ownership of the entire reused extent before any
            // fallible cap/mapping operation, including its unused suffix.
            for i in 0..page_count {
                if pool_backing {
                    self.processes.register_pool_allocation(pid, base_va + i * PAGE_SIZE,
                        start_slot + i, allocation.base_page + i, 1)?;
                } else {
                    self.processes.register_physical_allocation(pid, base_va + i * PAGE_SIZE,
                        start_slot + i, allocation.base_page + i, 1)?;
                }
            }
        }
        // Until each new page is registered, its suffix belongs to this
        // synchronous request alone and has not been published to the target.
        let mut fresh_batch = None;
        let mut fresh_claimed = 0;
        let result = (|| {
            if reuse_allocation.is_none() && page_count > 1 {
                // Lack of contiguous RAM keeps the existing single-page path.
                // Other errors must reach rollback, not become retries.
                match self.memory.allocate_physical_any(bytes) {
                    Ok(page) => { fresh_batch = Some(page); }
                    Err(CapabilityError::InvalidArgument) => {}
                    Err(error) => return Err(error),
                }
            }
            for i in 0..page_count {
                let slot = start_slot + i;
                let va = base_va + i * PAGE_SIZE;
                let node =
                    process_frame_chunk_descriptor(root_node, slot / PROCESS_FRAME_CHUNK_PAGES);
                let within = slot % PROCESS_FRAME_CHUNK_PAGES;
                let page = if let Some(allocation) = reuse_allocation {
                    let page = allocation.base_page + i;
                    self.memory.ensure_alpha_frame_at_physical_index(page)?;
                    let frame = self
                        .memory
                        .physical_frame_descriptor_from_index(page)
                        .ok_or(CapabilityError::InvalidArgument)?;
                    if !pool_backing { arch::node::copy(node, within, frame)?; }
                    page
                } else if let Some(first) = fresh_batch {
                    let page = first + i;
                    if pool_backing {
                        self.processes.register_pool_allocation(pid, va, slot, page, 1)?;
                    } else {
                        self.processes.register_physical_allocation(pid, va, slot, page, 1)?;
                    }
                    fresh_claimed = i + 1;
                    self.memory.ensure_alpha_frame_at_physical_index(page)?;
                    if !pool_backing {
                        let frame = self.memory.physical_frame_descriptor_from_index(page)
                            .ok_or(CapabilityError::InvalidArgument)?;
                        arch::node::copy(node, within, frame)?;
                    }
                    page
                } else if pool_backing {
                    let page = self.memory.allocate_physical_any(PAGE_SIZE)?;
                    self.processes.register_pool_allocation(pid, va, slot, page, 1)?;
                    self.memory.ensure_alpha_frame_at_physical_index(page)?;
                    page
                } else {
                    let page = self.memory.allocate_process_frame(
                        node,
                        PROCESS_FRAME_NODE_RADIX,
                        within,
                    )?;
                    self.processes
                        .register_physical_allocation(pid, va, slot, page, 1)?;
                    page
                };
                let (swapped, prepared) = if SWAP_INITIALIZATION_ROLE {
                    self.try_swapped_initialization_destination(page, source, i * PAGE_SIZE, source_pages.as_ref())?
                } else {
                    (None, None)
                };
                let destination = match swapped {
                    Some(address) => address,
                    None => self.map_initialization_page(page)?,
                };
                self.initialize_process_page(destination, source, i * PAGE_SIZE, source_pages.as_ref(), prepared)?;
                let frame = if pool_backing {
                    self.memory.physical_frame_descriptor_from_index(page)
                        .ok_or(CapabilityError::Fatal)?
                } else { process_frame_descriptor(root_node, slot) };
                let vm = self
                    .processes
                    .vm_space_mut(pid)
                    .ok_or(CapabilityError::InvalidArgument)?;
                if i == 0 || (va >> 12) & 63 == 0 { vm.prepare_frame_word(va, page_count - i); }
                if let Err(error) = self.memory.map_frame_strict_with_attribute(address_space, frame, va, attributes, vm) {
                    // An error can follow leaf installation or Frame insertion.
                    // Mark unknown presence before fallible metadata recording;
                    // ownership also identifies backing without a Frame record.
                    vm.mark_mapping_uncertain(va);
                    vm.record_frame(va, frame)
                        .map_err(|_| CapabilityError::Fatal)?;
                    return Err(error);
                }
                // Record actual published rights, never the old reservation.
                vm.set_permissions(va, permissions);
            }
            Ok(())
        })();
        if let Err(error) = result {
            if let Some(first) = fresh_batch {
                if fresh_claimed < page_count {
                    let address = (first + fresh_claimed) * PAGE_SIZE;
                    let size = (page_count - fresh_claimed) * PAGE_SIZE;
                    if self.memory.free_physical(address, size).is_err() {
                        // A failed free has not released its allocation. Keep
                        // the complete suffix owned for the normal quarantine
                        // path rather than discard its only ownership record.
                        for i in fresh_claimed..page_count {
                            if pool_backing {
                                self.processes.register_pool_allocation(pid, base_va + i * PAGE_SIZE,
                                    start_slot + i, first + i, 1)?;
                            } else {
                                self.processes.register_physical_allocation(pid, base_va + i * PAGE_SIZE,
                                    start_slot + i, first + i, 1)?;
                            }
                        }
                    }
                }
            }
            // Registered but not yet published pages are also owned here.
            // Release tolerates holes; every allocated page remains accounted.
            self.handle_mapping_release_request(OsRequestEvent {
                identifier: pid,
                code: nanami_uapi::os::OS_REQUEST_MAPPING_RELEASE,
                arg0: base_va,
                arg1: bytes,
                arg2: pid,
                arg3: nanami_uapi::os::OS_MAPPING_RELEASE_ALLOW_HOLES,
            })?;
            return Err(error);
        }
        Ok(base_va)
    }

    fn initialize_process_page(
        &mut self,
        destination: usize,
        source: Option<PageSource>,
        offset: usize,
        source_pages: Option<&InitializationSource>,
        prepared: Option<PreparedSourceFrame>,
    ) -> Result<(), CapabilityError> {
        let mut copied = 0;
        let mut prepared = prepared;
        if let Some(source) = source {
            let bytes = PAGE_SIZE.min(source.bytes.saturating_sub(offset));
            while copied < bytes {
                let address = source.address + offset + copied;
                let page = align_down(address, PAGE_SIZE);
                let within = address - page;
                let chunk = (bytes - copied).min(PAGE_SIZE - within);
                let frame = match prepared.take().filter(|ready|
                    copied == 0 && bytes <= PAGE_SIZE - within
                        && ready.pid == source.pid && ready.page == page)
                {
                    Some(ready) => ready.frame,
                    None => self.process_frame_for_page(source.pid, page)?,
                };
                let cached = self.cached_initializer_source(source.pid, page, frame, destination, source_pages);
                // A probe hit never changes mappings. Misses still use the
                // separate window, so the initialized destination stays pinned.
                let mapped = match cached {
                    Some(address) => address,
                    None => self.map_process_copy_frame(
                        source.pid, frame, if SWAP_INITIALIZATION_ROLE { Some(destination) } else { None },
                    )?,
                };
                unsafe {
                    ptr::copy_nonoverlapping(
                        (mapped + within) as *const u8,
                        (destination + copied) as *mut u8,
                        chunk,
                    );
                }
                copied += chunk;
            }
        }
        unsafe {
            ptr::write_bytes((destination + copied) as *mut u8, 0, PAGE_SIZE - copied);
        }
        Ok(())
    }

    /// Preserve a ready source alias only when the normal destination slot
    /// would evict it. CopyWindow owns the new destination's permanent pool cap.
    fn try_swapped_initialization_destination(
        &mut self,
        physical: usize,
        source: Option<PageSource>,
        offset: usize,
        source_pages: Option<&InitializationSource>,
    ) -> Result<(Option<usize>, Option<PreparedSourceFrame>), CapabilityError> {
        if !REUSE_INITIALIZATION_SOURCE { return Ok((None, None)); }
        let Some(source) = source else { return Ok((None, None)); };
        let bytes = PAGE_SIZE.min(source.bytes.saturating_sub(offset));
        let target_alias = self.initialization_alias_address(physical);
        let mut consumed = 0;
        let mut prepared = None;
        while consumed < bytes {
            let address = source.address + offset + consumed;
            let page = align_down(address, PAGE_SIZE);
            let frame = self.process_frame_for_page(source.pid, page)?;
            if source.pid != 0 && consumed == 0 && bytes <= PAGE_SIZE - (address - page) {
                prepared = Some(PreparedSourceFrame { pid: source.pid, page, frame });
            }
            if self.cached_initializer_source(source.pid, page, frame, usize::MAX, source_pages) == Some(target_alias) {
                let destination = self.memory.physical_frame_descriptor_from_index(physical)
                    .ok_or(CapabilityError::InvalidArgument)?;
                return self.map_process_copy_frame(0, destination, None)
                    .map(|address| (Some(address), prepared));
            }
            consumed += (bytes - consumed).min(PAGE_SIZE - (address - page));
        }
        Ok((None, prepared))
    }

    fn cached_initializer_source(
        &mut self, pid: usize, page: usize, frame: CapabilityDescriptor,
        destination: usize, source_pages: Option<&InitializationSource>,
    ) -> Option<usize> {
        if !REUSE_INITIALIZATION_SOURCE { return None; }
        self.cached_initialization_frame(frame).filter(|address| *address != destination)
            .or_else(|| source_pages.and_then(|pages| {
                self.cached_source_initialization_page(pid, page, frame, destination, pages)
            }))
    }

    pub(super) fn cached_source_initialization_page(
        &mut self,
        pid: usize,
        page: usize,
        frame: CapabilityDescriptor,
        destination: usize,
        source: &InitializationSource,
    ) -> Option<usize> {
        if source.pid != pid {
            return None;
        }
        let vm = self.processes.vm_space_mut(pid)?;
        if vm.mapping_is_uncertain(page) || vm.private_page_shared(page)
            || vm.permissions(page) & 1 == 0 || vm.find_frame(page) != Some(frame)
        {
            return None;
        }
        // Exact active page references only; no guessed physical address,
        // extent canonicalization, foreign/shared/device or stale child alias.
        let allocation = source.find_page(page)?;
        if frame != process_frame_descriptor(source.root, allocation.start_slot) {
            return None;
        }
        let address = self.cached_initialization_page(allocation.base_page)?;
        if address == destination {
            return None;
        }
        // Functional-only reachability build; absent from normal/timing builds.
        if option_env!("NANAMI_INIT_SOURCE_REUSE_DIAG") == Some("1") {
            use core::sync::atomic::{AtomicBool, Ordering};
            static REPORTED: AtomicBool = AtomicBool::new(false);
            if !REPORTED.swap(true, Ordering::Relaxed) {
                crate::force_info!("[init.source] reused-ready-pool-alias owner={}", pid);
            }
        }
        // Request-local use of an existing permanent Alpha pool alias. Nothing
        // is retained across event-loop turns; destination swaps use CopyWindow.
        Some(address)
    }
}
