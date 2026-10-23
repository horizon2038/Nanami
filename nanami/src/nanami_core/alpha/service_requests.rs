//! Managed public endpoints stay in Alpha's pool, not in a Provider arena.
//! This path does not interpret a device protocol or proxy normal client CALLs.
use super::*;
use crate::nanami_core::communication::bindings::Provider;

pub(super) fn grant_receiver(
    root: Word,
    slot: Word,
    endpoint: Word,
) -> Result<(), CapabilityError> {
    arch::node::copy(root, slot, endpoint)?;
    if let Err(error) = arch::node::demote(root, slot, IDENTIFIED_IPC_RIGHTS) {
        if arch::node::remove(root, slot).is_err() {
            return Err(CapabilityError::Fatal);
        }
        return Err(error);
    }
    Ok(())
}
use nanami_uapi::service::*;

impl Alpha {
    pub(super) fn service_directory(&self) -> Word {
        make_root_slot_descriptor(self.root.root_radix, SERVICE_DIRECTORY_SLOT)
    }

    pub(super) fn service_cap(&self, index: Word) -> Word {
        make_child_slot_descriptor(self.service_directory(), SERVICE_DIRECTORY_RADIX, index)
    }

    pub(super) fn notify_retired_provider_exit(&mut self, pid: Word) {
        if let Some(service) = self.communication.bindings.retired_service_for_owner(pid) {
            // The tested kernel's Node revoke leaves ordinary copied members.
            // Stop is proven here, but reaping/reusing this CSpace is not safe.
            let _ = self.processes.quarantine_resources(pid);
            if let Err(error) =
                arch::notification_port::notify(self.service_cap(64 + service.0 - 1))
            {
                error!("[binding] stopped={} wake failed: {:?}", pid, error);
            }
        }
    }

    fn service_admin(&self, pid: Word) -> Result<(), CapabilityError> {
        if !self
            .processes
            .find_entry_by_pid(pid)
            .is_some_and(|entry| pid != 0 && !entry.exited && entry.service_admin)
        {
            return Err(CapabilityError::PermissionDenied);
        }
        Ok(())
    }

    /// Create a known object, never invoke a type-specific ABI against an
    /// unverified caller-supplied capability.
    fn managed_notification(
        &mut self,
        pid: Word,
        slot: Word,
        alias: Word,
    ) -> Result<(), CapabilityError> {
        if !(23..224).contains(&slot) {
            return Err(CapabilityError::InvalidArgument);
        }
        let caller = self
            .processes
            .find_entry_by_pid(pid)
            .filter(|entry| !entry.exited)
            .ok_or(CapabilityError::InvalidArgument)?;
        let directory = self.service_directory();
        arch::generic::convert(
            self.process_arena_for_root(caller.root_node)?,
            CapabilityType::NotificationPort,
            0,
            1,
            caller.root_node,
            slot,
        )?;
        self.processes.root_types_mut(pid)?.notification(slot);
        let notification = make_child_slot_descriptor(caller.root_node, PROCESS_ROOT_RADIX, slot);
        arch::node::copy(directory, alias, notification)?;
        arch::notification_port::identify(self.service_cap(alias), BINDING_NOTIFICATION)?;
        arch::node::demote(caller.root_node, slot, IDENTIFIED_IPC_RIGHTS)?;
        Ok(())
    }

