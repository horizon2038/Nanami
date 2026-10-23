//! Maps opaque session backing without installing Frame authority in a client
//! or Provider. This is not a shared-memory queue implementation or DMA path.
use super::*;
use crate::nanami_core::communication::{
    bindings::{Provider, SessionOwner},
    buffers::{Buffers, Effects, MapError},
};
use nanami_uapi::service::*;

struct NativeEffects<'a> {
    alpha: &'a mut Alpha,
}

impl NativeEffects<'_> {
    fn frame(&mut self, physical: Word) -> Result<Word, CapabilityError> {
        self.alpha
            .memory
            .ensure_alpha_frame_at_physical_index(physical)?;
        self.alpha
            .memory
            .physical_frame_descriptor_from_index(physical)
            .ok_or(CapabilityError::InvalidDescriptor)
    }
}

impl Effects for NativeEffects<'_> {
    fn participants(&mut self, client: Word, provider: Word) -> Result<(), CapabilityError> {
        let client = self
            .alpha
            .processes
            .find_entry_by_pid(client)
            .filter(|entry| !entry.exited)
            .ok_or(CapabilityError::InvalidArgument)?;
        // A reaper retains memory-read and PCB administration authority over
        // its child even after its own Provider mapping is removed. Do not
        // represent that role combination as exclusive Provider withdrawal.
        if client.reaper_pid == provider || client.pid == provider {
            return Err(CapabilityError::PermissionDenied);
        }
        Ok(())
    }
    fn allocate(&mut self, pages: usize) -> Result<Word, CapabilityError> {
        self.alpha.memory.allocate_physical_any(pages * PAGE_SIZE)
    }
    fn reserve(&mut self, owner: Word, pages: usize) -> Result<Word, CapabilityError> {
        let entry = self
            .alpha
            .processes
            .find_entry_by_pid(owner)
            .filter(|entry| !entry.exited)
            .ok_or(CapabilityError::InvalidArgument)?;
        if entry.pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        self.alpha
            .processes
            .reserve_process_virtual_gap(owner, pages, PAGE_SIZE)
    }
    fn zero(&mut self, physical: Word) -> Result<(), CapabilityError> {
        let frame = self.frame(physical)?;
        self.alpha
            .map_alpha_temporary_frame(frame, PROCESS_ZERO_TEMP_BASE)?;
        unsafe {
            ptr::write_bytes(PROCESS_ZERO_TEMP_BASE as *mut u8, 0, PAGE_SIZE);
        }
        self.alpha
            .unmap_alpha_temporary_frame(frame, PROCESS_ZERO_TEMP_BASE)
    }
    fn map(&mut self, owner: Word, address: Word, physical: Word) -> Result<(), MapError> {
        let frame = self.frame(physical).map_err(MapError::NotInstalled)?;
        let entry = self
            .alpha
            .processes
            .find_entry_by_pid(owner)
            .filter(|entry| !entry.exited)
            .ok_or(MapError::NotInstalled(CapabilityError::InvalidArgument))?;
        let vm = self
            .alpha
            .processes
            .vm_space_mut(owner)
            .ok_or(MapError::NotInstalled(CapabilityError::InvalidArgument))?;
        if vm.find_frame(address).is_some() {
            return Err(MapError::NotInstalled(CapabilityError::IllegalOperation));
        }
        self.alpha
            .memory
            .ensure_page_tables(entry.address_space, address, vm)
            .map_err(MapError::NotInstalled)?;
        arch::address_space::map(
            entry.address_space,
            frame,
            address,
            nun::capability_call::address_space::Attribute::READ
                | nun::capability_call::address_space::Attribute::WRITE,
        )
        .map_err(|error| {
            if matches!(error, CapabilityError::Fatal) {
                MapError::MaybeInstalled(error)
            } else {
                MapError::NotInstalled(error)
            }
        })?;
        vm.record_frame(address, frame)
            .map_err(|_| MapError::MaybeInstalled(CapabilityError::InvalidArgument))
    }
    fn unmap(&mut self, owner: Word, address: Word, physical: Word) -> Result<(), CapabilityError> {
        self.alpha.invalidate_process_copy_mappings(owner)?;
        let frame = self.frame(physical)?;
        let entry = self
            .alpha
            .processes
            .find_entry_by_pid(owner)
            .ok_or(CapabilityError::InvalidArgument)?;
        arch::address_space::unmap(entry.address_space, frame, address)?;
        self.alpha
            .processes
            .vm_space_mut(owner)
            .ok_or(CapabilityError::InvalidArgument)?
            .forget_frame(address);
        Ok(())
    }
    fn receiver(
        &mut self,
        service: ServiceId,
        owner: Word,
        slot: Word,
    ) -> Result<(), CapabilityError> {
        let root = self
            .alpha
            .processes
            .find_entry_by_pid(owner)
            .filter(|entry| !entry.exited)
            .ok_or(CapabilityError::InvalidArgument)?
            .root_node;
        // Seed a rescan before publishing receive authority. Pending work may
        // have coalesced with management or arrived while no Provider was bound.
        self.alpha.rescan_service_event(service, owner)?;
        super::service_requests::grant_receiver(root, slot, self.alpha.service_cap(service.0 - 1))?;
        self.alpha.processes.root_types_mut(owner)?.ipc(slot);
        Ok(())
    }
}

