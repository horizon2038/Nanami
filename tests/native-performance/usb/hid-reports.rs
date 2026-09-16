use super::hid::{Event, Mouse, MouseReport};

const OPEN: &[u8] = &[0x05, 1, 0x09, 2, 0xa1, 1, 0x09, 1, 0xa1, 0];
const BUTTONS: &[u8] = &[
    0x05, 9, 0x19, 1, 0x29, 5, 0x15, 0, 0x25, 1, 0x75, 1, 0x95, 5, 0x81, 2,
];
const PADDING: &[u8] = &[0x75, 3, 0x95, 1, 0x81, 3];
const WHEEL: &[u8] = &[
    0x05, 1, 0x09, 0x38, 0x15, 0x81, 0x25, 0x7f, 0x75, 8, 0x95, 1, 0x81, 6,
];

// Synthetic layouts, not a claimed capture of the user's physical mouse.
pub(super) fn descriptor(id: u8, width: u8, padding: bool) -> Vec<u8> {
    let mut bytes = OPEN.to_vec();
    if id != 0 {
        bytes.extend([0x85, id]);
    }
    bytes.extend(BUTTONS);
    if padding {
        bytes.extend(PADDING);
    }
    let maximum = (1i32 << (width - 1)) - 1;
    bytes.extend([0x05, 1, 0x09, 0x30, 0x09, 0x31, 0x17]);
    bytes.extend((-maximum).to_le_bytes());
    bytes.push(0x27);
    bytes.extend(maximum.to_le_bytes());
    bytes.extend([0x75, width, 0x95, 2, 0x81, 6]);
    bytes.extend(WHEEL);
    bytes.extend([0xc0, 0xc0]);
    bytes
}

fn mouse(bytes: &[u8], packet: usize) -> Mouse {
    let layout = MouseReport::parse(bytes, packet).unwrap();
    assert!(layout.has_wheel());
    let mut mouse = Mouse::new();
    mouse.set_report_protocol(layout);
    mouse
}

fn button(code: usize, down: bool) -> Event {
    Event {
        kind: 2,
        code,
        x: down as i16,
        y: 0,
    }
}
fn movement(x: i16, y: i16) -> Event {
    Event {
        kind: 3,
        code: 0,
        x,
        y,
    }
}
fn wheel(x: i16) -> Event {
    Event {
        kind: 4,
        code: 0,
        x,
        y: 0,
    }
}

#[test]
fn byte_mouse_scrolls_both_directions_without_motion() {
    let mut mouse = mouse(&descriptor(0, 8, true), 4);
    let mut events = Vec::new();
    for delta in [1u8, 0xff, 0, 2] {
        mouse.report(&[0, 0, 0, delta], |e| events.push(e));
    }
    assert_eq!(events, [wheel(1), wheel(-1), wheel(2)]);
}

#[test]
fn numbered_wide_mouse_keeps_clicks_motion_and_scroll() {
    let mut mouse = mouse(&descriptor(7, 16, true), 8);
    let mut events = Vec::new();
    mouse.report(&[7, 0b101, 0xff, 0x7f, 1, 0x80, 0xfe], |e| events.push(e));
    mouse.report(&[7, 0b101, 0, 0, 0, 0, 0], |e| events.push(e));
    mouse.release(|e| events.push(e));
    mouse.release(|e| events.push(e));
    assert_eq!(
        events,
        [
            button(1, true),
            button(3, true),
            movement(32767, -32767),
            wheel(-2),
            button(1, false),
            button(3, false)
        ]
    );
}

#[test]
fn bit_packed_signed_axes_do_not_assume_byte_offsets() {
    let mut mouse = mouse(&descriptor(0, 12, false), 5);
    let mut packet = [0u8; 5];
    for (offset, width, value) in [(0, 5, 2u64), (5, 12, 0x801), (17, 12, 2047), (29, 8, 0x81)] {
        for bit in 0..width {
            packet[(offset + bit) / 8] |= (((value >> bit) & 1) as u8) << ((offset + bit) % 8);
        }
    }
    let mut events = Vec::new();
    mouse.report(&packet, |e| events.push(e));
    assert_eq!(
        events,
        [button(2, true), movement(-2047, 2047), wheel(-127)]
    );
}

