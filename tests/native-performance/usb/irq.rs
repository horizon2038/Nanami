//! Driver Manager's asynchronous ACPI routing protocol, without hardware.
#![allow(dead_code)]
extern crate self as libnanami;
extern crate self as nanami_services;
use std::cell::RefCell;
pub use std::println;
pub type Word = usize;
pub const PROCESS_SLOT_NOTIFICATION: Word = 21;
pub const OS_RESPONSE_OK: Word = 0;
pub const OS_RESPONSE_PERMISSION_DENIED: Word = 1;
pub const OS_RESPONSE_INVALID_ARGUMENT: Word = 2;
#[derive(Debug, PartialEq)]
pub enum RequestError {
    InvalidArgument,
    Transport,
}
pub mod device {
    pub const DEVICE_MANAGER_REQUEST_USB_PCI_ROUTE: usize = 0xd006;
    pub const DEVICE_MANAGER_REQUEST_USB_IRQ_REPORT: usize = 0xd007;
    pub const ACPI_ROUTE_NOTIFICATION: usize = 1;
    #[derive(Clone, Copy)]
    pub struct UsbControllerResource {
        pub irq: Option<usize>,
    }
}
#[derive(Default)]
struct State {
    copies: usize,
    notifications: usize,
    fail_notify: bool,
}
thread_local! { static STATE: RefCell<State> = RefCell::default(); }
pub fn request_notification_port_copy(
    pid: Word,
    source: Word,
    target: Word,
    badge: Word,
) -> Result<(), RequestError> {
    assert_eq!((pid, source, target, badge), (8, 21, 22, 1));
    STATE.with_borrow_mut(|state| {
        state.copies += 1;
        assert_eq!(state.copies, 1);
    });
    Ok(())
}
pub mod ipc {
    use super::*;
    pub struct ServiceRequest {
        pub identifier: Word,
        pub code: Word,
        pub arg0: Word,
        pub arg1: Word,
    }
    pub fn process_slot_descriptor(slot: Word) -> Word {
        slot
    }
    pub fn notification_notify(port: Word) -> Result<(), RequestError> {
        assert_eq!(port, 22);
        STATE.with_borrow_mut(|state| {
            if std::mem::take(&mut state.fail_notify) {
                return Err(RequestError::Transport);
            }
            state.notifications += 1;
            Ok(())
        })
    }
}
#[path = "../../../nanami/servers/core-services/driver-manager/src/usb_irq.rs"]
mod usb_irq;
use device::*;
use usb_irq::Routes;

fn setup() -> (Routes, [Option<UsbControllerResource>; 4]) {
    STATE.with_borrow_mut(|state| *state = State::default());
    let mut routes = Routes::default();
    routes.add(0, 0x1400, 1, None);
    (
        routes,
        [Some(UsbControllerResource { irq: None }), None, None, None],
    )
}
fn request(pid: Word, code: Word, index: Word, irq: Word) -> ipc::ServiceRequest {
    ipc::ServiceRequest {
        identifier: pid,
        code,
        arg0: index,
        arg1: irq,
    }
}

#[test]
fn routing_starts_asynchronously_once_and_publishes_result() {
    let (mut routes, mut controllers) = setup();
    for _ in 0..4 {
        assert!(!routes.query(0, 8).unwrap());
    }
    STATE.with_borrow(|state| assert_eq!((state.copies, state.notifications), (1, 1)));
    assert_eq!(
        routes.handle(
            request(8, DEVICE_MANAGER_REQUEST_USB_PCI_ROUTE, 0, 0),
            8,
            &mut controllers
        ),
        (0, 0x1400, 1)
    );
    assert_eq!(
        routes
            .handle(
                request(8, DEVICE_MANAGER_REQUEST_USB_IRQ_REPORT, 0, 19),
                8,
                &mut controllers
            )
            .0,
        OS_RESPONSE_OK
    );
    assert!(routes.query(0, 8).unwrap());
    assert_eq!(controllers[0].unwrap().irq, Some(19));
    assert_eq!(
        routes
            .handle(
                request(8, DEVICE_MANAGER_REQUEST_USB_PCI_ROUTE, 0, 0),
                8,
                &mut controllers
            )
            .2,
        0
    );
    // A late or repeated report cannot change a driver's live IRQ.
    assert_eq!(
        routes
            .handle(
                request(8, DEVICE_MANAGER_REQUEST_USB_IRQ_REPORT, 0, 20),
                8,
                &mut controllers
            )
            .0,
        OS_RESPONSE_INVALID_ARGUMENT
    );
    assert_eq!(controllers[0].unwrap().irq, Some(19));
}

