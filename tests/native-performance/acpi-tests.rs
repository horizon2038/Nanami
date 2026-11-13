//! Exercise the real loader memory policy and user-space ACPI walker together.
#![allow(dead_code)]
extern crate self as nanami_runtime;
use std::{cell::RefCell, collections::BTreeMap};
use uefi::boot::MemoryType;
pub type Word = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestError {
    InvalidArgument,
    Unsupported,
    Protocol,
    Status(Word),
}
impl std::fmt::Display for RequestError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{self:?}")
    }
}
#[macro_export]
macro_rules! println {
    ($($arg:tt)*) => { $crate::LOG.with(|log| log.borrow_mut().push(format!($($arg)*))) };
}
thread_local! { static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) }; }

#[path = "../../nanami/servers/libs/nanami-client/src/firmware.rs"]
mod acpi;
#[path = "../../spencer/a9nloader-rs/src/loader/memory/map.rs"]
mod map;
use map::MemoryMapType;
#[path = "../../spencer/a9nloader-rs/src/loader/memory/owned.rs"]
mod owned;
#[path = "../../spencer/a9nloader-rs/src/loader/memory/policy.rs"]
mod policy;

const PAGE: usize = 4096;
const RSDP: usize = 0x8c7c8000;
const XSDT: usize = RSDP + 0xa8;
const RSDT: usize = RSDP + 0x28;
const HPET: usize = 0x8c80be80;
#[derive(Default)]
struct Firmware {
    pages: BTreeMap<Word, Box<[u8; PAGE]>>,
    kinds: BTreeMap<Word, MemoryType>,
    mappings: Vec<Word>,
}
thread_local! { static FIRMWARE: RefCell<Firmware> = RefCell::new(Firmware::default()); }
pub fn request_mmio(address: Word, bytes: Word) -> Result<(Word, Word), RequestError> {
    assert_eq!(bytes, PAGE);
    assert_eq!(address % PAGE, 0);
    FIRMWARE.with(|firmware| {
        let mut firmware = firmware.borrow_mut();
        firmware.mappings.push(address);
        let kind = firmware
            .kinds
            .get(&address)
            .copied()
            .unwrap_or(MemoryType::RESERVED);
        if policy::classify(kind) != map::MemoryMapType::Device {
            return Err(RequestError::Status(1));
        }
        Ok((
            address,
            firmware.pages.get(&address).unwrap().as_ptr() as Word,
        ))
    })
}
fn write(address: Word, bytes: &[u8], kind: MemoryType) {
    FIRMWARE.with(|firmware| {
        let mut firmware = firmware.borrow_mut();
        for (offset, &byte) in bytes.iter().enumerate() {
            let address = address + offset;
            let page = address & !(PAGE - 1);
            firmware.kinds.insert(page, kind);
            firmware
                .pages
                .entry(page)
                .or_insert_with(|| Box::new([0; PAGE]))[address % PAGE] = byte;
        }
    });
}
fn checksum(bytes: &mut [u8], index: usize) {
    bytes[index] = 0;
    bytes[index] = 0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, &byte| sum.wrapping_add(byte)));
}
fn sdt(signature: &[u8; 4], length: usize) -> Vec<u8> {
    let mut bytes = vec![0; length];
    bytes[..4].copy_from_slice(signature);
    bytes[4..8].copy_from_slice(&(length as u32).to_le_bytes());
    bytes[8] = 1;
    bytes
}
fn install(kind: MemoryType, hpet: Word) {
    FIRMWARE.with(|firmware| *firmware.borrow_mut() = Firmware::default());
    LOG.with(|log| log.borrow_mut().clear());
    let mut table = sdt(b"HPET", 56);
    table[44..52].copy_from_slice(&0xfed00000u64.to_le_bytes());
    checksum(&mut table, 9);
    write(hpet, &table, kind);
    let mut xsdt = sdt(b"XSDT", 44);
    xsdt[36..44].copy_from_slice(&(hpet as u64).to_le_bytes());
    checksum(&mut xsdt, 9);
    write(XSDT, &xsdt, kind);
    let mut rsdt = sdt(b"RSDT", 40);
    rsdt[36..40].copy_from_slice(&(hpet as u32).to_le_bytes());
    checksum(&mut rsdt, 9);
    write(RSDT, &rsdt, kind);
    let mut rsdp = [0; 36];
    rsdp[..8].copy_from_slice(b"RSD PTR ");
    rsdp[15] = 2;
    rsdp[16..20].copy_from_slice(&(RSDT as u32).to_le_bytes());
    rsdp[20..24].copy_from_slice(&36u32.to_le_bytes());
    rsdp[24..32].copy_from_slice(&(XSDT as u64).to_le_bytes());
    checksum(&mut rsdp[..20], 8);
    checksum(&mut rsdp, 32);
    write(RSDP, &rsdp, kind);
}

