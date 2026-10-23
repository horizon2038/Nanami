extern crate self as nun;
pub use a9n_abi::{CapabilityError, Word};
#[path = "../../nanami/src/nanami_core/communication/bindings.rs"]
mod bindings;
use bindings::{Bindings, PROVIDERS, SESSIONS};
use nanami_uapi::service::*;
use std::cell::RefCell;

fn provider(model: &mut Bindings, owner: usize, interface: usize) -> ProviderId {
    model
        .register_provider(owner, interface, 28, 29, |_| Ok(()))
        .unwrap()
}

#[test]
fn stable_service_and_session_survive_a_serial_receiver_exchange() {
    let mut model = Bindings::new();
    let first = provider(&mut model, 7, 0x5151);
    let second = provider(&mut model, 8, 0x5151);
    let receivers = RefCell::new(Vec::new());
    let service = model
        .publish(1, first, |_, provider| {
            receivers.borrow_mut().push(provider.owner);
            Ok(())
        })
        .unwrap();
    let session = model.connect(2, service, 30, |_| Ok(())).unwrap();
    let before = model.service(service).unwrap().state;
    model
        .prepare(1, service, before.generation, second)
        .unwrap();
    assert_eq!(*receivers.borrow(), vec![7]);
    assert!(model
        .commit(1, service, before.generation, |_, _| panic!(
            "unquiesced receiver installed"
        ))
        .is_err());
    model
        .quiesce(1, service, before.generation, |_| Ok(()))
        .unwrap();
    model
        .acknowledge(7, first, before.generation, |_, provider| {
            assert_eq!(receivers.borrow_mut().pop(), Some(provider.owner));
            Ok(())
        })
        .unwrap();
    assert!(receivers.borrow().is_empty());
    model
        .commit(1, service, before.generation, |_, provider| {
            assert!(receivers.borrow().is_empty());
            receivers.borrow_mut().push(provider.owner);
            Ok(())
        })
        .unwrap();
    assert_eq!(*receivers.borrow(), vec![8]);
    assert_eq!(model.session(session).unwrap().service, service);
    assert_eq!(model.service(service).unwrap().provider, second);
    assert_eq!(
        model.service(service).unwrap().state.generation,
        before.generation + 1
    );
    assert!(model.provider_binding(7, first).is_err());
    model.retire_provider(7, first, |_| Ok(())).unwrap();
    assert_eq!(model.retired_service_for_owner(7), Some(service));
    assert_eq!(model.service(service).unwrap().provider, second);
}

#[test]
fn incompatible_or_unauthorized_preparation_does_not_change_binding() {
    let mut model = Bindings::new();
    let first = provider(&mut model, 7, 11);
    let bad = provider(&mut model, 8, 12);
    let second = provider(&mut model, 9, 11);
    let service = model.publish(1, first, |_, _| Ok(())).unwrap();
    assert!(model.prepare(2, service, 1, second).is_err());
    assert!(model.prepare(1, service, 1, bad).is_err());
    assert!(model.prepare(1, service, 0, second).is_err());
    assert_eq!(
        model.service(service).unwrap().state.phase,
        BindingPhase::Active
    );
    assert_eq!(model.service(service).unwrap().provider, first);
    assert!(model.provider_binding(999, first).is_err());
    assert!(model
        .retire_provider(7, first, |_| panic!("live provider retired"))
        .is_err());
}

