//! Exercise the production PCI inventory/BAR logic against config-space devices.
#![allow(dead_code)]
extern crate alloc;
extern crate self as nanami_client;
extern crate self as nanami_runtime;
extern crate self as nanami_uapi;
use std::collections::BTreeMap;
pub type Word = usize;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    InvalidArgument,
    Unsupported,
    Protocol,
    Transport,
    Status(Word),
}
pub const OS_RESPONSE_INVALID_ARGUMENT: Word = 1;
pub const OS_RESPONSE_INVALID_DESCRIPTOR: Word = 3;
pub const OS_RESPONSE_OK: Word = 0;
thread_local! {
    static CONNECT: std::cell::RefCell<(Word, Word, Word)> = const { std::cell::RefCell::new((0, 0, 0)) };
}
pub fn connect_service_by_name(name: &str, slot: Word) -> Result<(), RequestError> {
    assert_eq!((name, slot), ("timer-service", 26));
    CONNECT.with(|state| {
        let mut state = state.borrow_mut();
        state.0 += 1;
        if state.0 <= state.2 {
            return Err(RequestError::Status(OS_RESPONSE_INVALID_ARGUMENT));
        }
        Ok(())
    })
}
pub fn yield_now() {
    CONNECT.with(|state| state.borrow_mut().1 += 1);
}
#[path = "../../nanami/servers/libs/nanami-client/src/pci.rs"]
mod client;
#[path = "../../nanami/servers/core-services/pci-server/src/config.rs"]
mod config;
#[path = "../../nanami/servers/core-services/pci-server/src/names.rs"]
mod names;
#[path = "../../nanami/servers/uapi/src/pci.rs"]
pub mod pci;
#[path = "../../nanami/servers/libs/nanami-client/src/registry/startup.rs"]
mod startup;
pub mod registry {
    pub use crate::startup::connect_boot_service;
}
pub mod ipc {
    pub fn process_slot_descriptor(slot: usize) -> usize {
        slot
    }
}
use config::Access;
use pci::{Address, Device};

thread_local! {
    static IDENTITY: std::cell::RefCell<Option<names::Names>> = const { std::cell::RefCell::new(None) };
    static BAD_REPLY: std::cell::Cell<Option<(Word, Word)>> = const { std::cell::Cell::new(None) };
}
pub fn call_port(
    _: Word,
    code: Word,
    index: Word,
    field: Word,
    offset: Word,
    _: Word,
    words: u8,
) -> Result<(Word, Word, Word), RequestError> {
    assert_eq!((code, words), (pci::PCI_REQUEST_NAME, 4));
    if let Some((length, chunk)) = BAD_REPLY.with(|reply| reply.get()) {
        return Ok((0, length, chunk));
    }
    if index != 0 {
        return Ok((OS_RESPONSE_INVALID_ARGUMENT, 0, 0));
    }
    IDENTITY.with(|identity| {
        let (length, chunk) = identity.borrow().as_ref().unwrap().chunk(field, offset)?;
        Ok((0, length, chunk))
    })
}

#[test]
fn published_id_database_identifies_intel_and_class_names_through_the_real_client() {
    let device = Device {
        address: Address::new(0, 2, 0),
        vendor_id: 0x8086,
        device_id: 0x3e92,
        class: 0x030000,
        header: 0,
    };
    IDENTITY.with(|identity| *identity.borrow_mut() = Some(names::Names::identify(device)));
    let mut buffer = [0; pci::PCI_NAME_BUFFER_BYTES];
    assert_eq!(
        client::name(16, 0, pci::PCI_NAME_VENDOR, &mut buffer).unwrap(),
        "Intel Corporation"
    );
    assert!(client::name(16, 0, pci::PCI_NAME_DEVICE, &mut buffer)
        .unwrap()
        .contains("UHD Graphics 630"));
    assert_eq!(
        client::name(16, 0, pci::PCI_NAME_SUBCLASS, &mut buffer).unwrap(),
        "VGA compatible controller"
    );
    assert!(client::name(16, 0, pci::PCI_NAME_VENDOR, &mut [0; 4]).is_err());
    assert_eq!(
        client::name(16, 1, pci::PCI_NAME_VENDOR, &mut buffer),
        Err(RequestError::Status(1))
    );
    assert_eq!(
        client::name(16, 0, 5, &mut buffer),
        Err(RequestError::InvalidArgument)
    );
}

#[test]
fn unknown_ids_remain_enumerable_and_have_empty_names() {
    let device = Device {
        address: Address::new(0, 2, 0),
        vendor_id: 0xeeee,
        device_id: 0xffff,
        class: 0xeefffe,
        header: 0,
    };
    let names = names::Names::identify(device);
    for field in 0..=pci::PCI_NAME_INTERFACE {
        assert_eq!(names.chunk(field, 0), Ok((0, 0)));
    }
    assert_eq!(names.chunk(0, 1), Err(RequestError::InvalidArgument));
    assert_eq!(names.get(5), Err(RequestError::InvalidArgument));
}

