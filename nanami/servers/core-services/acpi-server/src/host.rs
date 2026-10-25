//! Hardware access for the single-threaded userspace AML interpreter.
use acpi::{aml::AmlError, Handle, Handler, PciAddress, PhysicalMapping};
use alloc::{rc::Rc, vec::Vec};
use core::{cell::RefCell, mem, ptr::NonNull};
use nanami_runtime::{RequestError, Word};

const IO_SLOT: Word = 16;
const HIGH_IO_SLOT: Word = 17;
const PCI_SLOT: Word = 23;
const TIMER_SLOT: Word = 22;
const PAGE_SIZE: usize = 4096;

struct Mapping {
    physical: usize,
    virtual_address: usize,
    bytes: usize,
}

struct State {
    mappings: Vec<Mapping>,
    mutexes: Vec<usize>,
}

#[derive(Clone)]
pub struct Host(Rc<RefCell<State>>);

impl Host {
    pub fn new() -> Result<Self, RequestError> {
        nanami_client::registry::connect_boot_service("timer-service", TIMER_SLOT)?;
        nanami_runtime::request_io_port(0, 0xcf7, IO_SLOT)?;
        nanami_runtime::request_io_port(0xd00, 0xffff, HIGH_IO_SLOT)?;
        nanami_client::pci::connect(PCI_SLOT)?;
        Ok(Self(Rc::new(RefCell::new(State {
            mappings: Vec::new(),
            mutexes: Vec::new(),
        }))))
    }

    fn map(&self, address: usize, size: usize) -> usize {
        let end = address
            .checked_add(size)
            .expect("ACPI physical range overflow");
        assert!(size != 0 && size <= 1024 * 1024, "invalid ACPI mapping");
        let mut state = self.0.borrow_mut();
        for mapping in &state.mappings {
            if address >= mapping.physical && end <= mapping.physical + mapping.bytes {
                return mapping.virtual_address + address - mapping.physical;
            }
        }
        let base = address & !(PAGE_SIZE - 1);
        let bytes = end
            .checked_add(PAGE_SIZE - 1)
            .expect("ACPI mapping overflow")
            & !(PAGE_SIZE - 1);
        let bytes = bytes - base;
        let (_, virtual_address) = nanami_runtime::request_mmio(base, bytes)
            .unwrap_or_else(|error| panic!("ACPI map {:#x}+{:#x}: {}", base, bytes, error));
        state.mappings.push(Mapping {
            physical: base,
            virtual_address,
            bytes,
        });
        virtual_address + address - base
    }

    fn pointer<T>(&self, address: usize) -> *mut T {
        assert_eq!(address % mem::align_of::<T>(), 0, "unaligned ACPI register");
        self.map(address, mem::size_of::<T>()) as *mut T
    }

    fn io_read(&self, port: u16, width: usize) -> usize {
        nanami_runtime::io::io_read(
            nanami_runtime::ipc::process_slot_descriptor(self.io_slot(port)),
            port as Word,
            width,
        )
        .expect("ACPI I/O read failed")
    }

    fn io_write(&self, port: u16, width: usize, value: usize) {
        nanami_runtime::io::io_write(
            nanami_runtime::ipc::process_slot_descriptor(self.io_slot(port)),
            port as Word,
            width,
            value,
        )
        .expect("ACPI I/O write failed");
    }

    fn io_slot(&self, port: u16) -> Word {
        if port < 0xcf8 {
            return IO_SLOT;
        }
        assert!(
            port >= 0xd00,
            "PCI configuration ports belong to pci-server"
        );
        HIGH_IO_SLOT
    }
}

macro_rules! memory_access {
    ($read:ident, $write:ident, $ty:ty) => {
        fn $read(&self, address: usize) -> $ty {
            unsafe { core::ptr::read_volatile(self.pointer::<$ty>(address)) }
        }
        fn $write(&self, address: usize, value: $ty) {
            unsafe { core::ptr::write_volatile(self.pointer::<$ty>(address), value) }
        }
    };
}

macro_rules! io_access {
    ($read:ident, $write:ident, $ty:ty) => {
        fn $read(&self, port: u16) -> $ty {
            self.io_read(port, mem::size_of::<$ty>()) as $ty
        }
        fn $write(&self, port: u16, value: $ty) {
            self.io_write(port, mem::size_of::<$ty>(), value as usize);
        }
    };
}

