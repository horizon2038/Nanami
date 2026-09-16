#![allow(dead_code)]
extern crate alloc;
extern crate self as libnanami;
pub use std::println;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    Unsupported,
    Transport,
    Protocol,
}
mod usb {
    pub(crate) use crate::hid;
}

#[path = "../../../nanami/servers/core-services/usb-server/src/xhci/capabilities.rs"]
mod capabilities;
#[path = "capability-tests.rs"]
mod capability_tests;
#[path = "../../../nanami/servers/core-services/usb-server/src/usb/descriptors.rs"]
mod descriptors;
#[path = "../../../nanami/servers/core-services/usb-server/src/usb/hid.rs"]
pub(crate) mod hid;
#[path = "hid-negotiation.rs"]
mod hid_negotiation;
#[path = "hid-reports.rs"]
mod hid_reports;
#[path = "../../../nanami/servers/core-services/usb-server/src/xhci/protocol.rs"]
mod protocol;
#[path = "../../../nanami/servers/core-services/usb-server/src/xhci/registers.rs"]
mod registers;
#[path = "../../../nanami/servers/core-services/usb-server/src/xhci/ring.rs"]
mod ring;

use hid::{Event, Keyboard, Mouse};
use ring::{Consumer, Producer, Trb, CYCLE, ENTRIES};

fn key(code: usize, pressed: bool) -> Event {
    Event {
        kind: 1,
        code,
        x: pressed as i16,
        y: 0,
    }
}

#[test]
fn keyboard_transitions_and_identical_reports() {
    let mut keyboard = Keyboard::new();
    let mut events = Vec::new();
    keyboard.report(&[2, 0, 4, 5, 0, 0, 0, 0], |e| events.push(e));
    assert_eq!(events, [key(0x2a, true), key(0x1e, true), key(0x30, true)]);
    events.clear();
    keyboard.report(&[2, 0, 5, 4, 0, 0, 0, 0], |e| events.push(e));
    assert!(events.is_empty());
    keyboard.report(&[0, 0, 5, 0, 0, 0, 0, 0], |e| events.push(e));
    assert_eq!(events, [key(0x2a, false), key(0x1e, false)]);
    events.clear();
    keyboard.release(|e| events.push(e));
    keyboard.release(|e| events.push(e));
    assert_eq!(events, [key(0x30, false)]);
}

#[test]
fn keyboard_rollover_and_short_reports_preserve_state() {
    let mut keyboard = Keyboard::new();
    keyboard.report(&[1, 0, 4, 0, 0, 0, 0, 0], |_| {});
    let mut events = Vec::new();
    for error in 1..=3 {
        keyboard.report(&[0, 0, error, error, error, error, error, error], |e| {
            events.push(e)
        });
    }
    for length in 0..8 {
        keyboard.report(&[0; 8][..length], |e| events.push(e));
    }
    assert!(events.is_empty());
    keyboard.release(|e| events.push(e));
    assert_eq!(events, [key(0x1d, false), key(0x1e, false)]);
}

#[test]
fn keyboard_duplicate_usages_generate_one_press_and_release() {
    let mut keyboard = Keyboard::new();
    let mut events = Vec::new();
    keyboard.report(&[0, 0, 4, 4, 4, 4, 4, 4], |e| events.push(e));
    keyboard.release(|e| events.push(e));
    assert_eq!(events, [key(0x1e, true), key(0x1e, false)]);
}

#[test]
fn keyboard_modifiers_and_extended_keys_match_ps2_codes() {
    let mut keyboard = Keyboard::new();
    let mut events = Vec::new();
    keyboard.report(&[0xf0, 0, 76, 79, 80, 81, 82, 88], |e| events.push(e));
    assert_eq!(
        events,
        [0x11d, 0x36, 0x138, 0x15c, 0x153, 0x14d, 0x14b, 0x150, 0x148, 0x11c]
            .map(|code| key(code, true))
    );
}

#[test]
fn mouse_signed_motion_buttons_and_detach() {
    let mut mouse = Mouse::new();
    let mut events = Vec::new();
    mouse.report(&[7, 128, 127, 99], |e| events.push(e));
    assert_eq!(
        events,
        [
            Event {
                kind: 2,
                code: 1,
                x: 1,
                y: 0
            },
            Event {
                kind: 2,
                code: 2,
                x: 1,
                y: 0
            },
            Event {
                kind: 2,
                code: 3,
                x: 1,
                y: 0
            },
            Event {
                kind: 3,
                code: 0,
                x: -128,
                y: 127
            },
        ]
    );
    events.clear();
    mouse.report(&[0, 0], |e| events.push(e));
    assert!(events.is_empty());
    mouse.release(|e| events.push(e));
    mouse.release(|e| events.push(e));
    assert_eq!(
        events,
        [1, 2, 3].map(|code| Event {
            kind: 2,
            code,
            x: 0,
            y: 0
        })
    );
}

