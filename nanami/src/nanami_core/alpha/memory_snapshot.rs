//! Immutable owned RAM snapshots mapped privately into supervised sealed tasks.
use super::*;
use crate::nanami_core::vm_space::{FRAME_WORD_BYTES, FRAME_WORD_PAGES};

// Request-local admission only. Construct once, consume before aliasing; no
// source mutation or reentrant dispatch occurs during target conflict checks.
struct SnapshotSourceAdmission {
    caller: usize,
    source: usize,
    end: usize,
    root: CapabilityDescriptor,
    reservation: crate::nanami_core::process::SharedMemoryReservation,
}

// Both variants describe fully checked backing for this synchronous request.
// Opaque immutable sources normally have consecutive physical pages; retain
// the original collected/newest-record path for every other ownership shape.
enum SnapshotBacking {
    Contiguous { first: usize, pages: usize },
    Pages(Vec<crate::nanami_core::process::PhysicalPageReference>),
}

impl SnapshotBacking {
    fn len(&self) -> usize {
        match self {
            Self::Contiguous { pages, .. } => *pages,
            Self::Pages(rows) => rows.len(),
        }
    }

    fn physical_page(&self, offset: usize) -> usize {
        match self {
            Self::Contiguous { first, .. } => first + offset,
            Self::Pages(rows) => rows[offset].allocation.base_page,
        }
    }
}

#[cfg(test)]
std::thread_local! {
    static BACKING_PLANS: core::cell::Cell<(usize, usize)> = const { core::cell::Cell::new((0, 0)) };
}

#[cfg(test)]
pub(super) fn take_backing_plans() -> (usize, usize) {
    BACKING_PLANS.with(|plans| plans.replace((0, 0)))
}

impl Alpha {
    pub(super) fn handle_process_private_snapshot(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let descriptor = self.read_process_memory_into_static_buffer(
            request.identifier,
            request.arg3,
            2 * core::mem::size_of::<Word>(),
        )?;
        let [source, permissions] =
            unsafe { ptr::read_unaligned(descriptor.as_ptr() as *const [Word; 2]) };
        let eager = if permissions & nanami_uapi::os::OS_PRIVATE_SNAPSHOT_DEFER != 0 {
            if permissions & !nanami_uapi::os::OS_PRIVATE_SNAPSHOT_DEFER != 1 {
                return Err(CapabilityError::InvalidArgument);
            }
            let descriptor = self.read_process_memory_into_static_buffer(request.identifier,
                request.arg3, 3 * core::mem::size_of::<Word>())?;
            Some(unsafe { ptr::read_unaligned(descriptor.as_ptr().add(2 * core::mem::size_of::<Word>()) as *const Word) })
        } else { None };
        self.map_private_snapshot_with_prefix(
            request.identifier,
            request.arg0,
            source,
            request.arg1,
            request.arg2,
            permissions & !nanami_uapi::os::OS_PRIVATE_SNAPSHOT_DEFER,
            None,
            eager,
        )
    }

    pub(super) fn map_process_private_snapshot(
        &mut self,
        caller: usize,
        target: usize,
        source: usize,
        base: usize,
        bytes: usize,
        permissions: usize,
    ) -> Result<(), CapabilityError> {
        self.map_process_private_snapshot_in_slots(caller, target, source, base, bytes, permissions, None)
    }

    pub(super) fn map_process_private_snapshot_in_slots(
        &mut self, caller: usize, target: usize, source: usize, base: usize, bytes: usize,
        permissions: usize, reserved_first: Option<usize>,
    ) -> Result<(), CapabilityError> {
        self.map_private_snapshot_with_prefix(caller, target, source, base, bytes, permissions,
            reserved_first, None)
    }

