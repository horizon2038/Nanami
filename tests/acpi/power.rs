//! Exercise the production AML/S5 sequence without any host hardware access.
#![allow(dead_code)]
extern crate alloc;
#[path = "../../nanami/servers/core-services/acpi-server/src/power.rs"]
mod power;

#[cfg(test)]
#[path = "routing.rs"]
mod routing_tests;

#[cfg(test)]
mod references;

#[cfg(test)]
mod host {
    pub use super::tests::Mock as Host;
}
#[cfg(test)]
#[path = "../../nanami/servers/core-services/acpi-server/src/firmware.rs"]
mod firmware;

#[cfg(test)]
mod tests {
    use super::power::*;
    use acpi::{
        aml::{object::Object, AmlError, Interpreter},
        registers::FixedRegisters,
        sdt::fadt::Fadt,
        Handle, Handler, PciAddress, PhysicalMapping,
    };
    use std::{cell::RefCell, collections::BTreeMap, rc::Rc, sync::Arc};

    #[derive(Default)]
    struct State {
        ports: BTreeMap<u16, u32>,
        writes: Vec<(u16, u32)>,
        ns: u64,
        enable: bool,
        memory: Vec<u64>,
    }
    #[derive(Clone, Default)]
    pub struct Mock(Rc<RefCell<State>>);
    impl Mock {
        pub fn set_pci(&self, _: acpi::platform::PciConfigRegions) {}

        fn write_port(&self, port: u16, value: u32, width: usize) {
            if port == 0x400 || port == 0x500 {
                assert_eq!(width, 16);
            }
            let mut state = self.0.borrow_mut();
            state.ports.insert(port, value);
            state.writes.push((port, value));
            if port == 0xb2 && value == 0xa1 && state.enable {
                state.ports.insert(0x404, 1);
            }
            if port == 0xb2 && value == 0xa0 {
                state.ports.insert(0x404, 0);
            }
        }
    }
    macro_rules! memory {
        ($read:ident, $write:ident, $ty:ty) => {
            fn $read(&self, _: usize) -> $ty {
                panic!("unexpected MMIO")
            }
            fn $write(&self, _: usize, _: $ty) {
                panic!("unexpected MMIO")
            }
        };
    }
    macro_rules! pci {
        ($read:ident, $write:ident, $ty:ty) => {
            fn $read(&self, _: PciAddress, _: u16) -> $ty {
                panic!("unexpected PCI")
            }
            fn $write(&self, _: PciAddress, _: u16, _: $ty) {
                panic!("unexpected PCI")
            }
        };
    }
    impl Handler for Mock {
        unsafe fn map_physical_region<T>(
            &self,
            address: usize,
            bytes: usize,
        ) -> PhysicalMapping<Self, T> {
            let state = self.0.borrow();
            let span = bytes.max(std::mem::size_of::<T>());
            assert!(address + span <= state.memory.len() * 8);
            let pointer = state
                .memory
                .as_ptr()
                .cast::<u8>()
                .add(address)
                .cast_mut()
                .cast::<T>();
            PhysicalMapping {
                physical_start: address,
                virtual_start: std::ptr::NonNull::new(pointer).unwrap(),
                region_length: bytes,
                mapped_length: span,
                handler: self.clone(),
            }
        }
        fn unmap_physical_region<T>(_: &PhysicalMapping<Self, T>) {}
        memory!(read_u8, write_u8, u8);
        memory!(read_u16, write_u16, u16);
        memory!(read_u32, write_u32, u32);
        memory!(read_u64, write_u64, u64);
        pci!(read_pci_u8, write_pci_u8, u8);
        pci!(read_pci_u16, write_pci_u16, u16);
        pci!(read_pci_u32, write_pci_u32, u32);
        fn read_io_u8(&self, port: u16) -> u8 {
            self.read_io_u32(port) as u8
        }
        fn read_io_u16(&self, port: u16) -> u16 {
            self.read_io_u32(port) as u16
        }
        fn read_io_u32(&self, port: u16) -> u32 {
            *self.0.borrow().ports.get(&port).unwrap_or(&0)
        }
        fn write_io_u8(&self, port: u16, value: u8) {
            self.write_port(port, value.into(), 8);
        }
        fn write_io_u16(&self, port: u16, value: u16) {
            self.write_port(port, value.into(), 16);
        }
        fn write_io_u32(&self, port: u16, value: u32) {
            self.write_port(port, value, 32);
        }
        fn nanos_since_boot(&self) -> u64 {
            self.0.borrow().ns
        }
        fn sleep(&self, ms: u64) {
            self.0.borrow_mut().ns += ms * 1_000_000;
        }
        fn stall(&self, us: u64) {
            self.0.borrow_mut().ns += us * 1000;
        }
        fn create_mutex(&self) -> Handle {
            Handle(0)
        }
        fn acquire(&self, _: Handle, _: u16) -> Result<(), AmlError> {
            Ok(())
        }
        fn release(&self, _: Handle) {}
    }