#[test]
fn nvs_and_reclaim_tables_are_discoverable_without_modifying_firmware() {
    for kind in [MemoryType::ACPI_NON_VOLATILE, MemoryType::ACPI_RECLAIM] {
        install(kind, HPET);
        let before = FIRMWARE.with(|firmware| firmware.borrow().pages.clone());
        assert_eq!(acpi::find_hpet_mmio_base(RSDP), Ok(Some(0xfed00000)));
        FIRMWARE.with(|firmware| {
            let firmware = firmware.borrow();
            assert_eq!(firmware.pages, before);
            assert_eq!(firmware.mappings, [RSDP, HPET & !(PAGE - 1)]);
        });
        LOG.with(|log| assert!(log.borrow().is_empty()));
    }
}

#[test]
fn fadt_i8042_flag_controls_legacy_controller_discovery() {
    let kind = MemoryType::ACPI_RECLAIM;
    install(kind, HPET);
    assert_eq!(acpi::has_i8042(RSDP), Ok(true)); // no FADT: legacy probe
    for (revision, flags, present) in [(6, 0, false), (6, 2, true), (1, 0, true)] {
        let mut table = sdt(b"FACP", 116);
        table[8] = revision;
        table[109] = flags;
        checksum(&mut table, 9);
        write(HPET, &table, kind);
        assert_eq!(acpi::has_i8042(RSDP), Ok(present));
    }
    let mut table = sdt(b"FACP", 100);
    table[8] = 6;
    checksum(&mut table, 9);
    write(HPET, &table, kind);
    assert_eq!(acpi::has_i8042(RSDP), Err(RequestError::Protocol));
}

#[test]
fn unusable_and_runtime_firmware_code_are_still_not_exposed() {
    for kind in [
        MemoryType::RUNTIME_SERVICES_CODE,
        MemoryType::UNUSABLE,
        MemoryType::PAL_CODE,
    ] {
        assert_eq!(policy::classify(kind), map::MemoryMapType::Reserved);
        install(kind, HPET);
        assert_eq!(
            acpi::find_hpet_mmio_base(RSDP),
            Err(RequestError::Status(1))
        );
        LOG.with(|log| assert!(log.borrow()[0].contains("ACPI map failed page=0x8c7c8000")));
    }
}

#[test]
fn aml_firmware_data_pages_remain_mappable_but_never_allocator_ram() {
    // Physical-machine shutdown accesses this page after table loading.
    // Its actual EFI type must be checked on that machine, not guessed here.
    const OPREGION: usize = 0x8c3a3000;
    for kind in [
        MemoryType::BOOT_SERVICES_DATA,
        MemoryType::BOOT_SERVICES_CODE,
        MemoryType::RUNTIME_SERVICES_DATA,
        MemoryType::RESERVED,
    ] {
        install(kind, HPET);
        let contents = [0x5a; PAGE];
        write(OPREGION, &contents, kind);
        assert_eq!(policy::classify(kind), map::MemoryMapType::Device);
        let (_, address) = request_mmio(OPREGION, PAGE).unwrap();
        assert_eq!(
            unsafe { std::slice::from_raw_parts(address as *const u8, PAGE) },
            contents
        );
        FIRMWARE.with(|firmware| assert_eq!(&firmware.borrow().pages[&OPREGION][..], contents));
    }
}

