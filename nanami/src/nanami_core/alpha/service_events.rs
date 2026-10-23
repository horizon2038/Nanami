//! Managed work notifications use Alpha-owned objects and direct client NOTIFY.
//! Slot-local identities distinguish work, management and resource rescan hints.
use super::*;
use crate::nanami_core::communication::{
    bindings::{Provider, SessionOwner},
    events::{Effects, Events},
};
use nanami_uapi::service::*;

struct NativeEffects<'a> {
    alpha: &'a mut Alpha,
    provider: Option<Provider>,
}

impl NativeEffects<'_> {
    fn root(&self, owner: Word) -> Result<Word, CapabilityError> {
        self.alpha
            .processes
            .find_entry_by_pid(owner)
            .filter(|entry| owner != 0 && !entry.exited)
            .map(|entry| entry.root_node)
            .ok_or(CapabilityError::InvalidArgument)
    }
}

impl Effects for NativeEffects<'_> {
    fn create(&mut self, service: ServiceId) -> Result<(), CapabilityError> {
        if !self.alpha.event_directory_ready {
            arch::generic::convert(
                self.alpha.memory.kernel_object_generic(),
                CapabilityType::Node,
                SERVICE_DIRECTORY_RADIX,
                1,
                self.alpha.root.root_descriptor,
                EVENT_DIRECTORY_SLOT,
            )?;
            self.alpha.event_directory_ready = true;
        }
        let directory = self.alpha.event_directory();
        let index = service.0 - 1;
        arch::generic::convert(
            self.alpha.memory.kernel_object_generic(),
            CapabilityType::NotificationPort,
            0,
            1,
            directory,
            index,
        )?;
        let source = self.alpha.event_cap(index);
        arch::notification_port::identify(source, SERVICE_WORK_NOTIFICATION)?;
        for (offset, identifier) in [
            (32, BINDING_NOTIFICATION),
            (64, SERVICE_RESOURCE_NOTIFICATION),
        ] {
            arch::node::copy(directory, offset + index, source)?;
            arch::notification_port::identify(self.alpha.event_cap(offset + index), identifier)?;
        }
        Ok(())
    }
    fn install(
        &mut self,
        service: ServiceId,
        owner: Word,
        slot: Word,
    ) -> Result<(), CapabilityError> {
        let root = self.root(owner)?;
        super::service_requests::grant_receiver(root, slot, self.alpha.event_cap(service.0 - 1))?;
        self.alpha
            .processes
            .root_types_mut(owner)?
            .notification(slot);
        Ok(())
    }
    fn withdraw(&mut self, _: ServiceId, owner: Word, slot: Word) -> Result<(), CapabilityError> {
        let provider = self
            .provider
            .filter(|provider| provider.owner == owner)
            .ok_or(CapabilityError::InvalidArgument)?;
        let entry = self
            .alpha
            .processes
            .find_entry_by_pid(owner)
            .ok_or(CapabilityError::InvalidArgument)?;
        // CONFIGURE validates the source's Notification type in the kernel;
        // never dispatch a Notification opcode against an unverified slot.
        // Also remove the PCB's internal event alias, not only its visible slot.
        let config = nun::capability_call::process_control_block::ConfigurationInfo::new(
            false, false, false, true, false, false, false, false, false, false,
        );
        arch::process_control_block::configure(
            entry.pcb,
            config,
            0,
            0,
            0,
            make_child_slot_descriptor(
                entry.root_node,
                PROCESS_ROOT_RADIX,
                provider.notification_slot,
            ),
            0,
            0,
            0,
            0,
            0,
            0,
        )?;
        self.alpha.processes.root_types_mut(owner)?.clear(slot);
        arch::node::remove(entry.root_node, slot)
    }
    fn client(
        &mut self,
        service: ServiceId,
        owner: Word,
        slot: Word,
    ) -> Result<(), CapabilityError> {
        let root = self.root(owner)?;
        if self.alpha.processes.root_types_mut(owner)?.occupied(slot) {
            return Err(CapabilityError::InvalidArgument);
        }
        arch::node::mint(root, slot, self.alpha.event_cap(service.0 - 1), 0b10)?;
        self.alpha
            .processes
            .root_types_mut(owner)?
            .notification(slot);
        Ok(())
    }
    fn close(
        &mut self,
        session: SessionId,
        owner: Word,
        slot: Word,
    ) -> Result<(), CapabilityError> {
        let root = self.root(owner)?;
        let source = make_child_slot_descriptor(root, PROCESS_ROOT_RADIX, slot);
        arch::node::movec(
            self.alpha.event_directory(),
            96 + (session.0 & !SESSION_IDENTIFIER_BIT) - 1,
            source,
        )?;
        self.alpha.processes.root_types_mut(owner)?.clear(slot);
        Ok(())
    }
}

