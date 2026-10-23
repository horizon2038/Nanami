//! Service-owned, coalescing work hints. No queue layout or device protocol.
//! Client WRITE-only aliases remain charged after close on the pinned kernel.
use super::bindings::{SERVICES, SESSIONS};
use nanami_uapi::service::{ServiceId, SessionId, SESSION_IDENTIFIER_BIT};
use nun::{CapabilityError, Word};

#[derive(Clone, Copy, Debug, Default)]
struct Event {
    initialized: bool,
    receiver: Option<(Word, Word)>,
    faulted: bool,
}

#[derive(Clone, Copy, Debug)]
struct Client {
    service: ServiceId,
    owner: Word,
    slot: Word,
    ready: bool,
    closed: bool,
}

pub trait Effects {
    fn create(&mut self, service: ServiceId) -> Result<(), CapabilityError>;
    fn install(
        &mut self,
        service: ServiceId,
        owner: Word,
        slot: Word,
    ) -> Result<(), CapabilityError>;
    /// Rebind the stopped-in-ACK receiver to its private management object
    /// before removing its public event alias, including the PCB-held alias.
    fn withdraw(
        &mut self,
        service: ServiceId,
        owner: Word,
        slot: Word,
    ) -> Result<(), CapabilityError>;
    fn client(
        &mut self,
        service: ServiceId,
        owner: Word,
        slot: Word,
    ) -> Result<(), CapabilityError>;
    fn close(&mut self, session: SessionId, owner: Word, slot: Word)
        -> Result<(), CapabilityError>;
}

pub struct Events {
    services: [Event; SERVICES],
    clients: [Option<Client>; SESSIONS],
}

fn service_index(id: ServiceId) -> Result<usize, CapabilityError> {
    id.0.checked_sub(1)
        .filter(|index| *index < SERVICES)
        .ok_or(CapabilityError::InvalidArgument)
}

fn session_index(id: SessionId) -> Result<usize, CapabilityError> {
    if id.0 & SESSION_IDENTIFIER_BIT == 0 {
        return Err(CapabilityError::InvalidArgument);
    }
    (id.0 & !SESSION_IDENTIFIER_BIT)
        .checked_sub(1)
        .filter(|index| *index < SESSIONS)
        .ok_or(CapabilityError::InvalidArgument)
}

impl Events {
    pub const fn new() -> Self {
        Self {
            services: [Event {
                initialized: false,
                receiver: None,
                faulted: false,
            }; SERVICES],
            clients: [None; SESSIONS],
        }
    }

    pub fn has_receiver(&self, service: ServiceId, owner: Word) -> bool {
        service_index(service).ok().is_some_and(|index| {
            let event = self.services[index];
            !event.faulted && event.receiver.is_some_and(|receiver| receiver.0 == owner)
        })
    }

    pub fn owns_process_slot(&self, owner: Word, slot: Word) -> bool {
        self.clients
            .iter()
            .flatten()
            .any(|client| !client.closed && client.owner == owner && client.slot == slot)
    }

    pub fn install(
        &mut self,
        service: ServiceId,
        owner: Word,
        slot: Word,
        effects: &mut impl Effects,
    ) -> Result<(), CapabilityError> {
        let event = &mut self.services[service_index(service)?];
        if event.faulted {
            return Err(CapabilityError::Fatal);
        }
        if owner == 0 || !(23..224).contains(&slot) || event.receiver.is_some() {
            return Err(CapabilityError::InvalidArgument);
        }
        if !event.initialized {
            if effects.create(service).is_err() {
                // Construction may have created some objects/aliases. Never
                // reuse these slots or pretend their backing was reclaimed.
                event.faulted = true;
                return Err(CapabilityError::Fatal);
            }
            event.initialized = true;
        }
        if let Err(error) = effects.install(service, owner, slot) {
            event.faulted = matches!(error, CapabilityError::Fatal);
            return Err(error);
        }
        event.receiver = Some((owner, slot));
        Ok(())
    }

    pub fn withdraw(
        &mut self,
        service: ServiceId,
        owner: Word,
        effects: &mut impl Effects,
    ) -> Result<(), CapabilityError> {
        let event = &mut self.services[service_index(service)?];
        if event.faulted {
            return Err(CapabilityError::Fatal);
        }
        let Some((receiver, slot)) = event.receiver else {
            return Ok(());
        };
        if receiver != owner {
            return Err(CapabilityError::PermissionDenied);
        }
        if effects.withdraw(service, owner, slot).is_err() {
            event.faulted = true;
            return Err(CapabilityError::Fatal);
        }
        event.receiver = None;
        Ok(())
    }

    pub fn connect(
        &mut self,
        session: SessionId,
        service: ServiceId,
        owner: Word,
        slot: Word,
        effects: &mut impl Effects,
    ) -> Result<(), CapabilityError> {
        let index = session_index(session)?;
        let event = &mut self.services[service_index(service)?];
        if event.faulted {
            return Err(CapabilityError::Fatal);
        }
        if !event.initialized || owner == 0 || !(23..224).contains(&slot) {
            return Err(CapabilityError::InvalidArgument);
        }
        if let Some(client) = self.clients[index] {
            if client.service != service || client.owner != owner || client.slot != slot {
                return Err(CapabilityError::PermissionDenied);
            }
            if client.ready && !client.closed {
                return Ok(());
            }
            if !client.ready {
                return Err(CapabilityError::Fatal);
            }
            return Err(CapabilityError::IllegalOperation);
        }
        let result = effects.client(service, owner, slot);
        if result.is_ok() || matches!(result, Err(CapabilityError::Fatal)) {
            self.clients[index] = Some(Client {
                service,
                owner,
                slot,
                ready: result.is_ok(),
                closed: false,
            });
        }
        if matches!(result, Err(CapabilityError::Fatal)) {
            event.faulted = true;
        }
        result
    }

    pub fn close(
        &mut self,
        session: SessionId,
        effects: &mut impl Effects,
    ) -> Result<(), CapabilityError> {
        let Some(client) = self.clients[session_index(session)?].as_mut() else {
            return Ok(());
        };
        if client.closed {
            return Ok(());
        }
        if !client.ready {
            return Err(CapabilityError::Fatal);
        }
        if let Err(error) = effects.close(session, client.owner, client.slot) {
            if matches!(error, CapabilityError::Fatal) {
                self.services[service_index(client.service)?].faulted = true;
            }
            return Err(error);
        }
        client.closed = true;
        Ok(())
    }
}
