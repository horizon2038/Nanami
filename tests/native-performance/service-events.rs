extern crate self as nun;
pub use a9n_abi::{CapabilityError, Word};
use nanami_uapi::service::*;
use std::collections::{BTreeMap, BTreeSet};
mod bindings {
    pub const SERVICES: usize = 32;
    pub const SESSIONS: usize = 64;
}
#[path = "../../nanami/src/nanami_core/communication/events.rs"]
mod events;
use events::{Effects, Events};

#[derive(Default)]
struct Kernel {
    objects: BTreeSet<usize>,
    receivers: BTreeMap<(usize, usize), usize>,
    bound: BTreeMap<usize, usize>,
    clients: BTreeMap<(usize, usize), usize>,
    quarantine: BTreeMap<usize, usize>,
    pending: BTreeMap<usize, usize>,
    creates: usize,
    fail_create: bool,
    fail_install: bool,
    fail_withdraw: bool,
    fail_client: bool,
    fatal_client: bool,
}

impl Effects for Kernel {
    fn create(&mut self, service: ServiceId) -> Result<(), CapabilityError> {
        self.creates += 1;
        assert!(self.objects.insert(service.0));
        if self.fail_create {
            return Err(CapabilityError::Fatal);
        }
        Ok(())
    }
    fn install(
        &mut self,
        service: ServiceId,
        owner: Word,
        slot: Word,
    ) -> Result<(), CapabilityError> {
        if self.fail_install {
            return Err(CapabilityError::InvalidArgument);
        }
        assert!(self.receivers.insert((owner, slot), service.0).is_none());
        Ok(())
    }
    fn withdraw(
        &mut self,
        service: ServiceId,
        owner: Word,
        slot: Word,
    ) -> Result<(), CapabilityError> {
        if self.fail_withdraw {
            return Err(CapabilityError::Fatal);
        }
        self.bound.remove(&owner);
        assert_eq!(self.receivers.remove(&(owner, slot)), Some(service.0));
        Ok(())
    }
    fn client(
        &mut self,
        service: ServiceId,
        owner: Word,
        slot: Word,
    ) -> Result<(), CapabilityError> {
        if self.fatal_client {
            return Err(CapabilityError::Fatal);
        }
        if self.fail_client {
            return Err(CapabilityError::InvalidArgument);
        }
        assert!(self.clients.insert((owner, slot), service.0).is_none());
        Ok(())
    }
    fn close(
        &mut self,
        session: SessionId,
        owner: Word,
        slot: Word,
    ) -> Result<(), CapabilityError> {
        let object = self.clients.remove(&(owner, slot)).unwrap();
        assert!(self.quarantine.insert(session.0, object).is_none());
        Ok(())
    }
}

fn session(index: usize) -> SessionId {
    SessionId(SESSION_IDENTIFIER_BIT | index)
}

#[test]
fn pending_work_and_client_identity_survive_receiver_exchange() {
    let mut events = Events::new();
    let mut kernel = Kernel::default();
    let service = ServiceId(1);
    events.install(service, 2, 30, &mut kernel).unwrap();
    events
        .connect(session(1), service, 1, 32, &mut kernel)
        .unwrap();
    kernel.bound.insert(2, service.0);
    kernel.pending.insert(service.0, SERVICE_WORK_NOTIFICATION);
    events.withdraw(service, 2, &mut kernel).unwrap();
    assert!(!kernel.bound.contains_key(&2));
    assert!(!kernel.receivers.contains_key(&(2, 30)));
    events.install(service, 3, 31, &mut kernel).unwrap();
    assert_eq!(kernel.clients[&(1, 32)], service.0);
    assert_eq!(kernel.receivers[&(3, 31)], service.0);
    assert_eq!(kernel.pending[&service.0], SERVICE_WORK_NOTIFICATION);
    assert_eq!(kernel.creates, 1);
}

#[test]
fn ordinary_install_failure_allows_exact_abort_without_a_second_object() {
    let mut events = Events::new();
    let mut kernel = Kernel::default();
    events.install(ServiceId(1), 2, 30, &mut kernel).unwrap();
    events.withdraw(ServiceId(1), 2, &mut kernel).unwrap();
    kernel.fail_install = true;
    assert!(events.install(ServiceId(1), 3, 30, &mut kernel).is_err());
    assert!(!events.has_receiver(ServiceId(1), 3));
    kernel.fail_install = false;
    events.install(ServiceId(1), 2, 30, &mut kernel).unwrap();
    assert!(events.has_receiver(ServiceId(1), 2));
    assert_eq!(kernel.creates, 1);
}

#[test]
fn failed_detachment_freezes_the_resource_instead_of_installing_two_receivers() {
    let mut events = Events::new();
    let mut kernel = Kernel::default();
    events.install(ServiceId(1), 2, 30, &mut kernel).unwrap();
    kernel.fail_withdraw = true;
    assert!(matches!(
        events.withdraw(ServiceId(1), 2, &mut kernel),
        Err(CapabilityError::Fatal)
    ));
    kernel.fail_withdraw = false;
    assert!(matches!(
        events.install(ServiceId(1), 3, 30, &mut kernel),
        Err(CapabilityError::Fatal)
    ));
    assert_eq!(kernel.receivers.len(), 1);
}