#[test]
fn interleaved_boot_firmware_does_not_exhaust_kernel_map_or_expose_reservations() {
    use map::{MemoryMapEntry as Entry, MemoryMapType::*};
    let mut owned = owned::Reservations::new();
    owned.record(0x80000000 + 99 * PAGE, 1).unwrap();
    let entries = (0..200).map(|index| {
        let kind = if index == 99 {
            MemoryType::RESERVED // e.g. loader/kernel allocation, never bridge it
        } else if index % 2 == 0 {
            MemoryType::BOOT_SERVICES_DATA
        } else {
            MemoryType::BOOT_SERVICES_CODE
        };
        Entry {
            physical_address_start: 0x80000000 + index * PAGE,
            page_count: 1,
            memory_type: policy::classify(kind),
        }
    });
    let mut output = [Entry::EMPTY; 256];
    let count = map::build_memory_map_excluding(
        entries,
        owned.entries(),
        &mut output,
        0x80000000 + 200 * PAGE,
    )
    .unwrap();
    assert_eq!(count, 3);
    let count = map::export_memory_map(&mut output, count).unwrap();
    assert_eq!(count, 7);
    assert_eq!(output[0].memory_type, Device);
    assert_eq!(
        output[..4]
            .iter()
            .map(|entry| entry.page_count)
            .sum::<usize>(),
        99
    );
    assert_eq!(output[4].physical_address_start, 0x80000000 + 100 * PAGE);
    assert_eq!(
        output[4..7]
            .iter()
            .map(|entry| entry.page_count)
            .sum::<usize>(),
        100
    );
    assert!(output[..count]
        .iter()
        .all(|entry| entry.page_count.is_power_of_two()));
    assert_eq!(output[7], Entry::EMPTY);
}

#[test]
fn coalesced_firmware_export_preserves_every_page_beyond_eight_generic_pieces() {
    use map::{MemoryMapEntry as Entry, MemoryMapType::*};
    let mut output = [Entry::EMPTY; 128];
    output[0] = Entry {
        physical_address_start: 0x80000000,
        page_count: 0x1ffff,
        memory_type: Device,
    };
    output[1] = Entry {
        physical_address_start: 0x1_00000000,
        page_count: 3,
        memory_type: Free,
    };
    let count = map::export_memory_map(&mut output, 2).unwrap();
    assert_eq!(count, 19);
    let mut end = 0x80000000;
    for entry in &output[..17] {
        assert_eq!(entry.physical_address_start, end);
        assert_eq!(entry.memory_type, Device);
        assert!(entry.page_count.is_power_of_two());
        end += entry.page_count * PAGE;
    }
    assert_eq!(end, 0x80000000 + 0x1ffff * PAGE);
    assert_eq!(output[17].physical_address_start, 0x1_00000000);
    assert_eq!(output[18].physical_address_start, 0x1_00000000 + 2 * PAGE);
    assert_eq!(output[18].memory_type, Free);
}

#[test]
fn kernel_export_checks_generic_slot_budget_not_only_map_entry_count() {
    use map::{MemoryMapEntry as Entry, MemoryMapType::*};
    let mut output = [Entry {
        physical_address_start: 0x80000000,
        page_count: 255,
        memory_type: Device,
    }; 17];
    assert_eq!(
        map::export_memory_map(&mut output, 17),
        Err(map::MapError::Capacity)
    );
}

#[test]
fn failure_diagnostic_identifies_child_page_not_rsdp() {
    install(MemoryType::ACPI_NON_VOLATILE, HPET);
    FIRMWARE.with(|firmware| {
        firmware
            .borrow_mut()
            .kinds
            .insert(HPET & !(PAGE - 1), MemoryType::UNUSABLE);
    });
    assert_eq!(
        acpi::find_hpet_mmio_base(RSDP),
        Err(RequestError::Status(1))
    );
    LOG.with(|log| {
        assert_eq!(log.borrow().len(), 1);
        assert!(log.borrow()[0].contains("ACPI map failed page=0x8c80b000"));
    });
}

