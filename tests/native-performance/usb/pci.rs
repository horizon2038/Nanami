#![allow(dead_code)]
extern crate alloc;
extern crate self as nanami_client;
extern crate self as nanami_runtime;
extern crate self as nanami_uapi;

use std::cell::RefCell;
pub type Word = usize;
const SLOT_PCI_SERVICE: Word = 16;
pub const OS_RESPONSE_OK: Word = 0;
pub const OS_RESPONSE_INVALID_ARGUMENT: Word = 1;
pub const OS_RESPONSE_INVALID_DESCRIPTOR: Word = 3;
#[derive(Debug, PartialEq)]
pub enum RequestError {
    InvalidArgument,
    Unsupported,
    Protocol,
    Transport,
    Status(Word),
}
pub mod device {
    #[derive(Debug)]
    pub struct UsbControllerResource {
        pub physical: usize,
        pub bytes: usize,
        pub irq: Option<usize>,
    }
}
#[macro_export]
macro_rules! println { ($($arg:tt)*) => { { let _ = format_args!($($arg)*); } }; }
pub mod ipc {
    pub fn process_slot_descriptor(slot: usize) -> usize {
        slot
    }
}
pub fn connect_service_by_name(_: &str, _: Word) -> Result<(), RequestError> {
    Ok(())
}
pub fn yield_now() {}
pub mod registry {
    pub fn connect_boot_service(_: &str, _: usize) -> Result<(), crate::RequestError> {
        Ok(())
    }
}
#[path = "../../../nanami/servers/libs/nanami-client/src/pci.rs"]
mod pci_client;
#[path = "../../../nanami/servers/uapi/src/pci.rs"]
mod pci_uapi;
pub mod pci {
    pub use crate::pci_client::{disable_message_interrupts, probe_bar, read, write};
    pub use crate::pci_uapi::*;
}
#[path = "../../../nanami/servers/core-services/pci-server/src/config.rs"]
mod config;

struct Backend;
impl config::Access for Backend {
    fn read(
        &mut self,
        address: pci::Address,
        offset: Word,
        width: Word,
    ) -> Result<u32, RequestError> {
        io::io_write(
            16,
            0xcf8,
            4,
            0x80000000 | ((address.device as Word) << 11) | (offset & !3),
        )?;
        let value = io::io_read(16, 0xcfc, 4)? as u32;
        let mask = u32::MAX >> ((4 - width) * 8);
        Ok((value >> ((offset & 3) * 8)) & mask)
    }
    fn write(
        &mut self,
        address: pci::Address,
        offset: Word,
        width: Word,
        value: u32,
    ) -> Result<(), RequestError> {
        io::io_write(
            16,
            0xcf8,
            4,
            0x80000000 | ((address.device as Word) << 11) | (offset & !3),
        )?;
        io::io_write(16, 0xcfc + (offset & 3), width, value as Word)
    }
}
pub fn call_port(
    _: Word,
    code: Word,
    a: Word,
    b: Word,
    c: Word,
    _: Word,
    words: u8,
) -> Result<(Word, Word, Word), RequestError> {
    use config::Access;
    let address = pci::Address::from_packed(a).ok_or(RequestError::InvalidArgument)?;
    let mut backend = Backend;
    match code {
        pci::PCI_REQUEST_READ => {
            assert_eq!(words, 4);
            Ok((0, backend.read(address, b, c)? as Word, 0))
        }
        pci::PCI_REQUEST_WRITE => {
            assert_eq!(words, 4);
            backend.write(address, b & 0xffff, b >> 16, c as u32)?;
            Ok((0, 0, 0))
        }
        pci::PCI_REQUEST_PROBE_BAR => {
            assert_eq!(words, 3);
            let device = pci::Device {
                address,
                vendor_id: 0,
                device_id: 0,
                class: 0x0c0330,
                header: 0,
            };
            let bar = config::probe_bar(&mut backend, device, b)?;
            Ok((
                0,
                bar.physical,
                bar.bytes | if bar.io { pci::PCI_BAR_IO } else { 0 },
            ))
        }
        _ => Err(RequestError::Unsupported),
    }
}

struct FakePci {
    config: [u32; 64],
    selected: usize,
    masks: [u32; 2],
    writes: Vec<(usize, usize, u32)>,
    fail_probe: bool,
}
impl Default for FakePci {
    fn default() -> Self {
        let mut config = [0; 64];
        config[1] = 0x10_0406; // Capabilities list; memory/master and INTx-disable.
        config[2] = 0x0c033000;
        config[4] = 0xc1000004; // 64-bit BAR below 4 GiB.
        config[0x34 / 4] = 0x40;
        config[0x40 / 4] = 0x0081_4811; // MSI-X enabled, next = 0x48.
        config[0x40 / 4] |= 1 << 31;
        config[0x48 / 4] = 0x0001_0005; // MSI enabled, end of list.
        config[0x3c / 4] = 0x10b; // INTA, line 11.
        Self {
            config,
            selected: 0,
            masks: [0xffffc004, u32::MAX],
            writes: Vec::new(),
            fail_probe: false,
        }
    }
}
thread_local! { static PCI: RefCell<FakePci> = RefCell::new(FakePci::default()); }
pub mod io {
    use super::*;
    pub fn io_read(_: Word, address: Word, width: Word) -> Result<Word, RequestError> {
        assert_eq!((address, width), (0xcfc, 4));
        PCI.with(|state| {
            let mut pci = state.borrow_mut();
            let index = pci.selected / 4;
            if matches!(index, 4 | 5) && pci.config[index] == u32::MAX {
                if pci.fail_probe {
                    pci.fail_probe = false;
                    return Err(RequestError::Transport);
                }
                return Ok(pci.masks[index - 4] as Word);
            }
            Ok(pci.config[index] as Word)
        })
    }
    pub fn io_write(_: Word, address: Word, width: Word, value: Word) -> Result<(), RequestError> {
        PCI.with(|state| {
            let mut pci = state.borrow_mut();
            if address == 0xcf8 {
                assert_eq!(width, 4);
                assert_eq!(value & !0xff, 0x8000_3000); // BDF 00:06.0 only.
                pci.selected = value & 0xfc;
            } else {
                let offset = pci.selected + address - 0xcfc;
                pci.writes.push((offset, width, value as u32));
                if width == 4 {
                    pci.config[offset / 4] = value as u32;
                } else {
                    assert_eq!(width, 2);
                    let shift = (offset & 2) * 8;
                    pci.config[offset / 4] =
                        (pci.config[offset / 4] & !(0xffff << shift)) | ((value as u32) << shift);
                }
            }
            Ok(())
        })
    }
}