#[test]
fn partial_construction_is_never_retried_over_occupied_slots() {
    let mut events = Events::new();
    let mut kernel = Kernel {
        fail_create: true,
        ..Kernel::default()
    };
    assert!(matches!(
        events.install(ServiceId(1), 2, 30, &mut kernel),
        Err(CapabilityError::Fatal)
    ));
    kernel.fail_create = false;
    assert!(matches!(
        events.install(ServiceId(1), 2, 30, &mut kernel),
        Err(CapabilityError::Fatal)
    ));
    assert_eq!(kernel.creates, 1);
}

#[test]
fn wrong_owner_cannot_detach_the_current_receiver() {
    let mut events = Events::new();
    let mut kernel = Kernel::default();
    events.install(ServiceId(1), 2, 30, &mut kernel).unwrap();
    assert!(matches!(
        events.withdraw(ServiceId(1), 3, &mut kernel),
        Err(CapabilityError::PermissionDenied)
    ));
    assert!(events.has_receiver(ServiceId(1), 2));
    assert!(events.install(ServiceId(1), 3, 31, &mut kernel).is_err());
}

#[test]
fn client_grants_are_idempotent_scoped_and_retry_known_no_effect_errors() {
    let mut events = Events::new();
    let mut kernel = Kernel::default();
    events.install(ServiceId(1), 2, 30, &mut kernel).unwrap();
    kernel.fail_client = true;
    assert!(events
        .connect(session(1), ServiceId(1), 1, 32, &mut kernel)
        .is_err());
    kernel.fail_client = false;
    for _ in 0..2 {
        events
            .connect(session(1), ServiceId(1), 1, 33, &mut kernel)
            .unwrap();
    }
    assert_eq!(kernel.clients.len(), 1);
    assert!(matches!(
        events.connect(session(1), ServiceId(1), 4, 33, &mut kernel),
        Err(CapabilityError::PermissionDenied)
    ));
    assert!(matches!(
        events.connect(session(1), ServiceId(2), 1, 33, &mut kernel),
        Err(CapabilityError::InvalidArgument)
    ));
}

#[test]
fn closed_write_only_handle_is_withdrawn_but_backing_and_quota_stay_charged() {
    let mut events = Events::new();
    let mut kernel = Kernel::default();
    events.install(ServiceId(1), 2, 30, &mut kernel).unwrap();
    events
        .connect(session(1), ServiceId(1), 1, 32, &mut kernel)
        .unwrap();
    events.close(session(1), &mut kernel).unwrap();
    events.close(session(1), &mut kernel).unwrap();
    assert!(kernel.clients.is_empty());
    assert_eq!(kernel.quarantine.len(), 1);
    assert_eq!(kernel.objects.len(), 1);
    assert!(events
        .connect(session(1), ServiceId(1), 1, 32, &mut kernel)
        .is_err());
    assert!(events.has_receiver(ServiceId(1), 2));
}

#[test]
fn identifiers_profiles_and_packed_slots_are_checked() {
    assert_eq!(
        provider_slots(MANAGED_EVENT_PROFILE, 28 | (30 << 8), 29),
        Some((28, 30))
    );
    assert_eq!(
        provider_slots(MANAGED_BUFFER_PROFILE, 28, 29),
        Some((28, 0))
    );
    for (profile, receiver, notification) in [
        (MANAGED_BUFFER_PROFILE, 28 | (30 << 8), 29),
        (MANAGED_EVENT_PROFILE, 28 | (29 << 8), 29),
        (MANAGED_EVENT_PROFILE, 28 | (28 << 8), 29),
        (MANAGED_EVENT_PROFILE, 28 | (224 << 8), 29),
        (MANAGED_EVENT_PROFILE, 28, 29),
        (4, 28, 29),
    ] {
        assert!(provider_slots(profile, receiver, notification).is_none());
    }
    let mut events = Events::new();
    let mut kernel = Kernel::default();
    for id in [0, 33, usize::MAX] {
        assert!(events.install(ServiceId(id), 2, 30, &mut kernel).is_err());
    }
    events.install(ServiceId(1), 2, 30, &mut kernel).unwrap();
    for id in [0, 1, SESSION_IDENTIFIER_BIT, SESSION_IDENTIFIER_BIT | 65] {
        assert!(events
            .connect(SessionId(id), ServiceId(1), 1, 32, &mut kernel)
            .is_err());
    }
}

#[test]
fn uncertain_client_grant_cannot_be_falsely_reported_closed() {
    let mut events = Events::new();
    let mut kernel = Kernel {
        fatal_client: true,
        ..Kernel::default()
    };
    events.install(ServiceId(1), 2, 30, &mut kernel).unwrap();
    assert!(matches!(
        events.connect(session(1), ServiceId(1), 1, 32, &mut kernel),
        Err(CapabilityError::Fatal)
    ));
    assert!(matches!(
        events.close(session(1), &mut kernel),
        Err(CapabilityError::Fatal)
    ));
    kernel.fatal_client = false;
    assert!(matches!(
        events.connect(session(1), ServiceId(1), 1, 32, &mut kernel),
        Err(CapabilityError::Fatal)
    ));
}