fn config(body: &[u8]) -> Vec<u8> {
    let total = (9 + body.len()) as u16;
    let mut bytes = vec![9, 2, total as u8, (total >> 8) as u8, 2, 1, 0, 0x80, 50];
    bytes.extend_from_slice(body);
    bytes
}

fn interface(number: u8, protocol: u8) -> Vec<u8> {
    vec![
        9,
        4,
        number,
        0,
        1,
        3,
        1,
        protocol,
        0,
        9,
        0x21,
        0x11,
        1,
        0,
        1,
        0x22,
        63,
        0,
        7,
        5,
        0x81 + number,
        3,
        8,
        0,
        10,
    ]
}

#[test]
fn composite_boot_interfaces_are_separate() {
    let mut body = interface(0, 1);
    body.extend(interface(1, 2));
    let mut found = Vec::new();
    assert_eq!(
        descriptors::boot_interfaces(&config(&body), |i| found.push(i)),
        Ok(1)
    );
    assert_eq!(found.len(), 2);
    assert_eq!(
        (found[0].number, found[0].protocol, found[0].endpoint),
        (0, 1, 0x81)
    );
    assert_eq!(
        (found[1].number, found[1].protocol, found[1].endpoint),
        (1, 2, 0x82)
    );
}

#[test]
fn descriptor_truncations_and_zero_lengths_are_rejected() {
    let valid = config(&interface(0, 1));
    for end in 0..valid.len() {
        assert!(descriptors::boot_interfaces(&valid[..end], |_| {}).is_err());
    }
    for (offset, value) in [(9, 0), (9, 1), (9, 8), (18, 255), (27, 6), (5, 0)] {
        let mut bad = valid.clone();
        bad[offset] = value;
        assert!(descriptors::boot_interfaces(&bad, |_| {}).is_err());
    }
}

#[test]
fn unsupported_interfaces_and_endpoints_are_not_claimed() {
    for (offset, value) in [
        (3, 1),
        (5, 0xff),
        (6, 0),
        (7, 0), // alt, class, subclass, protocol
        (20, 1),
        (20, 0x80),
        (20, 0x91), // OUT, EP0, reserved address
        (21, 2),
        (22, 7),
        (23, 8),
        (24, 0), // bulk, short MPS, transactions, zero interval
    ] {
        let mut body = interface(0, 1);
        body[offset] = value;
        let mut found = Vec::new();
        assert_eq!(
            descriptors::boot_interfaces(&config(&body), |i| found.push(i)),
            Ok(1)
        );
        assert!(found.is_empty(), "offset={offset} value={value}");
    }
}

#[repr(C, align(4096))]
struct Page([Trb; ENTRIES]);
fn page() -> Box<Page> {
    Box::new(Page([Trb::default(); ENTRIES]))
}
fn trb(parameter: u64) -> Trb {
    Trb {
        parameter,
        status: 8,
        control: (1 << 10) | ring::IOC,
    }
}

#[test]
fn producer_requires_completion_before_reusing_dma() {
    let mut memory = page();
    let mut producer = unsafe { Producer::new(0x10000, memory.0.as_mut_ptr() as usize) };
    assert_eq!(producer.submit(&[]), None);
    assert_eq!(producer.submit(&[trb(123)]), Some(0x10000));
    assert_eq!(memory.0[0].parameter, 123);
    assert_eq!(memory.0[0].control & CYCLE, 1);
    assert_eq!(producer.submit(&[trb(456)]), None);
    assert_eq!(memory.0[1], Trb::default());
    producer.complete();
    assert_eq!(producer.submit(&[trb(456)]), Some(0x10010));
}

#[test]
fn stopped_endpoint_ring_reset_clears_old_cycle_and_pending_state() {
    let mut memory = page();
    let mut producer = unsafe { Producer::new(0x10000, memory.0.as_mut_ptr() as usize) };
    producer.submit(&[trb(123)]).unwrap();
    // Models ownership after a successful Reset/Stop Endpoint command.
    unsafe { producer.reset() };
    assert!(memory.0.iter().all(|&entry| entry == Trb::default()));
    assert_eq!(producer.submit(&[trb(456)]), Some(0x10000));
    assert_eq!(memory.0[0].control & CYCLE, 1);
    assert_eq!(memory.0[0].parameter, 456);
}

