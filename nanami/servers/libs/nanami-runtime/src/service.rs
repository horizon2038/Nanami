//! Managed Service control. Normal requests use the granted endpoint directly.
use crate::{call_os_port, pack_service_name_24, RequestError, Word, OS_RESPONSE_OK};
pub use nanami_uapi::service::*;

fn response(result: (Word, Word, Word)) -> Result<(Word, Word), RequestError> {
    if result.0 != OS_RESPONSE_OK {
        return Err(RequestError::Status(result.0));
    }
    Ok((result.1, result.2))
}

pub fn control(
    operation: Word,
    id: Word,
    generation: Word,
    argument: Word,
) -> Result<(Word, Word), RequestError> {
    response(call_os_port(
        OS_REQUEST_BINDING_CONTROL,
        operation,
        id,
        generation,
        argument,
        5,
    )?)
}

pub fn register_provider(
    interface: Word,
    receiver_slot: Word,
    notification_slot: Word,
) -> Result<ProviderId, RequestError> {
    register_provider_profile(
        interface,
        receiver_slot,
        notification_slot,
        STATELESS_CONTROL_PROFILE,
    )
}

pub fn register_provider_profile(
    interface: Word,
    receiver_slot: Word,
    notification_slot: Word,
    profile: Word,
) -> Result<ProviderId, RequestError> {
    let result = response(call_os_port(
        OS_REQUEST_PROVIDER_REGISTER,
        interface,
        receiver_slot,
        notification_slot,
        profile,
        5,
    )?)?;
    Ok(ProviderId(result.0))
}

pub fn create_buffer(session: SessionId, bytes: Word) -> Result<SessionBuffer, RequestError> {
    let (address, bytes) = control(CONTROL_BUFFER_CREATE, session.0, bytes, 0)?;
    Ok(SessionBuffer { address, bytes })
}

pub fn session_buffer(session: SessionId) -> Result<SessionBuffer, RequestError> {
    let (address, bytes) = control(CONTROL_BUFFER_VIEW, session.0, 0, 0)?;
    Ok(SessionBuffer { address, bytes })
}

/// Grant one stable WRITE-only pending-work handle for this local Session.
/// Queue contents/cursors, not Notification counts, are authoritative.
pub fn connect_event(session: SessionId, destination_slot: Word) -> Result<Word, RequestError> {
    control(CONTROL_EVENT_CONNECT, session.0, destination_slot, 0)?;
    Ok(crate::ipc::process_slot_descriptor(destination_slot))
}

/// Enumerate current opaque data Sessions only when a resource-rescan hint is
/// received. Ordinary work notifications need no Alpha metadata request.
pub fn buffer_session(
    provider: ProviderId,
    ordinal: Word,
) -> Result<Option<SessionId>, RequestError> {
    let (session, _) = control(CONTROL_PROVIDER_SESSION, provider.0, ordinal, 0)?;
    Ok((session != 0).then_some(SessionId(session)))
}

pub fn publish(name: &str, provider: ProviderId) -> Result<ServiceId, RequestError> {
    let (a, b, c) = pack_service_name_24(name)?;
    Ok(ServiceId(
        response(call_os_port(
            OS_REQUEST_SERVICE_PUBLISH,
            a,
            b,
            c,
            provider.0,
            5,
        )?)?
        .0,
    ))
}

pub fn connect(name: &str, destination_slot: Word) -> Result<Connection, RequestError> {
    let (a, b, c) = pack_service_name_24(name)?;
    let (session, service) = response(call_os_port(
        OS_REQUEST_SESSION_CONNECT,
        destination_slot,
        a,
        b,
        c,
        5,
    )?)?;
    Ok(Connection {
        endpoint: crate::ipc::process_slot_descriptor(destination_slot),
        session: SessionId(session),
        service: ServiceId(service),
    })
}

pub fn provider_lookup(local_pid: Word) -> Result<(ProviderId, Word), RequestError> {
    let (id, interface) = control(CONTROL_PROVIDER_LOOKUP, local_pid, 0, 0)?;
    Ok((ProviderId(id), interface))
}

