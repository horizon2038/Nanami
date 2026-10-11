//! Exercise the same namespace and resource decoding used on real machines.
#[path = "../../nanami/servers/core-services/acpi-server/src/routing.rs"]
mod routing;

use super::tests::Mock;
use acpi::{
    aml::{
        namespace::{AmlName, NamespaceLevelKind},
        object::{Object, ReferenceKind, WrappedObject},
        Interpreter,
    },
    registers::FixedRegisters,
    sdt::fadt::Fadt,
};
use routing::{Error, Route, Routing};
use std::{cell::Cell, rc::Rc, str::FromStr, sync::Arc};

const ROOT: &str = "\\_SB.PC00"; // No hard-coded PCI0 assumption.
const BDF: usize = 0x1400; // The reported machine's 00:14.0.

fn name(path: &str) -> AmlName {
    AmlName::from_str(path).unwrap()
}
fn put(interpreter: &Interpreter<Mock>, path: &str, value: Object) {
    interpreter
        .namespace
        .lock()
        .insert(name(path), value.wrap())
        .unwrap();
}
fn device(interpreter: &Interpreter<Mock>, path: &str) -> WrappedObject {
    let object = Object::Device.wrap();
    let mut namespace = interpreter.namespace.lock();
    namespace
        .add_level(name(path), NamespaceLevelKind::Device)
        .unwrap();
    namespace.insert(name(path), object.clone()).unwrap();
    object
}
fn setup() -> Arc<Interpreter<Mock>> {
    let host = Mock::default();
    let mut fadt: Fadt = unsafe { std::mem::zeroed() };
    fadt.header.revision = 1;
    fadt.pm1a_control_block = 0x404;
    fadt.pm1_control_length = 2;
    fadt.pm1a_event_block = 0x400;
    fadt.pm1_event_length = 4;
    let registers = Arc::new(FixedRegisters::new(&fadt, host.clone()).unwrap());
    let interpreter = Arc::new(Interpreter::new(host, 2, registers, None));
    device(&interpreter, ROOT);
    put(
        &interpreter,
        &format!("{ROOT}._HID"),
        Object::Integer(0x080a_d041),
    );
    interpreter
}
fn entry(address: u64, pin: u64, source: Object, index: u64) -> WrappedObject {
    Object::Package(vec![
        Object::Integer(address).wrap(),
        Object::Integer(pin).wrap(),
        source.wrap(),
        Object::Integer(index).wrap(),
    ])
    .wrap()
}
fn prt(interpreter: &Interpreter<Mock>, entries: Vec<WrappedObject>) {
    put(
        interpreter,
        &format!("{ROOT}._PRT"),
        Object::Package(entries),
    );
}
fn direct(interpreter: &Interpreter<Mock>, gsi: u64) {
    prt(
        interpreter,
        vec![entry(0x14_ffff, 0, Object::Integer(0), gsi)],
    );
}
fn extended(gsi: u32, flags: u8) -> Vec<u8> {
    let mut bytes = vec![0x89, 6, 0, flags, 1];
    bytes.extend(gsi.to_le_bytes());
    bytes.extend([0x79, 0]);
    bytes
}
fn link(bytes: Vec<u8>, index: u64, reference: bool) -> Result<Route, Error> {
    let interpreter = setup();
    let object = device(&interpreter, "\\_SB.LNKX");
    put(&interpreter, "\\_SB.LNKX._CRS", Object::Buffer(bytes));
    let source = if reference {
        Object::Reference {
            kind: ReferenceKind::RefOf,
            inner: object,
        }
    } else {
        Object::String("LNKX".into())
    };
    prt(&interpreter, vec![entry(0x14_ffff, 0, source, index)]);
    Routing::new(interpreter)?.route(BDF, 1)
}

#[test]
fn pic_apic_selection_precedes_prt_and_s5_is_not_required() {
    let interpreter = setup();
    let pic = Rc::new(Cell::new(0));
    let observed = pic.clone();
    put(
        &interpreter,
        "\\_PIC",
        Object::native_method(1, move |args| {
            assert_eq!(args[0].as_integer()?, 1);
            observed.set(observed.get() + 1);
            Ok(Object::Integer(0).wrap())
        }),
    );
    let observed = pic.clone();
    put(
        &interpreter,
        &format!("{ROOT}._PRT"),
        Object::native_method(0, move |_| {
            assert_eq!(observed.get(), 1);
            Ok(Object::Package(vec![entry(0x14_ffff, 0, Object::Integer(0), 16)]).wrap())
        }),
    );
    assert_eq!(
        Routing::new(interpreter)
            .unwrap()
            .route(BDF, 1)
            .unwrap()
            .supported_irq()
            .unwrap(),
        16
    );
    assert_eq!(pic.get(), 1);
}

#[test]
fn direct_routes_match_device_function_and_zero_based_pin() {
    let interpreter = setup();
    prt(
        &interpreter,
        vec![
            entry(0x14_0001, 0, Object::Integer(0), 20),
            entry(0x14_0000, 1, Object::Integer(0), 21),
            entry(0x14_0000, 0, Object::Integer(0), 19),
        ],
    );
    let routing = Routing::new(interpreter).unwrap();
    assert_eq!(routing.route(BDF, 1).unwrap().gsi, 19);
    assert_eq!(routing.route(BDF | 1, 1).unwrap().gsi, 20);
    assert_eq!(routing.route(BDF, 2).unwrap().gsi, 21);
    assert!(matches!(routing.route(0x1500, 1), Err(Error::NoRoute)));
}