    pub(super) fn map_private_snapshot_with_prefix(
        &mut self, caller: usize, target: usize, source: usize, base: usize, bytes: usize,
        permissions: usize, reserved_first: Option<usize>, eager: Option<usize>,
    ) -> Result<(), CapabilityError> {
        if !cfg!(any(target_arch = "x86_64", test)) {
            return Err(CapabilityError::IllegalOperation);
        }
        if caller == 0
            || caller == target
            || source == 0
            || base == 0
            || bytes == 0
            || (source | base | bytes) & (PAGE_SIZE - 1) != 0
            || (permissions != 1 && permissions != 5)
        {
            return Err(CapabilityError::InvalidArgument);
        }
        if eager.is_some_and(|prefix| permissions != 1 || prefix == 0
            || prefix > bytes || prefix & (PAGE_SIZE - 1) != 0) {
            return Err(CapabilityError::InvalidArgument);
        }
        source
            .checked_add(bytes)
            .ok_or(CapabilityError::InvalidArgument)?;
        let end = base
            .checked_add(bytes)
            .ok_or(CapabilityError::InvalidArgument)?;
        self
            .processes
            .find_entry_by_pid(caller)
            .filter(|entry| {
                !entry.exited && !entry.resource_quarantined && !entry.capabilities_sealed
            })
            .ok_or(CapabilityError::PermissionDenied)?;
        self.processes
            .find_entry_by_pid(target)
            .filter(|entry| {
                !entry.exited
                    && !entry.resource_quarantined
                    && entry.capabilities_sealed
                    && entry.reaper_pid == caller
            })
            .ok_or(CapabilityError::PermissionDenied)?;
        // Keep source admission before target conflicts, but carry its result
        // through those read-only checks rather than scanning the source twice.
        let admitted = self.admit_private_snapshot_source(caller, source, bytes)?;
        let target_vm = self
            .processes
            .vm_space_mut(target)
            .ok_or(CapabilityError::InvalidArgument)?;
        if target_vm.has_backed_pages(base, end)
            || !self
                .processes
                .physical_pages_in_range(target, base, end)
                .is_empty()
            || self.processes.has_lazy_mapping_at_pages(target, base, end)
        {
            return Err(CapabilityError::IllegalOperation);
        }
        let backing = self.snapshot_source_backing(admitted)?;
        #[cfg(test)]
        BACKING_PLANS.with(|plans| {
            let (spans, vectors) = plans.get();
            plans.set(match &backing {
                SnapshotBacking::Contiguous { .. } => (spans + 1, vectors),
                SnapshotBacking::Pages(_) => (spans, vectors + 1),
            });
        });
        #[cfg(not(test))]
        if matches!(&backing, SnapshotBacking::Contiguous { .. })
            && option_env!("NANAMI_SNAPSHOT_SPAN_DIAG") == Some("1") {
            crate::force_info!("[snapshot.source-span] pid={} source-pid={} pages={} reference-vector=0",
                target, caller, backing.len());
        }
        let (root, space, first) = if let Some(first) = reserved_first {
            let (root, space) = self.processes.preallocated_process_frame_slots(
                target, first, backing.len(), PROCESS_FRAME_TOTAL_PAGES,
            ).ok_or(CapabilityError::IllegalOperation)?;
            (root, space, first)
        } else {
            self.processes.reserve_process_heap_at(
                target, base, backing.len(), PAGE_SIZE, PROCESS_FRAME_TOTAL_PAGES,
            )?
        };
        // Pool aliases own RAM references and reserved slots, but no child
        // Frame caps. Preserve root validation without creating empty nodes.
        // A later private replacement/fork still ensures its own cap nodes.
        self.process_root_slot(root)?;
        // Do not fallback-copy after this point. Ownership/caps/PTEs may have
        // committed a prefix; ordinary private mapping release/root reap owns it.
        self.invalidate_process_copy_mappings(target)?;
        let page_count = backing.len();
        let attributes = nun::capability_call::address_space::Attribute::from_bits(permissions)
            .ok_or(CapabilityError::InvalidArgument)?;
        // Source admission already validated this complete, ordered reference
        // set. No process event can intervene in this synchronous constructor.
        // Publish conservative source sharing and independently releasable
        // target references once, before any target MAP can fail. Unmapped
        // tail ownership remains visible to ordinary range/root cleanup.
        self.processes.vm_space_mut(caller).ok_or(CapabilityError::Fatal)?
            .mark_private_range_shared(source, source + bytes);
        self.processes.register_pool_aliases(target, base, first,
            (0..page_count).map(|page| backing.physical_page(page)))?;
        let mut offset = 0;
        while offset < page_count {
            let va = base + offset * PAGE_SIZE;
            let deferred = eager.is_some_and(|prefix| offset * PAGE_SIZE >= prefix);
            if deferred && va & (FRAME_WORD_BYTES - 1) == 0 && page_count - offset >= FRAME_WORD_PAGES {
                let mut frames = [0; FRAME_WORD_PAGES];
                // The contiguous plan already checked every source reference.
                // Resolve Ready conversion ranges once per word, retaining the
                // scalar path for fragmented backing or an unavailable range.
                // Nothing is published until the complete word is collected.
                let collected = match &backing {
                    SnapshotBacking::Contiguous { first, .. } => self.memory
                        .read_ready_pool_frames(first + offset, &mut frames).is_ok(),
                    SnapshotBacking::Pages(_) => false,
                };
                if !collected {
                    for (index, frame) in frames.iter_mut().enumerate() {
                        *frame = self.memory.physical_frame_descriptor_from_index(backing.physical_page(offset + index))
                            .ok_or(CapabilityError::PermissionDenied)?;
                    }
                }
                self.processes.vm_space_mut(target).ok_or(CapabilityError::Fatal)?
                    .record_deferred_frame_word(va, &frames, true)
                    .map_err(|_| CapabilityError::Fatal)?;
                offset += FRAME_WORD_PAGES;
                continue;
            }
            let frame = self.memory.physical_frame_descriptor_from_index(backing.physical_page(offset))
                .ok_or(CapabilityError::PermissionDenied)?;
            let vm = self
                .processes
                .vm_space_mut(target)
                .ok_or(CapabilityError::Fatal)?;
            vm.mark_private_page_shared(va, true);
            if deferred { vm.mark_mapping_deferred(va); }
            if !deferred && !vm.page_tables_ready(va) {
                self.memory.ensure_page_tables(space, va, vm)?;
            }
            if offset == 0 || (va >> 12) & 63 == 0 { vm.prepare_frame_word(va, page_count - offset); }
            // Record before MAP, including a possible post-install failure.
            vm.record_frame(va, frame)
                .map_err(|_| CapabilityError::Fatal)?;
            vm.set_permissions(va, permissions);
            offset += 1;
            if deferred { continue; }
            if let Err(error) = arch::address_space::map(space, frame, va, attributes) {
                vm.mark_mapping_uncertain(va);
                return Err(error);
            }
        }
        Ok(())
    }