pub fn provider_binding(provider: ProviderId) -> Result<BindingState, RequestError> {
    let (service, state) = control(CONTROL_PROVIDER_BINDING, provider.0, 0, 0)?;
    Ok(BindingState::decode(ServiceId(service), state))
}

pub fn binding_state(service: ServiceId) -> Result<BindingState, RequestError> {
    let (service, state) = control(CONTROL_STATE, service.0, 0, 0)?;
    Ok(BindingState::decode(ServiceId(service), state))
}

pub fn prepare(state: BindingState, provider: ProviderId) -> Result<(), RequestError> {
    control(
        CONTROL_PREPARE,
        state.service.0,
        state.generation,
        provider.0,
    )
    .map(|_| ())
}

/// Alpha creates the notification in an empty slot on the first transaction;
/// reuse that same slot for subsequent transactions of this Service.
pub fn quiesce(state: BindingState, notification_slot: Word) -> Result<(), RequestError> {
    control(
        CONTROL_QUIESCE,
        state.service.0,
        state.generation,
        notification_slot,
    )
    .map(|_| ())
}

pub fn commit(state: BindingState) -> Result<(), RequestError> {
    control(CONTROL_COMMIT, state.service.0, state.generation, 0).map(|_| ())
}

pub fn abort(state: BindingState) -> Result<(), RequestError> {
    control(CONTROL_ABORT, state.service.0, state.generation, 0).map(|_| ())
}

/// Withdraws the client handle. Its WO alias is retained in bounded quarantine,
/// not reclaimed/reused, because the current kernel refuses attenuated deletion.
pub fn close(session: SessionId) -> Result<(), RequestError> {
    control(CONTROL_CLOSE, session.0, 0, 0).map(|_| ())
}

pub fn retire_provider(provider: ProviderId) -> Result<(), RequestError> {
    control(CONTROL_PROVIDER_RETIRE, provider.0, 0, 0).map(|_| ())
}

pub enum ProviderEvent {
    Request(crate::ipc::ServiceRequest),
    Event(crate::ipc::ServiceEvent),
    Retired,
}

/// Sequential cooperative receiver. It never acknowledges quiescence while a
/// reply is pending, and never retries a request after an uncertain reply error.
pub struct Receiver {
    pub id: ProviderId,
    endpoint: Word,
    notification: Word,
    event_notification: Option<Word>,
    bound_to_event: bool,
    pending_events: Word,
    binding: Option<BindingState>,
    was_bound: bool,
    reply_pending: bool,
    receives_until_poll: usize,
}

impl Receiver {
    pub fn register(
        interface: Word,
        receiver_slot: Word,
        notification_slot: Word,
    ) -> Result<Self, RequestError> {
        Self::register_profile(
            interface,
            receiver_slot,
            notification_slot,
            STATELESS_CONTROL_PROFILE,
            None,
        )
    }

    pub fn register_buffered(
        interface: Word,
        receiver_slot: Word,
        notification_slot: Word,
    ) -> Result<Self, RequestError> {
        Self::register_profile(
            interface,
            receiver_slot,
            notification_slot,
            MANAGED_BUFFER_PROFILE,
            None,
        )
    }

    pub fn register_buffered_events(
        interface: Word,
        receiver_slot: Word,
        notification_slot: Word,
        event_slot: Word,
    ) -> Result<Self, RequestError> {
        if !(23..224).contains(&event_slot) || !(23..224).contains(&receiver_slot) {
            return Err(RequestError::InvalidArgument);
        }
        Self::register_profile(
            interface,
            receiver_slot | (event_slot << 8),
            notification_slot,
            MANAGED_EVENT_PROFILE,
            Some(crate::ipc::process_slot_descriptor(event_slot)),
        )
    }

    fn register_profile(
        interface: Word,
        receiver_slot: Word,
        notification_slot: Word,
        profile: Word,
        event_notification: Option<Word>,
    ) -> Result<Self, RequestError> {
        let (receiver, _) = provider_slots(profile, receiver_slot, notification_slot)
            .ok_or(RequestError::InvalidArgument)?;
        let id = register_provider_profile(interface, receiver_slot, notification_slot, profile)?;
        let notification = crate::ipc::process_slot_descriptor(notification_slot);
        crate::ipc::bind_current_thread_notification(notification)?;
        Ok(Self {
            id,
            endpoint: crate::ipc::process_slot_descriptor(receiver),
            notification,
            event_notification,
            bound_to_event: false,
            pending_events: 0,
            binding: None,
            was_bound: false,
            reply_pending: false,
            receives_until_poll: 0,
        })
    }