    pub(super) fn handle_provider_register(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<Word, CapabilityError> {
        if !matches!(
            request.arg3,
            STATELESS_CONTROL_PROFILE | MANAGED_BUFFER_PROFILE | MANAGED_EVENT_PROFILE
        ) {
            return Err(CapabilityError::IllegalOperation);
        }
        let pid = request.identifier;
        let interface = request.arg0;
        let notification = request.arg2;
        let (receiver, event_slot) = provider_slots(request.arg3, request.arg1, notification)
            .ok_or(CapabilityError::InvalidArgument)?;
        if let Some((id, provider)) = self.communication.bindings.provider_for_owner(pid) {
            if provider.interface == interface
                && provider.receiver_slot == receiver
                && provider.notification_slot == notification
                && provider.profile == request.arg3
                && provider.event_slot == event_slot
            {
                return Ok(id.0);
            }
            return Err(CapabilityError::PermissionDenied);
        }
        if pid == 0
            || interface == 0
            || !(23..224).contains(&receiver)
            || receiver == notification
            || !(23..224).contains(&notification)
        {
            return Err(CapabilityError::InvalidArgument);
        }
        let root = self
            .processes
            .find_entry_by_pid(pid)
            .filter(|entry| !entry.exited)
            .ok_or(CapabilityError::InvalidArgument)?
            .root_node;
        let generic = self.process_arena_for_root(root)?;
        let directory = self.service_directory();
        let types = self.processes.root_types_mut(pid)?;
        if types.occupied(receiver)
            || types.occupied(notification)
            || (event_slot != 0 && types.occupied(event_slot))
        {
            return Err(CapabilityError::InvalidArgument);
        }
        let id = self.communication.bindings.register_provider_profile(
            pid,
            interface,
            request.arg1,
            notification,
            request.arg3,
            |id| {
                arch::generic::convert(
                    generic,
                    CapabilityType::NotificationPort,
                    0,
                    1,
                    root,
                    notification,
                )?;
                types.notification(notification);
                let source = make_child_slot_descriptor(root, PROCESS_ROOT_RADIX, notification);
                arch::node::copy(directory, 32 + id.0 - 1, source)?;
                let alias =
                    make_child_slot_descriptor(directory, SERVICE_DIRECTORY_RADIX, 32 + id.0 - 1);
                arch::notification_port::identify(alias, BINDING_NOTIFICATION)?;
                arch::node::demote(root, notification, IDENTIFIED_IPC_RIGHTS)
            },
        )?;
        Ok(id.0)
    }

    pub(super) fn handle_service_publish(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<Word, CapabilityError> {
        self.service_admin(request.identifier)?;
        let (raw, len) = decode_service_name_24(request.arg0, request.arg1, request.arg2)
            .ok_or(CapabilityError::InvalidArgument)?;
        let name =
            core::str::from_utf8(&raw[..len]).map_err(|_| CapabilityError::InvalidArgument)?;
        self.communication.services.can_publish(name)?;
        let provider = ProviderId(request.arg3);
        let instance = self.communication.bindings.provider(provider)?;
        let root = self
            .processes
            .find_entry_by_pid(instance.owner)
            .filter(|entry| !entry.exited)
            .ok_or(CapabilityError::InvalidArgument)?
            .root_node;
        let directory = self.service_directory();
        let generic = self.memory.kernel_object_generic();
        let mut bindings = core::mem::replace(
            &mut self.communication.bindings,
            crate::nanami_core::communication::bindings::Bindings::new(),
        );
        let result = bindings.publish(request.identifier, provider, |id, provider| {
            let slot = id.0 - 1;
            if self.service_endpoint_poisoned[slot] {
                return Err(CapabilityError::Fatal);
            }
            if !self.service_endpoints[slot] {
                arch::generic::convert(generic, CapabilityType::IpcPort, 0, 1, directory, slot)?;
                self.service_endpoints[slot] = true;
            }
            let endpoint = make_child_slot_descriptor(directory, SERVICE_DIRECTORY_RADIX, slot);
            let result = self
                .install_service_event(id, provider)
                .and_then(|_| self.rescan_service_event(id, provider.owner))
                .and_then(|_| grant_receiver(root, provider.receiver_slot, endpoint));
            if result.is_ok() {
                self.processes
                    .root_types_mut(provider.owner)?
                    .ipc(provider.receiver_slot);
            }
            if result.is_err() && self.withdraw_service_event(id, provider).is_err() {
                self.service_endpoint_poisoned[slot] = true;
                return Err(CapabilityError::Fatal);
            }
            if matches!(result, Err(CapabilityError::Fatal)) {
                self.service_endpoint_poisoned[slot] = true;
            }
            result
        });
        self.communication.bindings = bindings;
        let id = result?;
        self.communication
            .services
            .publish(name, id.0, self.service_cap(id.0 - 1))?;
        // The public binding is committed before this wake hint. A failed hint
        // cannot roll it back or justify replay of a client's request.
        if let Err(error) = arch::notification_port::notify(self.service_cap(32 + provider.0 - 1)) {
            error!("[binding] published={} wake failed: {:?}", id.0, error);
        }
        Ok(id.0)
    }

    pub(super) fn handle_session_connect(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(Word, Word), CapabilityError> {
        let caller = self
            .processes
            .find_entry_by_pid(request.identifier)
            .filter(|entry| request.identifier != 0 && !entry.exited)
            .ok_or(CapabilityError::PermissionDenied)?;
        validate_process_device_slot(request.arg0)?;
        let (raw, len) = decode_service_name_24(request.arg1, request.arg2, request.arg3)
            .ok_or(CapabilityError::InvalidArgument)?;
        let name =
            core::str::from_utf8(&raw[..len]).map_err(|_| CapabilityError::InvalidArgument)?;
        let id = ServiceId(
            self.communication
                .services
                .managed_id(name)
                .ok_or(CapabilityError::InvalidArgument)?,
        );
        let endpoint = self.service_cap(id.0 - 1);
        let imported = self.imported_services[id.0 - 1];
        let slot = request.arg0;
        if caller.root_types.occupied(slot) {
            return Err(CapabilityError::InvalidArgument);
        }
        let result = self
            .communication
            .bindings
            .connect(caller.pid, id, slot, |session| {
                if let Some(import) = imported {
                    // MINT attenuates in one operation. COPY followed by DEMOTE
                    // cannot clean up a W|COPY alias on the tested kernel.
                    return arch::node::mint(caller.root_node, slot, import.endpoint, 0b10);
                }
                arch::node::copy(caller.root_node, slot, endpoint)?;
                let alias = make_child_slot_descriptor(caller.root_node, PROCESS_ROOT_RADIX, slot);
                let result = arch::ipc_port::identify(alias, session.0)
                    .and_then(|_| arch::node::demote(caller.root_node, slot, 0b10));
                if result.is_err() && arch::node::remove(caller.root_node, slot).is_err() {
                    return Err(CapabilityError::Fatal);
                }
                result
            });
        let session = match result {
            Ok(session) => session,
            Err(CapabilityError::Fatal) => {
                // A failed rollback may have left an overpowered alias. The
                // caller is already blocked in this OS CALL: never reply and
                // resume it with unknown authority. Retain its backing.
                if let Err(error) = arch::process_control_block::suspend(caller.pcb) {
                    panic!(
                        "cannot stop caller with uncertain IPC authority: {:?}",
                        error
                    );
                }
                self.processes
                    .mark_exited(caller.pid, 0, OS_RESPONSE_FATAL)?;
                self.processes.quarantine_resources(caller.pid)?;
                return Err(CapabilityError::Fatal);
            }
            Err(error) => return Err(error),
        };
        self.processes.root_types_mut(caller.pid)?.ipc(slot);
        Ok((session.0, id.0))
    }

    pub(super) fn handle_binding_control(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(Word, Word), CapabilityError> {
        let caller = request.identifier;
        let id = request.arg1;
        let generation = request.arg2;
        let directory = self.service_directory();
        match request.arg0 {
            CONTROL_BUFFER_CREATE | CONTROL_BUFFER_VIEW => self.handle_session_buffer(request),
            CONTROL_EVENT_CONNECT | CONTROL_PROVIDER_SESSION => self.handle_session_event(request),
            CONTROL_PROVIDER_LOOKUP => {
                self.service_admin(caller)?;
                let (id, provider) = self
                    .communication
                    .bindings
                    .provider_for_owner(id)
                    .ok_or(CapabilityError::InvalidArgument)?;
                Ok((id.0, provider.interface))
            }
            CONTROL_PROVIDER_BINDING => {
                let binding = self
                    .communication
                    .bindings
                    .provider_binding(caller, ProviderId(id))?;
                Ok((binding.service.0, binding.encode()))
            }
            CONTROL_STATE => {
                self.service_admin(caller)?;
                let service = self.communication.bindings.service(ServiceId(id))?;
                if service.manager != caller {
                    return Err(CapabilityError::PermissionDenied);
                }
                Ok((id, service.state.encode()))
            }
            CONTROL_PREPARE => {
                self.service_admin(caller)?;
                self.communication.bindings.prepare(
                    caller,
                    ServiceId(id),
                    generation,
                    ProviderId(request.arg3),
                )?;
                Ok((0, 0))
            }
            CONTROL_QUIESCE => {
                self.service_admin(caller)?;
                let service = self.communication.bindings.service(ServiceId(id))?;
                if service.manager != caller
                    || service.state.generation != generation
                    || service.state.phase != BindingPhase::Prepared
                {
                    return Err(CapabilityError::PermissionDenied);
                }
                let index = id - 1;
                if let Some(existing) = self.service_wake_slots[index] {
                    if existing != request.arg3 {
                        return Err(CapabilityError::InvalidArgument);
                    }
                } else {
                    self.managed_notification(caller, request.arg3, 64 + index)?;
                    self.service_wake_slots[index] = Some(request.arg3);
                }
                let event_signal = if service.profile == MANAGED_EVENT_PROFILE {
                    Some(self.event_cap(32 + index))
                } else {
                    None
                };
                self.communication.bindings.quiesce(
                    caller,
                    ServiceId(id),
                    generation,
                    |provider| {
                        if let Some(signal) = event_signal {
                            arch::notification_port::notify(signal)?;
                        }
                        arch::notification_port::notify(make_child_slot_descriptor(
                            directory,
                            SERVICE_DIRECTORY_RADIX,
                            32 + provider.0 - 1,
                        ))
                    },
                )?;
                Ok((0, 0))
            }
            CONTROL_QUIESCED => {
                let state = self
                    .communication
                    .bindings
                    .provider_binding(caller, ProviderId(id))?;
                if state.generation != generation || state.phase != BindingPhase::Quiescing {
                    return Err(CapabilityError::InvalidArgument);
                }
                let provider = self.communication.bindings.provider(ProviderId(id))?;
                if let Err(error) = self.withdraw_service_event(state.service, provider) {
                    self.communication.bindings.fault(state.service);
                    return Err(error);
                }
                if let Err(error) = self.withdraw_session_buffers(state.service, caller) {
                    self.communication.bindings.fault(state.service);
                    return Err(error);
                }
                let processes = &mut self.processes;
                let service = self.communication.bindings.acknowledge(
                    caller,
                    ProviderId(id),
                    generation,
                    |_, provider| {
                        let root = processes
                            .find_entry_by_pid(provider.owner)
                            .ok_or(CapabilityError::InvalidArgument)?
                            .root_node;
                        processes
                            .root_types_mut(provider.owner)?
                            .clear(provider.receiver_slot);
                        arch::node::remove(root, provider.receiver_slot)
                            .map_err(|_| CapabilityError::Fatal)
                    },
                )?;
                if let Err(error) =
                    arch::notification_port::notify(self.service_cap(64 + service.0 - 1))
                {
                    error!("[binding] quiesced={} wake failed: {:?}", service.0, error);
                }
                Ok((0, 0))
            }
            CONTROL_COMMIT | CONTROL_ABORT => {
                self.service_admin(caller)?;
                let old = self.communication.bindings.service(ServiceId(id))?.provider;
                let mut bindings = core::mem::replace(
                    &mut self.communication.bindings,
                    crate::nanami_core::communication::bindings::Bindings::new(),
                );
                let install = |service: ServiceId, provider: Provider| {
                    self.install_buffered_receiver(service, provider)
                };
                let result = if request.arg0 == CONTROL_COMMIT {
                    bindings
                        .commit(caller, ServiceId(id), generation, install)
                        .map(|_| ())
                } else {
                    bindings.abort(caller, ServiceId(id), generation, install)
                };
                self.communication.bindings = bindings;
                result?;
                let provider = self.communication.bindings.service(ServiceId(id))?.provider;
                if let Err(error) =
                    arch::notification_port::notify(self.service_cap(32 + provider.0 - 1))
                {
                    error!("[binding] committed={} wake failed: {:?}", id, error);
                }
                if old != provider {
                    if let Err(error) =
                        arch::notification_port::notify(self.service_cap(32 + old.0 - 1))
                    {
                        error!("[binding] retired={} wake failed: {:?}", old.0, error);
                    }
                }
                Ok((0, 0))
            }
            CONTROL_CLOSE => {
                let session = SessionId(id);
                let entry = self.communication.bindings.session(session)?;
                if entry.owner
                    != crate::nanami_core::communication::bindings::SessionOwner::Process(caller)
                {
                    return Err(CapabilityError::PermissionDenied);
                }
                if self
                    .managed_buffers
                    .get(session)?
                    .is_some_and(|buffer| !buffer.closed)
                    && self
                        .communication
                        .bindings
                        .service(entry.service)?
                        .state
                        .phase
                        != BindingPhase::Quiesced
                {
                    return Err(CapabilityError::IllegalOperation);
                }
                if let Err(error) = self
                    .close_session_event(session)
                    .and_then(|_| self.close_session_buffer(session))
                {
                    if matches!(error, CapabilityError::Fatal) {
                        self.communication.bindings.fault(entry.service);
                    }
                    return Err(error);
                }
                let processes = &mut self.processes;
                self.communication
                    .bindings
                    .close(caller, session, |entry| {
                        let root = processes
                            .find_entry_by_pid(match entry.owner {
                                crate::nanami_core::communication::bindings::SessionOwner::Process(pid) => pid,
                                crate::nanami_core::communication::bindings::SessionOwner::Domain(_) => {
                                    return Err(CapabilityError::PermissionDenied);
                                }
                            })
                            .ok_or(CapabilityError::InvalidArgument)?
                            .root_node;
                        let handle =
                            make_child_slot_descriptor(root, PROCESS_ROOT_RADIX, entry.slot);
                        arch::node::movec(
                            directory,
                            96 + (id & !SESSION_IDENTIFIER_BIT) - 1,
                            handle,
                        )?;
                        processes.root_types_mut(caller)?.clear(entry.slot);
                        Ok(())
                    })?;
                Ok((0, 0))
            }
            CONTROL_PROVIDER_RETIRE => {
                self.communication
                    .bindings
                    .retire_provider(caller, ProviderId(id), |id| {
                        arch::node::remove(directory, 32 + id.0 - 1)
                    })?;
                Ok((0, 0))
            }
            _ => Err(CapabilityError::InvalidArgument),
        }
    }
}