#[test]
fn ahci_programming_interface_is_identified_and_bad_name_packets_are_rejected() {
    let device = Device {
        address: Address::new(0, 3, 0),
        vendor_id: 0x8086,
        device_id: 0x2922,
        class: 0x010601,
        header: 0,
    };
    let names = names::Names::identify(device);
    assert_eq!(
        names.get(pci::PCI_NAME_SUBCLASS).unwrap(),
        "SATA controller"
    );
    assert_eq!(names.get(pci::PCI_NAME_INTERFACE).unwrap(), "AHCI 1.0");
    let mut buffer = [0; 32];
    for packet in [(usize::MAX, 0), (1, 0xff)] {
        BAD_REPLY.with(|reply| reply.set(Some(packet)));
        assert_eq!(
            client::name(16, 0, 0, &mut buffer),
            Err(RequestError::Protocol)
        );
    }
    BAD_REPLY.with(|reply| reply.set(None));
}

#[test]
fn published_database_names_fit_the_protocol_buffer() {
    fn fits(name: &str) {
        assert!(
            name.len() <= pci::PCI_NAME_BUFFER_BYTES,
            "name too long: {name}"
        );
    }
    for vendor in pci_ids_rs::Vendors::iter() {
        fits(vendor.name());
        for device in vendor.devices() {
            fits(device.name());
        }
    }
    for class in pci_ids_rs::Classes::iter() {
        fits(class.name());
        for subclass in class.subclasses() {
            fits(subclass.name());
            for interface in subclass.prog_ifs() {
                fits(interface.name());
            }
        }
    }
}

#[test]
fn boot_connection_retries_registration_without_spinning_forever() {
    CONNECT.with(|state| *state.borrow_mut() = (0, 0, 3));
    startup::connect_boot_service("timer-service", 26).unwrap();
    CONNECT.with(|state| assert_eq!(*state.borrow(), (4, 3, 3)));
    CONNECT.with(|state| *state.borrow_mut() = (0, 0, usize::MAX));
    assert_eq!(
        startup::connect_boot_service("timer-service", 26),
        Err(RequestError::Transport)
    );
    CONNECT.with(|state| assert_eq!(*state.borrow(), (4096, 4096, usize::MAX)));
}

#[derive(Default)]
struct Bus {
    words: BTreeMap<(Word, Word), u32>,
    masks: BTreeMap<(Word, Word), u32>,
    writes: Vec<(Word, Word, Word, u32)>,
    fail_restore: Option<Word>,
    fail_mask: bool,
}
impl Access for Bus {
    fn read(&mut self, address: Address, offset: Word, width: Word) -> Result<u32, RequestError> {
        assert!(pci::valid_register(address, offset, width));
        let key = (address.packed(), offset & !3);
        let value = self.words.get(&key).copied().unwrap_or(u32::MAX);
        let value = if value == u32::MAX && self.masks.contains_key(&key) {
            if self.fail_mask {
                return Err(RequestError::Transport);
            }
            self.masks[&key]
        } else {
            value
        };
        Ok((value >> ((offset & 3) * 8)) & (u32::MAX >> ((4 - width) * 8)))
    }
    fn write(
        &mut self,
        address: Address,
        offset: Word,
        width: Word,
        value: u32,
    ) -> Result<(), RequestError> {
        self.writes.push((address.packed(), offset, width, value));
        if self.fail_restore == Some(offset) && value != u32::MAX {
            return Err(RequestError::Transport);
        }
        let word = self
            .words
            .entry((address.packed(), offset & !3))
            .or_default();
        if width == 4 {
            // Status bits are W1C; a DWORD command update would lose them.
            if offset == 4 {
                *word = (*word & 0xffff0000 & !value) | (value & 0xffff);
            } else {
                *word = value;
            }
        } else {
            assert_eq!(width, 2);
            let shift = (offset & 3) * 8;
            *word = (*word & !(0xffff << shift)) | ((value & 0xffff) << shift);
        }
        Ok(())
    }
}
impl Bus {
    fn device(&mut self, bus: u8, slot: u8, function: u8, header: u8, class: u32) -> Device {
        let address = Address::new(bus, slot, function);
        for offset in (0..256).step_by(4) {
            self.words.insert((address.packed(), offset), 0);
        }
        self.words.insert((address.packed(), 0), 0x12348086);
        self.words.insert((address.packed(), 8), class << 8);
        self.words
            .insert((address.packed(), 0xc), (header as u32) << 16);
        Device {
            address,
            vendor_id: 0x8086,
            device_id: 0x1234,
            header,
            class,
        }
    }
    fn wide_bar(&mut self) -> Device {
        let device = self.device(0, 2, 0, 0, 0x030000);
        let address = device.address.packed();
        self.words.insert((address, 4), 0xf9000007);
        self.words.insert((address, 0x10), 0xc100000c);
        self.words.insert((address, 0x14), 2);
        self.masks.insert((address, 0x10), 0xffff000c);
        self.masks.insert((address, 0x14), u32::MAX);
        device
    }
}

