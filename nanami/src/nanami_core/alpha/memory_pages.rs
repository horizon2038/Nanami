//! Private RAM construction: initialize each byte once before publishing pages.
use super::*;
use crate::nanami_core::vm_space::{FRAME_WORD_BYTES, FRAME_WORD_PAGES};

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

#[derive(Clone, Copy)]
pub(super) struct ProcessPageRange {
    pub pid: usize,
    pub root: usize,
    pub space: usize,
    pub base: usize,
    pub slot: usize,
    pub pages: usize,
    pub permissions: usize,
    pub pool: bool,
    // Absolute threshold so split ranges preserve one request's eager prefix.
    pub deferred_from: Option<usize>,
}

impl ProcessPageRange {
    pub fn part(self, offset: usize, pages: usize) -> Self {
        debug_assert!(offset <= self.pages && pages <= self.pages - offset);
        Self { base: self.base + offset * PAGE_SIZE, slot: self.slot + offset, pages, ..self }
    }
}

pub(super) fn initializer_after(source: Option<PageSource>, pages: usize) -> Option<PageSource> {
    let source = source?;
    let skip = pages * PAGE_SIZE;
    (skip < source.bytes).then(|| PageSource { address: source.address + skip,
        bytes: source.bytes - skip, ..source })
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
pub(super) struct PreparedSourceFrame {
    pid: usize,
    page: usize,
    frame: CapabilityDescriptor,
}

pub(super) fn prepared_pool_allowed(raw: bool, auto_resume: bool, final_permissions: bool) -> bool {
    cfg!(target_arch = "x86_64") && raw && !auto_resume && final_permissions
}

impl Alpha {
    /// Validate a caller-owned prefix before reserving fresh snapshot RAM.
    pub(super) fn validate_owned_memory_initializer(
        &mut self, source: PageSource, capacity: usize,
    ) -> Result<(), CapabilityError> {
        if source.address == 0 || source.bytes == 0 || source.bytes > capacity {
            return Err(CapabilityError::InvalidArgument);
        }
        let end = source.address.checked_add(source.bytes)
            .ok_or(CapabilityError::InvalidArgument)?;
        self.validate_initializer_frames(source, end, false).map(|_| ())
    }

    fn validate_initializer_frames(
        &mut self, source: PageSource, end: usize, classify_pool: bool,
    ) -> Result<bool, CapabilityError> {
        let start = align_down(source.address, PAGE_SIZE);
        if end - start >= 16 * PAGE_SIZE {
            if let Some((end, vm)) = align_up_checked(end, PAGE_SIZE).zip(self.processes.vm_space(source.pid)) {
                let mut pool_only = true;
                let complete = if classify_pool {
                    vm.visit_aligned_frame_run(start, end, |frame| {
                        pool_only = pool_only && self.memory.is_pool_frame_descriptor(frame);
                    })
                } else {
                    vm.fully_backed_aligned_range(start, end)
                };
                if complete { return Ok(pool_only); }
            }
        }
        // Holes may need lazy materialization. Resolve in the original order
        // and propagate the first failure before reserving destination RAM.
        let mut pool_only = true;
        for va in (start..end).step_by(PAGE_SIZE) {
            let frame = self.process_frame_for_page(source.pid, va)?;
            if classify_pool { pool_only = pool_only && self.memory.is_pool_frame_descriptor(frame); }
        }
        Ok(pool_only)
    }

    /// No process leaf exists yet. Copy each prefix byte once and clear only
    /// its suffix, using the same pinned-source handling as private backing.
    pub(super) fn initialize_owned_physical_pages(
        &mut self, first: usize, pages: usize, source: PageSource,
    ) -> Result<(), CapabilityError> {
        first.checked_add(pages).ok_or(CapabilityError::InvalidArgument)?;
        let capacity = pages.checked_mul(PAGE_SIZE).ok_or(CapabilityError::InvalidArgument)?;
        if pages == 0 || source.bytes > capacity { return Err(CapabilityError::InvalidArgument); }
        let mut i = 0;
        while i < pages {
            let page = first + i;
            let (swapped, prepared) = if SWAP_INITIALIZATION_ROLE {
                self.try_swapped_initialization_destination(page, Some(source), i * PAGE_SIZE, None)?
            } else { (None, None) };
            let (destination, count) = match swapped {
                Some(address) => (address, 1),
                None => self.map_initialization_run(page, pages - i)?,
            };
            self.initialize_process_span(destination..destination + count * PAGE_SIZE,
                Some(source), i * PAGE_SIZE, None, prepared, count > 1 || SWAP_INITIALIZATION_ROLE)?;
            i += count;
        }
        Ok(())
    }

    /// Use a temporary huge Frame only while a new owned object is private.
    /// Its final pool identities are real 4K Frames, with unchanged rights and
    /// per-page ownership. Warm/fragmented RAM retains the existing path.
    pub(super) fn initialize_owned_with_large_frames(
        &mut self, first: usize, pages: usize, source: Option<PageSource>,
    ) -> Result<bool, CapabilityError> {
        use super::memory_large::{LARGE_PAGE_BYTES, LARGE_PAGE_COUNT};
        let end = first.checked_add(pages).ok_or(CapabilityError::InvalidArgument)?;
        let bytes = pages.checked_mul(PAGE_SIZE).ok_or(CapabilityError::InvalidArgument)?;
        if pages == 0 || source.is_some_and(|source|
            source.bytes > bytes || source.address.checked_add(source.bytes).is_none()) {
            return Err(CapabilityError::InvalidArgument);
        }
        if pages < LARGE_PAGE_COUNT { return Ok(false); }
        let Some(mut candidate) = align_up_checked(first, LARGE_PAGE_COUNT) else { return Ok(false); };
        let mut done = 0;
        let mut huge = 0;
        while end.saturating_sub(candidate) >= LARGE_PAGE_COUNT {
            if let Some(allocation) = self.memory.prepare_reserved_large_frame(candidate)? {
                // Protect the untyped chunk before building the small prefix,
                // whose normal batched conversion might otherwise consume it.
                let offset = candidate - first;
                self.initialize_owned_small_span(first + done, offset - done, source, done)?;
                if allocation.large {
                    let frame = self.memory.physical_frame_descriptor_from_index(candidate)
                        .ok_or(CapabilityError::InvalidArgument)?;
                    let (output, count) = self.memory.map_private_large_alias(frame, None)?
                        .ok_or(CapabilityError::InvalidArgument)?;
                    if count < LARGE_PAGE_COUNT { return Err(CapabilityError::InvalidArgument); }
                    self.initialize_process_span(output..output + LARGE_PAGE_BYTES,
                        source, offset * PAGE_SIZE, None, None, true)?;
                    self.memory.demote_private_large_pool(candidate)?;
                    huge += 1;
                } else {
                    self.initialize_owned_small_span(candidate, LARGE_PAGE_COUNT, source, offset)?;
                }
                done = offset + LARGE_PAGE_COUNT;
            }
            candidate += LARGE_PAGE_COUNT;
        }
        #[cfg(not(test))]
        if option_env!("NANAMI_OWNED_HUGE_INIT_TRACE") == Some("1") {
            crate::force_info!("[alpha.owned-init] first={} pages={} huge={} initialized={} bytes={} zero={}",
                first, pages, huge, usize::from(done != 0), source.map_or(0, |source| source.bytes),
                usize::from(source.is_none()));
        }
        if done == 0 { return Ok(false); }
        self.initialize_owned_small_span(first + done, pages - done, source, done)?;
        Ok(true)
    }

    fn initialize_owned_small_span(&mut self, first: usize, pages: usize,
        source: Option<PageSource>, offset_pages: usize) -> Result<(), CapabilityError> {
        if pages == 0 { return Ok(()); }
        self.memory.ensure_alpha_frames_for_allocation(first, pages)?;
        let offset = offset_pages * PAGE_SIZE;
        if let Some(source) = source {
            let part = PageSource { pid: source.pid,
                address: source.address.checked_add(offset.min(source.bytes))
                    .ok_or(CapabilityError::InvalidArgument)?,
                bytes: (pages * PAGE_SIZE).min(source.bytes.saturating_sub(offset)) };
            self.initialize_owned_physical_pages(first, pages, part)
        } else {
            self.zero_physical_pages(first, pages)
        }
    }

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
        let frame_hint = self.memory.allocation_frame_hint(batch.first, batch.count);
        self.memory.ensure_alpha_frame_for_allocation(physical, frame_hint)?;
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
        self.map_process_initialized_pages_inner(pid, requested_base, page_count, source,
            permissions, reserved, None, false)
    }

    pub(super) fn map_process_initialized_pages_deferred(
        &mut self, pid: usize, base: usize, page_count: usize,
        source: Option<PageSource>, eager_bytes: usize,
    ) -> Result<usize, CapabilityError> {
        self.map_process_initialized_pages_inner(pid, Some(base), page_count, source, 1,
            None, Some(eager_bytes), false)
    }

    /// Private fork must copy retained PROT_NONE bytes without granting even
    /// temporary guest access. This internal entry has no wire-level caller.
    pub(super) fn map_process_cloned_unmapped_pages(
        &mut self, pid: usize, base: usize, page_count: usize, source: PageSource,
    ) -> Result<usize, CapabilityError> {
        self.map_process_initialized_pages_inner(pid, Some(base), page_count,
            Some(source), 0, None, None, true)
    }

    fn map_process_initialized_pages_inner(
        &mut self, pid: usize, requested_base: Option<usize>, page_count: usize,
        source: Option<PageSource>, permissions: usize,
        reserved: Option<crate::nanami_core::process::FreshZeroFrameSlot>, eager: Option<usize>,
        unmapped_clone: bool,
    ) -> Result<usize, CapabilityError> {
        if (permissions == 0 && !unmapped_clone) || permissions & !7 != 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let bytes = page_count
            .checked_mul(PAGE_SIZE)
            .ok_or(CapabilityError::InvalidArgument)?;
        if pid == 0 || page_count == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let owner = self.processes.memory_owner_pid(pid);
        let pool_backing = self.processes.find_entry_by_pid(owner)
            .is_some_and(|entry| entry.capabilities_sealed);
        if unmapped_clone && (!pool_backing || permissions != 0 || reserved.is_some()
            || eager.is_some() || requested_base.is_none() || source.is_none()
            || self.processes.find_entry_by_pid(pid)
                .is_none_or(|entry| entry.exited || entry.resource_quarantined)) {
            return Err(CapabilityError::InvalidArgument);
        }
        let mut deferred_from = if let Some(prefix) = eager {
            if !pool_backing || permissions != 1 || reserved.is_some()
                || prefix == 0 || prefix > bytes || prefix & (PAGE_SIZE - 1) != 0
                || self.processes.find_entry_by_pid(pid)
                    .is_none_or(|entry| entry.exited || entry.resource_quarantined)
            { return Err(CapabilityError::InvalidArgument); }
            Some(requested_base.filter(|base| *base != 0 && *base & (PAGE_SIZE - 1) == 0)
                .and_then(|base| base.checked_add(prefix)).ok_or(CapabilityError::InvalidArgument)?)
        } else { None };
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
                pool_source_only = self.validate_initializer_frames(source, end, REUSE_INITIALIZATION_SOURCE)?;
            }
        }
        // ELF alignment can make a small file's temporary readonly mapping
        // span a multi-MiB zero suffix. Initialize and retain that RAM, but do
        // not publish cold 4K leaves before the loader removes the gap. Huge
        // allocation still runs first and may cover this suffix with one leaf.
        let defer_zero_tail = cfg!(any(target_arch = "x86_64", test)) && eager.is_none() && pool_backing
            && reserved.is_none() && permissions != 0 && permissions & 2 == 0
            && option_env!("NANAMI_EAGER_FILE_ZERO_TAIL") != Some("1")
            && self.processes.find_entry_by_pid(pid).is_some_and(|entry|
                entry.capabilities_sealed && !entry.exited && !entry.resource_quarantined);
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
            // A 2-MiB request placed after a small allocation otherwise has
            // no complete huge interval. Only automatic sealed-pool mappings
            // prefer this alignment; fixed/native reservations keep their VA.
            let alignment = if cfg!(target_arch = "x86_64") && pool_backing
                && page_count >= super::memory_large::LARGE_PAGE_COUNT {
                super::memory_large::LARGE_PAGE_BYTES
            } else { 1 };
            self.processes.reserve_process_heap_preferred_alignment(
                pid,
                page_count,
                PAGE_SIZE,
                PROCESS_FRAME_TOTAL_PAGES,
                alignment,
            )?
        };
        // Resolve automatic placement before admitting the cold zero tail too.
        // The same current-rights fault path handles both fixed and automatic
        // readonly reservations; retained extents and genuine huge pages still
        // take their existing paths inside the population constructor.
        if defer_zero_tail {
            let prefix = align_up_checked(source.map_or(0, |source| source.bytes), PAGE_SIZE)
                .ok_or(CapabilityError::InvalidArgument)?;
            if bytes.saturating_sub(prefix) >= 128 * PAGE_SIZE {
                deferred_from = Some(base_va.checked_add(prefix).ok_or(CapabilityError::InvalidArgument)?);
            }
        }
        self.populate_process_reserved_pages(ProcessPageRange { pid, root: root_node,
            space: address_space, base: base_va, slot: start_slot, pages: page_count,
            permissions, pool: pool_backing, deferred_from }, source, source_pages.as_ref())?;
        #[cfg(not(test))]
        if option_env!("NANAMI_FILE_ZERO_TAIL_DIAG") == Some("1") && eager.is_none()
            && pool_backing && permissions != 0 && permissions & 2 == 0
            && bytes.saturating_sub(source.map_or(0, |source| source.bytes).next_multiple_of(PAGE_SIZE)) >= 128 * PAGE_SIZE
        {
            let vm = self.processes.vm_space(pid).ok_or(CapabilityError::Fatal)?;
            let pending = (0..page_count).filter(|page| vm.mapping_deferred(base_va + page * PAGE_SIZE)).count();
            let huge = self.memory.stable_private_large_mappings(address_space, base_va, base_va + bytes)
                .filter(|leaf| leaf.permissions != 0 && leaf.base >= base_va
                    && leaf.base + super::memory_large::LARGE_PAGE_BYTES <= base_va + bytes).count();
            let leaves = page_count - pending - huge * (super::memory_large::LARGE_PAGE_COUNT - 1);
            crate::force_info!("[image.zero-tail] pid={} base={:#x} pages={} source-bytes={} pending={} leaves={} huge={}",
                pid, base_va, page_count, source.map_or(0, |source| source.bytes), pending, leaves, huge);
        }
        Ok(base_va)
    }

    pub(super) fn populate_process_small_pages(
        &mut self, range: ProcessPageRange, source: Option<PageSource>, source_pages: Option<&InitializationSource>,
        preallocated: Option<usize>, preregistered: bool,
        reuse: Option<crate::nanami_core::process::ProcessPhysicalAllocation>,
    ) -> Result<(), CapabilityError> {
        self.populate_process_small_pages_inner(range, source, source_pages, preallocated, preregistered, reuse, false)
    }

    fn populate_process_small_pages_inner(
        &mut self, range: ProcessPageRange, source: Option<PageSource>, source_pages: Option<&InitializationSource>,
        preallocated: Option<usize>, preregistered: bool,
        reuse: Option<crate::nanami_core::process::ProcessPhysicalAllocation>, bounded: bool,
    ) -> Result<(), CapabilityError> {
        let ProcessPageRange { pid, root: root_node, space: address_space, base: base_va,
            slot: start_slot, pages: page_count, permissions, pool: pool_backing, deferred_from } = range;
        let bytes = page_count * PAGE_SIZE;
        let attributes = nun::capability_call::address_space::Attribute::from_bits(permissions)
            .ok_or(CapabilityError::InvalidArgument)?;
        if pool_backing {
            self.process_root_slot(root_node)?;
        } else {
            self.ensure_process_frame_chunks(pid, root_node, start_slot, page_count)?;
        }
        let reuse_allocation = if preregistered { None } else { reuse.or_else(|| self.processes
            .take_deferred_physical_allocation(pid, base_va, page_count)) };
        // Cold readonly zero tails need bytes, not distinct physical identity.
        // Keep huge allocation and retained extents ahead of this fallback.
        // Existing private sharing/COW owns one reference per virtual page.
        let zero_from = source.map_or(0, |source| source.bytes.div_ceil(PAGE_SIZE))
            .max(deferred_from.map_or(page_count, |first| first.saturating_sub(base_va).div_ceil(PAGE_SIZE)));
        let shared_zero = cfg!(any(target_arch = "x86_64", test))
            && pool_backing && preallocated.is_none() && !preregistered
            && reuse_allocation.is_none() && permissions != 0 && permissions & 2 == 0
            && page_count.saturating_sub(zero_from) >= 128
            && self.processes.find_entry_by_pid(pid).is_some_and(|entry|
                entry.capabilities_sealed && !entry.exited && !entry.resource_quarantined);
        if shared_zero {
            let enabled = option_env!("NANAMI_SHARED_FILE_ZERO_TAIL") != Some("0");
            #[cfg(not(test))]
            if option_env!("NANAMI_SHARED_FILE_ZERO_DIAG") == Some("1") {
                crate::force_info!("[image.zero-sharing] pid={} base={:#x} prefix={} pages={} rights={} shared={}",
                    pid, base_va, zero_from, page_count - zero_from, permissions, enabled);
            }
            if enabled {
                let result = (|| {
                    if zero_from != 0 {
                        self.populate_process_small_pages_inner(range.part(0, zero_from), source,
                            source_pages, None, false, None, true)?;
                    }
                    self.populate_shared_zero_pages(range.part(zero_from, page_count - zero_from))
                })();
                if let Err(error) = result {
                    self.handle_mapping_release_request(OsRequestEvent {
                        identifier: pid, code: nanami_uapi::os::OS_REQUEST_MAPPING_RELEASE,
                        arg0: base_va, arg1: bytes, arg2: pid,
                        arg3: nanami_uapi::os::OS_MAPPING_RELEASE_ALLOW_HOLES,
                    })?;
                    return Err(error);
                }
                return Ok(());
            }
        }
        let bulk_registration = option_env!("NANAMI_SCALAR_POOL_REGISTRATION") != Some("1");
        if let Some(allocation) = reuse_allocation {
            // Transfer ownership of the entire reused extent before any
            // fallible cap/mapping operation, including its unused suffix.
            if pool_backing && bulk_registration {
                self.processes.register_pool_page_run(pid, base_va, start_slot,
                    allocation.base_page, page_count)?;
            } else {
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
        }
        // Until each new page is registered, its suffix belongs to this
        // synchronous request alone and has not been published to the target.
        let mut fresh_batch = preallocated;
        let mut fresh_claimed = if preregistered { page_count } else { 0 };
        let mut registered_batch = preregistered;
        let result = (|| {
            if reuse_allocation.is_none() && page_count > 1 && fresh_batch.is_none() {
                // Lack of contiguous RAM keeps the existing single-page path.
                // Other errors must reach rollback, not become retries.
                match self.memory.allocate_physical_any(bytes) {
                    Ok(page) => { fresh_batch = Some(page); }
                    Err(CapabilityError::InvalidArgument) => {}
                    Err(error) => return Err(error),
                }
            }
            if bulk_registration && pool_backing && page_count > 1 && !registered_batch {
                if let Some(first) = fresh_batch {
                    // Keep independent page references and their prepend order,
                    // but find/update the owner's list head once for the run.
                    // Claim the entire private batch before any fallible MAP.
                    self.processes.register_pool_page_run(pid, base_va, start_slot, first, page_count)?;
                    fresh_claimed = page_count;
                    registered_batch = true;
                }
            }
            let frame_hint = fresh_batch.and_then(|first| if bounded {
                self.memory.small_allocation_frame_hint(first, page_count)
            } else { self.memory.allocation_frame_hint(first, page_count) });
            let end = base_va + bytes;
            let deferred_words = deferred_from
                .and_then(|first| align_up_checked(first.max(base_va), FRAME_WORD_BYTES))
                .unwrap_or(end)..align_down(end, FRAME_WORD_BYTES);
            let mut deferred_frames = [0; FRAME_WORD_PAGES];
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
                    if registered_batch {
                        // This whole batch was published as owned before any
                        // initializer, including a preceding unaligned prefix.
                    } else if pool_backing {
                        self.processes.register_pool_allocation(pid, va, slot, page, 1)?;
                    } else {
                        self.processes.register_physical_allocation(pid, va, slot, page, 1)?;
                    }
                    if !registered_batch { fresh_claimed = i + 1; }
                    self.memory.ensure_alpha_frame_for_allocation(page, frame_hint)?;
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
                    self.try_swapped_initialization_destination(page, source, i * PAGE_SIZE, source_pages)?
                } else {
                    (None, None)
                };
                let destination = match swapped {
                    Some(address) => address,
                    None => self.map_initialization_page(page)?,
                };
                self.initialize_process_page(destination, source, i * PAGE_SIZE, source_pages, prepared)?;
                let frame = if pool_backing {
                    self.memory.physical_frame_descriptor_from_index(page)
                        .ok_or(CapabilityError::Fatal)?
                } else { process_frame_descriptor(root_node, slot) };
                if deferred_words.contains(&va) {
                    let word = align_down(va, FRAME_WORD_BYTES);
                    let index = (va - word) / PAGE_SIZE;
                    deferred_frames[index] = frame;
                    // A failed initializer leaves the current word unpublished.
                    // Its individual ownership records still drive rollback.
                    if index + 1 == FRAME_WORD_PAGES {
                        let vm = self.processes.vm_space_mut(pid).ok_or(CapabilityError::InvalidArgument)?;
                        vm.record_deferred_frame_word(word, &deferred_frames, false)
                            .map_err(|_| CapabilityError::Fatal)?;
                        if permissions != 1 {
                            vm.set_permission_words(word, word + FRAME_WORD_BYTES, permissions);
                        }
                    }
                    continue;
                }
                let vm = self
                    .processes
                    .vm_space_mut(pid)
                    .ok_or(CapabilityError::InvalidArgument)?;
                if i == 0 || (va >> 12) & 63 == 0 { vm.prepare_frame_word(va, page_count - i); }
                if permissions == 0 || deferred_from.is_some_and(|first| va >= first) {
                    // The page is initialized and privately owned, but no
                    // guest PTE exists. Keep actual absence separate from R.
                    if permissions == 0 { vm.confirm_unmapped(va); }
                    else { vm.mark_mapping_deferred(va); }
                    vm.set_permissions(va, permissions);
                    let tables_ready = vm.page_tables_ready(va);
                    vm.record_frame(va, frame).map_err(|_| CapabilityError::Fatal)?;
                    // record_frame normally follows a guest MAP. Retained
                    // zero-rights backing proves no new page-table presence.
                    if permissions == 0 && !tables_ready { vm.invalidate_page_table_region(va); }
                    continue;
                }
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
        #[cfg(not(test))]
        if option_env!("NANAMI_POOL_REGISTRATION_DIAG") == Some("1") && pool_backing
            && page_count > 1 && !preregistered
            && (reuse_allocation.is_some() || fresh_batch.is_some())
        {
            crate::force_info!("[image.pool-registration] pid={} base={:#x} pages={} calls={} reused={}",
                pid, base_va, page_count, if bulk_registration { 1 } else { page_count },
                usize::from(reuse_allocation.is_some()));
        }
        Ok(())
    }

    pub(super) fn initialize_process_page(
        &mut self,
        destination: usize,
        source: Option<PageSource>,
        offset: usize,
        source_pages: Option<&InitializationSource>,
        prepared: Option<PreparedSourceFrame>,
    ) -> Result<(), CapabilityError> {
        self.initialize_process_span(destination..destination + PAGE_SIZE, source, offset,
            source_pages, prepared, SWAP_INITIALIZATION_ROLE)
    }

    /// The output is a live contiguous alias for fresh backing. Walk source
    /// page boundaries once and keep the entire output pinned when using a
    /// large alias, including when the small-page role swap is disabled.
    pub(super) fn initialize_process_span(
        &mut self,
        output: core::ops::Range<usize>,
        source: Option<PageSource>,
        offset: usize,
        source_pages: Option<&InitializationSource>,
        prepared: Option<PreparedSourceFrame>,
        pin_destination: bool,
    ) -> Result<(), CapabilityError> {
        let capacity = output.len();
        let destination = output.start;
        let mut copied = 0;
        let mut prepared = prepared;
        if let Some(source) = source {
            let bytes = capacity.min(source.bytes.saturating_sub(offset));
            while copied < bytes {
                let address = source.address + offset + copied;
                let page = align_down(address, PAGE_SIZE);
                let within = address - page;
                let chunk = (bytes - copied).min(PAGE_SIZE - within);
                let frame = match prepared.take().filter(|ready|
                    copied == 0 && ready.pid == source.pid && ready.page == page)
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
                        source.pid, frame, pin_destination.then_some(destination),
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
            ptr::write_bytes((destination + copied) as *mut u8, 0, capacity - copied);
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
        // Both source pieces have been resolved before borrowing a destination.
        // Initialization pins this address on every later CopyWindow miss.
        // Keep Pending initializer cleanup and an existing initializer hit on
        // their original paths; only reuse an exact Ready permanent pool cap.
        let destination = if self.initialization_page_needs_mapping(physical) {
            self.memory.physical_frame_descriptor_from_index(physical)
                .and_then(|frame| self.cached_pool_copy_frame(frame))
        } else { None };
        #[cfg(not(test))]
        if option_env!("NANAMI_INIT_DEST_REUSE_DIAG") == Some("1") {
            use core::sync::atomic::{AtomicUsize, Ordering};
            static PROBES: AtomicUsize = AtomicUsize::new(0);
            static REUSED: AtomicUsize = AtomicUsize::new(0);
            let probes = PROBES.fetch_add(1, Ordering::Relaxed) + 1;
            let reused = REUSED.fetch_add(usize::from(destination.is_some()), Ordering::Relaxed)
                + usize::from(destination.is_some());
            if probes.is_power_of_two() || (destination.is_some() && reused == 1) {
                crate::force_info!("[init.dest] probes={} reused={}", probes, reused);
            }
        }
        Ok((destination, prepared))
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

    fn populate_shared_zero_pages(&mut self, range: ProcessPageRange) -> Result<(), CapabilityError> {
        let first = self.memory.allocate_physical_any(PAGE_SIZE)?;
        // Claim every alias before fallible conversion/initialization. A failed
        // physical free retains the sole allocation for ordinary rollback.
        if let Err(error) = self.processes.register_pool_aliases(range.pid, range.base, range.slot,
            core::iter::repeat_n(first, range.pages))
        {
            if self.memory.free_physical(first * PAGE_SIZE, PAGE_SIZE).is_err() {
                self.processes.register_pool_allocation(range.pid, range.base, range.slot, first, 1)?;
            }
            return Err(error);
        }
        let hint = self.memory.small_allocation_frame_hint(first, 1);
        self.memory.ensure_alpha_frame_for_allocation(first, hint)?;
        self.zero_physical_pages(first, 1)?;
        let frame = self.memory.physical_frame_descriptor_from_index(first)
            .ok_or(CapabilityError::Fatal)?;
        let vm = self.processes.vm_space_mut(range.pid).ok_or(CapabilityError::Fatal)?;
        let mut offset = 0;
        while offset < range.pages {
            let va = range.base + offset * PAGE_SIZE;
            if va & (FRAME_WORD_BYTES - 1) == 0 && range.pages - offset >= FRAME_WORD_PAGES {
                vm.record_deferred_frame_word(va, &[frame; FRAME_WORD_PAGES], true)
                    .map_err(|_| CapabilityError::Fatal)?;
                if range.permissions != 1 {
                    vm.set_permission_words(va, va + FRAME_WORD_BYTES, range.permissions);
                }
                offset += FRAME_WORD_PAGES;
            } else {
                vm.mark_private_page_shared(va, true);
                vm.mark_mapping_deferred(va);
                vm.set_permissions(va, range.permissions);
                vm.record_frame(va, frame).map_err(|_| CapabilityError::Fatal)?;
                offset += 1;
            }
        }
        Ok(())
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
