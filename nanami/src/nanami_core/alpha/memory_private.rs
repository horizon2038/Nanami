//! Opt-in private fork aliases. Writable leaves use COW; all detachments require
//! supervisor VM quiescence and revoked writable views.
use super::*;
use crate::nanami_core::process::{PhysicalPageReference, ProcessMemoryContext, ProcessPhysicalAllocation};

const DEFER_FORK_READONLY: bool = match option_env!("NANAMI_DEFER_FORK_READONLY") {
    Some(value) => value.len() == 1 && value.as_bytes()[0] == b'1',
    None => true,
};

#[cfg(test)]
static DETACH_CONTEXT_RESOLUTIONS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(super) fn take_detach_context_resolutions() -> usize {
    DETACH_CONTEXT_RESOLUTIONS.swap(0, core::sync::atomic::Ordering::Relaxed)
}

impl Alpha {
    #[cfg(test)]
    pub(super) fn clone_process_vm_shared_readonly(
        &mut self,
        source: usize,
        target: usize,
    ) -> Result<(), CapabilityError> {
        self.clone_process_vm_private(source, target, false)
    }

    pub(super) fn clone_process_vm_private_cow(
        &mut self, source: usize, target: usize,
    ) -> Result<(), CapabilityError> {
        self.clone_process_vm_private(source, target, true)
    }