#[test]
fn firmware_reserved_is_mappable_but_adjacent_loader_kernel_and_init_pages_are_not() {
    use map::{MemoryMapEntry as Entry, MemoryMapType::*};
    let base = 0x80000000;
    let mut private = owned::Reservations::new();
    // Allocation order is not physical order. EFI may merge all five ranges
    // into one Reserved descriptor, so tracking the EFI type is insufficient.
    private.record(base + 8 * PAGE, 3).unwrap(); // init
    private.record(base + 2 * PAGE, 2).unwrap(); // kernel
    private.record(base + 6 * PAGE, 1).unwrap(); // trampoline fixture
    let entries = [Entry {
        physical_address_start: base,
        page_count: 16,
        memory_type: policy::classify(MemoryType::RESERVED),
    }];
    let mut output = [Entry::EMPTY; 16];
    let count =
        map::build_memory_map_excluding(entries, private.entries(), &mut output, base + 16 * PAGE)
            .unwrap();
    let count = map::export_memory_map(&mut output, count).unwrap();
    for page in 0..16 {
        let address = base + page * PAGE;
        let exported = output[..count].iter().find(|entry| {
            address >= entry.physical_address_start
                && address < entry.physical_address_start + entry.page_count * PAGE
        });
        if (2..4).contains(&page) || page == 6 || (8..11).contains(&page) {
            assert!(exported.is_none(), "loader allocation gained a capability");
        } else {
            assert_eq!(exported.unwrap().memory_type, Device);
        }
    }
}

#[test]
fn explicit_private_reservations_cover_descriptor_boundaries_and_device_gaps() {
    use map::{MemoryMapEntry as Entry, MemoryMapType::*};
    let base = 0x80000000;
    let mut private = owned::Reservations::new();
    private.record(base + PAGE, 4).unwrap();
    let entries = [
        Entry {
            physical_address_start: base,
            page_count: 2,
            memory_type: Device,
        },
        Entry {
            physical_address_start: base + 4 * PAGE,
            page_count: 2,
            memory_type: Free,
        },
    ];
    let mut output = [Entry::EMPTY; 16];
    let count =
        map::build_memory_map_excluding(entries, private.entries(), &mut output, base + 6 * PAGE)
            .unwrap();
    assert_eq!(count, 3);
    assert_eq!(
        output[0],
        Entry {
            physical_address_start: base,
            page_count: 1,
            memory_type: Device
        }
    );
    assert_eq!(
        output[1],
        Entry {
            physical_address_start: base + PAGE,
            page_count: 4,
            memory_type: Reserved
        }
    );
    assert_eq!(
        output[2],
        Entry {
            physical_address_start: base + 5 * PAGE,
            page_count: 1,
            memory_type: Free
        }
    );
}

#[test]
fn private_reservation_tracking_rejects_overlap_overflow_alignment_and_capacity() {
    let mut private = owned::Reservations::new();
    assert_eq!(private.record(1, 1), Err(map::MapError::InvalidRange));
    assert_eq!(private.record(0x1000, 0), Err(map::MapError::InvalidRange));
    assert_eq!(
        private.record(usize::MAX & !4095, 2),
        Err(map::MapError::InvalidRange)
    );
    private.record(0x2000, 2).unwrap();
    assert_eq!(private.record(0x1000, 2), Err(map::MapError::Overlap));
    assert_eq!(private.record(0x3000, 2), Err(map::MapError::Overlap));
    for index in 1..64 {
        private.record(0x2000 + index * 3 * PAGE, 1).unwrap();
    }
    assert_eq!(private.record(0x100000, 1), Err(map::MapError::Capacity));
    assert_eq!(private.entries().len(), 64);
}

#[test]
fn nvs_cross_page_hpet_table_and_acpi1_rsdt_remain_supported() {
    install(MemoryType::ACPI_NON_VOLATILE, 0x8c80bff0);
    assert_eq!(acpi::find_hpet_mmio_base(RSDP), Ok(Some(0xfed00000)));
    FIRMWARE.with(|firmware| {
        let mut firmware = firmware.borrow_mut();
        let rsdp = firmware.pages.get_mut(&RSDP).unwrap();
        rsdp[15] = 0;
        checksum(&mut rsdp[..20], 8);
    });
    assert_eq!(acpi::find_hpet_mmio_base(RSDP), Ok(Some(0xfed00000)));
}