    /// No backing or logical rights change. Cluster only initialized snapshot
    /// pages already owned by this current VM; never map PROT_NONE or uncertain
    /// leaves, and make a possibly published MAP visible to ordinary cleanup.
    pub(super) fn handle_populate_snapshot(&mut self, request: OsRequestEvent)
        -> Result<usize, CapabilityError> {
        let pid = request.arg0;
        let page = request.arg1;
        let required = request.arg2;
        if page == 0 || page & (PAGE_SIZE - 1) != 0 || !matches!(required, 1 | 2 | 4)
            || request.arg3 != 0 { return Err(CapabilityError::InvalidArgument); }
        self.processes.find_entry_by_pid(pid).filter(|entry| entry.capabilities_sealed
            && entry.reaper_pid == request.identifier && request.identifier != 0
            && request.identifier != pid && !entry.exited && !entry.resource_quarantined)
            .ok_or(CapabilityError::PermissionDenied)?;
        let context = self.processes.memory_context(pid).ok_or(CapabilityError::InvalidArgument)?;
        let vm = self.processes.vm_space(pid).ok_or(CapabilityError::InvalidArgument)?;
        if !vm.snapshot_page_managed(page) || vm.mapping_is_uncertain(page)
            || vm.permissions(page) & required == 0 { return Ok(0); }
        let frame = vm.find_frame(page).ok_or(CapabilityError::PermissionDenied)?;
        let allocation = self.processes.find_active_physical_allocation_reference(pid, page, 1)
            .ok_or(CapabilityError::PermissionDenied)?;
        let expected = if allocation.owns_frame_cap {
            Some(process_frame_descriptor(context.root_node, allocation.start_slot))
        } else { self.memory.physical_frame_descriptor_from_index(allocation.base_page) };
        if self.processes.physical_reference_count(allocation) == 0 || expected != Some(frame) {
            return Err(CapabilityError::PermissionDenied);
        }
        if !vm.mapping_deferred(page) {
            return Ok(usize::from(vm.frame_may_be_present(page)
                && vm.hardware_permissions(page) & required != 0));
        }
        // Prefetch up to 2 MiB of existing 4 KiB leaves. DSO read-only data
        // often remains in the initial reservation without another MAP_FIXED.
        // A small fault cluster makes those pages pay repeated kernel traps.
        const CLUSTER: usize = 512 * PAGE_SIZE;
        let base = page & !(CLUSTER - 1);
        let end = base.checked_add(CLUSTER).ok_or(CapabilityError::InvalidArgument)?;
        for va in (base..end).step_by(PAGE_SIZE) {
            let vm = self.processes.vm_space(pid).ok_or(CapabilityError::InvalidArgument)?;
            if !vm.mapping_deferred(va) || vm.mapping_is_uncertain(va) || vm.permissions(va) == 0 {
                continue;
            }
            let frame = vm.find_frame(va).ok_or(CapabilityError::Fatal)?;
            let permissions = vm.permissions(va);
            let allocation = self.processes.find_active_physical_allocation_reference(pid, va, 1)
                .filter(|allocation| !allocation.owns_frame_cap)
                .ok_or(CapabilityError::PermissionDenied)?;
            if self.processes.physical_reference_count(allocation) == 0
                || self.memory.physical_frame_descriptor_from_index(allocation.base_page) != Some(frame) {
                return Err(CapabilityError::PermissionDenied);
            }
            let vm = self.processes.vm_space_mut(pid).ok_or(CapabilityError::Fatal)?;
            if !vm.page_tables_ready(va) { self.memory.ensure_page_tables(context.address_space, va, vm)?; }
            let hardware = if vm.private_page_cow_write(va) { permissions & !2 } else { permissions };
            let attributes = nun::capability_call::address_space::Attribute::from_bits(hardware)
                .ok_or(CapabilityError::Fatal)?;
            if let Err(error) = arch::address_space::map(context.address_space, frame, va, attributes) {
                vm.mark_mapping_uncertain(va);
                return Err(error);
            }
            vm.complete_deferred_mapping(va);
        }
        Ok(1)
    }

