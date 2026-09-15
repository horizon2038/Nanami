use super::{capabilities::*, protocol::*};

fn parse_speeds(major: u8, minor: u8, entries: &[u32]) -> Speeds {
    Speeds::parse(major, minor, entries, |_, reason| panic!("{reason}")).unwrap()
}

fn usb3(id: u32, mantissa: u32) -> u32 {
    (mantissa << 16) | (2 << 4) | (1 << 8) | id
}

#[test]
fn ssic_rates_below_five_gigabits_are_valid_usb3() {
    // xHCI 1.2b Table 7-12, including Series B rounded mantissas.
    let entries: Vec<_> = [1248, 2496, 4992, 1458, 2915, 5830]
        .into_iter()
        .enumerate()
        .map(|(i, rate)| usb3(i as u32 + 1, rate))
        .collect();
    assert_eq!(
        parse_speeds(3, 0, &entries),
        Speeds {
            usb2: 0,
            usb3: 0x7e
        }
    );
}

#[test]
fn defaults_follow_protocol_revision_and_explicit_entries_replace_them() {
    assert_eq!(parse_speeds(2, 0, &[]).usb2, DEFAULT_SPEEDS);
    for (minor, mask) in [(0, 0x10), (0x10, 0x30), (0x20, 0xf0)] {
        assert_eq!(parse_speeds(3, minor, &[]).usb3, mask);
        assert_eq!(parse_speeds(3, minor, &[usb3(9, 5000)]).usb3, 1 << 9);
    }
    for (major, minor) in [(2, 0x10), (3, 0x30), (4, 0)] {
        assert_eq!(parse_speeds(major, minor, &[]), Speeds::default());
    }
}

#[test]
fn unsupported_usb2_speed_does_not_discard_supported_siblings() {
    let entries = [(12 << 16) | (2 << 4) | 7, (42 << 16) | (2 << 4) | 8];
    let mut skipped = Vec::new();
    let speeds = Speeds::parse(2, 0, &entries, |i, reason| skipped.push((i, reason))).unwrap();
    assert_eq!(classify(speeds.usb2, 7), 1);
    assert_eq!(classify(speeds.usb2, 8), 0);
    assert_eq!(skipped, [(1, "unsupported USB2 bit rate")]);
}

#[test]
fn unsupported_link_modes_are_not_malformed_capabilities() {
    let entries = [
        usb3(4, 5000),
        usb3(5, 5000) & !(1 << 8),  // half-duplex
        usb3(6, 5000) | (2 << 14),  // unknown link protocol
        usb3(7, 5000) | (1 << 6),   // unknown PSI type
        usb3(8, 10000) | (1 << 14), // SuperSpeedPlus
    ];
    let mut skipped = Vec::new();
    let speeds = Speeds::parse(3, 0x10, &entries, |i, _| skipped.push(i)).unwrap();
    assert_eq!(speeds.usb3, (1 << 4) | (1 << 8));
    assert_eq!(skipped, [1, 2, 3]);
    let mut skipped = Vec::new();
    assert_eq!(
        Speeds::parse(2, 0, &[entries[0]], |i, _| skipped.push(i)).unwrap(),
        Speeds::default()
    );
    assert_eq!(skipped, [0]);
}

#[test]
fn duplicate_unknown_or_known_speed_ids_are_rejected() {
    for (major, psi) in [
        (2, (42 << 16) | 8),
        (2, (12 << 16) | (2 << 4) | 7),
        (3, usb3(4, 5000)),
    ] {
        let error = Speeds::parse(major, 0, &[psi, psi], |_, _| {}).unwrap_err();
        assert_eq!((error.index, error.reason), (1, "duplicate PSIV"));
    }
}

#[test]
fn zero_id_and_zero_mantissa_are_rejected() {
    for (entry, reason) in [
        (usb3(0, 5000), "reserved PSIV zero"),
        (usb3(4, 0), "zero speed mantissa"),
    ] {
        let error = Speeds::parse(3, 0, &[entry], |_, _| {}).unwrap_err();
        assert_eq!((error.index, error.reason), (0, reason));
    }
}

