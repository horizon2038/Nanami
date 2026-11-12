//! Shape changes require the supervisor's VM barrier. No other address space
//! can share these huge Frames: fork splits them before creating private aliases,
//! and page-view export requires actual 4K pool capabilities.
use super::*;
use super::mapping_protection::LargeProtection;
use super::memory_pages::{initializer_after, InitializationSource, ProcessPageRange};
use crate::nanami_core::vm_space::FRAME_WORD_PAGES;

pub(super) const LARGE_PAGE_BYTES: usize = 1 << 21;
pub(super) const LARGE_PAGE_COUNT: usize = LARGE_PAGE_BYTES / PAGE_SIZE;

impl Alpha {
    pub(super) fn populate_process_reserved_pages(&mut self, range: ProcessPageRange,
        source: Option<PageSource>, source_pages: Option<&InitializationSource>) -> Result<(), CapabilityError> {
        if !range.pool || range.pages < LARGE_PAGE_COUNT {
            return self.populate_process_small_pages(range, source, source_pages, None, false, None);
        }
        let Some(first_base) = align_up_checked(range.base, LARGE_PAGE_BYTES) else {
            return self.populate_process_small_pages(range, source, source_pages, None, false, None);
        };
        let mut prefix = (first_base - range.base) / PAGE_SIZE;
        if !range.pool || prefix > range.pages || range.pages - prefix < LARGE_PAGE_COUNT {
            return self.populate_process_small_pages(range, source, source_pages, None, false, None);
        }
        if let Some(reuse) = self.processes.take_deferred_physical_allocation(range.pid, range.base, range.pages) {
            return self.populate_process_small_pages(range, source, source_pages, None, false, Some(reuse));
        }
        self.process_root_slot(range.root)?;
        // A retained small-page table blocks only its own 2-MiB interval.
        // Find a later usable interval before allocating the small prefix,
        // so that prefix allocation cannot consume its untyped large RAM.
        while range.pages - prefix >= LARGE_PAGE_COUNT
            && !self.memory.private_large_virtual_available(range.space, range.base + prefix * PAGE_SIZE)? {
            prefix += LARGE_PAGE_COUNT;
        }
        if range.pages - prefix < LARGE_PAGE_COUNT {
            return self.populate_process_small_pages(range, source, source_pages, None, false, None);
        }
        let Some(first) = self.memory.allocate_private_large_frame()? else {
            return self.populate_process_small_pages(range, source, source_pages, None, false, None);
        };
        let result = (|| {
            // Register the first huge batch before constructing the prefix.
            // A failure there must not lose this still-unpublished ownership.
            let first_range = range.part(prefix, LARGE_PAGE_COUNT);
            self.register_private_large_pages(first_range, first.first)?;
            if prefix != 0 {
                self.populate_process_small_pages(range.part(0, prefix), source, source_pages, None, false, None)?;
            }
            let mut offset = prefix;
            let mut pending = Some(first);
            while range.pages - offset >= LARGE_PAGE_COUNT {
                let (allocation, registered) = match pending.take() {
                    Some(allocation) => (allocation, true),
                    None => {
                        if !self.memory.private_large_virtual_available(range.space, range.base + offset * PAGE_SIZE)? {
                            let begin = offset;
                            loop {
                                offset += LARGE_PAGE_COUNT;
                                if range.pages - offset < LARGE_PAGE_COUNT
                                    || self.memory.private_large_virtual_available(range.space, range.base + offset * PAGE_SIZE)? {
                                    break;
                                }
                            }
                            self.populate_process_small_pages(range.part(begin, offset - begin),
                                initializer_after(source, begin), source_pages, None, false, None)?;
                            continue;
                        }
                        match self.memory.allocate_private_large_frame()? {
                            Some(allocation) => (allocation, false),
                            None => break,
                        }
                    },
                };
                let part = range.part(offset, LARGE_PAGE_COUNT);
                if !registered { self.register_private_large_pages(part, allocation.first)?; }
                let initializer = initializer_after(source, offset);
                if allocation.large {
                    self.populate_process_large_leaf(part, allocation.first, initializer, source_pages)?;
                } else {
                    self.populate_process_small_pages(part, initializer, source_pages,
                        Some(allocation.first), true, None)?;
                }
                offset += LARGE_PAGE_COUNT;
                if !allocation.large { break; }
            }
            if offset < range.pages {
                self.populate_process_small_pages(range.part(offset, range.pages - offset),
                    initializer_after(source, offset), source_pages, None, false, None)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.handle_mapping_release_request(OsRequestEvent {
                identifier: range.pid, code: nanami_uapi::os::OS_REQUEST_MAPPING_RELEASE,
                arg0: range.base, arg1: range.pages * PAGE_SIZE, arg2: range.pid,
                arg3: nanami_uapi::os::OS_MAPPING_RELEASE_ALLOW_HOLES,
            })?;
            return Err(error);
        }
        Ok(())
    }

    fn register_private_large_pages(&mut self, range: ProcessPageRange, first: usize)
        -> Result<(), CapabilityError> {
        self.processes.register_pool_page_run(range.pid, range.base, range.slot, first, LARGE_PAGE_COUNT)
    }

    fn populate_process_large_leaf(&mut self, range: ProcessPageRange, first: usize,
        source: Option<PageSource>, source_pages: Option<&InitializationSource>) -> Result<(), CapabilityError> {
        // Initialize every byte before the single guest MAP. Source lookups
        // remain live, and a source huge alias cannot evict the pinned output.
        if source.is_none() {
            self.zero_physical_pages(first, LARGE_PAGE_COUNT)?;
        } else {
            let mut page = 0;
            while page < LARGE_PAGE_COUNT {
                let (output, count) = self.map_initialization_run(first + page, LARGE_PAGE_COUNT - page)?;
                self.initialize_process_span(output..output + count * PAGE_SIZE,
                    source, page * PAGE_SIZE, source_pages, None, true)?;
                page += count;
            }
        }
        let vm = self.processes.vm_space_mut(range.pid).ok_or(CapabilityError::InvalidArgument)?;
        let end = range.base + LARGE_PAGE_BYTES;
        vm.set_permission_words(range.base, end, 0);
        for page in (0..LARGE_PAGE_COUNT).step_by(FRAME_WORD_PAGES) {
            let va = range.base + page * PAGE_SIZE;
            let mut frames = [0; FRAME_WORD_PAGES];
            self.memory.read_private_large_frames(first + page, &mut frames)?;
            vm.record_unmapped_frame_word(va, &frames).map_err(|_| CapabilityError::Fatal)?;
        }
        vm.invalidate_page_table_region(range.base);
        if let Err(error) = self.memory.map_private_large_frame(first, range.space, range.base, range.permissions) {
            for page in 0..LARGE_PAGE_COUNT { vm.mark_mapping_uncertain(range.base + page * PAGE_SIZE); }
            return Err(error);
        }
        if range.permissions != 0 { vm.set_permission_words(range.base, end, range.permissions); }
        #[cfg(not(test))]
        {
            use core::sync::atomic::{AtomicUsize, Ordering};
            static REPORTS: AtomicUsize = AtomicUsize::new(0);
            if range.permissions != 0 && REPORTS.fetch_add(1, Ordering::Relaxed) < 16 {
                crate::force_info!("[guest.huge] pid={} va={:#x} pages=512", range.pid, range.base);
            }
        }
        Ok(())
    }

    pub(super) fn split_private_large_mappings(&mut self, pid: usize,
        start: usize, end: usize, discard_covered: bool) -> Result<(), CapabilityError> {
        self.split_private_large_mappings_inner(pid, start, end, discard_covered, None)
    }

    pub(super) fn split_private_large_mappings_for_fork(&mut self, pid: usize,
        writable_cow: bool) -> Result<(), CapabilityError> {
        self.split_private_large_mappings_inner(pid, 0, usize::MAX, false, Some(writable_cow))
    }

    fn split_private_large_mappings_inner(&mut self, pid: usize,
        start: usize, end: usize, discard_covered: bool, copied_fork: Option<bool>) -> Result<(), CapabilityError> {
        if !self.memory.has_private_large_mappings() { return Ok(()); }
        let context = self.processes.memory_context(pid).ok_or(CapabilityError::InvalidArgument)?;
        loop {
            let mapping = if let Some(cow) = copied_fork {
                let vm = self.processes.vm_space(pid);
                self.memory.private_large_mapping_requiring_split(context.address_space, start, end, |base, permissions| {
                    // Read-only aliases and writable COW need real 4K caps.
                    // Eager private copies read huge slices through Alpha's
                    // trusted aliases, so a stable parent needs no demotion.
                    (matches!(permissions, 0 | 2 | 6) || (!cow && permissions & 2 != 0))
                        && vm.is_some_and(|vm| {
                            let limit = base + LARGE_PAGE_BYTES;
                            vm.backed_range_with_permissions(base, limit, permissions)
                                && !vm.has_uncertain_mappings(base, limit)
                                && !vm.has_deferred_mappings(base, limit)
                                && !vm.private_range_shared(base, limit)
                        })
                })
            } else {
                self.memory.private_large_mapping(context.address_space, start, end)
            };
            let Some(mapping) = mapping else { break; };
            let discard = discard_covered && start <= mapping.base && mapping.base + LARGE_PAGE_BYTES <= end;
            self.split_private_large_mapping(pid, context.address_space, mapping, discard, None)?;
        }
        Ok(())
    }

    pub(super) fn prepare_large_protection(&mut self, pid: usize, start: usize, end: usize,
        permissions: usize) -> Result<Vec<LargeProtection>, CapabilityError> {
        if !self.memory.has_private_large_mappings() { return Ok(Vec::new()); }
        let space = self.processes.memory_context(pid).ok_or(CapabilityError::InvalidArgument)?.address_space;
        let vm = self.processes.vm_space(pid);
        let mut partial = false;
        let mut leaves: Vec<_> = self.memory.stable_private_large_mappings(space, start, end)
            .filter_map(|mapping| {
                let vm = vm?;
                let base = mapping.base;
                let limit = base + LARGE_PAGE_BYTES;
                if vm.has_uncertain_mappings(base, limit) || vm.has_deferred_mappings(base, limit)
                    || vm.private_range_shared(base, limit)
                    || !vm.backed_range_with_permissions(base, limit, mapping.permissions)
                {
                    return None;
                }
                // A changing partial leaf can split last and repair itself.
                // A second partial edge uses the ordinary page transaction.
                if mapping.permissions != permissions && (base < start || end < limit) {
                    if partial { return None; }
                    partial = true;
                }
                Some(LargeProtection { range: base..limit, first: mapping.first, permissions: mapping.permissions })
            }).collect();
        leaves.sort_unstable_by_key(|leaf| leaf.range.start);
        while let Some(mapping) = self.memory.private_large_mapping_requiring_split(space, start, end, |base, _| {
            leaves.binary_search_by_key(&base, |leaf| leaf.range.start).is_ok()
        }) {
            self.split_private_large_mapping(pid, space, mapping, false, None)?;
        }
        Ok(leaves)
    }

    pub(super) fn apply_large_protection(&mut self, pid: usize, space: usize,
        leaves: &[LargeProtection], range: core::ops::Range<usize>, permissions: usize)
        -> Result<(), CapabilityError> {
        let whole = |leaf: &LargeProtection| {
            leaf.permissions != permissions
                && range.start <= leaf.range.start && leaf.range.end <= range.end
        };
        let mut committed = 0;
        let change = (|| {
            for (index, leaf) in leaves.iter().enumerate().filter(|(_, leaf)| whole(leaf)) {
                self.protect_large_leaf(pid, space, leaf, permissions, false)?;
                committed = index + 1;
            }
            if let Some(leaf) = leaves.iter().find(|leaf| {
                leaf.permissions != permissions
                    && (leaf.range.start < range.start || range.end < leaf.range.end)
            }) {
                let mapping = self.memory.private_large_protection(space, leaf.range.start)
                    .ok_or(CapabilityError::Fatal)?;
                self.split_private_large_mapping(pid, space, mapping, false, Some((range.clone(), permissions)))?;
            }
            Ok(())
        })();
        if let Err(error) = change {
            // The failing leaf repairs itself or keeps its backing quarantined.
            // Restore all previously committed whole leaves, even if one repair fails.
            let mut result = error;
            for leaf in leaves[..committed].iter().rev().filter(|leaf| whole(leaf)) {
                if self.protect_large_leaf(pid, space, leaf, leaf.permissions, true).is_err() {
                    result = CapabilityError::Fatal;
                }
            }
            return Err(result);
        }
        Ok(())
    }

    fn protect_large_leaf(&mut self, pid: usize, space: usize, leaf: &LargeProtection,
        permissions: usize, restoring: bool) -> Result<(), CapabilityError> {
        let vm = self.processes.vm_space_mut(pid).ok_or(CapabilityError::Fatal)?;
        if let Err(error) = self.memory.protect_private_large_frame(leaf.first, permissions) {
            if restoring { self.memory.quarantine_private_large_mapping(leaf.first); }
            if restoring || self.memory.private_large_mapping(space, leaf.range.start, leaf.range.end)
                .is_some_and(|mapping| mapping.quarantined)
            {
                for va in leaf.range.clone().step_by(PAGE_SIZE) { vm.mark_mapping_uncertain(va); }
            }
            return Err(error);
        }
        vm.set_permission_words(leaf.range.start, leaf.range.end, permissions);
        Ok(())
    }

    fn split_private_large_mapping(&mut self, pid: usize, space: usize,
        mapping: crate::nanami_core::memory::PrivateLargeMapping, discard: bool,
        protection: Option<(core::ops::Range<usize>, usize)>) -> Result<(), CapabilityError> {
        if mapping.quarantined { return Err(CapabilityError::Fatal); }
        let result = (|| {
            self.memory.unmap_private_large_frame(mapping.first)?;
            let vm = self.processes.vm_space_mut(pid).ok_or(CapabilityError::InvalidArgument)?;
            if !mapping.small {
                // The huge UNMAP ACK proves every slice absent. Keep RAM
                // ownership and the old logical rights in Mapping until
                // either the entire split or its rollback commits.
                for page in 0..LARGE_PAGE_COUNT {
                    let va = mapping.base + page * PAGE_SIZE;
                    vm.confirm_unmapped(va);
                }
            }
            self.memory.demote_private_large_pool(mapping.first)?;
            let vm = self.processes.vm_space_mut(pid).ok_or(CapabilityError::InvalidArgument)?;
            for page in 0..LARGE_PAGE_COUNT {
                let va = mapping.base + page * PAGE_SIZE;
                let frame = self.memory.physical_frame_descriptor_from_index(mapping.first + page)
                    .ok_or(CapabilityError::Fatal)?;
                // A retained Small phase only occurs after an incomplete
                // split. Do not overwrite a possibly installed 4K leaf.
                if mapping.small && vm.frame_may_be_present(va) {
                    arch::address_space::unmap(space, frame, va)?;
                }
                vm.record_frame(va, frame).map_err(|_| CapabilityError::Fatal)?;
                vm.confirm_unmapped(va);
            }
            vm.invalidate_page_table_region(mapping.base);
            if !discard && (mapping.permissions != 0 || protection.as_ref().is_some_and(|(_, rights)| *rights != 0)) {
                self.memory.ensure_private_large_split_table(mapping.first, vm)?;
                for page in 0..LARGE_PAGE_COUNT {
                    let va = mapping.base + page * PAGE_SIZE;
                    let permissions = match &protection {
                        Some((range, rights)) if range.contains(&va) => *rights,
                        _ => mapping.permissions,
                    };
                    if permissions == 0 { continue; }
                    let attributes = nun::capability_call::address_space::Attribute::from_bits(permissions)
                        .ok_or(CapabilityError::Fatal)?;
                    let frame = self.memory.physical_frame_descriptor_from_index(mapping.first + page)
                        .ok_or(CapabilityError::Fatal)?;
                    if let Err(error) = self.memory.map_frame_strict_with_attribute(space, frame, va, attributes, vm) {
                        vm.mark_mapping_uncertain(va);
                        return Err(error);
                    }
                    vm.set_permissions(va, permissions);
                }
            }
            self.memory.finish_private_large_split(mapping.first)
        })();
        if let Err(error) = result {
            // The barrier is still held. The newly created PT can be
            // unlinked with an AS shootdown, then its 4K caps revoked and
            // the same RAM restored as a huge leaf with original rights.
            if self.memory.restore_private_large_frame(mapping.first).is_err() {
                self.memory.quarantine_private_large_mapping(mapping.first);
                if let Some(vm) = self.processes.vm_space_mut(pid) {
                    for page in 0..LARGE_PAGE_COUNT {
                        vm.mark_mapping_uncertain(mapping.base + page * PAGE_SIZE);
                    }
                }
                return Err(CapabilityError::Fatal);
            }
            let vm = self.processes.vm_space_mut(pid).ok_or(CapabilityError::Fatal)?;
            for page in 0..LARGE_PAGE_COUNT {
                let va = mapping.base + page * PAGE_SIZE;
                let frame = self.memory.physical_frame_descriptor_from_index(mapping.first + page)
                    .ok_or(CapabilityError::Fatal)?;
                vm.record_frame(va, frame).map_err(|_| CapabilityError::Fatal)?;
                vm.confirm_unmapped(va);
                vm.set_permissions(va, mapping.permissions);
            }
            vm.forget_page_table_slot(mapping.base);
            return Err(error);
        }
        Ok(())
    }
}