#[test]
fn package_cid_nonzero_root_bus_and_segment_selection() {
    let interpreter = setup();
    put(
        &interpreter,
        &format!("{ROOT}._HID"),
        Object::String("VENDOR00".into()),
    );
    put(
        &interpreter,
        &format!("{ROOT}._CID"),
        Object::Package(vec![Object::String("PNP0A03".into()).wrap()]),
    );
    put(&interpreter, &format!("{ROOT}._BBN"), Object::Integer(7));
    direct(&interpreter, 22);
    let routing = Routing::new(interpreter.clone()).unwrap();
    assert_eq!(routing.route(BDF | (7 << 16), 1).unwrap().gsi, 22);
    assert!(matches!(routing.route(BDF, 1), Err(Error::NoRoute)));
    put(&interpreter, &format!("{ROOT}._SEG"), Object::Integer(1));
    assert!(matches!(
        routing.route(BDF | (7 << 16), 1),
        Err(Error::NoRoute)
    ));
}

#[test]
fn non_root_bus_is_not_guessed_using_root_prt() {
    let interpreter = setup();
    direct(&interpreter, 16);
    assert!(matches!(
        Routing::new(interpreter).unwrap().route(BDF | (1 << 16), 1),
        Err(Error::NoRoute)
    ));
}

#[test]
fn malformed_or_ambiguous_prt_is_rejected() {
    for entries in [
        vec![Object::Integer(0).wrap()],
        vec![Object::Package(vec![]).wrap()],
        vec![entry(0x14_ffff, 4, Object::Integer(0), 16)],
        vec![entry(0x20_ffff, 0, Object::Integer(0), 16)],
        vec![entry(0x14_0008, 0, Object::Integer(0), 16)],
        vec![entry(0x14_ffff, 0, Object::Integer(0), u64::MAX)],
        vec![
            entry(0x14_ffff, 0, Object::Integer(0), 16),
            entry(0x14_0000, 0, Object::Integer(0), 17),
        ],
    ] {
        let interpreter = setup();
        prt(&interpreter, entries);
        assert!(matches!(
            Routing::new(interpreter).unwrap().route(BDF, 1),
            Err(Error::Malformed)
        ));
    }
}

#[test]
fn duplicate_root_and_invalid_pci_address_are_rejected() {
    let interpreter = setup();
    direct(&interpreter, 16);
    device(&interpreter, "\\_SB.PC01");
    put(
        &interpreter,
        "\\_SB.PC01._HID",
        Object::String("PNP0A03".into()),
    );
    let routing = Routing::new(interpreter).unwrap();
    assert!(matches!(routing.route(BDF, 1), Err(Error::Malformed)));
    for (bdf, pin) in [(BDF, 0), (BDF, 5), (BDF | 8, 1), (0x2000, 1)] {
        assert!(matches!(routing.route(bdf, pin), Err(Error::Malformed)));
    }
}

#[test]
fn relative_and_reference_link_devices_resolve_extended_irq() {
    for reference in [false, true] {
        let route = link(extended(19, 0x0d), 0, reference).unwrap();
        assert_eq!(
            route,
            Route {
                gsi: 19,
                level: true,
                low: true
            }
        );
        assert_eq!(route.supported_irq().unwrap(), 19);
    }
}

#[test]
fn source_index_selects_resource_descriptor_not_interrupt_array() {
    let mut bytes = vec![0x47, 1, 0, 0, 0, 0, 1, 1]; // I/O descriptor before IRQ.
    bytes.extend(extended(20, 0x0d));
    assert_eq!(link(bytes.clone(), 1, false).unwrap().gsi, 20);
    assert!(matches!(link(bytes, 0, false), Err(Error::Unsupported)));
}

#[test]
fn short_irq_flags_and_supported_hal_policy_are_not_conflated() {
    let route = link(vec![0x23, 0, 8, 0x18, 0x79, 0], 0, false).unwrap();
    assert_eq!(
        route,
        Route {
            gsi: 11,
            level: true,
            low: true
        }
    );
    assert!(matches!(route.supported_irq(), Err(Error::Unsupported)));
    let route = link(vec![0x22, 0, 8, 0x79, 0], 0, false).unwrap();
    assert_eq!(
        route,
        Route {
            gsi: 11,
            level: false,
            low: false
        }
    );
    for (irq, level, low) in [
        (15, true, true),
        (24, true, true),
        (19, false, true),
        (19, true, false),
    ] {
        assert!(matches!(
            Route {
                gsi: irq,
                level,
                low
            }
            .supported_irq(),
            Err(Error::Unsupported)
        ));
    }
}

#[test]
fn malformed_crs_never_panics_or_accepts_ambiguous_irqs() {
    for bytes in [
        vec![],
        vec![0x89],
        vec![0x89, 255, 255],
        vec![0x22, 1],
        vec![0x79],
        vec![0x23, 0, 0, 0x18],
        vec![0x23, 0, 12, 0x18],
        vec![0x89, 6, 0, 0x0d, 2, 19, 0, 0, 0],
        extended(19, 0x0c),
    ] {
        assert!(link(bytes, 0, false).is_err());
    }
    assert!(link(extended(19, 0x0d), 1, false).is_err());
    let valid = extended(19, 0x0d);
    for length in 0..9 {
        assert!(link(valid[..length].to_vec(), 0, false).is_err());
    }
}

#[test]
fn interrupt_from_a_named_resource_source_is_not_treated_as_a_gsi() {
    let mut bytes = extended(19, 0x0d);
    bytes.truncate(9);
    bytes.extend([0, b'G', b'P', b'I', b'O', 0]);
    bytes[1] = 12;
    bytes.extend([0x79, 0]);
    assert!(matches!(link(bytes, 0, false), Err(Error::Unsupported)));
}