#[test]
fn asymmetric_pairs_must_be_adjacent_and_consistent() {
    let rx = usb3(5, 10000) | (2 << 6) | (1 << 14);
    let tx = usb3(5, 5000) | (3 << 6) | (1 << 14);
    assert_eq!(parse_speeds(3, 0x10, &[rx, tx]).usb3, 1 << 5);
    for (entries, index, reason) in [
        (vec![rx], 0, "asymmetric Rx without Tx"),
        (vec![tx], 0, "asymmetric Tx without Rx"),
        (
            vec![rx, usb3(4, 5000), tx],
            1,
            "asymmetric Rx/Tx pairing mismatch",
        ),
        (vec![rx, tx ^ 1], 1, "asymmetric Rx/Tx pairing mismatch"),
        (
            vec![rx, tx ^ (1 << 14)],
            1,
            "asymmetric link attributes differ",
        ),
        (vec![rx, tx & 0xffff], 1, "zero speed mantissa"),
        (vec![rx, tx, rx, tx], 2, "duplicate PSIV"),
    ] {
        let error = Speeds::parse(3, 0x10, &entries, |_, _| {}).unwrap_err();
        assert_eq!((error.index, error.reason), (index, reason));
    }
}

#[test]
fn unsupported_asymmetric_pair_skips_both_directions_only() {
    let entries = [
        usb3(5, 1000) | (2 << 6) | (2 << 14),
        usb3(5, 2000) | (3 << 6) | (2 << 14),
        usb3(7, 5000),
    ];
    let mut skipped = Vec::new();
    let speeds = Speeds::parse(3, 0, &entries, |i, _| skipped.push(i)).unwrap();
    assert_eq!(speeds.usb3, 1 << 7);
    assert_eq!(skipped, [0, 1]);
}

#[test]
fn unknown_capability_needs_only_its_four_byte_header() {
    let mut caps = Capabilities::new(1 << 16, 8);
    let cap = caps
        .next(&mut |offset| {
            assert_eq!(offset, 4);
            0xfe
        })
        .unwrap()
        .unwrap();
    cap.require(4).unwrap();
    assert!(cap.require(8).is_err());
    assert!(caps
        .next(&mut |_| panic!("end of chain"))
        .unwrap()
        .is_none());
}

#[test]
fn legacy_capability_needs_eight_not_sixteen_bytes() {
    let mut caps = Capabilities::new(1 << 16, 12);
    let cap = caps.next(&mut |_| 1).unwrap().unwrap();
    cap.require(8).unwrap();
    assert!(cap.require(16).is_err());
}

#[test]
fn absent_or_out_of_bar_header_is_never_read() {
    assert!(Capabilities::new(0, 0)
        .next(&mut |_| panic!())
        .unwrap()
        .is_none());
    for bytes in 0..8 {
        let error = Capabilities::new(1 << 16, bytes)
            .next(&mut |_| panic!())
            .unwrap_err();
        assert_eq!(error.offset, 4);
        assert_eq!(error.value, None);
    }
}

#[test]
fn invalid_next_pointer_reports_its_origin_header() {
    let error = Capabilities::new(1 << 16, 12)
        .next(&mut |_| 0xff01)
        .unwrap_err();
    assert_eq!(error.offset, 4);
    assert_eq!(error.value, Some(0xff01));
    assert_eq!(error.reason, "next extended capability outside BAR");
}

#[test]
fn forward_chain_is_bar_bounded_not_limited_to_256_entries() {
    let mut caps = Capabilities::new(1 << 16, 4 + 300 * 4);
    for index in 0..300 {
        let cap = caps
            .next(&mut |offset| {
                assert_eq!(offset, 4 + index * 4);
                if index == 299 {
                    0xff
                } else {
                    0x1ff
                }
            })
            .unwrap()
            .unwrap();
        assert_eq!(cap.offset, 4 + index * 4);
        cap.require(4).unwrap();
    }
    assert!(caps.next(&mut |_| panic!()).unwrap().is_none());
}

fn protocol_cap(header: u32, bytes: usize) -> Capability {
    Capabilities::new(1 << 16, bytes)
        .next(&mut |_| header)
        .unwrap()
        .unwrap()
}