    pub(super) fn clone_process_vm_private(
        &mut self, source: usize, target: usize, writable_cow: bool,
    ) -> Result<(), CapabilityError> {
        // Host tests exercise ownership with modeled page tables. Other
        // production backends retain eager copying until their TLB contract is verified.
        if !cfg!(any(target_arch = "x86_64", test)) {
            return self.clone_process_vm(source, target);
        }
        let parent = self
            .processes
            .find_entry_by_pid(source)
            .ok_or(CapabilityError::InvalidArgument)?;
        let child = self
            .processes
            .find_entry_by_pid(target)
            .ok_or(CapabilityError::InvalidArgument)?;
        if !parent.capabilities_sealed
            || !child.capabilities_sealed
            || parent.resource_quarantined
            || child.memory_owner != target
            || parent.reaper_pid != child.reaper_pid
        {
            return Err(CapabilityError::PermissionDenied);
        }
        if self
            .processes
            .vm_space_mut(target)
            .is_none_or(|vm| vm.has_backed_pages(0, usize::MAX))
        {
            return Err(CapabilityError::IllegalOperation);
        }
        let source_memory = self
            .processes
            .memory_entry(source)
            .ok_or(CapabilityError::InvalidArgument)?;
        self.processes
            .memory_entry(target)
            .ok_or(CapabilityError::InvalidArgument)?;
        self.split_private_large_mappings_for_fork(source, writable_cow)?;
        let vm = self
            .processes
            .vm_space_mut(source)
            .ok_or(CapabilityError::InvalidArgument)?;
        if vm.has_uncertain_mappings(0, usize::MAX) {
            return Err(CapabilityError::Fatal);
        }
        let pages = vm.backed_pages(0, usize::MAX);
        // One request-local ownership snapshot, never a list search per page.
        // Write-only and PROT_NONE pages retain eager copies. COW writable
        // pages must also be readable so removing WRITE leaves a real leaf.
        // Skip ownership collection when no permissions admit sharing.
        // Copy the fixed, immediately-written stack and small writable runs
        // eagerly; COW still needs a contiguous run of at least128KiB. Keep
        // the first128KiB of the anonymous arena writable for hot allocator
        // state. Beyond it, copy only pages actually written after fork.
        // Page-fault diagnostics now default OFF, so their old serial cost
        // no longer requires copying the larger512KiB arena prefix.
        // The rest retains COW. This is a request-local admission plan, not a
        // persistent ownership proof.
        let writable = |&(va, permissions): &(usize, usize)| matches!(permissions, 3 | 7)
            && !(USER_STACK_BASE..USER_STACK_BASE + USER_STACK_PAGES * PAGE_SIZE).contains(&va)
            && !(USER_ANONYMOUS_MAP_BASE..USER_ANONYMOUS_MAP_BASE + 128 * 1024).contains(&va);
        let mut cow_runs = Vec::new();
        if writable_cow {
            for run in pages.chunk_by(|a, b| writable(a) && writable(b)
                && a.0.checked_add(PAGE_SIZE) == Some(b.0))
            {
                if run.len() >= 32 && writable(&run[0]) {
                    cow_runs.push((run[0].0, run[run.len() - 1].0));
                }
            }
        }
        let can_share = |va: usize, permissions: usize| permissions != 0 && permissions & !5 == 0
            || cow_runs.get(cow_runs.partition_point(|&(_, end)| end < va))
                .is_some_and(|&(start, _)| start <= va);
        let share_start = pages.iter().position(|&(va, permissions)| can_share(va, permissions))
            .unwrap_or(pages.len());
        let share_end = if share_start < pages.len() {
            pages.iter().rposition(|&(va, permissions)| can_share(va, permissions)).unwrap() + 1
        } else { share_start };
        let candidates = &pages[share_start..share_end];
        let has_candidates = !candidates.is_empty();
        let mut references = if has_candidates {
            // Only exact ownership records at candidate VAs can be shared.
            // Keep the original stable/newest selection inside this span,
            // without collecting writable heap/stack tails outside it.
            self.processes.private_page_references(
                source, candidates[0].0, candidates[candidates.len() - 1].0.saturating_add(1),
            )
        } else { Vec::new() };
        // Eligibility only reads metadata. Keep the current owner VM borrowed
        // for this pass; publication below still obtains mutable VMs per use.
        let source_vm = if has_candidates { self.processes.vm_space(source) } else { None };
        let mut reference_index = 0;
        let mut shared_count = 0;
        for &(va, permissions) in candidates {
            if !can_share(va, permissions) {
                continue;
            }
            // Both snapshots are in ascending VA order. Consume ownership
            // gaps once rather than restarting a search at every backed page.
            while reference_index < references.len()
                && references[reference_index].allocation.base_va < va
            {
                reference_index += 1;
            }
            let candidate = references.get(reference_index)
                .filter(|r| r.allocation.base_va == va).map(|_| reference_index);
            let eligible = if let Some(index) = candidate {
                let allocation = references[index].allocation;
                let count = self.processes.physical_reference_count(allocation);
                let vm = source_vm.ok_or(CapabilityError::InvalidArgument)?;
                let known_private_alias = vm.private_page_shared(va);
                let frame = vm.find_frame(va);
                let expected = if allocation.owns_frame_cap {
                    Some(process_frame_descriptor(source_memory.root_node, allocation.start_slot))
                } else { self.memory.physical_frame_descriptor_from_index(allocation.base_page) };
                count != 0
                    && (known_private_alias || count == 1)
                    && expected.is_some() && frame == expected
                    && self
                        .processes
                        .shared_memory_reservation(source, va, 1)
                        .is_none()
            } else {
                false
            };
            if eligible {
                // Only overwrite an already-consumed prefix: shared_count is
                // never greater than reference_index. Keep exact ownership
                // handles in VA order without a second per-page allocation.
                references[shared_count] = references[candidate.unwrap()];
                shared_count += 1;
            }
        }
        references.truncate(shared_count);
        // Both lists are VA ordered. The cursor stays on the next unconsumed
        // shared page through eager runs and advances only after a shared run.
        let mut shared_cursor = 0;
        let mut begin = 0;
        #[cfg(not(test))]
        let mut deferred_pages = 0usize;
        while begin < pages.len() {
            let share = references.get(shared_cursor)
                .is_some_and(|reference| reference.allocation.base_va == pages[begin].0);
            let mut end = begin + 1;
            while end < pages.len()
                && pages[end - 1].0.checked_add(PAGE_SIZE) == Some(pages[end].0)
                && references.get(shared_cursor + if share { end - begin } else { 0 })
                    .is_some_and(|reference| reference.allocation.base_va == pages[end].0) == share
                && (share || pages[end].1 == pages[begin].1)
            {
                end += 1;
            }
            let run = &pages[begin..end];
            let base = run[0].0;
            if share {
                let (root, space, first) = self.processes.reserve_process_heap_at(
                    target,
                    base,
                    run.len(),
                    PAGE_SIZE,
                    PROCESS_FRAME_TOTAL_PAGES,
                )?;
                self.process_root_slot(root)?;
                let mut readonly_start = 0;
                let mut readonly_end = 0;
                for (offset, &(va, permissions)) in run.iter().enumerate() {
                    if writable_cow && DEFER_FORK_READONLY && offset >= readonly_end {
                        readonly_start = offset;
                        readonly_end = offset;
                        while readonly_end < run.len() && matches!(run[readonly_end].1, 1 | 4 | 5) {
                            readonly_end += 1;
                        }
                    }
                    // A fork child often immediately execs. Keep all owned
                    // initialized backing, but avoid registering the unused
                    // tail of large readonly runs. The existing snapshot
                    // resolver populates absent leaves in bounded2MiB clusters.
                    // Writable COW/stack/data keep their current eager policy.
                    let deferred = writable_cow && DEFER_FORK_READONLY
                        && readonly_end - readonly_start >= 128
                        && offset >= readonly_start + 16 && offset < readonly_end;
                    let allocation = references[shared_cursor + offset].allocation;
                    let cow_write = permissions & 2 != 0;
                    if permissions & 2 != 0 && !self.processes.vm_space(source)
                        .is_some_and(|vm| vm.private_page_cow_write(va))
                    {
                        let frame = self.process_frame_for_page(source, va)?;
                        let old_attributes = nun::capability_call::address_space::Attribute::from_bits(permissions)
                            .ok_or(CapabilityError::Fatal)?;
                        let readonly = nun::capability_call::address_space::Attribute::from_bits(permissions & !2)
                            .ok_or(CapabilityError::Fatal)?;
                        // Revoke WRITE and acknowledge its shootdown before
                        // registering any child alias. Logical rights stay
                        // unchanged. A failed downgrade restores the old leaf;
                        // failed restoration pins ownership as uncertain.
                        let downgraded = arch::address_space::unmap(source_memory.address_space, frame, va)
                            .and_then(|()| arch::address_space::map(source_memory.address_space, frame, va, readonly));
                        if let Err(error) = downgraded {
                            return self.restore_private_page(source, source_memory.address_space,
                                va, frame, frame, old_attributes, error);
                        }
                        let vm = self.processes.vm_space_mut(source).ok_or(CapabilityError::Fatal)?;
                        vm.mark_private_page_cow_write(va, true);
                    }
                    // Conservative markers survive a failed unpublished child;
                    // its references remain held until normal spawn rollback.
                    self.processes
                        .vm_space_mut(source)
                        .ok_or(CapabilityError::Fatal)?
                        .mark_private_page_shared(va, true);
                    self.processes.register_pool_alias(
                        target,
                        va,
                        first + offset,
                        allocation.base_page,
                    )?;
                    let frame = self.memory.physical_frame_descriptor_from_index(allocation.base_page)
                        .ok_or(CapabilityError::Fatal)?;
                    let vm = self
                        .processes
                        .vm_space_mut(target)
                        .ok_or(CapabilityError::Fatal)?;
                    vm.mark_private_page_shared(va, true);
                    if cow_write { vm.mark_private_page_cow_write(va, true); }
                    vm.set_permissions(va, permissions);
                    if deferred {
                        vm.mark_mapping_deferred(va);
                        if offset == 0 || (va >> 12) & 63 == 0 {
                            vm.prepare_frame_word(va, run.len() - offset);
                        }
                        vm.record_frame(va, frame).map_err(|_| CapabilityError::Fatal)?;
                        #[cfg(not(test))]
                        { deferred_pages += 1; }
                        continue;
                    }
                    if !vm.page_tables_ready(va) {
                        self.memory.ensure_page_tables(space, va, vm)?;
                    }
                    if offset == 0 || (va >> 12) & 63 == 0 { vm.prepare_frame_word(va, run.len() - offset); }
                    let attributes =
                        nun::capability_call::address_space::Attribute::from_bits(vm.hardware_permissions(va))
                            .ok_or(CapabilityError::InvalidArgument)?;
                    let mapped = arch::address_space::map(space, frame, va, attributes);
                    // Retain possible post-MAP side effects for root rollback.
                    vm.record_frame(va, frame)
                        .map_err(|_| CapabilityError::Fatal)?;
                    mapped?;
                }
            } else {
                let permissions = run[0].1;
                let initializer = PageSource { pid: source, address: base, bytes: run.len() * PAGE_SIZE };
                if permissions == 0 {
                    self.map_process_cloned_unmapped_pages(target, base, run.len(), initializer)?;
                } else {
                    self.map_process_initialized_pages_protected(
                        target, Some(base), run.len(), Some(initializer), permissions,
                    )?;
                }
            }
            if share { shared_cursor += run.len(); }
            begin = end;
        }
        #[cfg(not(test))]
        if option_env!("NANAMI_FORK_READONLY_DIAG") == Some("1") {
            crate::force_info!("[fork.readonly] source={} target={} pages={} shared={} deferred={} copied={}",
                source, target, pages.len(), shared_count, deferred_pages, pages.len() - shared_count);
        }
        Ok(())
    }