#[test]
fn short_unknown_and_invalid_reports_preserve_held_buttons() {
    let mut mouse = mouse(&descriptor(3, 16, true), 8);
    mouse.report(&[3, 1, 0, 0, 0, 0, 0], |_| {});
    let mut events = Vec::new();
    for length in 0..7 {
        mouse.report(&[3, 0, 0, 0, 0, 0, 0][..length], |e| events.push(e));
    }
    mouse.report(&[4, 0, 0, 0, 0, 0, 0], |e| events.push(e));
    // -128 is outside this wheel's logical range; ignore the entire report.
    mouse.report(&[3, 0, 0, 0, 0, 0, 0x80], |e| events.push(e));
    assert!(events.is_empty());
    mouse.release(|e| events.push(e));
    assert_eq!(events, [button(1, false)]);
}

#[test]
fn separate_wheel_report_does_not_release_other_reports_buttons() {
    let mut desc = descriptor(1, 8, true);
    let at = desc.len() - WHEEL.len() - 2;
    desc.splice(at..at, [0x85, 2]);
    let mut mouse = mouse(&desc, 8);
    let mut events = Vec::new();
    mouse.report(&[1, 1, 4, 0], |e| events.push(e));
    mouse.report(&[2, 1], |e| events.push(e));
    mouse.report(&[2, 0xff], |e| events.push(e));
    mouse.release(|e| events.push(e));
    assert_eq!(
        events,
        [
            button(1, true),
            movement(4, 0),
            wheel(1),
            wheel(-1),
            button(1, false)
        ]
    );
}

#[test]
fn push_pop_and_feature_output_items_do_not_shift_input_fields() {
    let mut desc = descriptor(0, 8, true);
    let at = desc.len() - WHEEL.len() - 2;
    desc.splice(
        at..at,
        [
            0xa4, 0x06, 0x00, 0xff, 0x75, 32, 0x95, 20, 0x09, 1, 0xb1, 2, 0x09, 2, 0x91, 2, 0xb4,
        ],
    );
    let mut mouse = mouse(&desc, 4);
    let mut events = Vec::new();
    mouse.report(&[0, 0xfe, 3, 1], |e| events.push(e));
    assert_eq!(events, [movement(-2, 3), wheel(1)]);
}

#[test]
fn vendor_input_and_constant_padding_advance_offsets_without_events() {
    let mut desc = descriptor(0, 8, true);
    desc.splice(
        OPEN.len()..OPEN.len(),
        [
            0xa4, 0x06, 0, 0xff, 0x09, 1, 0x75, 8, 0x95, 1, 0x81, 2, 0xb4,
        ],
    );
    let mut mouse = mouse(&desc, 5);
    let mut events = Vec::new();
    mouse.report(&[0xff, 0, 0, 0, 1], |e| events.push(e));
    assert_eq!(events, [wheel(1)]);
}

#[test]
fn extended_usages_and_usage_ranges_are_resolved_by_page() {
    let mut desc = descriptor(0, 8, true);
    let at = desc.len() - WHEEL.len() - 2;
    desc.splice(at..at + 4, [0x06, 0, 0xff, 0x0b, 0x38, 0, 1, 0]);
    let mut mouse = mouse(&desc, 4);
    let mut events = Vec::new();
    mouse.report(&[7, 0, 0, 1], |e| events.push(e));
    assert_eq!(
        events,
        [button(1, true), button(2, true), button(3, true), wheel(1)]
    );
}