#[test]
fn failed_prepare_quiesce_withdraw_and_commit_have_no_false_commit() {
    let mut model = Bindings::new();
    assert!(model
        .register_provider(7, 11, 28, 29, |_| Err(CapabilityError::InvalidArgument))
        .is_err());
    let first = provider(&mut model, 7, 11);
    assert_eq!(first.0, 1);
    let second = provider(&mut model, 8, 11);
    assert!(model
        .publish(1, first, |_, _| Err(CapabilityError::InvalidArgument))
        .is_err());
    let service = model.publish(1, first, |_, _| Ok(())).unwrap();
    assert_eq!(service.0, 1);
    model.prepare(1, service, 1, second).unwrap();
    assert!(model
        .quiesce(1, service, 1, |_| Err(CapabilityError::InvalidArgument))
        .is_err());
    assert_eq!(
        model.service(service).unwrap().state.phase,
        BindingPhase::Prepared
    );
    model.quiesce(1, service, 1, |_| Ok(())).unwrap();
    assert!(model
        .acknowledge(8, first, 1, |_, _| panic!("foreign acknowledgement"))
        .is_err());
    assert!(model
        .acknowledge(7, first, 1, |_, _| Err(CapabilityError::InvalidArgument))
        .is_err());
    assert_eq!(
        model.service(service).unwrap().state.phase,
        BindingPhase::Quiescing
    );
    model.acknowledge(7, first, 1, |_, _| Ok(())).unwrap();
    assert!(model
        .commit(1, service, 1, |_, _| Err(CapabilityError::InvalidArgument))
        .is_err());
    assert_eq!(model.service(service).unwrap().provider, first);
    assert_eq!(
        model.service(service).unwrap().state.phase,
        BindingPhase::Quiesced
    );
    model
        .abort(1, service, 1, |_, provider| {
            assert_eq!(provider.owner, 7);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        model.service(service).unwrap().state.phase,
        BindingPhase::Active
    );
    assert!(model.provider(second).unwrap().service.is_none());
}

#[test]
fn uncertain_capability_failure_freezes_the_binding_instead_of_duplicating_receivers() {
    let mut model = Bindings::new();
    let a = provider(&mut model, 7, 11);
    let b = provider(&mut model, 8, 11);
    let service = model.publish(1, a, |_, _| Ok(())).unwrap();
    model.prepare(1, service, 1, b).unwrap();
    model.quiesce(1, service, 1, |_| Ok(())).unwrap();
    model.acknowledge(7, a, 1, |_, _| Ok(())).unwrap();
    assert!(model
        .commit(1, service, 1, |_, _| Err(CapabilityError::Fatal))
        .is_err());
    assert_eq!(
        model.service(service).unwrap().state.phase,
        BindingPhase::Faulted
    );
    assert!(model
        .abort(1, service, 1, |_, _| panic!("unknown authority restored"))
        .is_err());
    assert!(model
        .commit(1, service, 1, |_, _| panic!("unknown receiver retried"))
        .is_err());
}

#[test]
fn quotas_remain_charged_for_quarantined_closed_aliases() {
    let mut model = Bindings::new();
    let instance = provider(&mut model, 7, 11);
    let service = model.publish(1, instance, |_, _| Ok(())).unwrap();
    for index in 0..SESSIONS {
        let session = model.connect(2, service, index + 30, |_| Ok(())).unwrap();
        assert!(model
            .close(3, session, |_| panic!("foreign handle closed"))
            .is_err());
        model
            .close(2, session, |entry| {
                assert_eq!(entry.slot, index + 30);
                Ok(())
            })
            .unwrap();
        model
            .close(2, session, |_| panic!("double withdrawal"))
            .unwrap();
        assert!(model.session(session).unwrap().closed);
    }
    assert!(model
        .connect(2, service, 200, |_| panic!("quota bypass"))
        .is_err());
    assert!(model.session(SessionId(1)).is_err());
    assert!(model.session(SessionId(usize::MAX)).is_err());
}

#[test]
fn provider_and_publication_capacity_are_bounded_and_failed_aliases_do_not_publish() {
    let mut model = Bindings::new();
    for index in 0..PROVIDERS {
        let instance = provider(&mut model, index + 1, 11);
        let retried = provider(&mut model, index + 1, 11);
        assert_eq!(instance, retried);
        model.publish(99, instance, |_, _| Ok(())).unwrap();
    }
    assert!(model
        .register_provider(999, 11, 28, 29, |_| panic!("quota bypass"))
        .is_err());
    assert!(model.service(ServiceId(0)).is_err());
    assert!(model.service(ServiceId(PROVIDERS + 1)).is_err());
}

#[test]
fn failed_connection_and_restore_keep_uncertain_resources_charged() {
    let mut model = Bindings::new();
    let a = provider(&mut model, 7, 11);
    let service = model.publish(1, a, |_, _| Ok(())).unwrap();
    assert!(model
        .connect(2, service, 30, |_| Err(CapabilityError::Fatal))
        .is_err());
    assert!(
        model
            .session(SessionId(SESSION_IDENTIFIER_BIT | 1))
            .unwrap()
            .closed
    );
    assert_eq!(
        model.service(service).unwrap().state.phase,
        BindingPhase::Faulted
    );
    assert!(model
        .connect(2, service, 31, |_| panic!("faulted service reused"))
        .is_err());
    let mut model = Bindings::new();
    let a = provider(&mut model, 7, 11);
    let b = provider(&mut model, 8, 11);
    let service = model.publish(1, a, |_, _| Ok(())).unwrap();
    model.prepare(1, service, 1, b).unwrap();
    model.quiesce(1, service, 1, |_| Ok(())).unwrap();
    model.acknowledge(7, a, 1, |_, _| Ok(())).unwrap();
    assert!(model
        .abort(1, service, 1, |_, _| Err(CapabilityError::Fatal))
        .is_err());
    assert_eq!(
        model.service(service).unwrap().state.phase,
        BindingPhase::Faulted
    );
}

#[test]
fn domain_session_ownership_cannot_be_confused_with_a_local_pid() {
    let mut model = Bindings::new();
    let first = provider(&mut model, 2, 99);
    let service = model.publish(1, first, |_, _| Ok(())).unwrap();
    let domain = model.connect_domain(7, service, 0, |_| Ok(())).unwrap();
    assert_eq!(
        model.session(domain).unwrap().owner,
        bindings::SessionOwner::Domain(7)
    );
    let mut withdrawn = false;
    assert!(model
        .close(7, domain, |_| {
            withdrawn = true;
            Ok(())
        })
        .is_err());
    assert!(!withdrawn);
    assert!(model
        .connect_domain(0, service, 0, |_| panic!("invalid domain installed"))
        .is_err());
}

#[test]
fn imported_services_have_no_local_provider_takeover_and_keep_session_charges() {
    let mut model = Bindings::new();
    let imported = model.import_service(2).unwrap();
    let successor = provider(&mut model, 9, 99);
    assert!(model.prepare(1, imported, 1, successor).is_err());
    assert!(model.prepare(0, imported, 1, successor).is_err());
    let first = model.connect(1, imported, 31, |_| Ok(())).unwrap();
    assert!(model.has_live_sessions(1));
    model.close(1, first, |_| Ok(())).unwrap();
    assert!(!model.has_live_sessions(1));
    model.connect_domain(4, imported, 0, |_| Ok(())).unwrap();
    assert!(model
        .connect(2, imported, 32, |_| panic!("quota overflow installed"))
        .is_err());
    assert!(model.import_service(0).is_err());
    assert!(model.import_service(SESSIONS + 1).is_err());
}

#[test]
fn buffer_profiles_must_match_and_partial_withdrawal_freezes_the_binding() {
    let mut model = Bindings::new();
    let first = model
        .register_provider_profile(2, 99, 28, 29, MANAGED_BUFFER_PROFILE, |_| Ok(()))
        .unwrap();
    let plain = provider(&mut model, 3, 99);
    let next = model
        .register_provider_profile(4, 99, 28, 29, MANAGED_BUFFER_PROFILE, |_| Ok(()))
        .unwrap();
    let service = model.publish(1, first, |_, _| Ok(())).unwrap();
    assert!(model.prepare(1, service, 1, plain).is_err());
    model.prepare(1, service, 1, next).unwrap();
    model.quiesce(1, service, 1, |_| Ok(())).unwrap();
    assert!(model
        .acknowledge(2, first, 1, |_, _| Err(CapabilityError::Fatal))
        .is_err());
    assert_eq!(
        model.service(service).unwrap().state.phase,
        BindingPhase::Faulted
    );
    assert!(model
        .commit(1, service, 1, |_, _| panic!("uncertain mappings committed"))
        .is_err());
    assert!(model
        .abort(1, service, 1, |_, _| panic!("uncertain mappings resumed"))
        .is_err());
    assert!(model.has_provider_authority(2));
}
