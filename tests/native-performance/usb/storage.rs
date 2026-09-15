#![allow(dead_code)]
extern crate self as a9n_abi;
extern crate self as libnanami;
extern crate self as nanami_services;
use std::{cell::RefCell, collections::VecDeque};
pub type Word = usize;
pub type CapabilityDescriptor = usize;
pub const OS_RESPONSE_OK: Word = 0;
#[derive(Debug, PartialEq)]
pub enum RequestError {
    Protocol,
    Transport,
    Status(Word),
}
#[derive(Default)]
struct ClientFake {
    connects: usize,
    decisions: VecDeque<Word>,
    roots: Vec<Word>,
    sleeps: usize,
}
thread_local! { static CLIENT: RefCell<ClientFake> = RefCell::new(ClientFake::default()); }
pub fn connect_service_by_name(name: &str, slot: Word) -> Result<(), RequestError> {
    assert_eq!((name, slot), (device::DEVICE_MANAGER_SERVICE, 28));
    CLIENT.with(|client| client.borrow_mut().connects += 1);
    Ok(())
}
pub fn yield_now() {
    panic!("timer is available in this test");
}
pub mod ipc {
    pub fn process_slot_descriptor(slot: usize) -> usize {
        slot
    }
}
pub mod registry {
    pub fn connect_timer_service(slot: usize) -> Result<(), super::RequestError> {
        assert_eq!(slot, 29);
        Ok(())
    }
}
pub mod timer {
    pub fn timer_service_sleep_milliseconds(
        port: usize,
        ms: usize,
    ) -> Result<(), super::RequestError> {
        assert_eq!((port, ms), (29, 100));
        super::CLIENT.with(|client| client.borrow_mut().sleeps += 1);
        Ok(())
    }
}
fn call_port(
    port: Word,
    code: Word,
    roots: Word,
    _: Word,
    _: Word,
    _: Word,
    length: u8,
) -> Result<(Word, Word, Word), RequestError> {
    assert!(matches!(port, 25 | 28));
    assert_eq!(
        (code, length),
        (device::DEVICE_MANAGER_REQUEST_STORAGE_PROBE, 2),
        "request must transmit both its code and root count"
    );
    CLIENT.with(|client| {
        let mut client = client.borrow_mut();
        client.roots.push(roots);
        Ok((
            OS_RESPONSE_OK,
            client
                .decisions
                .pop_front()
                .unwrap_or(device::STORAGE_PENDING),
            0,
        ))
    })
}
#[path = "../../../nanami/servers/sdk/rust/nanami-services/src/device.rs"]
pub mod device;
#[path = "../../../nanami/servers/core-services/driver-manager/src/storage.rs"]
mod selection;
#[path = "../../../nanami/servers/core-services/usb-server/src/usb/mass_storage.rs"]
mod wire;

fn configuration(super_speed: bool) -> Vec<u8> {
    let mut bytes = vec![9, 2, 0, 0, 1, 1, 0, 0x80, 50, 9, 4, 0, 0, 2, 8, 6, 0x50, 0];
    for endpoint in [1, 0x82] {
        bytes.extend([7, 5, endpoint, 2, 0, if super_speed { 4 } else { 2 }, 0]);
        if super_speed {
            bytes.extend([6, 48, 15, 0, 0, 0]);
        }
    }
    bytes[2] = bytes.len() as u8;
    bytes
}

#[test]
fn bulk_interfaces_and_superspeed_companions() {
    for ss in [false, true] {
        let interface = wire::interface(&configuration(ss), ss).unwrap().unwrap();
        assert_eq!(interface.number, 0);
        assert_eq!(
            (interface.output.address, interface.input.address),
            (1, 0x82)
        );
        assert_eq!(interface.input.packet, if ss { 1024 } else { 512 });
        assert_eq!(interface.input.burst, if ss { 15 } else { 0 });
    }
}

#[test]
fn malformed_storage_descriptors_are_rejected() {
    for ss in [false, true] {
        let good = configuration(ss);
        for end in 0..good.len() {
            assert!(wire::interface(&good[..end], ss).is_err());
        }
        for (offset, value) in [
            (0, 8),
            (5, 0),
            (9, 0),
            (13, 1),
            (18, 6),
            (20, 0x80),
            (20, 0x91),
            (21, 3),
            (23, 8),
        ] {
            let mut bad = good.clone();
            bad[offset] = value;
            assert!(
                wire::interface(&bad, ss).is_err(),
                "offset={offset} ss={ss}"
            );
        }
    }
    let mut missing = configuration(false);
    missing[23] = 4;
    missing[30] = 4;
    assert!(wire::interface(&missing, true).is_err());
    for (offset, value) in [(26, 47), (27, 16), (28, 1), (29, 1), (30, 1)] {
        let mut bad = configuration(true);
        bad[offset] = value;
        assert!(wire::interface(&bad, true).is_err());
    }
}

#[test]
fn uas_alternate_settings_and_other_classes_are_not_claimed() {
    for (offset, value) in [(12, 1), (14, 3), (15, 4), (16, 0x62)] {
        let mut bytes = configuration(false);
        bytes[offset] = value;
        assert_eq!(wire::interface(&bytes, false), Ok(None));
    }
}