impl Alpha {
    fn with_buffers<T>(
        &mut self,
        action: impl FnOnce(&mut Buffers, &mut NativeEffects<'_>) -> Result<T, CapabilityError>,
    ) -> Result<T, CapabilityError> {
        // Alpha is single-threaded. No event-loop reentry or external Alpha RPC
        // happens inside these effects; restore ownership on every Result path.
        let mut buffers = core::mem::replace(&mut self.managed_buffers, Buffers::new());
        let result = action(&mut buffers, &mut NativeEffects { alpha: self });
        self.managed_buffers = buffers;
        result
    }

    pub(super) fn handle_session_buffer(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(Word, Word), CapabilityError> {
        let id = SessionId(request.arg1);
        let session = self.communication.bindings.session(id)?;
        let service = self.communication.bindings.service(session.service)?;
        if session.closed
            || !matches!(
                service.profile,
                MANAGED_BUFFER_PROFILE | MANAGED_EVENT_PROFILE
            )
        {
            return Err(CapabilityError::IllegalOperation);
        }
        let provider = self.communication.bindings.provider(service.provider)?;
        if request.arg0 == CONTROL_BUFFER_VIEW {
            if provider.owner != request.identifier || !service.state.phase.permits_buffer_view() {
                return Err(CapabilityError::PermissionDenied);
            }
            let view = self.managed_buffers.view(id, provider.owner)?;
            return Ok((view.address, view.bytes));
        }
        if !matches!(
            service.state.phase,
            BindingPhase::Active | BindingPhase::Prepared
        ) {
            return Err(CapabilityError::IllegalOperation);
        }
        if session.owner != SessionOwner::Process(request.identifier) {
            return Err(CapabilityError::PermissionDenied);
        }
        if request.arg2 == 0 || request.arg2 > SESSION_BUFFER_MAX_BYTES || request.arg3 != 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let pages = request.arg2.div_ceil(PAGE_SIZE);
        let result = self.with_buffers(|buffers, effects| {
            buffers.create(
                id,
                session.service,
                request.identifier,
                provider.owner,
                pages,
                effects,
            )
        });
        if matches!(result, Err(CapabilityError::Fatal)) {
            self.communication.bindings.fault(session.service);
        }
        let view = result?;
        self.rescan_service_event(session.service, provider.owner)?;
        Ok((view.address, view.bytes))
    }

    pub(super) fn withdraw_session_buffers(
        &mut self,
        service: ServiceId,
        provider: Word,
    ) -> Result<(), CapabilityError> {
        self.with_buffers(|buffers, effects| buffers.withdraw(service, provider, effects))
    }

    pub(super) fn install_buffered_receiver(
        &mut self,
        service: ServiceId,
        provider: Provider,
    ) -> Result<(), CapabilityError> {
        self.install_service_event(service, provider)?;
        let result = self.with_buffers(|buffers, effects| {
            buffers.install(service, provider.owner, provider.receiver_slot, effects)
        });
        if result.is_err() && self.withdraw_service_event(service, provider).is_err() {
            return Err(CapabilityError::Fatal);
        }
        result
    }

    pub(super) fn close_session_buffer(
        &mut self,
        session: SessionId,
    ) -> Result<(), CapabilityError> {
        self.with_buffers(|buffers, effects| buffers.close(session, effects))
    }
}
