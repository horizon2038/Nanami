//! Immutable owned RAM snapshots mapped privately into supervised sealed tasks.
use super::*;

// Request-local admission only. Construct once, consume before aliasing; no
// source mutation or reentrant dispatch occurs during target conflict checks.
struct SnapshotSourceAdmission {
    caller: usize,
    source: usize,
    end: usize,
    root: CapabilityDescriptor,
    reservation: crate::nanami_core::process::SharedMemoryReservation,
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
        self.map_process_private_snapshot(
            request.identifier,
            request.arg0,
            source,
            request.arg1,
            request.arg2,
            permissions,
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
        let (_, references) = self.canonical_private_snapshot_source(admitted)?;
        let (root, space, first) = if let Some(first) = reserved_first {
            let (root, space) = self.processes.preallocated_process_frame_slots(
                target, first, references.len(), PROCESS_FRAME_TOTAL_PAGES,
            ).ok_or(CapabilityError::IllegalOperation)?;
            (root, space, first)
        } else {
            self.processes.reserve_process_heap_at(
                target, base, references.len(), PAGE_SIZE, PROCESS_FRAME_TOTAL_PAGES,
            )?
        };
        // Pool aliases own RAM references and reserved slots, but no child
        // Frame caps. Preserve root validation without creating empty nodes.
        // A later private replacement/fork still ensures its own cap nodes.
        self.process_root_slot(root)?;
        // Do not fallback-copy after this point. Ownership/caps/PTEs may have
        // committed a prefix; ordinary private mapping release/root reap owns it.
        self.invalidate_process_copy_mappings(target)?;
        let page_count = references.len();
        for (offset, reference) in references.into_iter().enumerate() {
            let page = reference.allocation;
            let va = base + offset * PAGE_SIZE;
            let slot = first + offset;
            self.processes
                .vm_space_mut(caller)
                .ok_or(CapabilityError::Fatal)?
                .mark_private_page_shared(page.base_va, true);
            self.processes
                .register_pool_alias(target, va, slot, page.base_page)?;
            let frame = self.memory.physical_frame_descriptor_from_index(page.base_page)
                .ok_or(CapabilityError::PermissionDenied)?;
            let vm = self
                .processes
                .vm_space_mut(target)
                .ok_or(CapabilityError::Fatal)?;
            vm.mark_private_page_shared(va, true);
            if !vm.page_tables_ready(va) {
                self.memory.ensure_page_tables(space, va, vm)?;
            }
            if offset == 0 || (va >> 12) & 63 == 0 { vm.prepare_frame_word(va, page_count - offset); }
            // Record before MAP, including a possible post-install failure.
            vm.record_frame(va, frame)
                .map_err(|_| CapabilityError::Fatal)?;
            vm.set_permissions(va, permissions);
            let attributes = nun::capability_call::address_space::Attribute::from_bits(permissions)
                .ok_or(CapabilityError::InvalidArgument)?;
            if let Err(error) = arch::address_space::map(space, frame, va, attributes) {
                vm.mark_mapping_uncertain(va);
                return Err(error);
            }
        }
        Ok(())
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
        let vm = self.processes.vm_space(caller);
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
            valid &= self.snapshot_page_valid(&admitted, reference.allocation, vm);
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
        if !self.processes.vm_space_mut(caller).ok_or(CapabilityError::InvalidArgument)?
            .readonly_backed_range(reservation.base_va, reservation_end)
        { return Err(CapabilityError::PermissionDenied); }
        Ok(SnapshotSourceAdmission { caller, source, end: source_end, root: source_root, reservation })
    }

    fn canonical_private_snapshot_source(
        &mut self, admitted: SnapshotSourceAdmission,
    ) -> Result<(CapabilityDescriptor, Vec<crate::nanami_core::process::PhysicalPageReference>), CapabilityError> {
        self.processes.canonicalize_shared_memory(admitted.caller, admitted.reservation)?;
        self.validated_snapshot_references(admitted)
    }

    fn validated_snapshot_references(
        &self, admitted: SnapshotSourceAdmission,
    ) -> Result<(CapabilityDescriptor, Vec<crate::nanami_core::process::PhysicalPageReference>), CapabilityError> {
        let SnapshotSourceAdmission { caller, source, end: source_end, root: source_root, .. } = admitted;
        let references = self.processes.private_page_references_reserved(caller, source, source_end);
        if references.len() != (source_end - source) / PAGE_SIZE { return Err(CapabilityError::PermissionDenied); }
        let vm = self.processes.vm_space(caller);
        for (offset, reference) in references.iter().enumerate() {
            let page = reference.allocation;
            let va = source + offset * PAGE_SIZE;
            if page.base_va != va || !self.snapshot_page_valid(&admitted, page, vm) {
                return Err(CapabilityError::PermissionDenied);
            }
        }
        Ok((source_root, references))
    }

    fn snapshot_page_valid(
        &self, admitted: &SnapshotSourceAdmission,
        page: crate::nanami_core::process::ProcessPhysicalAllocation,
        vm: Option<&crate::nanami_core::vm_space::VmSpace>,
    ) -> bool {
        let slot = admitted.reservation.start_slot + (page.base_va - admitted.reservation.base_va) / PAGE_SIZE;
        let count = self.processes.physical_reference_count(page);
        let Some(pool) = self.memory.physical_frame_descriptor_from_index(page.base_page) else { return false; };
        let child = process_frame_descriptor(admitted.root, slot);
        let Some(vm) = vm else { return false; };
        page.start_slot == slot && count != 0
            && (count == 1 || vm.private_page_shared(page.base_va))
            && vm.find_frame(page.base_va).is_some_and(|frame| frame == pool || frame == child)
    }
}