#[test]
fn duplicate_bulk_endpoints_and_storage_interfaces_are_rejected() {
    let mut bytes = configuration(false);
    bytes[27] = 2; // two OUT endpoints, no IN
    assert!(wire::interface(&bytes, false).is_err());
    let mut bytes = configuration(false);
    bytes.extend_from_slice(&configuration(false)[9..]);
    bytes[2] = bytes.len() as u8;
    assert!(wire::interface(&bytes, false).is_err());
}

#[test]
fn cbw_has_exact_wire_size_endian_and_padding() {
    let cbw = wire::cbw(0x12345678, 3, &[0x28; 10], 0x4000, true).unwrap();
    assert_eq!(cbw.len(), 31);
    assert_eq!(
        &cbw[..15],
        &[0x55, 0x53, 0x42, 0x43, 0x78, 0x56, 0x34, 0x12, 0, 0x40, 0, 0, 0x80, 3, 10]
    );
    assert_eq!(&cbw[15..25], &[0x28; 10]);
    assert_eq!(&cbw[25..], &[0; 6]);
    assert!(wire::cbw(0, 16, &[0; 6], 0, false).is_err());
    assert!(wire::cbw(0, 0, &[], 0, false).is_err());
    assert!(wire::cbw(0, 0, &[0; 17], 0, false).is_err());
    assert!(wire::cbw(0, 0, &[0; 6], usize::MAX, false).is_err());
}

fn csw(tag: u32, residue: u32, status: u8) -> Vec<u8> {
    let mut bytes = b"USBS".to_vec();
    bytes.extend(tag.to_le_bytes());
    bytes.extend(residue.to_le_bytes());
    bytes.push(status);
    bytes
}

#[test]
fn csw_checks_tag_signature_length_residue_and_phase() {
    let good = csw(42, 0, 0);
    assert_eq!(wire::csw(&good, 42, 512, 512), Ok((true, 512)));
    assert_eq!(wire::csw(&csw(42, 512, 1), 42, 512, 0), Ok((false, 0)));
    assert_eq!(wire::csw(&csw(42, 256, 0), 42, 512, 256), Ok((true, 256)));
    assert_eq!(wire::csw(&csw(42, 256, 0), 42, 512, 512), Ok((true, 256)));
    for len in 0..13 {
        assert!(wire::csw(&good[..len], 42, 512, 512).is_err());
    }
    assert!(wire::csw(&good, 41, 512, 512).is_err());
    assert!(wire::csw(&good, 42, 512, 128).is_err());
    assert!(wire::csw(&good, 42, 512, 513).is_err());
    for status in 2..=255 {
        assert!(wire::csw(&csw(42, 0, status), 42, 512, 512).is_err());
    }
    assert!(wire::csw(&csw(42, 513, 1), 42, 512, 512).is_err());
    let mut bad = good;
    bad[0] = 0;
    assert!(wire::csw(&bad, 42, 512, 512).is_err());
}

#[test]
fn scsi_lba_endianness_and_32_bit_boundary() {
    let (cdb, length) = wire::read_write(0x12345678, 32, false).unwrap();
    assert_eq!(length, 10);
    assert_eq!(cdb[0], 0x28);
    assert_eq!(&cdb[2..6], &[0x12, 0x34, 0x56, 0x78]);
    assert_eq!(&cdb[7..9], &[0, 32]);
    let (cdb, length) = wire::read_write(u32::MAX as u64, 2, true).unwrap();
    assert_eq!(length, 16);
    assert_eq!(cdb[0], 0x8a);
    assert_eq!(&cdb[2..10], &(u32::MAX as u64).to_be_bytes());
    assert_eq!(&cdb[10..14], &2u32.to_be_bytes());
    assert!(wire::read_write(1, 0, true).is_err());
    assert!(wire::read_write(u64::MAX, 2, true).is_err());
}

#[test]
fn block_requests_cannot_escape_root_or_overflow_dma() {
    assert_eq!(wire::block_range(4096, 100, 0, 16), Some((4096, 16384)));
    assert_eq!(wire::block_range(4096, 100, 49, 1), Some((4194, 1024)));
    for (block, count) in [
        (50, 1),
        (49, 2),
        (0, 0),
        (0, 17),
        (usize::MAX, 1),
        (1, usize::MAX),
    ] {
        assert_eq!(wire::block_range(4096, 100, block, count), None);
    }
    assert_eq!(wire::block_range(u64::MAX, 100, 1, 1), None);
}

#[test]
fn storage_selection_waits_for_every_driver_in_either_order() {
    use device::*;
    for order in [[0, 1], [1, 0]] {
        for winner in 0..2 {
            let mut selection = selection::Selection::new();
            selection.pids = [5, 7];
            let first = order[0];
            let second = order[1];
            assert_eq!(
                selection.report(selection.pids[first], (first == winner) as usize),
                Ok(STORAGE_PENDING)
            );
            assert_eq!(
                selection.report(selection.pids[second], (second == winner) as usize),
                Ok(if second == winner {
                    STORAGE_SELECTED
                } else {
                    STORAGE_NOT_SELECTED
                })
            );
            assert_eq!(
                selection.report(selection.pids[winner], 1),
                Ok(STORAGE_SELECTED)
            );
        }
    }
}