macro_rules! pci_access {
    ($read:ident, $write:ident, $ty:ty) => {
        fn $read(&self, address: PciAddress, offset: u16) -> $ty {
            let address = nanami_client::pci::Address {
                segment: address.segment(),
                bus: address.bus(),
                device: address.device(),
                function: address.function(),
            };
            nanami_client::pci::read(
                nanami_runtime::ipc::process_slot_descriptor(PCI_SLOT),
                address,
                offset as Word,
                mem::size_of::<$ty>(),
            )
            .expect("ACPI PCI read failed") as $ty
        }
        fn $write(&self, address: PciAddress, offset: u16, value: $ty) {
            let address = nanami_client::pci::Address {
                segment: address.segment(),
                bus: address.bus(),
                device: address.device(),
                function: address.function(),
            };
            nanami_client::pci::write(
                nanami_runtime::ipc::process_slot_descriptor(PCI_SLOT),
                address,
                offset as Word,
                mem::size_of::<$ty>(),
                value as u32,
            )
            .expect("ACPI PCI write failed");
        }
    };
}

impl Handler for Host {
    unsafe fn map_physical_region<T>(
        &self,
        address: usize,
        size: usize,
    ) -> PhysicalMapping<Self, T> {
        // Some ACPI structures have version-dependent trailing fields. Map at
        // least the Rust type's extent, even if an older table is shorter.
        let mapped_length = size.max(mem::size_of::<T>());
        let pointer = self.map(address, mapped_length) as *mut T;
        assert_eq!(pointer as usize % mem::align_of::<T>(), 0);
        PhysicalMapping {
            physical_start: address,
            virtual_start: NonNull::new(pointer).unwrap(),
            region_length: size,
            mapped_length,
            handler: self.clone(),
        }
    }

    fn unmap_physical_region<T>(_: &PhysicalMapping<Self, T>) {
        // Firmware/operation-region mappings are cached for this server's
        // lifetime; Alpha releases them when the process exits.
    }

    memory_access!(read_u8, write_u8, u8);
    memory_access!(read_u16, write_u16, u16);
    memory_access!(read_u32, write_u32, u32);
    memory_access!(read_u64, write_u64, u64);
    io_access!(read_io_u8, write_io_u8, u8);
    io_access!(read_io_u16, write_io_u16, u16);
    io_access!(read_io_u32, write_io_u32, u32);
    pci_access!(read_pci_u8, write_pci_u8, u8);
    pci_access!(read_pci_u16, write_pci_u16, u16);
    pci_access!(read_pci_u32, write_pci_u32, u32);

    fn nanos_since_boot(&self) -> u64 {
        let (ticks, hz) = nanami_client::timer::timer_service_monotonic_ticks(
            nanami_runtime::ipc::process_slot_descriptor(TIMER_SLOT),
        )
        .expect("ACPI timer unavailable");
        assert!(hz != 0);
        ((ticks as u128 * 1_000_000_000) / hz as u128) as u64
    }

    fn stall(&self, microseconds: u64) {
        let start = self.nanos_since_boot();
        let duration = microseconds.saturating_mul(1000);
        while self.nanos_since_boot().saturating_sub(start) < duration {
            core::hint::spin_loop();
        }
    }

    fn sleep(&self, milliseconds: u64) {
        nanami_client::timer::timer_service_sleep_milliseconds(
            nanami_runtime::ipc::process_slot_descriptor(TIMER_SLOT),
            milliseconds as Word,
        )
        .expect("ACPI sleep failed");
    }

    fn create_mutex(&self) -> Handle {
        let mut state = self.0.borrow_mut();
        let handle = Handle(
            state
                .mutexes
                .len()
                .try_into()
                .expect("too many AML mutexes"),
        );
        state.mutexes.push(0);
        handle
    }

    fn acquire(&self, mutex: Handle, _timeout: u16) -> Result<(), AmlError> {
        // All AML runs on one thread, including recursive method invocations.
        let mut state = self.0.borrow_mut();
        let count = &mut state.mutexes[mutex.0 as usize];
        *count = count.checked_add(1).expect("AML mutex recursion overflow");
        Ok(())
    }

    fn release(&self, mutex: Handle) {
        let mut state = self.0.borrow_mut();
        let count = &mut state.mutexes[mutex.0 as usize];
        *count = count.checked_sub(1).expect("unowned AML mutex release");
    }
}