    pub(super) fn setup(aml: &[u8], pm1b: bool) -> (Power<Mock>, Mock) {
        let host = Mock::default();
        // FADT is a packed table of integers, byte arrays and MaybeUninit fields.
        let mut fadt: Fadt = unsafe { std::mem::zeroed() };
        fadt.header.revision = 1;
        fadt.pm1a_control_block = 0x404;
        fadt.pm1b_control_block = if pm1b { 0x504 } else { 0 };
        fadt.pm1a_event_block = 0x400;
        fadt.pm1_event_length = 4;
        fadt.pm1_control_length = 2;
        fadt.smi_cmd_port = 0xb2;
        fadt.acpi_enable = 0xa1;
        fadt.acpi_disable = 0xa0;
        let registers = Arc::new(FixedRegisters::new(&fadt, host.clone()).unwrap());
        let interpreter = Interpreter::new(host.clone(), 2, registers.clone(), None);
        interpreter.load_table(aml).unwrap();
        (
            Power::new(host.clone(), interpreter, registers, &fadt).unwrap(),
            host,
        )
    }

    pub(super) const S5: &[u8] = b"\x08\\_S5_\x12\x06\x02\x0a\x05\x0a\x06";

    #[test]
    fn s5_uses_aml_not_a_byte_search_and_has_no_boot_side_effects() {
        let (power, host) = setup(S5, true);
        assert_eq!(power.sleep_types(), [5, 6]);
        assert!(host.0.borrow().writes.is_empty());
    }

    #[test]
    fn different_pm1a_and_pm1b_types_and_preserved_bits() {
        let (power, host) = setup(S5, true);
        host.0.borrow_mut().ports.insert(0x404, 0x401);
        host.0.borrow_mut().ports.insert(0x504, 0x803);
        power.prepare().unwrap();
        assert!(matches!(power.enter_s5(), Err(acpi::AcpiError::Timeout)));
        assert_eq!(
            host.0.borrow().writes,
            [
                (0x400, 0x8000),
                (0x404, 0x1401),
                (0x504, 0x1803),
                (0x404, 0x3401),
                (0x504, 0x3803)
            ]
        );
    }

    #[test]
    fn absent_pm1b_is_not_written() {
        let (power, host) = setup(S5, false);
        host.0.borrow_mut().ports.insert(0x404, 1);
        power.prepare().unwrap();
        assert!(power.enter_s5().is_err());
        assert_eq!(host.0.borrow().writes.len(), 3);
    }

    #[test]
    fn acpi_mode_enable_has_timeout_and_does_not_enter_s5_on_failure() {
        let (power, host) = setup(S5, false);
        assert!(matches!(power.prepare(), Err(acpi::AcpiError::Timeout)));
        assert_eq!(host.0.borrow().writes, [(0xb2, 0xa1)]);
        assert_eq!(host.0.borrow().ns, 3_000_000_000);
    }

    #[test]
    fn acpi_mode_can_be_enabled() {
        let (power, host) = setup(S5, false);
        host.0.borrow_mut().enable = true;
        power.prepare().unwrap();
        assert_eq!(host.read_io_u16(0x404) & 1, 1);
        power.restore_mode().unwrap();
        assert_eq!(host.read_io_u16(0x404) & 1, 0);
    }

    #[test]
    fn invalid_sleep_packages_are_rejected() {
        assert!(sleep_types(&Object::Integer(5)).is_err());
        assert!(sleep_types(&Object::Package(vec![Object::Integer(5).wrap()])).is_err());
        assert!(sleep_types(&Object::Package(vec![
            Object::Integer(8).wrap(),
            Object::Integer(0).wrap()
        ]))
        .is_err());
    }

    #[test]
    fn control_does_not_replay_global_lock_release_or_previous_sleep() {
        assert_eq!(sleep_control(0xffff, 0), 0xc3fb);
    }