#[test]
fn malformed_and_unsupported_descriptors_are_rejected() {
    let desc = descriptor(1, 8, true);
    for end in 0..desc.len() {
        assert!(MouseReport::parse(&desc[..end], 8).is_err());
    }
    for suffix in [
        &[0xb4][..],
        &[0xc0],
        &[0xa4],
        &[0xfe, 0, 0],
        &[0xa9, 1],
        &[0x85, 0],
        &[0x86, 0, 1],
        &[0x19, 3, 0x29, 1],
        &[0x75],
    ] {
        let mut bad = desc.clone();
        bad.extend(suffix);
        assert!(MouseReport::parse(&bad, 8).is_err());
    }
    assert!(MouseReport::parse(&desc, 4).is_err()); // Report ID also occupies a byte.
    assert!(MouseReport::parse(&vec![0; 4097], 8).is_err());
    assert!(MouseReport::parse(&desc, 1025).is_err());
    // Same ID with an absolute, rather than relative, wheel must not be misread.
    let mut absolute = desc.clone();
    let flags = absolute.len() - 3;
    absolute[flags] = 2;
    assert!(MouseReport::parse(&absolute, 8).is_err());
    // Changing ID after unnumbered inputs is not a valid mixed report format.
    let mut mixed = descriptor(0, 8, true);
    mixed.splice(
        mixed.len() - WHEEL.len() - 2..mixed.len() - WHEEL.len() - 2,
        [0x85, 1],
    );
    assert!(MouseReport::parse(&mixed, 8).is_err());
}

#[test]
fn enormous_counts_and_sizes_fail_before_expanding_fields() {
    let mut desc = OPEN.to_vec();
    desc.extend([
        0x77, 0xff, 0xff, 0xff, 0xff, 0x97, 0xff, 0xff, 0xff, 0xff, 0x81, 2,
    ]);
    assert!(MouseReport::parse(&desc, 1024).is_err());
    let mut desc = OPEN.to_vec();
    desc.extend([0x75, 1, 0x97, 0xff, 0xff, 0xff, 0x7f, 0x81, 2]);
    assert!(MouseReport::parse(&desc, 1024).is_err());
}

#[test]
fn unrelated_application_cannot_supply_mouse_fields() {
    let mut desc = descriptor(0, 8, true);
    desc[3] = 5; // Gamepad, not Mouse.
    assert!(MouseReport::parse(&desc, 4).is_err());
}

#[test]
fn boot_fallback_never_guesses_a_fourth_byte_is_a_wheel() {
    let mut mouse = Mouse::new();
    let mut events = Vec::new();
    mouse.report(&[0, 0, 0, 1], |e| events.push(e));
    assert!(events.is_empty());
    mouse.report(&[1, 1, 0, 0xff], |e| events.push(e));
    assert_eq!(events, [button(1, true), movement(1, 0)]);
}

#[test]
fn configuration_report_lengths_are_per_interface_and_bounded() {
    let mut body = super::interface(0, 1);
    let mut second = super::interface(1, 2);
    second[16] = 52;
    body.extend(second);
    let mut found = Vec::new();
    super::descriptors::boot_interfaces(&super::config(&body), |i| found.push(i)).unwrap();
    assert_eq!(
        found.iter().map(|i| i.report_length).collect::<Vec<_>>(),
        [63, 52]
    );
    for (offset, value) in [(14, 0), (14, 2), (16, 0)] {
        let mut bad = super::interface(0, 2);
        bad[offset] = value;
        assert!(super::descriptors::boot_interfaces(&super::config(&bad), |_| {}).is_err());
    }
}

#[test]
fn mutated_descriptors_and_packets_never_read_out_of_bounds() {
    for (id, width, padding) in [(0, 8, true), (7, 16, true), (2, 12, false)] {
        let valid = descriptor(id, width, padding);
        for at in 0..valid.len() {
            for value in [0, 1, 2, 0x7f, 0x80, 0xfe, 0xff] {
                let mut mutated = valid.clone();
                mutated[at] = value;
                let Ok(layout) = MouseReport::parse(&mutated, 64) else {
                    continue;
                };
                let mut mouse = Mouse::new();
                mouse.set_report_protocol(layout);
                for len in 0..=64 {
                    let mut packet = vec![value; len];
                    if id != 0 && len != 0 {
                        packet[0] = id;
                    }
                    mouse.report(&packet, |event| assert!((2..=4).contains(&event.kind)));
                }
                mouse.release(|event| assert_eq!((event.kind, event.x), (2, 0)));
            }
        }
    }
}
