//! Production mapping transaction with only page/capability effects substituted.
extern crate self as nun;
pub use a9n_abi::{CapabilityError, Word};
#[path = "../../nanami/src/nanami_core/communication/bindings.rs"]
mod bindings;
#[path = "../../nanami/src/nanami_core/communication/buffers.rs"]
mod buffers;
use buffers::{Buffers, Effects, MapError};
use nanami_uapi::service::*;
use std::collections::BTreeMap;

#[derive(Default)]
struct Memory {
    maps: BTreeMap<(Word, Word), Word>,
    next_va: BTreeMap<Word, Word>,
    contents: BTreeMap<Word, Word>,
    allocations: usize,
    map_calls: usize,
    fail_map: Option<usize>,
    fail_unmap: bool,
    fail_receiver: bool,
    receivers: Vec<Word>,
    reapers: BTreeMap<Word, Word>,
}
impl Effects for Memory {
    fn participants(&mut self, client: Word, provider: Word) -> Result<(), CapabilityError> {
        if self.reapers.get(&client) == Some(&provider) {
            return Err(CapabilityError::PermissionDenied);
        }
        Ok(())
    }
    fn allocate(&mut self, pages: usize) -> Result<Word, CapabilityError> {
        let base = 100 + self.allocations * 16;
        self.allocations += 1;
        for page in 0..pages {
            self.contents.insert(base + page, Word::MAX);
        }
        Ok(base)
    }
    fn reserve(&mut self, owner: Word, pages: usize) -> Result<Word, CapabilityError> {
        let next = self.next_va.entry(owner).or_insert(owner * 0x100000);
        let address = *next;
        *next += pages * 4096;
        Ok(address)
    }
    fn zero(&mut self, physical: Word) -> Result<(), CapabilityError> {
        *self.contents.get_mut(&physical).unwrap() = 0;
        Ok(())
    }
    fn map(&mut self, owner: Word, address: Word, physical: Word) -> Result<(), MapError> {
        if self.maps.contains_key(&(owner, address)) {
            return Err(MapError::NotInstalled(CapabilityError::IllegalOperation));
        }
        assert!(self.maps.insert((owner, address), physical).is_none());
        let index = self.map_calls;
        self.map_calls += 1;
        if self.fail_map == Some(index) {
            return Err(MapError::MaybeInstalled(CapabilityError::InvalidArgument));
        }
        Ok(())
    }
    fn unmap(&mut self, owner: Word, address: Word, _: Word) -> Result<(), CapabilityError> {
        if self.fail_unmap {
            return Err(CapabilityError::Fatal);
        }
        self.maps.remove(&(owner, address));
        Ok(())
    }
    fn receiver(&mut self, _: ServiceId, owner: Word, slot: Word) -> Result<(), CapabilityError> {
        assert_eq!(slot, 28);
        if self.fail_receiver {
            return Err(CapabilityError::InvalidArgument);
        }
        self.receivers.push(owner);
        Ok(())
    }
}
fn id(index: Word) -> SessionId {
    SessionId(SESSION_IDENTIFIER_BIT | index)
}

#[test]
fn resource_rescan_lists_only_ready_live_sessions_for_the_selected_service() {
    let mut buffers = Buffers::new();
    let mut memory = Memory::default();
    for (session, service) in [(1, 1), (2, 2), (3, 1)] {
        buffers
            .create(id(session), ServiceId(service), 1, 2, 1, &mut memory)
            .unwrap();
    }
    assert_eq!(buffers.session_at(ServiceId(1), 0), Some(id(1)));
    assert_eq!(buffers.session_at(ServiceId(1), 1), Some(id(3)));
    assert_eq!(buffers.session_at(ServiceId(1), 2), None);
    buffers.withdraw(ServiceId(1), 2, &mut memory).unwrap();
    buffers.close(id(1), &mut memory).unwrap();
    assert_eq!(buffers.session_at(ServiceId(1), 0), Some(id(3)));
    assert_eq!(buffers.session_at(ServiceId(1), 1), None);
    assert_eq!(buffers.session_at(ServiceId(2), 0), Some(id(2)));
}