    fn active_notification(&self) -> Word {
        if self.bound_to_event {
            self.event_notification.unwrap()
        } else {
            self.notification
        }
    }

    fn bind_event(&mut self, active: bool) -> Result<(), RequestError> {
        if self.event_notification.is_some() && self.bound_to_event != active {
            let notification = if active {
                self.event_notification.unwrap()
            } else {
                self.notification
            };
            crate::ipc::bind_current_thread_notification(notification)?;
            self.bound_to_event = active;
        }
        Ok(())
    }

    pub fn receive(&mut self) -> Result<ProviderEvent, RequestError> {
        if self.reply_pending {
            return Err(RequestError::Protocol);
        }
        loop {
            // A9N's RECEIVE consumes a queued sender before checking the bound
            // notification. Bound management work must not starve under a
            // continuous CALL stream. One nonblocking kernel poll per batch
            // bounds this without an Alpha RPC on every normal request.
            if self.receives_until_poll == 0 {
                let pending = crate::ipc::notification_poll(self.active_notification())?;
                self.receives_until_poll = 32;
                if pending & BINDING_NOTIFICATION != 0 {
                    self.binding = None;
                }
                self.pending_events |= pending & !BINDING_NOTIFICATION;
            }
            if self.binding.is_none() {
                match provider_binding(self.id) {
                    Ok(state) => {
                        if state.phase == BindingPhase::Quiescing {
                            // Detach the PCB-held alias before Alpha removes
                            // the visible event capability and data mappings.
                            self.bind_event(false)?;
                            control(CONTROL_QUIESCED, self.id.0, state.generation, 0)?;
                            if self.event_notification.is_some() {
                                // Every reactivation seeds a resource rescan;
                                // authoritative queue cursors remain in data.
                                self.pending_events = 0;
                            }
                            continue;
                        }
                        if matches!(state.phase, BindingPhase::Quiesced | BindingPhase::Faulted) {
                            self.bind_event(false)?;
                            crate::ipc::notification_wait(self.notification)?;
                            continue;
                        }
                        self.was_bound = true;
                        self.bind_event(true)?;
                        self.binding = Some(state);
                    }
                    Err(RequestError::Status(crate::OS_RESPONSE_INVALID_ARGUMENT)) => {
                        if self.was_bound {
                            return Ok(ProviderEvent::Retired);
                        }
                        crate::ipc::notification_wait(self.notification)?;
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
            if self.pending_events != 0 {
                let identifier = core::mem::take(&mut self.pending_events);
                return Ok(ProviderEvent::Event(
                    crate::ipc::ServiceEvent::Notification {
                        identifier,
                        value: 0,
                    },
                ));
            }
            self.receives_until_poll -= 1;
            let event = crate::ipc::service_receive_event(self.endpoint)?;
            match event {
                crate::ipc::ServiceEvent::Notification { identifier, .. }
                    if identifier & BINDING_NOTIFICATION != 0 =>
                {
                    self.pending_events |= identifier & !BINDING_NOTIFICATION;
                    self.binding = None;
                }
                crate::ipc::ServiceEvent::Request(request) => {
                    self.reply_pending = true;
                    return Ok(ProviderEvent::Request(request));
                }
                event @ crate::ipc::ServiceEvent::Fault { .. } => {
                    self.reply_pending = true;
                    return Ok(ProviderEvent::Event(event));
                }
                event => return Ok(ProviderEvent::Event(event)),
            }
        }
    }

    pub fn reply(&mut self, status: Word, first: Word, second: Word) -> Result<(), RequestError> {
        if !self.reply_pending {
            return Err(RequestError::Protocol);
        }
        crate::ipc::service_reply(self.endpoint, status, first, second)?;
        self.reply_pending = false;
        Ok(())
    }
}