    pub(super) fn handle_process_private_write(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        use nanami_uapi::os::{OS_PRIVATE_WRITE_DETACH, OS_PRIVATE_WRITE_QUERY, OS_PRIVATE_WRITE_DETACH_DEFERRED};
        let pid = request.arg0;
        let target = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        // Like authorized memory copies, cleanup may target a stopped/exited
        // task until reap. Robust futex and clear_tid updates still affect live
        // CLONE_VM peers. Reaper authorization and the VM barrier remain
        // mandatory; physical ownership is retained throughout this interval.
        if request.identifier == 0
            || request.identifier == pid
            || target.reaper_pid != request.identifier
        {
            return Err(CapabilityError::PermissionDenied);
        }
        if request.arg2 == 0
            || !matches!(request.arg3, OS_PRIVATE_WRITE_QUERY | OS_PRIVATE_WRITE_DETACH | OS_PRIVATE_WRITE_DETACH_DEFERRED)
        {
            return Err(CapabilityError::InvalidArgument);
        }
        let defer_replacement = request.arg3 == OS_PRIVATE_WRITE_DETACH_DEFERRED;
        if defer_replacement && !target.capabilities_sealed {
            return Err(CapabilityError::PermissionDenied);
        }
        let end = request
            .arg1
            .checked_add(request.arg2)
            .ok_or(CapabilityError::InvalidArgument)?;
        let start = align_down(request.arg1, PAGE_SIZE);
        let vm = self
            .processes
            .vm_space_mut(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if request.arg3 == OS_PRIVATE_WRITE_QUERY {
            return Ok(usize::from(vm.private_range_shared(start, end)));
        }
        let pages = vm.shared_private_pages(start, end);
        if pages.is_empty() {
            return Ok(0);
        }
        // DETACH is a trusted supervisor contract: all users of this VM are
        // already trapped, parked, prepared, stopped, or synchronously suspended.
        let references = self.processes.private_page_references(pid, start, end);
        self.invalidate_process_copy_mappings(pid)?;
        // Only this request's owner/root/space identity is reused. Kernel
        // capability calls do not dispatch another Alpha process event.
        let mut memory_context = None;
        let mut index = 0;
        for va in pages {
            // shared_private_pages and references are sorted; references keep
            // their stable ownership handles across earlier detach commits.
            while index < references.len() && references[index].allocation.base_va < va {
                index += 1;
            }
            if references.get(index).is_none_or(|r| r.allocation.base_va != va) {
                return Err(CapabilityError::Fatal);
            }
            self.detach_private_page(pid, references[index], &mut memory_context, defer_replacement)?;
        }
        // Report a completed nonempty detach, not merely an admitted request.
        // Fault repair can use this commit result instead of a prior QUERY.
        Ok(1)
    }

    fn detach_private_page(
        &mut self,
        pid: usize,
        reference: PhysicalPageReference,
        context: &mut Option<ProcessMemoryContext>,
        defer_replacement: bool,
    ) -> Result<(), CapabilityError> {
        let old = reference.allocation;
        let count = self.processes.physical_reference_count(old);
        if count == 0 {
            return Err(CapabilityError::Fatal);
        }
        let memory = match *context {
            Some(memory) => memory,
            None => {
                #[cfg(test)]
                DETACH_CONTEXT_RESOLUTIONS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                let memory = self.processes.memory_context(pid)
                    .ok_or(CapabilityError::InvalidArgument)?;
                *context = Some(memory);
                memory
            }
        };
        let old_frame = if old.owns_frame_cap {
            process_frame_descriptor(memory.root_node, old.start_slot)
        } else {
            self.memory.physical_frame_descriptor_from_index(old.base_page)
                .ok_or(CapabilityError::Fatal)?
        };
        let vm = self
            .processes
            .vm_space_mut(pid)
            .ok_or(CapabilityError::Fatal)?;
        if vm.mapping_is_uncertain(old.base_va)
            || vm.find_frame(old.base_va) != Some(old_frame)
            || (vm.permissions(old.base_va) & 2 != 0 && !vm.private_page_cow_write(old.base_va))
        {
            return Err(CapabilityError::Fatal);
        }
        let permissions = vm.permissions(old.base_va);
        let hardware = vm.hardware_permissions(old.base_va);
        // Child-cap removal retains the existing strict restoration path.
        // Pool aliases need no cap removal after an acknowledged UNMAP, so
        // their initialized replacement can remain absent for final PROTECT.
        let defer_replacement = defer_replacement && !old.owns_frame_cap;
        let old_attributes = nun::capability_call::address_space::Attribute::from_bits(hardware)
            .ok_or(CapabilityError::Fatal)?;
        if count == 1 {
            if vm.mapping_deferred(old.base_va) {
                // Last-owner repair does not need a PTE. The caller's later
                // protection/population can map this same initialized RAM.
                vm.mark_private_page_shared(old.base_va, false);
                return Ok(());
            }
            if hardware != permissions {
                if let Err(error) = arch::address_space::unmap(memory.address_space, old_frame, old.base_va) {
                    vm.mark_mapping_uncertain(old.base_va);
                    return Err(error);
                }
                let attributes = nun::capability_call::address_space::Attribute::from_bits(permissions)
                    .ok_or(CapabilityError::Fatal)?;
                if let Err(error) = arch::address_space::map(memory.address_space, old_frame, old.base_va, attributes) {
                    return self.restore_private_page(pid, memory.address_space, old.base_va,
                        old_frame, old_frame, old_attributes, error);
                }
            }
            vm.mark_private_page_shared(old.base_va, false);
            return Ok(());
        }
        let (root, space, slot) = self
            .processes
            .reserve_private_replacement_slot(pid, PROCESS_FRAME_TOTAL_PAGES)?;
        let owner = self.processes.memory_owner_pid(pid);
        let pool_backing = self.processes.find_entry_by_pid(owner)
            .is_some_and(|entry| entry.capabilities_sealed);
        let physical = if pool_backing {
            self.process_root_slot(root)?;
            self.memory.allocate_physical_any(PAGE_SIZE)?
        } else {
            self.ensure_process_frame_chunks(pid, root, slot, 1)?;
            self.memory.allocate_process_frame(
                process_frame_chunk_descriptor(root, slot / PROCESS_FRAME_CHUNK_PAGES),
                PROCESS_FRAME_NODE_RADIX,
                slot % PROCESS_FRAME_CHUNK_PAGES,
            )?
        };
        let replacement = ProcessPhysicalAllocation {
            base_va: old.base_va,
            start_slot: slot,
            base_page: physical,
            page_count: 1,
            owns_frame_cap: !pool_backing,
        };
        // An unpublished replacement is still owned, including on any error.
        self.processes
            .defer_physical_allocation_for_pid(pid, replacement);
        if physical == old.base_page {
            return Err(CapabilityError::Fatal);
        }
        // Track even a failed/partial conversion before any fallible pool work.
        let new_frame = if pool_backing {
            self.memory.ensure_alpha_frame_at_physical_index(physical)?;
            self.memory.physical_frame_descriptor_from_index(physical)
                .ok_or(CapabilityError::Fatal)?
        } else { process_frame_descriptor(root, slot) };
        // Cache the permanent physical-pool capability, not the child slot
        // which is removed below. Otherwise a later slot reuse could hit a
        // stale copy-window alias after the initial PID invalidation.
        let source_frame = self
            .memory
            .physical_frame_descriptor_from_index(old.base_page)
            .ok_or(CapabilityError::Fatal)?;
        let source = self.map_process_transfer_frame(pid, source_frame, None)?;
        // A ready initializer alias is already a current permanent pool Frame.
        // Keep it pinned: a direct-slot collision puts the destination in the
        // separate copy window instead of evicting the source before copying.
        let collision = source == self.initialization_alias_address(physical);
        let destination = if collision {
            let pool_frame = self.memory.physical_frame_descriptor_from_index(physical)
                .ok_or(CapabilityError::Fatal)?;
            self.map_process_copy_frame(0, pool_frame, Some(source))?
        } else {
            self.map_initialization_page(physical)?
        };
        #[cfg(not(test))]
        if option_env!("NANAMI_INIT_SOURCE_REUSE_DIAG") == Some("1")
            && self.cached_initialization_frame(source_frame) == Some(source)
        {
            use core::sync::atomic::{AtomicU64, Ordering};
            static DIRECT: AtomicU64 = AtomicU64::new(0);
            static COLLIDING: AtomicU64 = AtomicU64::new(0);
            let reports = if collision { &COLLIDING } else { &DIRECT };
            let bit = 1u64 << (pid & 63);
            if reports.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
                crate::force_info!("[private.copy] reused-ready-initializer pid={} collision={}", pid, collision);
            }
        }
        unsafe {
            ptr::copy_nonoverlapping(source as *const u8, destination as *mut u8, PAGE_SIZE);
        }
        let attributes = nun::capability_call::address_space::Attribute::from_bits(permissions)
            .ok_or(CapabilityError::Fatal)?;
        if hardware != 0 {
            if let Err(error) = arch::address_space::unmap(space, old_frame, old.base_va) {
                self.processes.vm_space_mut(pid).ok_or(CapabilityError::Fatal)?
                    .mark_mapping_uncertain(old.base_va);
                return Err(error);
            }
            if defer_replacement {
                // Only after the old leaf's removal and shootdown ACK. Keep
                // even a later bookkeeping failure faultable and referenced;
                // until ownership swaps, this still resolves the old RAM.
                self.processes.vm_space_mut(pid).ok_or(CapabilityError::Fatal)?
                    .mark_mapping_deferred(old.base_va);
            } else {
                if let Err(error) = arch::address_space::map(space, new_frame, old.base_va, attributes) {
                    return self.restore_private_page(pid, space, old.base_va,
                        old_frame, new_frame, old_attributes, error);
                }
            }
        }
        if let Err(error) = if old.owns_frame_cap { arch::node::remove(
            process_frame_chunk_descriptor(root, old.start_slot / PROCESS_FRAME_CHUNK_PAGES),
            old.start_slot % PROCESS_FRAME_CHUNK_PAGES,
        ) } else { Ok(()) } {
            if permissions != 0 {
                return self.restore_private_page(
                    pid,
                    space,
                    old.base_va,
                    old_frame,
                    new_frame,
                    old_attributes,
                    error,
                );
            }
            return Err(error);
        }
        let last = self
            .processes
            .replace_private_backing(reference, replacement)?;
        let vm = self
            .processes
            .vm_space_mut(pid)
            .ok_or(CapabilityError::Fatal)?;
        vm.record_frame(old.base_va, new_frame)
            .map_err(|_| CapabilityError::Fatal)?;
        vm.mark_private_page_shared(old.base_va, false);
        #[cfg(not(test))]
        if defer_replacement && hardware != 0 && option_env!("NANAMI_DETACH_DEFERRED_DIAG") == Some("1") {
            use core::sync::atomic::{AtomicUsize, Ordering};
            static REPORTS: AtomicUsize = AtomicUsize::new(0);
            let count = REPORTS.fetch_add(1, Ordering::Relaxed) + 1;
            if count <= 8 || count.is_power_of_two() {
                crate::force_info!("[private.deferred] count={} pid={} va={:#x} old-rights={} intermediate-maps=0", count, pid, old.base_va, permissions);
            }
        }
        self.processes
            .recycle_private_frame_slot(pid, old.start_slot);
        if last {
            self.memory
                .free_physical(old.base_page * PAGE_SIZE, PAGE_SIZE)?;
        }
        Ok(())
    }

    fn restore_private_page(
        &mut self,
        pid: usize,
        space: CapabilityDescriptor,
        va: usize,
        old: CapabilityDescriptor,
        new: CapabilityDescriptor,
        attributes: nun::capability_call::address_space::Attribute,
        error: CapabilityError,
    ) -> Result<(), CapabilityError> {
        // UNMAP may fail because MAP installed nothing. Only a successful strict
        // restoration proves recovery. Never free either backing on failure.
        let _ = arch::address_space::unmap(space, new, va);
        if arch::address_space::map(space, old, va, attributes).is_err() {
            let vm = self.processes
                .vm_space_mut(pid)
                .ok_or(CapabilityError::Fatal)?;
            vm.mark_mapping_uncertain(va);
            vm.record_frame(va, new)
                .map_err(|_| CapabilityError::Fatal)?;
            return Err(CapabilityError::Fatal);
        }
        Err(error)
    }
}
