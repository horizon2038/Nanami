//! Real AML control methods, not native callbacks: exercise Arg/Local references.
use super::{
    power::Power,
    tests::{setup, Mock, S5},
};
use acpi::{
    aml::object::{Object, ReferenceKind},
    AcpiError, Handler,
};
use std::str::FromStr;

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

#[test]
fn pts_can_use_store_as_a_logical_operand() {
    // If (LEqual (Store (Arg0, Local0), 5)) { Sleep (Local0) }
    let (power, host) = method(b"_PTS", b"\xa0\x0a\x93\x70\x68\x60\x0a\x05\x5b\x22\x60");
    power.prepare().unwrap();
    assert_eq!(host.nanos_since_boot(), 5_000_000);
    assert_eq!(host.read_io_u16(0x404), 1);
}

#[test]
fn pts_can_test_acquire_result_before_sleeping() {
    // Mutex (MUT0, 0); If (LEqual (Acquire (MUT0, 0xffff), Zero)) { Sleep (Arg0) }
    let (power, host) = method(
        b"_PTS",
        b"\x5b\x01MUT0\x00\xa0\x0e\x93\x5b\x23MUT0\xff\xff\x00\x5b\x22\x68",
    );
    power.prepare().unwrap();
    assert_eq!(host.nanos_since_boot(), 5_000_000);
    assert_eq!(host.read_io_u16(0x404), 1);
}

#[test]
fn acquire_timeout_is_a_nonzero_expression_not_an_interpreter_failure() {
    let (power, host) = method(
        b"_PTS",
        b"\x5b\x01MUT0\x00\xa0\x0e\x93\x5b\x23MUT0\x00\x00\x00\x5b\x22\x68",
    );
    host.set_acquire_timeout(true);
    power.prepare().unwrap();
    assert_eq!(host.nanos_since_boot(), 0);
    assert_eq!(host.read_io_u16(0x404), 1);
}

#[test]
fn invalid_uninitialized_comparison_reports_location_without_enabling_sleep() {
    // If (LEqual (Local0, Zero)) { Sleep (One) }: Local0 really is undefined.
    let (power, host) = method(b"_PTS", b"\xa0\x07\x93\x60\x00\x5b\x22\x01");
    assert!(matches!(power.prepare(), Err(AcpiError::Aml(_))));
    let (scope, offset) = power.last_error_location().unwrap();
    assert_eq!(scope.to_string(), "\\_PTS");
    assert_eq!(offset, 5);
    assert_eq!(host.nanos_since_boot(), 0);
    assert_eq!(host.read_io_u16(0x404), 1);
}

fn mid(source: Object, index: u64, length: u64, use_local: bool) -> Object {
    // Method (MID0, 3) { [Store (Arg0, Local0)]
    //   Return (Mid (Arg0/Local0, Arg1, Arg2, Local1)) }
    let mut body = Vec::new();
    if use_local {
        body.extend_from_slice(b"\x70\x68\x60");
    }
    body.extend_from_slice(&[0xa4, 0x9e]);
    body.push(if use_local { 0x60 } else { 0x68 });
    body.extend_from_slice(b"\x69\x6a");
    // Also exercise an aliased source/target: extracting must finish before
    // overwriting Local0, and Return must still receive the new value.
    body.push(if use_local { 0x60 } else { 0x61 });
    let mut aml = S5.to_vec();
    aml.extend_from_slice(&[0x14, (7 + body.len()) as u8, b'\\']);
    aml.extend_from_slice(b"MID0\x03");
    aml.extend_from_slice(&body);
    let (_, _, interpreter) = super::tests::setup_interpreter(&aml, false);
    let result = interpreter
        .evaluate(
            acpi::aml::namespace::AmlName::from_str("\\MID0").unwrap(),
            vec![
                source.wrap(),
                Object::Integer(index).wrap(),
                Object::Integer(length).wrap(),
            ],
        )
        .unwrap()
        .unwrap_transparent_reference();
    (*result).clone()
}

#[test]
fn mid_resolves_arg_and_local_strings_and_clamps_ranges_without_overflow() {
    for local in [false, true] {
        for (index, length, expected) in [
            (0, 0, ""),
            (1, 2, "BC"),
            (4, 100, "EF"),
            (1, u64::MAX, "BCDEF"),
            (6, 1, ""),
            (u64::MAX, u64::MAX, ""),
        ] {
            let Object::String(result) = mid(Object::String("ABCDEF".into()), index, length, local)
            else {
                panic!("Mid(String) must preserve the source type");
            };
            assert_eq!(result, expected);
        }
    }
}