#[test]
fn bad_checksum_still_fails_closed() {
    install(MemoryType::ACPI_NON_VOLATILE, HPET);
    FIRMWARE.with(|firmware| firmware.borrow_mut().pages.get_mut(&RSDP).unwrap()[8] ^= 1);
    assert_eq!(acpi::find_hpet_mmio_base(RSDP), Err(RequestError::Protocol));
}

fn install_mcfg(regions: &[(u64, u16, u8, u8)]) {
    let kind = MemoryType::ACPI_NON_VOLATILE;
    install(kind, HPET);
    let mut table = sdt(b"MCFG", 44 + regions.len() * 16);
    for (index, &(physical, segment, first, last)) in regions.iter().enumerate() {
        let offset = 44 + index * 16;
        table[offset..offset + 8].copy_from_slice(&physical.to_le_bytes());
        table[offset + 8..offset + 10].copy_from_slice(&segment.to_le_bytes());
        table[offset + 10] = first;
        table[offset + 11] = last;
    }
    checksum(&mut table, 9);
    write(HPET, &table, kind);
}

#[test]
fn mcfg_nonzero_bus_ranges_still_use_bus_zero_relative_bases() {
    install_mcfg(&[(0xe0000000, 3, 0x40, 0x4f)]);
    let regions = acpi::find_pci_ecam(RSDP).unwrap();
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].address(3, 0x42, 5, 2), Some(0xe422a000));
    assert_eq!(regions[0].address(0, 0x42, 5, 2), None);
    assert_eq!(regions[0].address(3, 0x3f, 5, 2), None);
    assert_eq!(regions[0].address(3, 0x42, 32, 0), None);
}

#[test]
fn mcfg_rejects_overlapping_bus_ranges_unaligned_bases_and_bad_checksum() {
    for regions in [
        vec![(0xe0000000, 0, 0, 15), (0xf0000000, 0, 15, 31)],
        vec![(0xe0000001, 0, 0, 15)],
        vec![(0xe0000000, 0, 16, 15)],
    ] {
        install_mcfg(&regions);
        assert_eq!(acpi::find_pci_ecam(RSDP), Err(RequestError::Protocol));
    }
    install_mcfg(&[(0xe0000000, 0, 0, 15)]);
    let corrupt = HPET + 9;
    FIRMWARE.with(|firmware| {
        firmware
            .borrow_mut()
            .pages
            .get_mut(&(corrupt & !(PAGE - 1)))
            .unwrap()[corrupt % PAGE] ^= 1
    });
    assert_eq!(acpi::find_pci_ecam(RSDP), Err(RequestError::Protocol));
}

#[test]
fn absent_mcfg_is_a_valid_legacy_pci_platform() {
    install(MemoryType::ACPI_NON_VOLATILE, HPET);
    assert_eq!(acpi::find_pci_ecam(RSDP), Ok(Vec::new()));
    assert_eq!(acpi::find_pci_ecam(0), Ok(Vec::new()));
}

#[test]
fn classification_and_map_merging_never_turn_nvs_into_free_ram() {
    use map::{MemoryMapEntry as Entry, MemoryMapType::*};
    let entries = [
        MemoryType::CONVENTIONAL,
        MemoryType::ACPI_NON_VOLATILE,
        MemoryType::ACPI_RECLAIM,
        MemoryType::RUNTIME_SERVICES_DATA,
        MemoryType::CONVENTIONAL,
    ]
    .into_iter()
    .enumerate()
    .map(|(index, kind)| Entry {
        physical_address_start: 0x80000000 + index * PAGE,
        page_count: 1,
        memory_type: policy::classify(kind),
    });
    let mut output = [Entry::EMPTY; 8];
    let count = map::build_memory_map(entries, &mut output, 0x80005000).unwrap();
    assert_eq!(count, 3);
    assert_eq!(
        output[..3]
            .iter()
            .map(|entry| entry.memory_type)
            .collect::<Vec<_>>(),
        [Free, Device, Free]
    );
    assert_eq!(output[1].physical_address_start, 0x80001000);
    assert_eq!(output[1].page_count, 3);
    assert_eq!(policy::classify(MemoryType::PERSISTENT_MEMORY), Free);
    assert_eq!(policy::classify(MemoryType::MMIO), Device);
}
