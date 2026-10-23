//! Resource-bounded local identity/binding state. Capability effects are supplied
//! by Alpha and must succeed before a public state transition is committed.
use nanami_uapi::service::{
    BindingPhase, BindingState, ProviderId, ServiceId, SessionId, SESSION_IDENTIFIER_BIT,
};
use nun::{CapabilityError, Word};

pub const PROVIDERS: usize = 32;
pub const SERVICES: usize = 32;
pub const SESSIONS: usize = 64;

#[derive(Clone, Copy, Debug)]
pub struct Provider {
    pub owner: Word,
    pub interface: Word,
    pub profile: Word,
    pub receiver_slot: Word,
    pub notification_slot: Word,
    pub event_slot: Word,
    pub service: Option<ServiceId>,
    pub last_service: Option<ServiceId>,
    pub retired: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Service {
    pub manager: Word,
    pub provider: ProviderId,
    pub pending: Option<ProviderId>,
    pub state: BindingState,
    pub session_limit: usize,
    pub profile: Word,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionOwner {
    Process(Word),
    /// Scoped to this Alpha's delegation ledger, never a process PID.
    Domain(Word),
}

#[derive(Clone, Copy, Debug)]
pub struct Session {
    pub owner: SessionOwner,
    pub slot: Word,
    pub service: ServiceId,
    pub closed: bool,
}

pub struct Bindings {
    providers: [Option<Provider>; PROVIDERS],
    services: [Option<Service>; SERVICES],
    sessions: [Option<Session>; SESSIONS],
    provider_count: usize,
    service_count: usize,
    session_count: usize,
}

fn invalid<T>() -> Result<T, CapabilityError> {
    Err(CapabilityError::InvalidArgument)
}
fn denied<T>() -> Result<T, CapabilityError> {
    Err(CapabilityError::PermissionDenied)
}

impl Bindings {
    pub const fn new() -> Self {
        Self {
            providers: [None; PROVIDERS],
            services: [None; SERVICES],
            sessions: [None; SESSIONS],
            provider_count: 0,
            service_count: 0,
            session_count: 0,
        }
    }

    pub fn provider(&self, id: ProviderId) -> Result<Provider, CapabilityError> {
        self.providers
            .get(id.0.wrapping_sub(1))
            .copied()
            .flatten()
            .filter(|provider| !provider.retired)
            .ok_or(CapabilityError::InvalidArgument)
    }

    pub fn service(&self, id: ServiceId) -> Result<Service, CapabilityError> {
        self.services
            .get(id.0.wrapping_sub(1))
            .copied()
            .flatten()
            .ok_or(CapabilityError::InvalidArgument)
    }

    pub fn session(&self, id: SessionId) -> Result<Session, CapabilityError> {
        if id.0 & SESSION_IDENTIFIER_BIT == 0 {
            return invalid();
        }
        self.sessions
            .get((id.0 & !SESSION_IDENTIFIER_BIT).wrapping_sub(1))
            .copied()
            .flatten()
            .ok_or(CapabilityError::InvalidArgument)
    }

    pub fn has_live_sessions(&self, pid: Word) -> bool {
        self.sessions[..self.session_count]
            .iter()
            .flatten()
            .any(|session| session.owner == SessionOwner::Process(pid) && !session.closed)
    }

    pub fn owns_process_slot(&self, pid: Word, slot: Word) -> bool {
        self.providers.iter().flatten().any(|provider| {
            provider.owner == pid
                && (provider.receiver_slot == slot
                    || provider.notification_slot == slot
                    || (provider.event_slot != 0 && provider.event_slot == slot))
        }) || self.sessions.iter().flatten().any(|session| {
            !session.closed && session.owner == SessionOwner::Process(pid) && session.slot == slot
        })
    }

    pub fn provider_for_owner(&self, owner: Word) -> Option<(ProviderId, Provider)> {
        self.providers
            .iter()
            .enumerate()
            .find_map(|(index, entry)| {
                entry
                    .filter(|entry| !entry.retired && entry.owner == owner)
                    .map(|entry| (ProviderId(index + 1), entry))
            })
    }

    pub fn register_provider(
        &mut self,
        owner: Word,
        interface: Word,
        receiver_slot: Word,
        notification_slot: Word,
        prepare: impl FnMut(ProviderId) -> Result<(), CapabilityError>,
    ) -> Result<ProviderId, CapabilityError> {
        self.register_provider_profile(
            owner,
            interface,
            receiver_slot,
            notification_slot,
            nanami_uapi::service::STATELESS_CONTROL_PROFILE,
            prepare,
        )
    }

    pub fn register_provider_profile(
        &mut self,
        owner: Word,
        interface: Word,
        receiver_slot: Word,
        notification_slot: Word,
        profile: Word,
        mut prepare: impl FnMut(ProviderId) -> Result<(), CapabilityError>,
    ) -> Result<ProviderId, CapabilityError> {
        let (receiver_slot, event_slot) =
            nanami_uapi::service::provider_slots(profile, receiver_slot, notification_slot)
                .ok_or(CapabilityError::InvalidArgument)?;
        if owner == 0 || interface == 0 {
            return invalid();
        }
        if let Some((id, existing)) = self.provider_for_owner(owner) {
            if existing.interface == interface
                && existing.receiver_slot == receiver_slot
                && existing.notification_slot == notification_slot
                && existing.profile == profile
                && existing.event_slot == event_slot
            {
                return Ok(id);
            }
            return denied();
        }
        if self.provider_count == PROVIDERS {
            return invalid();
        }
        let id = ProviderId(self.provider_count + 1);
        prepare(id)?;
        self.providers[self.provider_count] = Some(Provider {
            owner,
            interface,
            profile,
            receiver_slot,
            notification_slot,
            event_slot,
            service: None,
            last_service: None,
            retired: false,
        });
        self.provider_count += 1;
        Ok(id)
    }

    pub fn publish(
        &mut self,
        manager: Word,
        provider: ProviderId,
        mut install: impl FnMut(ServiceId, Provider) -> Result<(), CapabilityError>,
    ) -> Result<ServiceId, CapabilityError> {
        let instance = self.provider(provider)?;
        if manager == 0 || instance.service.is_some() || self.service_count == SERVICES {
            return invalid();
        }
        let id = ServiceId(self.service_count + 1);
        install(id, instance)?;
        self.providers[provider.0 - 1].as_mut().unwrap().service = Some(id);
        self.services[self.service_count] = Some(Service {
            manager,
            provider,
            pending: None,
            state: BindingState {
                service: id,
                generation: 1,
                phase: BindingPhase::Active,
            },
            session_limit: SESSIONS,
            profile: instance.profile,
        });
        self.service_count += 1;
        Ok(id)
    }

    /// An upstream endpoint has no locally selectable Provider or manager.
    /// Its binding is controlled only in the source Alpha, through that endpoint.
    pub fn import_service(&mut self, session_limit: usize) -> Result<ServiceId, CapabilityError> {
        if !(1..=SESSIONS).contains(&session_limit) || self.service_count == SERVICES {
            return invalid();
        }
        let id = ServiceId(self.service_count + 1);
        self.services[self.service_count] = Some(Service {
            manager: 0,
            provider: ProviderId(0),
            pending: None,
            state: BindingState {
                service: id,
                generation: 1,
                phase: BindingPhase::Active,
            },
            session_limit,
            profile: nanami_uapi::service::STATELESS_CONTROL_PROFILE,
        });
        self.service_count += 1;
        Ok(id)
    }

    fn owned(
        &self,
        caller: Word,
        id: ServiceId,
        generation: Word,
    ) -> Result<Service, CapabilityError> {
        let service = self.service(id)?;
        if caller == 0 || service.manager != caller {
            return denied();
        }
        if service.state.generation != generation {
            return invalid();
        }
        Ok(service)
    }

    pub fn prepare(
        &mut self,
        caller: Word,
        id: ServiceId,
        generation: Word,
        next: ProviderId,
    ) -> Result<(), CapabilityError> {
        let service = self.owned(caller, id, generation)?;
        let target = self.provider(next)?;
        if service.state.phase != BindingPhase::Active
            || next == service.provider
            || target.service.is_some()
            || target.interface != self.provider(service.provider)?.interface
            || target.profile != service.profile
            || generation >= Word::MAX >> 3
        {
            return invalid();
        }
        self.providers[next.0 - 1].as_mut().unwrap().service = Some(id);
        let service = self.services[id.0 - 1].as_mut().unwrap();
        service.pending = Some(next);
        service.state.phase = BindingPhase::Prepared;
        Ok(())
    }

    pub fn quiesce(
        &mut self,
        caller: Word,
        id: ServiceId,
        generation: Word,
        mut signal: impl FnMut(ProviderId) -> Result<(), CapabilityError>,
    ) -> Result<(), CapabilityError> {
        let service = self.owned(caller, id, generation)?;
        if service.state.phase != BindingPhase::Prepared {
            return invalid();
        }
        signal(service.provider)?;
        self.services[id.0 - 1].as_mut().unwrap().state.phase = BindingPhase::Quiescing;
        Ok(())
    }

    pub fn provider_binding(
        &self,
        caller: Word,
        id: ProviderId,
    ) -> Result<BindingState, CapabilityError> {
        let provider = self.provider(id)?;
        if caller != provider.owner {
            return denied();
        }
        let service = self.service(provider.service.ok_or(CapabilityError::InvalidArgument)?)?;
        if service.provider != id {
            return invalid();
        }
        Ok(service.state)
    }

    /// The cooperative Provider must finish every accepted CALL before this
    /// request. It is blocked on Alpha's OS CALL, not waiting on the public port.
    pub fn acknowledge(
        &mut self,
        caller: Word,
        id: ProviderId,
        generation: Word,
        mut withdraw: impl FnMut(ServiceId, Provider) -> Result<(), CapabilityError>,
    ) -> Result<ServiceId, CapabilityError> {
        let provider = self.provider(id)?;
        let state = self.provider_binding(caller, id)?;
        if state.generation != generation || state.phase != BindingPhase::Quiescing {
            return invalid();
        }
        if let Err(error) = withdraw(state.service, provider) {
            if matches!(error, CapabilityError::Fatal) {
                self.fault(state.service);
            }
            return Err(error);
        }
        self.services[state.service.0 - 1]
            .as_mut()
            .unwrap()
            .state
            .phase = BindingPhase::Quiesced;
        Ok(state.service)
    }

    pub fn fault(&mut self, id: ServiceId) {
        if let Some(Some(service)) = self.services.get_mut(id.0.wrapping_sub(1)) {
            service.state.phase = BindingPhase::Faulted;
        }
    }

    pub fn has_provider_authority(&self, pid: Word) -> bool {
        self.providers
            .iter()
            .flatten()
            .any(|provider| provider.owner == pid)
    }

    pub fn commit(
        &mut self,
        caller: Word,
        id: ServiceId,
        generation: Word,
        mut install: impl FnMut(ServiceId, Provider) -> Result<(), CapabilityError>,
    ) -> Result<ProviderId, CapabilityError> {
        let service = self.owned(caller, id, generation)?;
        if service.state.phase != BindingPhase::Quiesced {
            return invalid();
        }
        let next = service.pending.ok_or(CapabilityError::InvalidArgument)?;
        if let Err(error) = install(id, self.provider(next)?) {
            if matches!(error, CapabilityError::Fatal) {
                self.services[id.0 - 1].as_mut().unwrap().state.phase = BindingPhase::Faulted;
            }
            return Err(error);
        }
        self.providers[service.provider.0 - 1]
            .as_mut()
            .unwrap()
            .service = None;
        self.providers[service.provider.0 - 1]
            .as_mut()
            .unwrap()
            .last_service = Some(id);
        let service = self.services[id.0 - 1].as_mut().unwrap();
        service.provider = next;
        service.pending = None;
        service.state.generation += 1;
        service.state.phase = BindingPhase::Active;
        Ok(next)
    }

    pub fn abort(
        &mut self,
        caller: Word,
        id: ServiceId,
        generation: Word,
        mut restore: impl FnMut(ServiceId, Provider) -> Result<(), CapabilityError>,
    ) -> Result<(), CapabilityError> {
        let service = self.owned(caller, id, generation)?;
        if !matches!(
            service.state.phase,
            BindingPhase::Prepared | BindingPhase::Quiesced
        ) {
            return invalid();
        }
        if service.state.phase == BindingPhase::Quiesced {
            if let Err(error) = restore(id, self.provider(service.provider)?) {
                if matches!(error, CapabilityError::Fatal) {
                    self.services[id.0 - 1].as_mut().unwrap().state.phase = BindingPhase::Faulted;
                }
                return Err(error);
            }
        }
        if let Some(pending) = service.pending {
            self.providers[pending.0 - 1].as_mut().unwrap().service = None;
        }
        let service = self.services[id.0 - 1].as_mut().unwrap();
        service.pending = None;
        service.state.phase = BindingPhase::Active;
        Ok(())
    }

    pub fn connect(
        &mut self,
        client: Word,
        id: ServiceId,
        slot: Word,
        install: impl FnMut(SessionId) -> Result<(), CapabilityError>,
    ) -> Result<SessionId, CapabilityError> {
        if client == 0 {
            return invalid();
        }
        self.connect_owned(SessionOwner::Process(client), id, slot, install)
    }

    pub fn connect_domain(
        &mut self,
        domain: Word,
        id: ServiceId,
        slot: Word,
        install: impl FnMut(SessionId) -> Result<(), CapabilityError>,
    ) -> Result<SessionId, CapabilityError> {
        if domain == 0 {
            return invalid();
        }
        self.connect_owned(SessionOwner::Domain(domain), id, slot, install)
    }

    fn connect_owned(
        &mut self,
        owner: SessionOwner,
        id: ServiceId,
        slot: Word,
        mut install: impl FnMut(SessionId) -> Result<(), CapabilityError>,
    ) -> Result<SessionId, CapabilityError> {
        if self.service(id)?.state.phase == BindingPhase::Faulted {
            return Err(CapabilityError::Fatal);
        }
        let charged = self.sessions[..self.session_count]
            .iter()
            .flatten()
            .filter(|session| session.service == id)
            .count();
        if self.session_count == SESSIONS || charged >= self.service(id)?.session_limit {
            return invalid();
        }
        let session = SessionId(SESSION_IDENTIFIER_BIT | (self.session_count + 1));
        if let Err(error) = install(session) {
            if matches!(error, CapabilityError::Fatal) {
                self.sessions[self.session_count] = Some(Session {
                    owner,
                    slot,
                    service: id,
                    closed: true,
                });
                self.session_count += 1;
                self.services[id.0 - 1].as_mut().unwrap().state.phase = BindingPhase::Faulted;
            }
            return Err(error);
        }
        self.sessions[self.session_count] = Some(Session {
            owner,
            slot,
            service: id,
            closed: false,
        });
        self.session_count += 1;
        Ok(session)
    }

    pub fn close(
        &mut self,
        caller: Word,
        id: SessionId,
        mut withdraw: impl FnMut(Session) -> Result<(), CapabilityError>,
    ) -> Result<(), CapabilityError> {
        let session = self.session(id)?;
        if session.owner != SessionOwner::Process(caller) {
            return denied();
        }
        if session.closed {
            return Ok(());
        }
        withdraw(session)?;
        self.sessions[(id.0 & !SESSION_IDENTIFIER_BIT) - 1]
            .as_mut()
            .unwrap()
            .closed = true;
        // The attenuated alias stays quarantined until A9N can delete it. Do
        // not pretend a slot/endpoint reference has been reclaimed or reused.
        Ok(())
    }

    pub fn retire_provider(
        &mut self,
        caller: Word,
        id: ProviderId,
        mut release: impl FnMut(ProviderId) -> Result<(), CapabilityError>,
    ) -> Result<(), CapabilityError> {
        let provider = self.provider(id)?;
        if caller != provider.owner {
            return denied();
        }
        if provider.service.is_some() {
            return invalid();
        }
        release(id)?;
        self.providers[id.0 - 1].as_mut().unwrap().retired = true;
        Ok(())
    }

    pub fn retired_service_for_owner(&self, owner: Word) -> Option<ServiceId> {
        self.providers
            .iter()
            .flatten()
            .find(|provider| provider.owner == owner && provider.retired)
            .and_then(|provider| provider.last_service)
    }
}