impl Alpha {
    pub(super) fn event_directory(&self) -> Word {
        make_root_slot_descriptor(self.root.root_radix, EVENT_DIRECTORY_SLOT)
    }
    pub(super) fn event_cap(&self, index: Word) -> Word {
        make_child_slot_descriptor(self.event_directory(), SERVICE_DIRECTORY_RADIX, index)
    }
    fn with_events<T>(
        &mut self,
        provider: Option<Provider>,
        action: impl FnOnce(&mut Events, &mut NativeEffects<'_>) -> Result<T, CapabilityError>,
    ) -> Result<T, CapabilityError> {
        let mut events = core::mem::replace(&mut self.managed_events, Events::new());
        let result = action(
            &mut events,
            &mut NativeEffects {
                alpha: self,
                provider,
            },
        );
        self.managed_events = events;
        result
    }
    pub(super) fn install_service_event(
        &mut self,
        service: ServiceId,
        provider: Provider,
    ) -> Result<(), CapabilityError> {
        if provider.profile != MANAGED_EVENT_PROFILE {
            return Ok(());
        }
        self.with_events(Some(provider), |events, effects| {
            events.install(service, provider.owner, provider.event_slot, effects)
        })
    }
    pub(super) fn withdraw_service_event(
        &mut self,
        service: ServiceId,
        provider: Provider,
    ) -> Result<(), CapabilityError> {
        if provider.profile != MANAGED_EVENT_PROFILE {
            return Ok(());
        }
        self.with_events(Some(provider), |events, effects| {
            events.withdraw(service, provider.owner, effects)
        })
    }
    pub(super) fn rescan_service_event(
        &mut self,
        service: ServiceId,
        owner: Word,
    ) -> Result<(), CapabilityError> {
        if self.managed_events.has_receiver(service, owner) {
            arch::notification_port::notify(self.event_cap(64 + service.0 - 1))?;
        }
        Ok(())
    }
    pub(super) fn close_session_event(
        &mut self,
        session: SessionId,
    ) -> Result<(), CapabilityError> {
        self.with_events(None, |events, effects| events.close(session, effects))
    }
    pub(super) fn handle_session_event(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(Word, Word), CapabilityError> {
        if request.arg0 == CONTROL_PROVIDER_SESSION {
            let provider = ProviderId(request.arg1);
            let instance = self.communication.bindings.provider(provider)?;
            let state = self
                .communication
                .bindings
                .provider_binding(request.identifier, provider)?;
            if instance.profile != MANAGED_EVENT_PROFILE || !state.phase.permits_buffer_view() {
                return Err(CapabilityError::PermissionDenied);
            }
            let session = self.managed_buffers.session_at(state.service, request.arg2);
            return Ok((session.map_or(0, |id| id.0), 0));
        }
        let session = SessionId(request.arg1);
        let record = self.communication.bindings.session(session)?;
        let service = self.communication.bindings.service(record.service)?;
        if record.closed || record.owner != SessionOwner::Process(request.identifier) {
            return Err(CapabilityError::PermissionDenied);
        }
        if service.profile != MANAGED_EVENT_PROFILE
            || !matches!(
                service.state.phase,
                BindingPhase::Active | BindingPhase::Prepared
            )
            || request.arg3 != 0
        {
            return Err(CapabilityError::IllegalOperation);
        }
        let result = self.with_events(None, |events, effects| {
            events.connect(
                session,
                record.service,
                request.identifier,
                request.arg2,
                effects,
            )
        });
        if matches!(result, Err(CapabilityError::Fatal)) {
            self.communication.bindings.fault(record.service);
        }
        result?;
        Ok((0, 0))
    }
}