#[path = "../../../nanami/servers/core-services/driver-manager/src/arch/x86_64/usb.rs"]
mod usb;

#[test]
fn bar_probe_restores_address_and_disables_message_interrupts() {
    PCI.with(|pci| *pci.borrow_mut() = FakePci::default());
    let (resource, pin) = usb::prepare(0x600).unwrap();
    assert_eq!(pin, 1);
    assert_eq!(
        (resource.physical, resource.bytes, resource.irq),
        (0xc1000000, 0x4000, Some(11))
    );
    PCI.with(|pci| {
        let pci = pci.borrow();
        assert_eq!(pci.config[4], 0xc1000004);
        assert_eq!(pci.config[5], 0);
        assert_eq!(pci.config[1], 0x10_0006); // RW1C status untouched.
        assert_eq!(pci.config[0x40 / 4] & (1 << 31), 0);
        assert_ne!(pci.config[0x40 / 4] & (1 << 30), 0);
        assert_eq!(pci.config[0x48 / 4] & (1 << 16), 0);
        assert!(pci
            .writes
            .iter()
            .filter(|(offset, _, _)| *offset == 4)
            .all(|(_, width, _)| *width == 2));
    });
}

#[test]
fn high_mmio_preserves_the_firmware_assigned_64_bit_bar() {
    PCI.with(|pci| {
        *pci.borrow_mut() = FakePci::default();
        pci.borrow_mut().config[5] = 8;
    });
    let (resource, _) = usb::prepare(0x600).unwrap();
    assert_eq!((resource.physical, resource.bytes), (0x8c1000000, 0x4000));
    PCI.with(|pci| {
        let pci = pci.borrow();
        assert_eq!(pci.config[4], 0xc1000004);
        assert_eq!(pci.config[5], 8);
    });
}

#[test]
fn failed_probe_restores_firmware_bar_and_command() {
    PCI.with(|pci| {
        *pci.borrow_mut() = FakePci::default();
        pci.borrow_mut().fail_probe = true;
    });
    assert_eq!(usb::prepare(0x600).unwrap_err(), RequestError::Transport);
    PCI.with(|pci| {
        let pci = pci.borrow();
        assert_eq!(pci.config[4], 0xc1000004);
        assert_eq!(pci.config[5], 0);
        assert_eq!(pci.config[1], 0x10_0406);
    });
}

#[test]
fn thirty_two_bit_bar_and_missing_intx_are_supported() {
    PCI.with(|pci| {
        *pci.borrow_mut() = FakePci::default();
        let mut pci = pci.borrow_mut();
        pci.config[4] &= !4;
        pci.config[0x3c / 4] = 0xff;
    });
    let (resource, pin) = usb::prepare(0x600).unwrap();
    assert_eq!(pin, 0);
    assert_eq!((resource.bytes, resource.irq), (0x4000, None));
}

#[test]
fn cyclic_pci_capability_list_is_rejected() {
    PCI.with(|pci| {
        *pci.borrow_mut() = FakePci::default();
        pci.borrow_mut().config[0x48 / 4] |= 0x4000;
    });
    assert_eq!(usb::prepare(0x600).unwrap_err(), RequestError::Protocol);
}

#[test]
fn firmware_ioapic_lines_are_not_limited_to_isa_irqs() {
    for line in 16..=23 {
        PCI.with(|pci| {
            *pci.borrow_mut() = FakePci::default();
            pci.borrow_mut().config[0x3c / 4] = 0x100 | line;
        });
        let (resource, pin) = usb::prepare(0x600).unwrap();
        assert_eq!(pin, 1);
        assert_eq!(resource.irq, Some(line as usize));
    }
}

#[test]
fn missing_line_preserves_pin_for_acpi_routing() {
    for line in [0, 24, 255] {
        PCI.with(|pci| {
            *pci.borrow_mut() = FakePci::default();
            pci.borrow_mut().config[0x3c / 4] = 0x100 | line;
        });
        let (resource, pin) = usb::prepare(0x600).unwrap();
        assert_eq!((resource.irq, pin), (None, 1));
    }
}

#[test]
fn invalid_interrupt_pin_never_enables_irq() {
    for pin in [0, 5, 255] {
        PCI.with(|pci| {
            *pci.borrow_mut() = FakePci::default();
            pci.borrow_mut().config[0x3c / 4] = pin << 8 | 11;
        });
        assert_eq!(usb::prepare(0x600).unwrap().0.irq, None);
    }
}