#[test]
fn backing_and_client_va_survive_receiver_exchange_without_old_mapping() {
    let mut buffers = Buffers::new();
    let mut memory = Memory::default();
    let client = buffers
        .create(id(1), ServiceId(1), 1, 2, 2, &mut memory)
        .unwrap();
    assert_eq!(client.bytes, 8192);
    assert!(memory.contents.values().all(|value| *value == 0));
    let physical = memory.maps[&(1, client.address)];
    memory.contents.insert(physical, 1234);
    let old = buffers.view(id(1), 2).unwrap();
    assert!(buffers.has_mapping(2));
    buffers.withdraw(ServiceId(1), 2, &mut memory).unwrap();
    assert!(!buffers.has_mapping(2));
    assert!(!memory.maps.keys().any(|key| key.0 == 2));
    assert!(buffers.view(id(1), 2).is_err());
    buffers.install(ServiceId(1), 3, 28, &mut memory).unwrap();
    let new = buffers.view(id(1), 3).unwrap();
    assert_ne!(old.address, new.address);
    assert_eq!(memory.maps[&(3, new.address)], physical);
    assert_eq!(memory.contents[&physical], 1234);
    assert_eq!(memory.maps[&(1, client.address)], physical);
    assert_eq!(memory.receivers, [3]);
}

#[test]
fn abort_restores_the_old_providers_exact_va_and_contents() {
    let mut buffers = Buffers::new();
    let mut memory = Memory::default();
    buffers
        .create(id(1), ServiceId(1), 1, 2, 1, &mut memory)
        .unwrap();
    let old = buffers.view(id(1), 2).unwrap();
    buffers.withdraw(ServiceId(1), 2, &mut memory).unwrap();
    buffers.install(ServiceId(1), 2, 28, &mut memory).unwrap();
    assert_eq!(buffers.view(id(1), 2).unwrap().address, old.address);
}

#[test]
fn failed_create_cleans_even_the_page_whose_map_failed_after_installation() {
    let mut buffers = Buffers::new();
    let mut memory = Memory {
        fail_map: Some(1),
        ..Memory::default()
    };
    assert!(buffers
        .create(id(1), ServiceId(1), 1, 2, 2, &mut memory)
        .is_err());
    assert!(memory.maps.is_empty());
    assert!(buffers.get(id(1)).unwrap().unwrap().closed);
    assert_eq!(memory.allocations, 1);
    assert!(buffers
        .create(id(1), ServiceId(1), 1, 2, 2, &mut memory)
        .is_err());
    assert_eq!(
        memory.allocations, 1,
        "quarantined backing was replaced/reused"
    );
}

#[test]
fn failed_candidate_mapping_rolls_back_all_candidates_before_abort() {
    let mut buffers = Buffers::new();
    let mut memory = Memory::default();
    for session in 1..=2 {
        buffers
            .create(id(session), ServiceId(1), 1, 2, 2, &mut memory)
            .unwrap();
    }
    buffers.withdraw(ServiceId(1), 2, &mut memory).unwrap();
    memory.fail_map = Some(memory.map_calls + 2);
    assert!(buffers.install(ServiceId(1), 3, 28, &mut memory).is_err());
    assert!(!memory.maps.keys().any(|key| key.0 == 3));
    assert!(memory.receivers.is_empty());
    assert!(buffers.get(id(1)).unwrap().unwrap().candidate.is_none());
    memory.fail_map = None;
    buffers.install(ServiceId(1), 2, 28, &mut memory).unwrap();
    assert!(buffers.view(id(1), 2).is_ok());
    assert!(buffers.view(id(2), 2).is_ok());
}

#[test]
fn receiver_install_failure_does_not_publish_candidate_data_access() {
    let mut buffers = Buffers::new();
    let mut memory = Memory::default();
    buffers
        .create(id(1), ServiceId(1), 1, 2, 1, &mut memory)
        .unwrap();
    buffers.withdraw(ServiceId(1), 2, &mut memory).unwrap();
    memory.fail_receiver = true;
    assert!(buffers.install(ServiceId(1), 3, 28, &mut memory).is_err());
    assert!(!memory.maps.keys().any(|key| key.0 == 3));
    assert_eq!(buffers.get(id(1)).unwrap().unwrap().provider.owner, 2);
}