    /// Source-only preflight may canonicalize ownership records, but creates
    /// no aliases. Exec may choose its copy path if this rejects the buffer,
    /// before any target snapshot mapping starts. Revalidate at each commit.
    pub(super) fn prepare_private_snapshot_source(
        &mut self, caller: usize, source: usize, bytes: usize,
    ) -> Result<(CapabilityDescriptor, Vec<crate::nanami_core::process::PhysicalPageReference>), CapabilityError> {
        let admitted = self.admit_private_snapshot_source(caller, source, bytes)?;
        self.canonical_private_snapshot_source(admitted)
    }

    /// Admission-only entrypoints discard ownership handles. Canonicalization
    /// normally inserts pages in ascending VA order, yielding a dense reverse
    /// walk. Validate that shape and every page without allocating a snapshot.
    /// Any other shape retains the stable-sort/newest-duplicate semantics below.
    pub(super) fn check_private_snapshot_source(
        &mut self, caller: usize, source: usize, bytes: usize,
    ) -> Result<(), CapabilityError> {
        let admitted = self.admit_private_snapshot_source(caller, source, bytes)?;
        self.processes.canonicalize_shared_memory(caller, admitted.reservation)?;
        let mut next = admitted.end;
        let mut valid = true;
        let mut dense = true;
        // Metadata stays immutably borrowed throughout this walk. Resolve the
        // current owner VM once, but validate each page's live Frame/sharing.
        let mut frames = self.processes.vm_space(caller).map(|vm| vm.snapshot_page_cursor());
        for reference in self.processes.private_page_reference_walk(caller, source, admitted.end) {
            let Some(expected) = next.checked_sub(PAGE_SIZE).filter(|va| *va >= source) else {
                dense = false;
                break;
            };
            if reference.allocation.base_va != expected {
                dense = false;
                break;
            }
            next = expected;
            valid &= self.snapshot_page_valid(&admitted, reference.allocation, frames.as_mut());
        }
        if dense && next == source {
            return if valid { Ok(()) } else { Err(CapabilityError::PermissionDenied) };
        }
        self.validated_snapshot_references(admitted).map(|_| ())
    }