#[test]
fn inventory_follows_bridges_and_multifunction_without_revisiting_cycles() {
    let mut bus = Bus::default();
    let bridge = bus.device(0, 1, 0, 1, 0x060400);
    bus.words.insert((bridge.address.packed(), 0x18), 2 << 8);
    let multi = bus.device(2, 4, 0, 0x80, 0x020000);
    let gpu = bus.device(2, 4, 3, 0, 0x030000);
    let cycle = bus.device(2, 5, 0, 1, 0x060400);
    bus.words.insert((cycle.address.packed(), 0x18), 2 << 8);
    // Orphan nonzero functions are not discoverable without function zero.
    bus.device(0, 7, 1, 0, 0x020000);
    let devices = config::scan(&mut bus).unwrap();
    assert_eq!(devices, [bridge, multi, gpu, cycle]);
    assert!(bus.writes.is_empty());
}

#[test]
fn bar_probe_restores_both_halves_command_and_w1c_status() {
    let mut bus = Bus::default();
    let device = bus.wide_bar();
    let before = bus.words.clone();
    let bar = config::probe_bar(&mut bus, device, 0).unwrap();
    assert_eq!(bar.physical, 0x2c1000000);
    assert_eq!(bar.bytes, 0x10000);
    assert!(bar.prefetchable && !bar.io);
    assert_eq!(bus.words, before);
    assert!(bus
        .writes
        .iter()
        .filter(|(_, offset, _, _)| *offset == 4)
        .all(|(_, _, width, _)| *width == 2));
}

#[test]
fn probe_read_failure_still_restores_the_full_device_configuration() {
    let mut bus = Bus::default();
    let device = bus.wide_bar();
    let before = bus.words.clone();
    bus.fail_mask = true;
    assert_eq!(
        config::probe_bar(&mut bus, device, 0),
        Err(RequestError::Transport)
    );
    assert_eq!(bus.words, before);
}

#[test]
fn restore_failure_attempts_other_half_and_does_not_reenable_decoding() {
    let mut bus = Bus::default();
    let device = bus.wide_bar();
    bus.fail_restore = Some(0x10);
    assert_eq!(
        config::probe_bar(&mut bus, device, 0),
        Err(RequestError::Transport)
    );
    assert_eq!(bus.words[&(device.address.packed(), 0x14)], 2);
    assert_eq!(bus.words[&(device.address.packed(), 4)], 0xf9000000);
    assert_eq!(bus.writes.last().unwrap().1, 0x14);
}

#[test]
fn invalid_bar_indices_and_high_halves_cannot_mutate_the_device() {
    let mut bus = Bus::default();
    let device = bus.wide_bar();
    for index in [1, 6, usize::MAX] {
        assert_eq!(
            config::probe_bar(&mut bus, device, index),
            Err(RequestError::InvalidArgument)
        );
    }
    assert!(bus.writes.is_empty());
    let bridge = bus.device(0, 3, 0, 1, 0x060400);
    assert_eq!(
        config::probe_bar(&mut bus, bridge, 2),
        Err(RequestError::InvalidArgument)
    );
    assert!(bus.writes.is_empty());
}

#[test]
fn wire_format_validates_addresses_register_widths_and_device_metadata() {
    let address = Address {
        segment: 3,
        bus: 0xa1,
        device: 31,
        function: 7,
    };
    assert_eq!(Address::from_packed(address.packed()), Some(address));
    for invalid in [1 << 3, 1 << 13, 1 << 24, 1usize << 63] {
        assert_eq!(Address::from_packed(invalid), None);
    }
    assert!(pci::valid_register(address, 4092, 4));
    assert!(!pci::valid_register(address, 4095, 4));
    assert!(!pci::valid_register(address, 4096, 1));
    assert!(!pci::valid_register(address, 5, 2));
    assert!(!pci::valid_register(address, 0, 3));
    let device = Device {
        address,
        vendor_id: 0x8086,
        device_id: 0x3e92,
        class: 0x030000,
        header: 0x80,
    };
    assert_eq!(
        Device::from_words(address.packed(), device.packed_info()),
        Some(device)
    );
}