#[test]
fn uncertain_unmap_is_not_reported_as_withdrawn_or_reclaimable() {
    let mut buffers = Buffers::new();
    let mut memory = Memory::default();
    buffers
        .create(id(1), ServiceId(1), 1, 2, 1, &mut memory)
        .unwrap();
    memory.fail_unmap = true;
    assert!(matches!(
        buffers.withdraw(ServiceId(1), 2, &mut memory),
        Err(CapabilityError::Fatal)
    ));
    assert!(buffers.has_mapping(2));
    assert_eq!(buffers.get(id(1)).unwrap().unwrap().provider.installed, 1);
    assert!(buffers.install(ServiceId(1), 3, 28, &mut memory).is_err());
    assert!(memory.receivers.is_empty());
}

#[test]
fn close_requires_quiescence_and_keeps_backing_charged() {
    let mut buffers = Buffers::new();
    let mut memory = Memory::default();
    buffers
        .create(id(1), ServiceId(1), 1, 2, 1, &mut memory)
        .unwrap();
    assert!(buffers.close(id(1), &mut memory).is_err());
    buffers.withdraw(ServiceId(1), 2, &mut memory).unwrap();
    buffers.close(id(1), &mut memory).unwrap();
    assert!(memory.maps.is_empty());
    assert_eq!(memory.allocations, 1);
    assert_eq!(memory.contents.len(), 1);
    buffers.install(ServiceId(1), 3, 28, &mut memory).unwrap();
    assert!(buffers.view(id(1), 3).is_err());
    assert!(memory.maps.is_empty());
}

#[test]
fn bounds_and_client_provider_aliasing_fail_without_granting_authority() {
    let mut buffers = Buffers::new();
    let mut memory = Memory::default();
    for (session, client, provider, pages) in [
        (SessionId(1), 1, 2, 1),
        (id(65), 1, 2, 1),
        (id(1), 0, 2, 1),
        (id(1), 1, 1, 1),
        (id(1), 1, 2, 0),
        (id(1), 1, 2, 17),
    ] {
        assert!(buffers
            .create(session, ServiceId(1), client, provider, pages, &mut memory)
            .is_err());
    }
    assert_eq!(memory.allocations, 0);
    buffers
        .create(id(1), ServiceId(1), 1, 2, 1, &mut memory)
        .unwrap();
    buffers.withdraw(ServiceId(1), 2, &mut memory).unwrap();
    assert!(buffers.install(ServiceId(1), 1, 28, &mut memory).is_err());
    assert!(memory.receivers.is_empty());
}

#[test]
fn rejecting_an_existing_mapping_never_unmaps_the_other_owners_page() {
    let mut buffers = Buffers::new();
    let mut memory = Memory::default();
    memory.maps.insert((1, 0x100000), 999);
    assert!(buffers
        .create(id(1), ServiceId(1), 1, 2, 1, &mut memory)
        .is_err());
    assert_eq!(memory.maps.get(&(1, 0x100000)), Some(&999));
}

#[test]
fn quiescing_preserves_views_for_accepted_work_but_withdrawal_ends_them() {
    assert!(BindingPhase::Active.permits_buffer_view());
    assert!(BindingPhase::Prepared.permits_buffer_view());
    assert!(BindingPhase::Quiescing.permits_buffer_view());
    assert!(!BindingPhase::Quiesced.permits_buffer_view());
    assert!(!BindingPhase::Faulted.permits_buffer_view());
    let mut buffers = Buffers::new();
    let mut memory = Memory::default();
    buffers
        .create(id(1), ServiceId(1), 1, 2, 1, &mut memory)
        .unwrap();
    assert!(buffers.view(id(1), 2).is_ok());
    buffers.withdraw(ServiceId(1), 2, &mut memory).unwrap();
    assert!(buffers.view(id(1), 2).is_err());
}

#[test]
fn a_client_reaper_cannot_be_installed_as_its_withdrawable_provider() {
    let mut buffers = Buffers::new();
    let mut memory = Memory::default();
    memory.reapers.insert(1, 2);
    assert!(buffers
        .create(id(1), ServiceId(1), 1, 2, 1, &mut memory)
        .is_err());
    assert_eq!(memory.allocations, 0);
    memory.reapers.insert(1, 3);
    buffers
        .create(id(1), ServiceId(1), 1, 2, 1, &mut memory)
        .unwrap();
    buffers.withdraw(ServiceId(1), 2, &mut memory).unwrap();
    assert!(buffers.install(ServiceId(1), 3, 28, &mut memory).is_err());
    assert!(memory.receivers.is_empty());
    assert!(!memory.maps.keys().any(|key| key.0 == 3));
}