    fn admit_private_snapshot_source(
        &mut self, caller: usize, source: usize, bytes: usize,
    ) -> Result<SnapshotSourceAdmission, CapabilityError> {
        if caller == 0 || source == 0 || bytes == 0 || (source | bytes) & (PAGE_SIZE - 1) != 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let source_end = source.checked_add(bytes).ok_or(CapabilityError::InvalidArgument)?;
        let owner = self.processes.find_entry_by_pid(caller)
            .filter(|entry| !entry.exited && !entry.resource_quarantined && !entry.capabilities_sealed)
            .ok_or(CapabilityError::PermissionDenied)?;
        let source_root = self.processes.memory_entry(owner.pid)
            .ok_or(CapabilityError::InvalidArgument)?.root_node;
        let reservation = self.processes.shared_memory_containing_range(caller, source, bytes)
            .ok_or(CapabilityError::PermissionDenied)?;
        let reservation_end = reservation.base_va + reservation.page_count * PAGE_SIZE;
        // Whole-reservation immutability and exact canonical RAM ownership are
        // the same requirements used by the public private-snapshot operation.
        let vm = self.processes.vm_space_mut(caller).ok_or(CapabilityError::InvalidArgument)?;
        let readonly = if reservation.immutable_source() {
            // Source objects cannot gain WRITE or writable aliases. A native
            // R view is optional; absent leaves are valid initialized backing.
            vm.immutable_source_backed_range(reservation.base_va, reservation_end)
        } else { vm.readonly_backed_range(reservation.base_va, reservation_end) };
        if !readonly { return Err(CapabilityError::PermissionDenied); }
        Ok(SnapshotSourceAdmission { caller, source, end: source_end, root: source_root, reservation })
    }

    fn canonical_private_snapshot_source(
        &mut self, admitted: SnapshotSourceAdmission,
    ) -> Result<(CapabilityDescriptor, Vec<crate::nanami_core::process::PhysicalPageReference>), CapabilityError> {
        self.processes.canonicalize_shared_memory(admitted.caller, admitted.reservation)?;
        self.validated_snapshot_references(admitted)
    }

    fn snapshot_source_backing(
        &mut self, admitted: SnapshotSourceAdmission,
    ) -> Result<SnapshotBacking, CapabilityError> {
        self.processes.canonicalize_shared_memory(admitted.caller, admitted.reservation)?;
        if admitted.reservation.immutable_source() {
            if let Some(first) = self.contiguous_snapshot_source(&admitted)? {
                return Ok(SnapshotBacking::Contiguous {
                    first, pages: (admitted.end - admitted.source) / PAGE_SIZE,
                });
            }
        }
        self.validated_snapshot_references(admitted)
            .map(|(_, references)| SnapshotBacking::Pages(references))
    }

