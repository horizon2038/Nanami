//! Compile actual HID protocol selection, substituting just EP0 transport/DMA.
use super::{descriptors::BootInterface, hid::Mouse, hid_reports::descriptor, RequestError};
use std::collections::VecDeque;

struct Input;
struct Dma(Box<[u8; 0x4000]>);
impl Dma {
    fn virtual_at(&self, offset: usize) -> usize {
        self.0.as_ptr() as usize + offset
    }
}
struct Endpoint {
    interface: BootInterface,
    mouse: Mouse,
}
struct Device {
    endpoints: Vec<Endpoint>,
    dma: Dma,
}

#[derive(Debug, PartialEq, Eq)]
struct Request(u8, u8, u16, u16, usize);
#[derive(Default)]
struct Controller {
    descriptor: Vec<u8>,
    results: VecDeque<Result<usize, RequestError>>,
    requests: Vec<Request>,
}
impl Controller {
    fn control(
        &mut self,
        _: u8,
        device: &mut Device,
        kind: u8,
        request: u8,
        value: u16,
        index: u16,
        len: usize,
        _: &mut Input,
    ) -> Result<usize, RequestError> {
        self.requests
            .push(Request(kind, request, value, index, len));
        let actual = self.results.pop_front().unwrap_or(Ok(len))?;
        if request == 6 {
            assert_eq!((kind, value), (0x81, 0x2200));
            assert!(actual <= len && actual <= self.descriptor.len());
            device.dma.0[0x3000..0x3000 + actual].copy_from_slice(&self.descriptor[..actual]);
        }
        Ok(actual)
    }
}
#[path = "../../../nanami/servers/core-services/usb-server/src/xhci/hid.rs"]
mod production;

fn setup() -> (Controller, Device) {
    let desc = descriptor(7, 16, true);
    let interface = BootInterface {
        number: 3,
        protocol: 2,
        endpoint: 0x81,
        packet_size: 8,
        interval: 1,
        report_length: desc.len() as u16,
    };
    (
        Controller {
            descriptor: desc,
            ..Controller::default()
        },
        Device {
            endpoints: vec![Endpoint {
                interface,
                mouse: Mouse::new(),
            }],
            dma: Dma(Box::new([0; 0x4000])),
        },
    )
}
fn get(length: usize) -> Request {
    Request(0x81, 6, 0x2200, 3, length)
}
fn protocol(value: u16) -> Request {
    Request(0x21, 11, value, 3, 0)
}
fn boot_still_works(device: &mut Device) {
    let mut events = Vec::new();
    device.endpoints[0]
        .mouse
        .report(&[0, 1, 0, 1], |e| events.push(e));
    assert_eq!(events.len(), 1);
    assert_eq!((events[0].kind, events[0].x), (3, 1));
}

#[test]
fn report_protocol_is_selected_only_after_descriptor_validation() {
    let (mut controller, mut device) = setup();
    controller
        .configure_hid(1, &mut device, &mut Input)
        .unwrap();
    assert_eq!(
        controller.requests,
        [get(controller.descriptor.len()), protocol(1)]
    );
    let mut events = Vec::new();
    device.endpoints[0]
        .mouse
        .report(&[7, 0, 0, 0, 0, 0, 1], |e| events.push(e));
    assert_eq!(events.len(), 1);
    assert_eq!((events[0].kind, events[0].x), (4, 1));
}

#[test]
fn missing_or_oversized_descriptor_keeps_boot_protocol() {
    for length in [0, 4097] {
        let (mut controller, mut device) = setup();
        device.endpoints[0].interface.report_length = length;
        controller
            .configure_hid(1, &mut device, &mut Input)
            .unwrap();
        assert_eq!(controller.requests, [protocol(0)]);
        boot_still_works(&mut device);
    }
}

#[test]
fn malformed_short_and_stalled_descriptor_reads_fall_back() {
    for case in 0..3 {
        let (mut controller, mut device) = setup();
        let length = controller.descriptor.len();
        match case {
            0 => controller.descriptor[0] = 0xfe,
            1 => controller.results.push_back(Ok(length - 1)),
            _ => controller.results.push_back(Err(RequestError::Unsupported)),
        }
        controller
            .configure_hid(1, &mut device, &mut Input)
            .unwrap();
        assert_eq!(controller.requests, [get(length), protocol(0)]);
        boot_still_works(&mut device);
    }
}

#[test]
fn stalled_protocol_selection_does_not_install_report_decoder() {
    let (mut controller, mut device) = setup();
    let length = controller.descriptor.len();
    controller
        .results
        .extend([Ok(length), Err(RequestError::Unsupported)]);
    controller
        .configure_hid(1, &mut device, &mut Input)
        .unwrap();
    assert_eq!(controller.requests, [get(length), protocol(1), protocol(0)]);
    boot_still_works(&mut device);
}

#[test]
fn transport_failures_never_attempt_another_control_transfer() {
    for select in [false, true] {
        let (mut controller, mut device) = setup();
        if select {
            controller
                .results
                .push_back(Ok(controller.descriptor.len()));
        }
        controller.results.push_back(Err(RequestError::Transport));
        assert_eq!(
            controller.configure_hid(1, &mut device, &mut Input),
            Err(RequestError::Transport)
        );
        assert_eq!(controller.requests.len(), if select { 2 } else { 1 });
    }
}

#[test]
fn keyboard_stays_in_boot_protocol_with_set_idle() {
    let (mut controller, mut device) = setup();
    device.endpoints[0].interface.protocol = 1;
    controller
        .configure_hid(1, &mut device, &mut Input)
        .unwrap();
    assert_eq!(
        controller.requests,
        [protocol(0), Request(0x21, 10, 0, 3, 0)]
    );
}

#[test]
fn keyboard_and_mouse_interfaces_are_configured_independently() {
    let (mut controller, mut device) = setup();
    let mut keyboard = device.endpoints[0].interface;
    keyboard.number = 0;
    keyboard.protocol = 1;
    device.endpoints.push(Endpoint {
        interface: keyboard,
        mouse: Mouse::new(),
    });
    controller
        .configure_hid(1, &mut device, &mut Input)
        .unwrap();
    assert_eq!(
        controller.requests,
        [
            get(controller.descriptor.len()),
            protocol(1),
            Request(0x21, 11, 0, 0, 0),
            Request(0x21, 10, 0, 0, 0)
        ]
    );
}
