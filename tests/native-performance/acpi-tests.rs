//! Exercise the real loader memory policy and user-space ACPI walker together.
#![allow(dead_code)]
extern crate self as libnanami;
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

#[path = "../../nanami/servers/core-services/driver-manager/src/arch/x86_64/acpi.rs"]
mod acpi;
#[path = "../../spencer/a9nloader-rs/src/loader/memory/map.rs"]
mod map;
use map::MemoryMapType;
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
fn reserved_runtime_and_boot_services_are_still_not_exposed() {
    for kind in [
        MemoryType::RESERVED,
        MemoryType::RUNTIME_SERVICES_CODE,
        MemoryType::RUNTIME_SERVICES_DATA,
        MemoryType::BOOT_SERVICES_CODE,
        MemoryType::BOOT_SERVICES_DATA,
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
fn failure_diagnostic_identifies_child_page_not_rsdp() {
    install(MemoryType::ACPI_NON_VOLATILE, HPET);
    FIRMWARE.with(|firmware| {
        firmware
            .borrow_mut()
            .kinds
            .insert(HPET & !(PAGE - 1), MemoryType::RESERVED);
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
    assert_eq!(count, 4);
    assert_eq!(
        output[..4]
            .iter()
            .map(|entry| entry.memory_type)
            .collect::<Vec<_>>(),
        [Free, Device, Reserved, Free]
    );
    assert_eq!(output[1].physical_address_start, 0x80001000);
    assert_eq!(output[1].page_count, 2);
    assert_eq!(policy::classify(MemoryType::PERSISTENT_MEMORY), Free);
    assert_eq!(policy::classify(MemoryType::MMIO), Device);
}