#[test]
fn truncated_protocol_body_is_not_read() {
    for bytes in 8..20 {
        let cap = protocol_cap(0x0200_0002, bytes);
        assert!(cap.protocol(&mut |_| panic!(), &mut [false; 9]).is_err());
    }
    // Next points into the fixed protocol header.
    let cap = protocol_cap(0x0200_0302, 64);
    assert!(cap.protocol(&mut |_| panic!(), &mut [false; 9]).is_err());
}

#[test]
fn psi_cannot_overlap_next_capability_or_bar_end() {
    for (header, bytes) in [(0x0300_0402, 64), (0x0300_0002, 20)] {
        let cap = protocol_cap(header, bytes);
        let mut read = |offset| match offset {
            8 => 0x2042_5355,
            12 => (1 << 28) | (4 << 8) | 1,
            _ => panic!("read past validated body: {offset:#x}"),
        };
        assert!(cap.protocol(&mut read, &mut [false; 9]).is_err());
    }
}

fn read_protocol(
    cap: &Capability,
    compatible: u32,
    claimed: &mut [bool],
) -> Result<SupportedProtocol, super::capabilities::Error> {
    cap.protocol(
        &mut |offset| match offset - cap.offset {
            4 => 0x2042_5355,
            8 => compatible,
            12 => 3,
            16 => usb3(9, 4992),
            _ => panic!("unexpected read: {offset:#x}"),
        },
        claimed,
    )
}

#[test]
fn explicit_protocol_fields_and_speed_ids_are_preserved() {
    let cap = protocol_cap(0x0310_0002, 24);
    let mut claimed = [false; 9];
    let protocol = read_protocol(&cap, (1 << 28) | (4 << 8) | 5, &mut claimed).unwrap();
    assert_eq!(
        (
            protocol.name,
            protocol.major,
            protocol.minor,
            protocol.slot_type
        ),
        (0x2042_5355, 3, 0x10, 3)
    );
    assert_eq!(protocol.ports, 5..9);
    assert_eq!(
        &claimed,
        &[false, false, false, false, false, true, true, true, true]
    );
    assert_eq!(protocol.psi_count, 1);
    assert_eq!(
        parse_speeds(
            protocol.major,
            protocol.minor,
            &protocol.psi[..protocol.psi_count]
        )
        .usb3,
        1 << 9
    );
}

#[test]
fn port_ranges_reject_zero_overrun_and_overlaps_but_allow_gaps() {
    let cap = protocol_cap(0x0200_0002, 24);
    let mut claimed = [false; 9];
    for range in [(1 << 8), (5 << 8) | 5] {
        let error = read_protocol(&cap, range, &mut claimed).err().unwrap();
        assert_eq!((error.offset, error.value), (12, Some(range)));
    }
    assert!(!claimed.iter().any(|&used| used));
    read_protocol(&cap, (2 << 8) | 1, &mut claimed).unwrap();
    read_protocol(&cap, (2 << 8) | 5, &mut claimed).unwrap();
    assert!(read_protocol(&cap, (3 << 8) | 3, &mut claimed).is_err());
    // A rejected overlapping claim must not partially claim ports 3 and 4.
    read_protocol(&cap, (2 << 8) | 3, &mut claimed).unwrap();
}

#[test]
fn unknown_protocol_also_reserves_its_ports() {
    let cap = protocol_cap(0x0400_0002, 24);
    let mut claimed = [false; 9];
    assert_eq!(
        read_protocol(&cap, (2 << 8) | 1, &mut claimed)
            .unwrap()
            .major,
        4
    );
    let usb2 = protocol_cap(0x0200_0002, 24);
    assert!(read_protocol(&usb2, (2 << 8) | 1, &mut claimed).is_err());
}

#[test]
fn empty_qemu_usb3_capability_claims_no_ports() {
    let cap = protocol_cap(0x0300_0002, 20);
    let mut claimed = [false; 9];
    for first in [0, 9, 255] {
        assert!(read_protocol(&cap, first, &mut claimed)
            .unwrap()
            .ports
            .is_empty());
    }
    assert!(!claimed.iter().any(|&used| used));
}