    fn contiguous_snapshot_source(
        &self, admitted: &SnapshotSourceAdmission,
    ) -> Result<Option<usize>, CapabilityError> {
        let mut next = admitted.end;
        let mut first = None;
        let mut valid = true;
        let mut frames = self.processes.vm_space(admitted.caller).map(|vm| vm.snapshot_page_cursor());
        for reference in self.processes.private_page_reference_walk(admitted.caller, admitted.source, admitted.end) {
            let Some(va) = next.checked_sub(PAGE_SIZE).filter(|va| *va >= admitted.source) else {
                return Ok(None);
            };
            let page = reference.allocation;
            if page.base_va != va || page.owns_frame_cap { return Ok(None); }
            let offset = (va - admitted.source) / PAGE_SIZE;
            let Some(base) = page.base_page.checked_sub(offset) else { return Ok(None); };
            if first.is_some_and(|first| first != base) { return Ok(None); }
            first = Some(base);
            next = va;
            // Preserve every live Frame/slot/refcount check. Do not reject an
            // invalid older duplicate before the complete dense-shape proof:
            // the fallback's stable newest-record selection must still apply.
            valid &= self.snapshot_page_valid(admitted, page, frames.as_mut());
        }
        if next != admitted.source { return Ok(None); }
        if !valid { return Err(CapabilityError::PermissionDenied); }
        #[cfg(not(test))]
        if option_env!("NANAMI_SNAPSHOT_SPAN_DIAG") == Some("1") {
            if let Some(frames) = frames {
                let (words, dense, sparse) = frames.diagnostics();
                crate::force_info!("[snapshot.vm-source] source-pid={} pages={} words={} dense={} sparse-pages={}",
                    admitted.caller, (admitted.end - admitted.source) / PAGE_SIZE, words, dense, sparse);
            }
        }
        Ok(first)
    }

    #[cfg(test)]
    pub(super) fn test_snapshot_backing(
        &mut self, caller: usize, source: usize, bytes: usize,
    ) -> Result<(bool, usize), CapabilityError> {
        let admitted = self.admit_private_snapshot_source(caller, source, bytes)?;
        let backing = self.snapshot_source_backing(admitted)?;
        Ok((matches!(&backing, SnapshotBacking::Contiguous { .. }), backing.len()))
    }

    fn validated_snapshot_references(
        &self, admitted: SnapshotSourceAdmission,
    ) -> Result<(CapabilityDescriptor, Vec<crate::nanami_core::process::PhysicalPageReference>), CapabilityError> {
        let SnapshotSourceAdmission { caller, source, end: source_end, root: source_root, .. } = admitted;
        let references = self.processes.private_page_references_reserved(caller, source, source_end);
        if references.len() != (source_end - source) / PAGE_SIZE { return Err(CapabilityError::PermissionDenied); }
        let mut frames = self.processes.vm_space(caller).map(|vm| vm.snapshot_page_cursor());
        for (offset, reference) in references.iter().enumerate() {
            let page = reference.allocation;
            let va = source + offset * PAGE_SIZE;
            if page.base_va != va || !self.snapshot_page_valid(&admitted, page, frames.as_mut()) {
                return Err(CapabilityError::PermissionDenied);
            }
        }
        Ok((source_root, references))
    }

    fn snapshot_page_valid(
        &self, admitted: &SnapshotSourceAdmission,
        page: crate::nanami_core::process::ProcessPhysicalAllocation,
        frames: Option<&mut crate::nanami_core::vm_space::SnapshotPageCursor<'_>>,
    ) -> bool {
        let slot = admitted.reservation.start_slot + (page.base_va - admitted.reservation.base_va) / PAGE_SIZE;
        let count = self.processes.physical_reference_count(page);
        let Some(pool) = self.memory.physical_frame_descriptor_from_index(page.base_page) else { return false; };
        let child = process_frame_descriptor(admitted.root, slot);
        let Some(frames) = frames else { return false; };
        page.start_slot == slot && frames.page_matches(page.base_va, pool, child, count)
    }
}