    fn checksum(bytes: &mut [u8], index: usize) {
        bytes[index] = 0;
        bytes[index] =
            0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
    }

    fn firmware_fixture(s5_in_ssdt: bool) -> (Mock, Vec<u8>) {
        let host = Mock::default();
        let mut bytes = vec![0; 0x4000];
        bytes[0x100..0x108].copy_from_slice(b"RSD PTR ");
        bytes[0x110..0x114].copy_from_slice(&0x200u32.to_le_bytes());
        checksum(&mut bytes[0x100..0x114], 8);
        fn header(bytes: &mut [u8], signature: &[u8; 4], length: u32) {
            bytes[..4].copy_from_slice(signature);
            bytes[4..8].copy_from_slice(&length.to_le_bytes());
            bytes[8] = 1;
        }
        let root_len = if s5_in_ssdt { 44 } else { 40 };
        header(&mut bytes[0x200..], b"RSDT", root_len);
        bytes[0x224..0x228].copy_from_slice(&0x300u32.to_le_bytes());
        if s5_in_ssdt {
            bytes[0x228..0x22c].copy_from_slice(&0x1200u32.to_le_bytes());
        }
        checksum(&mut bytes[0x200..0x200 + root_len as usize], 9);
        let fadt = &mut bytes[0x300..0x374];
        header(fadt, b"FACP", 116);
        fadt[40..44].copy_from_slice(&0xff0u32.to_le_bytes()); // DSDT crosses a page boundary
        fadt[56..60].copy_from_slice(&0x400u32.to_le_bytes());
        fadt[64..68].copy_from_slice(&0x404u32.to_le_bytes());
        fadt[88] = 4;
        fadt[89] = 2;
        checksum(fadt, 9);
        let dsdt_len = if s5_in_ssdt { 36 } else { 36 + S5.len() };
        header(&mut bytes[0xff0..], b"DSDT", dsdt_len as u32);
        if !s5_in_ssdt {
            bytes[0xff0 + 36..0xff0 + dsdt_len].copy_from_slice(S5);
        }
        checksum(&mut bytes[0xff0..0xff0 + dsdt_len], 9);
        if s5_in_ssdt {
            header(&mut bytes[0x1200..], b"SSDT", (36 + S5.len()) as u32);
            bytes[0x1200 + 36..0x1200 + 36 + S5.len()].copy_from_slice(S5);
            checksum(&mut bytes[0x1200..0x1200 + 36 + S5.len()], 9);
        }
        (host, bytes)
    }

    fn install(host: &Mock, bytes: &[u8]) {
        host.0.borrow_mut().memory = bytes
            .chunks_exact(8)
            .map(|chunk| u64::from_ne_bytes(chunk.try_into().unwrap()))
            .collect();
    }

    #[test]
    fn validated_firmware_loads_cross_page_dsdt_and_ssdt_sleep_definition() {
        for s5_in_ssdt in [false, true] {
            let (host, bytes) = firmware_fixture(s5_in_ssdt);
            install(&host, &bytes);
            let power = super::firmware::load(host.clone(), 0x100)
                .unwrap()
                .power()
                .unwrap();
            assert_eq!(power.sleep_types(), [5, 6]);
            assert!(host.0.borrow().writes.is_empty());
        }
    }

    #[test]
    fn corrupt_rsdp_root_fadt_and_aml_abort_without_hardware_writes() {
        for corrupt_offset in [0x108, 0x209, 0x309, 0xff9, 0x1209] {
            let (host, mut bytes) = firmware_fixture(true);
            bytes[corrupt_offset] ^= 1;
            install(&host, &bytes);
            assert!(super::firmware::load(host.clone(), 0x100).is_err());
            assert!(host.0.borrow().writes.is_empty());
        }
    }

    #[test]
    fn missing_s5_does_not_block_firmware_routing_but_rejects_shutdown() {
        let (host, mut bytes) = firmware_fixture(false);
        bytes[0xff4..0xff8].copy_from_slice(&36u32.to_le_bytes());
        checksum(&mut bytes[0xff0..0xff0 + 36], 9);
        install(&host, &bytes);
        assert!(super::firmware::load(host.clone(), 0x100)
            .unwrap()
            .power()
            .is_err());
        assert!(host.0.borrow().writes.is_empty());
    }
}