#[test]
fn superspeed_psiv_is_separate_from_usb2_classes() {
    let entries = [
        (5 << 16) | (3 << 4) | (1 << 8) | 4,
        // Asymmetric Rx/Tx share a PSIV and can have different rates.
        (10 << 16) | (3 << 4) | (1 << 8) | (2 << 6) | 5,
        (5 << 16) | (3 << 4) | (1 << 8) | (3 << 6) | 5,
    ];
    let speeds = protocol::Speeds::parse(3, 0x10, &entries, |_, _| panic!()).unwrap();
    assert_eq!(speeds.usb3, (1 << 4) | (1 << 5));
    assert_eq!(speeds.usb2, 0);
}

#[test]
fn producer_wraps_with_link_and_toggles_cycle() {
    let mut memory = page();
    let mut producer = unsafe { Producer::new(0x10000, memory.0.as_mut_ptr() as usize) };
    for index in 0..(3 * (ENTRIES - 1)) {
        let slot = index % (ENTRIES - 1);
        let cycle = 1 ^ ((index / (ENTRIES - 1)) & 1) as u32;
        assert_eq!(
            producer.submit(&[trb(index as u64)]),
            Some(0x10000 + slot as u64 * 16)
        );
        assert_eq!(memory.0[slot].control & CYCLE, cycle);
        if index >= ENTRIES - 1 {
            assert_eq!(memory.0[ENTRIES - 1].kind(), 6);
            assert_eq!(memory.0[ENTRIES - 1].control & 3, 2 | (cycle ^ 1));
            assert_eq!(memory.0[ENTRIES - 1].parameter, 0x10000);
        }
        producer.complete();
    }
}

#[test]
fn multi_trb_td_can_cross_link() {
    let mut memory = page();
    let mut producer = unsafe { Producer::new(0x10000, memory.0.as_mut_ptr() as usize) };
    for _ in 0..254 {
        producer.submit(&[trb(0)]).unwrap();
        producer.complete();
    }
    assert_eq!(producer.submit(&[trb(1), trb(2), trb(3)]), Some(0x10010));
    assert_eq!(memory.0[254].parameter, 1);
    assert_eq!(memory.0[254].control & CYCLE, 1);
    assert_eq!(memory.0[255].kind(), 6);
    assert_eq!(memory.0[0].parameter, 2);
    assert_eq!(memory.0[1].parameter, 3);
    assert_eq!(memory.0[0].control & CYCLE, 0);
    assert_eq!(memory.0[1].control & CYCLE, 0);
}

#[test]
fn event_consumer_obeys_cycle_including_wrap() {
    let mut memory = page();
    let mut consumer = unsafe { Consumer::new(0x20000, memory.0.as_mut_ptr() as usize) };
    assert_eq!(consumer.pop(), None);
    for pass in 0..3 {
        for index in 0..ENTRIES {
            memory.0[index] = Trb {
                parameter: index as u64,
                status: 1 << 24,
                control: (32 << 10) | (2 << 24) | (3 << 16) | (1 ^ (pass & 1)),
            };
            let event = consumer.pop().unwrap();
            assert_eq!(event.parameter, index as u64);
            assert_eq!(
                (event.kind(), event.slot(), event.endpoint(), event.code()),
                (32, 2, 3, 1)
            );
            assert_eq!(
                consumer.next_physical(),
                0x20000 + (((index + 1) % ENTRIES) * 16) as u64
            );
        }
        assert_eq!(consumer.pop(), None);
    }
}

#[test]
fn port_writes_do_not_echo_rw1c_or_action_bits() {
    let preserved = (1 << 9) | (3 << 14) | (7 << 25);
    assert_eq!(registers::port_controls(u32::MAX), preserved);
    assert_eq!(
        registers::port_controls(registers::PORT_CHANGE | registers::PORT_RESET | 2),
        0
    );
}

#[test]
fn controller_speed_ids_are_not_assumed_to_be_standard() {
    assert_eq!(
        (1..=3)
            .map(|id| protocol::classify(protocol::DEFAULT_SPEEDS, id))
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    let entries = [
        (1500 << 16) | (1 << 4) | 7,
        (12 << 16) | (2 << 4) | 5,
        (480 << 16) | (2 << 4) | 9,
    ];
    let speeds = protocol::Speeds::parse(2, 0, &entries, |_, _| panic!()).unwrap();
    assert_eq!(protocol::classify(speeds.usb2, 7), 2);
    assert_eq!(protocol::classify(speeds.usb2, 5), 1);
    assert_eq!(protocol::classify(speeds.usb2, 9), 3);
    assert_eq!(protocol::classify(speeds.usb2, 1), 0);
    assert_eq!(speeds.usb3, 0);
}