#[test]
fn mid_resolves_buffer_references_and_preserves_embedded_zero_bytes() {
    for local in [false, true] {
        for (index, length, expected) in [
            (1, 2, vec![0, 3]),
            (2, 100, vec![3, 4]),
            (1, u64::MAX, vec![0, 3, 4]),
            (4, 1, vec![]),
            (u64::MAX, 1, vec![]),
            (0, 0, vec![]),
        ] {
            let Object::Buffer(result) =
                mid(Object::Buffer(vec![1, 0, 3, 4]), index, length, local)
            else {
                panic!("Mid(Buffer) must preserve the source type");
            };
            assert_eq!(result, expected);
        }
    }
}

#[test]
fn mid_preserves_empty_string_and_buffer_types() {
    assert!(
        matches!(mid(Object::String(String::new()), 0, u64::MAX, true), Object::String(value) if value.is_empty())
    );
    assert!(
        matches!(mid(Object::Buffer(vec![]), 0, u64::MAX, true), Object::Buffer(value) if value.is_empty())
    );
}

#[test]
fn pts_can_branch_on_a_mid_string_result_and_continue_to_s5() {
    // Store ("ABCDE", Local0); If (LEqual (Mid (Local0, 2, 99, Local1), "CDE"))
    // { Sleep (Arg0) }. This exercises the actual power path, not a native hook.
    let (power, host) = method(
        b"_PTS",
        b"\x70\x0dABCDE\x00\x60\xa0\x11\x93\x9e\x60\x0a\x02\x0a\x63\x61\x0dCDE\x00\x5b\x22\x68",
    );
    power.prepare().unwrap();
    assert_eq!(host.nanos_since_boot(), 5_000_000);
    assert!(matches!(power.enter_s5(), Err(AcpiError::Timeout)));
    assert_eq!(host.read_io_u16(0x404), 1 | (5 << 10) | (1 << 13));
}

fn compare(opcode: &[u8], left: Object, right: Object) -> bool {
    let mut aml = S5.to_vec();
    // Method (CMP0, 2) { Return (<comparison> (Arg0, Arg1)) }
    aml.extend_from_slice(&[0x14, (10 + opcode.len()) as u8, b'\\']);
    aml.extend_from_slice(b"CMP0\x02\xa4");
    aml.extend_from_slice(opcode);
    aml.extend_from_slice(b"\x68\x69");
    let (_, _, interpreter) = super::tests::setup_interpreter(&aml, false);
    interpreter
        .evaluate(
            acpi::aml::namespace::AmlName::from_str("\\CMP0").unwrap(),
            vec![left.wrap(), right.wrap()],
        )
        .unwrap()
        .as_integer()
        .unwrap()
        != 0
}

#[test]
fn string_comparison_uses_all_bytes_lengths_and_lexicographic_order() {
    let string = |value: &str| Object::String(value.into());
    assert!(compare(b"\x93", string("ABCDE"), string("ABCDE")));
    assert!(!compare(b"\x93", string("ABCDE"), string("ABCDF")));
    assert!(!compare(b"\x93", string("ABCD"), string("ABCDE")));
    assert!(compare(b"\x94", string("BA"), string("AZ")));
    assert!(compare(b"\x95", string("ABCD"), string("ABCDE")));
    assert!(compare(b"\x92\x93", string("ABCDE"), string("ABCDF")));
}

#[test]
fn buffer_comparison_handles_long_buffers_and_unsigned_lexicographic_order() {
    let buffer = |value: &[u8]| Object::Buffer(value.into());
    assert!(compare(
        b"\x93",
        buffer(&[1, 2, 3, 4, 5]),
        buffer(&[1, 2, 3, 4, 5])
    ));
    assert!(!compare(
        b"\x93",
        buffer(&[1, 2, 3, 4, 5]),
        buffer(&[1, 2, 3, 4, 6])
    ));
    assert!(compare(b"\x94", buffer(&[0xff, 0]), buffer(&[0x7f, 0xff])));
    assert!(compare(b"\x95", buffer(&[]), buffer(&[0])));
    assert!(!compare(b"\x93", buffer(&[]), buffer(&[0])));
}