#[test]
fn only_selected_acpi_server_can_read_or_report_routes() {
    let (mut routes, mut controllers) = setup();
    for code in [
        DEVICE_MANAGER_REQUEST_USB_PCI_ROUTE,
        DEVICE_MANAGER_REQUEST_USB_IRQ_REPORT,
    ] {
        for (pid, acpi) in [(0, 8), (7, 8), (0, 0)] {
            assert_eq!(
                routes
                    .handle(request(pid, code, 0, 19), acpi, &mut controllers)
                    .0,
                OS_RESPONSE_PERMISSION_DENIED
            );
        }
    }
    assert!(controllers[0].unwrap().irq.is_none());
}

#[test]
fn unsupported_irq_and_out_of_range_controller_do_not_change_resources() {
    let (mut routes, mut controllers) = setup();
    for irq in [1, 15, 24, 255, usize::MAX] {
        assert_eq!(
            routes
                .handle(
                    request(8, DEVICE_MANAGER_REQUEST_USB_IRQ_REPORT, 0, irq),
                    8,
                    &mut controllers
                )
                .0,
            OS_RESPONSE_INVALID_ARGUMENT
        );
    }
    for index in [4, usize::MAX] {
        assert_eq!(routes.query(index, 8), Err(RequestError::InvalidArgument));
        assert_eq!(
            routes
                .handle(
                    request(8, DEVICE_MANAGER_REQUEST_USB_IRQ_REPORT, index, 19),
                    8,
                    &mut controllers
                )
                .0,
            OS_RESPONSE_INVALID_ARGUMENT
        );
    }
    assert!(controllers[0].unwrap().irq.is_none());
    assert!(!routes.query(0, 8).unwrap());
}

#[test]
fn no_route_result_finishes_resolution_without_enabling_irq() {
    let (mut routes, mut controllers) = setup();
    assert_eq!(
        routes
            .handle(
                request(8, DEVICE_MANAGER_REQUEST_USB_IRQ_REPORT, 0, 0),
                8,
                &mut controllers
            )
            .0,
        OS_RESPONSE_OK
    );
    assert!(routes.query(0, 8).unwrap());
    assert!(controllers[0].unwrap().irq.is_none());
}

#[test]
fn valid_firmware_line_or_missing_pin_or_acpi_does_not_wait() {
    let (mut routes, _) = setup();
    routes.add(1, 0x1500, 1, Some(11));
    routes.add(2, 0x1600, 0, None);
    routes.add(3, 0x1700, 5, None);
    for index in 1..4 {
        assert!(routes.query(index, 8).unwrap());
    }
    assert!(routes.query(0, 0).unwrap());
    STATE.with_borrow(|state| assert_eq!((state.copies, state.notifications), (0, 0)));
}

#[test]
fn notification_failure_can_retry_without_overwriting_capability() {
    let (mut routes, _) = setup();
    STATE.with_borrow_mut(|state| state.fail_notify = true);
    assert_eq!(routes.query(0, 8), Err(RequestError::Transport));
    assert!(!routes.query(0, 8).unwrap());
    STATE.with_borrow(|state| assert_eq!((state.copies, state.notifications), (1, 1)));
}