#[test]
fn root_ambiguity_and_changed_or_unauthorized_reports_fail_closed() {
    use device::*;
    let mut selection = selection::Selection::new();
    selection.pids = [5, 7];
    assert!(selection.report(0, 1).is_err());
    assert!(selection.report(8, 1).is_err());
    assert!(selection.report(5, 3).is_err());
    assert_eq!(selection.report(5, 1), Ok(STORAGE_PENDING));
    assert!(selection.report(5, 0).is_err());
    assert_eq!(selection.report(7, 1), Ok(STORAGE_AMBIGUOUS));
    assert_eq!(selection.report(5, 1), Ok(STORAGE_AMBIGUOUS));
    let mut selection = selection::Selection::new();
    selection.pids = [0, 7];
    assert_eq!(selection.report(7, 2), Ok(STORAGE_AMBIGUOUS));
}

#[test]
fn single_driver_and_no_media_do_not_wait_for_nonexistent_drivers() {
    use device::*;
    for roots in [0, 1] {
        let mut selection = selection::Selection::new();
        selection.pids = [5, 0];
        assert_eq!(
            selection.report(5, roots),
            Ok(if roots == 1 {
                STORAGE_SELECTED
            } else {
                STORAGE_NOT_SELECTED
            })
        );
    }
}

#[test]
fn empty_scan_does_not_finalize_root_selection() {
    use device::*;
    let mut selection = selection::Selection::new();
    selection.pids = [5, 7];
    assert_eq!(selection.report(5, 0), Ok(STORAGE_PENDING));
    assert_eq!(selection.report(7, 0), Ok(STORAGE_NOT_SELECTED));
    assert_eq!(selection.report(7, 1), Ok(STORAGE_SELECTED));
    assert_eq!(selection.report(7, 1), Ok(STORAGE_SELECTED));
}

#[test]
fn late_candidate_cannot_replace_a_selected_root() {
    use device::*;
    let mut selection = selection::Selection::new();
    selection.pids = [5, 7];
    assert_eq!(selection.report(7, 0), Ok(STORAGE_PENDING));
    assert_eq!(selection.report(5, 1), Ok(STORAGE_SELECTED));
    assert_eq!(selection.report(7, 1), Ok(STORAGE_NOT_SELECTED));
    assert_eq!(selection.report(5, 1), Ok(STORAGE_SELECTED));
}

#[test]
fn rediscovery_before_selection_still_rejects_duplicate_roots() {
    use device::*;
    let mut selection = selection::Selection::new();
    selection.pids = [5, 7];
    assert_eq!(selection.report(7, 0), Ok(STORAGE_PENDING));
    assert_eq!(selection.report(7, 1), Ok(STORAGE_PENDING));
    assert_eq!(selection.report(5, 1), Ok(STORAGE_AMBIGUOUS));
    assert_eq!(selection.report(7, 1), Ok(STORAGE_AMBIGUOUS));
    assert!(selection.report(7, 0).is_err());
}

#[test]
fn root_client_sends_count_and_waits_without_busy_polling() {
    use device::*;
    CLIENT.with(|client| {
        *client.borrow_mut() = ClientFake {
            decisions: [STORAGE_PENDING, STORAGE_SELECTED].into(),
            ..ClientFake::default()
        }
    });
    assert_eq!(select_storage_root(1), Ok(true));
    CLIENT.with(|client| {
        let client = client.borrow();
        assert_eq!(client.roots, [1, 1]);
        assert_eq!(client.sleeps, 1);
    });
}

#[test]
fn non_candidate_client_reports_once_and_candidate_timeout_is_bounded() {
    CLIENT.with(|client| *client.borrow_mut() = ClientFake::default());
    assert_eq!(device::select_storage_root(0), Ok(false));
    CLIENT.with(|client| {
        assert_eq!(client.borrow().roots, [0]);
        assert_eq!(client.borrow().sleeps, 0);
    });
    CLIENT.with(|client| *client.borrow_mut() = ClientFake::default());
    assert_eq!(device::select_storage_root(1), Err(RequestError::Transport));
    CLIENT.with(|client| {
        assert_eq!(client.borrow().roots.len(), 301);
        assert_eq!(client.borrow().sleeps, 300);
    });
}

#[test]
fn rediscovery_reuses_the_existing_manager_capability() {
    use device::*;
    CLIENT.with(|client| {
        *client.borrow_mut() = ClientFake {
            decisions: [STORAGE_NOT_SELECTED, STORAGE_SELECTED].into(),
            ..ClientFake::default()
        };
    });
    assert_eq!(select_storage_root_on_port(25, 0), Ok(false));
    assert_eq!(select_storage_root_on_port(25, 1), Ok(true));
    CLIENT.with(|client| {
        let client = client.borrow();
        assert_eq!(client.roots, [0, 1]);
        assert_eq!(client.connects, 0);
    });
}
