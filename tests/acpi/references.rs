//! Real AML control methods, not native callbacks: exercise Arg/Local references.
use super::{
    power::Power,
    tests::{setup, Mock, S5},
};
use acpi::{
    aml::object::{Object, ReferenceKind},
    AcpiError, Handler,
};

fn method(name: &[u8; 4], body: &[u8]) -> (Power<Mock>, Mock) {
    // Method (absolute NameSeg, one argument). PkgLength includes itself.
    let length = 7 + body.len();
    assert!(length < 64);
    let mut aml = S5.to_vec();
    aml.extend_from_slice(&[0x14, length as u8, b'\\']);
    aml.extend_from_slice(name);
    aml.push(1);
    aml.extend_from_slice(body);
    let (power, host) = setup(&aml, false);
    host.write_io_u16(0x404, 1); // SCI_EN: already in ACPI mode.
    (power, host)
}

#[test]
fn pts_can_test_its_integer_argument() {
    // If (Arg0) { Sleep (One) }
    let (power, host) = method(b"_PTS", b"\xa0\x05\x68\x5b\x22\x01");
    power.prepare().unwrap();
    assert_eq!(host.nanos_since_boot(), 1_000_000);
    assert_eq!(host.read_io_u16(0x404), 1); // prepare must not set SLP_EN.
}

#[test]
fn pts_can_sleep_for_its_integer_argument() {
    // Sleep (Arg0)
    let (power, host) = method(b"_PTS", b"\x5b\x22\x68");
    power.prepare().unwrap();
    assert_eq!(host.nanos_since_boot(), 5_000_000);
}

#[test]
fn pts_can_update_and_read_a_local_integer() {
    // Store (Arg0, Local0); Increment (Local0); Decrement (Local0); Sleep (Local0)
    let (power, host) = method(b"_PTS", b"\x70\x68\x60\x75\x60\x76\x60\x5b\x22\x60");
    power.prepare().unwrap();
    assert_eq!(host.nanos_since_boot(), 5_000_000);
}

#[test]
fn gts_can_stall_for_its_integer_argument_before_s5() {
    // Stall (Arg0)
    let (power, host) = method(b"_GTS", b"\x5b\x21\x68");
    power.prepare().unwrap();
    // Mock hardware never powers off, so successful PM1 writes end in Timeout.
    assert!(matches!(power.enter_s5(), Err(AcpiError::Timeout)));
    assert_eq!(host.nanos_since_boot(), 1_000_005_000);
    assert_eq!(host.read_io_u16(0x404), 1 | (5 << 10) | (1 << 13));
}

#[test]
fn invalid_pts_predicate_still_aborts_without_sleep_enable() {
    // Store ("bad", Local0); If (Local0) { Sleep (One) }
    let (power, host) = method(b"_PTS", b"\x70\x0dbad\x00\x60\xa0\x05\x60\x5b\x22\x01");
    assert!(matches!(power.prepare(), Err(AcpiError::Aml(_))));
    assert_eq!(host.nanos_since_boot(), 0);
    assert_eq!(host.read_io_u16(0x404), 1);
}

#[test]
fn integer_access_unwraps_only_transparent_references() {
    let reference = Object::Reference {
        kind: ReferenceKind::LocalOrArg,
        inner: Object::Reference {
            kind: ReferenceKind::LocalOrArg,
            inner: Object::Integer(5).wrap(),
        }
        .wrap(),
    };
    assert_eq!(reference.as_integer().unwrap(), 5);
    for kind in [ReferenceKind::RefOf, ReferenceKind::Unresolved] {
        assert!(Object::Reference {
            kind,
            inner: Object::Integer(5).wrap(),
        }
        .as_integer()
        .is_err());
    }
}
