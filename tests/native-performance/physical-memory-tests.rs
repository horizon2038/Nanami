//! Execute the production memory manager against checked capability syscalls.
//! Frames are stored as batches so multi-TiB maps need no host-sized RAM image.
#![allow(dead_code)]
extern crate alloc;
extern crate self as nun;
use std::{cell::RefCell, collections::BTreeMap};
pub type Word = usize;
pub type CapabilityDescriptor = usize;
pub type CapabilityResult = Result<(), CapabilityError>;
pub const WORD_BITS: usize = usize::BITS as usize;
pub const BYTE_BITS: usize = 8;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityError {
    InvalidArgument,
    InvalidDescriptor,
    InvalidDepth,
    IllegalOperation,
    PermissionDenied,
    Fatal,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityType {
    Generic,
    Node,
    Frame,
    PageTable,
}
pub use a9n_abi::capability_call;
use nanami_uapi::alpha::{AlphaEnvironment, MemoryResource, RESOURCE_DEVICE};
#[macro_export]
macro_rules! info { ($($arg:tt)*) => {{ let _ = format_args!($($arg)*); }} }
#[path = "../../nanami/src/nanami_utils/descriptor.rs"]
pub mod descriptor;
pub mod nanami_utils {
    pub use crate::descriptor;
}
#[path = "../../nanami/src/nanami_core/kernel_object.rs"]
pub mod kernel_object;
#[path = "../../nanami/src/nanami_core/physical_allocator.rs"]
pub mod physical_allocator;
pub mod nanami_core {
    pub use crate::{kernel_object, physical_allocator};
    pub mod vm_space {
        pub trait VmTracker {
            fn page_tables_ready(&self, address: usize) -> bool;
            fn record_frame(&mut self, address: usize, descriptor: usize) -> Result<(), ()>;
            fn record_page_table(&mut self, address: usize, slot: usize) -> Result<(), ()>;
        }
    }
}
#[path = "../../nanami/src/nanami_core"]
mod production {
    pub mod memory;
}
use descriptor::{make_child_slot_descriptor, make_root_slot_descriptor};
use memory::MemoryManager;
use production::memory;

#[derive(Clone, Copy)]
struct Cap {
    kind: CapabilityType,
    cursor: usize,
    end: usize,
    radix: usize,
    device: bool,
}
#[derive(Default)]
struct Fake {
    caps: BTreeMap<usize, Cap>,
    frames: BTreeMap<usize, (usize, usize)>,
    fail_kind: Option<(CapabilityType, usize)>,
    calls: usize,
    nodes: usize,
    batches: usize,
    map_attributes: Vec<usize>,
    tables: BTreeMap<(usize, usize, usize), usize>,
    parents: BTreeMap<usize, usize>,
    fail_revoke: bool,
}
thread_local! { static FAKE: RefCell<Fake> = RefCell::new(Fake::default()); }
pub mod arch {
    pub mod generic {
        use super::super::*;
        pub fn convert(
            source: usize,
            kind: CapabilityType,
            radix: usize,
            count: usize,
            node: usize,
            slot: usize,
        ) -> CapabilityResult {
            FAKE.with(|fake| {
                let mut fake = fake.borrow_mut();
                if let Some((wanted, skip)) = fake.fail_kind {
                    if wanted == kind {
                        if skip == 0 {
                            fake.fail_kind = None;
                            return Err(CapabilityError::Fatal);
                        }
                        fake.fail_kind = Some((wanted, skip - 1));
                    }
                }
                let parent = fake.caps[&node];
                let original = fake.caps[&source];
                assert_eq!(parent.kind, CapabilityType::Node);
                assert_eq!(original.kind, CapabilityType::Generic);
                assert!(slot + count <= 1 << parent.radix);
                assert!(
                    !original.device
                        || matches!(kind, CapabilityType::Generic | CapabilityType::Frame)
                );
                let bits = match kind {
                    CapabilityType::Node => kernel_object::node_memory_size_bits(radix),
                    CapabilityType::PageTable => 12,
                    _ => radix,
                };
                let bytes = 1usize << bits;
                let start = (original.cursor + bytes - 1) & !(bytes - 1);
                let end = start + bytes * count;
                assert!(end <= original.end, "generic exhausted");
                // Main and overflow process metadata arenas are aligned
                // 2-MiB allocations from already reserved metadata pools.
                let process_arena = kind == CapabilityType::Generic && radix == 21;
                if kind != CapabilityType::Node && !process_arena {
                    // A partially occupied boot page is reserved, not a usable
                    // untyped prefix. No whole free page may be skipped.
                    assert_eq!(
                        start,
                        (original.cursor + 4095) & !4095,
                        "untyped alignment prefix lost"
                    );
                }
                if kind == CapabilityType::Frame {
                    assert_eq!(slot, 0);
                    assert!(fake.frames.insert(node, (start, count)).is_none());
                    fake.batches += 1;
                } else {
                    assert_eq!(count, 1);
                    let descriptor = if node == 0 {
                        make_root_slot_descriptor(12, slot)
                    } else {
                        make_child_slot_descriptor(node, parent.radix, slot)
                    };
                    assert!(!fake.caps.contains_key(&descriptor), "occupied destination");
                    fake.caps.insert(
                        descriptor,
                        Cap {
                            kind,
                            cursor: start,
                            end,
                            radix,
                            device: original.device,
                        },
                    );
                    fake.parents.insert(descriptor, source);
                    if kind == CapabilityType::Node {
                        fake.nodes += 1;
                    }
                }
                fake.calls += 1;
                fake.caps.get_mut(&source).unwrap().cursor = end;
                Ok(())
            })
        }
    }
    pub mod node {
        use super::super::*;
        pub fn copy(_: usize, _: usize, _: usize) -> CapabilityResult {
            Ok(())
        }
        pub fn revoke(node: usize, index: usize) -> CapabilityResult {
            FAKE.with_borrow_mut(|fake| {
                if fake.fail_revoke {
                    return Err(CapabilityError::IllegalOperation);
                }
                let target = make_child_slot_descriptor(node, fake.caps[&node].radix, index);
                assert_eq!(fake.caps[&target].kind, CapabilityType::Generic);
                loop {
                    let children: Vec<_> = fake
                        .parents
                        .iter()
                        .filter_map(|(&child, &parent)| {
                            if parent == target || !fake.caps.contains_key(&parent) {
                                Some(child)
                            } else {
                                None
                            }
                        })
                        .collect();
                    if children.is_empty() {
                        break;
                    }
                    for child in children {
                        fake.parents.remove(&child);
                        fake.caps.remove(&child);
                    }
                }
                let cap = fake.caps.get_mut(&target).unwrap();
                cap.cursor = cap.end - (1usize << cap.radix);
                Ok(())
            })
        }
    }
    pub mod address_space {
        use super::super::*;
        pub fn map(
            address_space: usize,
            object: usize,
            address: usize,
            attribute: capability_call::address_space::Attribute,
        ) -> CapabilityResult {
            FAKE.with_borrow_mut(|fake| {
                fake.map_attributes.push(attribute.bits());
                if let Some(cap) = fake.caps.get(&object) {
                    if cap.kind == CapabilityType::PageTable {
                        let key = (address_space, address >> (12 + 9 * cap.radix), cap.radix);
                        if fake.tables.contains_key(&key) {
                            return Err(CapabilityError::IllegalOperation);
                        }
                        fake.tables.insert(key, object);
                    }
                }
                Ok(())
            })
        }
        pub fn get_unset_depth(
            address_space: usize,
            address: usize,
            _: usize,
        ) -> Result<usize, CapabilityError> {
            FAKE.with_borrow(|fake| {
                for depth in (1..=3).rev() {
                    let key = (address_space, address >> (12 + 9 * depth), depth);
                    let Some(table) = fake.tables.get(&key) else {
                        return Ok(depth);
                    };
                    assert_eq!(fake.caps[table].kind, CapabilityType::PageTable);
                }
                Ok(0)
            })
        }
    }
}

fn boot(ram_radix: u8, online: bool) -> (MemoryManager, AlphaEnvironment) {
    boot_with_reserved_prefix(ram_radix, online, 0)
}

fn boot_with_reserved_prefix(
    ram_radix: u8,
    online: bool,
    ram_consumed: usize,
) -> (MemoryManager, AlphaEnvironment) {
    let mut info = AlphaEnvironment {
        resource_count: 5,
        bootstrap_resource: 1,
        ..AlphaEnvironment::default()
    };
    info.capabilities.root_radix = 12;
    // Deliberately unrelated to A9N's init slots, with reversed leaf indices.
    let node = make_root_slot_descriptor(12, 117);
    for (index, (address, radix, device)) in [
        (0x100000, 20, false),    // root, with 512 KiB already reserved
        (0x100000000, 30, false), // kernel objects and initial metadata pool
        (0x80000000, 26, false),  // alpha heap
        (0x200000000, ram_radix, false),
        (0x90000000, 45, true), // deliberately overlaps RAM, as firmware maps do
    ]
    .into_iter()
    .enumerate()
    {
        info.resources[index] = MemoryResource {
            capability: make_child_slot_descriptor(node, 7, 127 - index),
            address,
            size_radix: radix as usize,
            flags: if device { RESOURCE_DEVICE } else { 0 },
            consumed_bytes: if index == 0 { 0x80000 } else { 0 },
        };
    }
    info.resources[3].consumed_bytes = ram_consumed;
    FAKE.with(|fake| {
        let mut fake = fake.borrow_mut();
        *fake = Fake::default();
        fake.caps.insert(
            0,
            Cap {
                kind: CapabilityType::Node,
                cursor: 0,
                end: 0,
                radix: 12,
                device: false,
            },
        );
        fake.caps.insert(
            node,
            Cap {
                kind: CapabilityType::Node,
                cursor: 0,
                end: 0,
                radix: 7,
                device: false,
            },
        );
        for generic in &info.resources[..5] {
            fake.caps.insert(
                generic.capability,
                Cap {
                    kind: CapabilityType::Generic,
                    cursor: generic.address + generic.consumed_bytes,
                    end: generic.address + (1 << generic.size_radix),
                    radix: generic.size_radix as usize,
                    device: generic.is_device(),
                },
            );
        }
    });
    let mut memory = MemoryManager::bootstrap(&info).unwrap();
    memory
        .prepare_bootstrap_frames(2, 0x80000000, 8192)
        .unwrap();
    if online {
        memory.initialize_physical_allocator().unwrap();
        memory
            .allocate_physical_at(0x80000000, 0x2000000, false)
            .unwrap();
    }
    (memory, info)
}
fn map(memory: &mut MemoryManager, address: usize, device: bool) -> usize {
    if device {
        memory
            .ensure_alpha_frames_for_range_from_initial_generic(address, 4096, true)
            .unwrap();
    } else {
        memory.allocate_physical_at(address, 4096, false).unwrap();
        memory
            .ensure_alpha_frame_at_physical_index(address >> 12)
            .unwrap();
    }
    let descriptor = memory
        .physical_frame_descriptor_from_index(address >> 12)
        .unwrap();
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert!(
            fake.frames.iter().any(|(&node, &(base, pages))| {
                let radix = fake.caps[&node].radix;
                address >= base
                    && address - base < pages * 4096
                    && make_child_slot_descriptor(node, radix, (address - base) >> 12) == descriptor
            }),
            "descriptor does not reference the requested physical frame"
        );
    });
    descriptor
}

#[test]
fn bootstrap_cost_is_independent_of_installed_ram() {
    let mut costs = Vec::new();
    for radix in [30, 33, 35, 40, 44] {
        let (memory, _) = boot(radix, false);
        costs.push(FAKE.with(|fake| {
            let fake = fake.borrow();
            (fake.calls, fake.nodes, fake.batches)
        }));
        assert!(memory
            .physical_frame_descriptor_from_index(0x80000000 >> 12)
            .is_some());
        assert!(memory
            .physical_frame_descriptor_from_index(0x200000000 >> 12)
            .is_none());
    }
    assert!(costs.iter().all(|cost| *cost == costs[0]));
    assert_eq!(
        costs[0].2, 1,
        "only the alpha heap is materialized before heap init"
    );
}

#[test]
fn subpage_boot_reservations_exclude_the_whole_page_from_allocation_and_frame_sources() {
    for consumed in [1, 128, 2048, 4095, 4097, 0x200080] {
        let (mut memory, _) = boot_with_reserved_prefix(33, true, consumed);
        let rounded = (consumed + 4095) & !4095;
        assert!(memory
            .allocate_physical_at(0x200000000, 4096, false)
            .is_err());
        assert!(memory
            .allocate_physical_at(0x200000000 + rounded - 4096, 4096, false)
            .is_err());
        assert!(memory
            .ensure_alpha_frame_at_physical_index((0x200000000 + rounded - 4096) >> 12)
            .is_err());
        map(&mut memory, 0x200000000 + rounded, false);
    }
}

#[test]
fn sparse_ram_and_high_mmio_leave_holes_unmaterialized() {
    let (mut memory, _) = boot(40, true);
    let before = FAKE.with(|fake| fake.borrow().batches);
    for address in [0x200000000, 0xf000000000, 0x208000000] {
        map(&mut memory, address, false);
    }
    for address in [0x800000000, 0xa1220000, 0xa1210000, 0x90000000, 0xfed00000] {
        map(&mut memory, address, true);
    }
    assert_eq!(FAKE.with(|fake| fake.borrow().batches) - before, 7);
    assert!(memory
        .physical_frame_descriptor_from_index(0x400000000 >> 12)
        .is_none());
}

#[test]
fn directory_and_metadata_pools_grow_beyond_old_fixed_limit() {
    let (mut memory, _) = boot(40, true);
    let free = memory.physical_memory_info().unwrap().free_pages;
    let mut first = 0;
    for index in 0..1100 {
        let address = 0x200000000 + index * 0x1000000;
        let descriptor = map(&mut memory, address, false);
        if index == 0 {
            first = descriptor;
        }
    }
    assert_eq!(
        memory.physical_frame_descriptor_from_index(0x200000000 >> 12),
        Some(first)
    );
    assert!(memory.physical_memory_info().unwrap().free_pages < free - 1100);
    assert_eq!(FAKE.with(|fake| fake.borrow().batches), 1101);
}

#[test]
fn process_arenas_grow_past_bootstrap_pool_and_reuse_root_slots() {
    let (mut memory, _) = boot(33, true);
    let before = memory.physical_memory_info().unwrap().free_pages;
    let mut descriptors = Vec::new();
    for slot in 0..100 {
        let descriptor = memory.ensure_process_arena(slot).unwrap();
        descriptors.push(descriptor);
        let start = FAKE.with(|fake| fake.borrow().caps[&descriptor].cursor);
        assert!(memory.allocate_physical_at(start, 4096, false).is_err());
        assert!(memory
            .ensure_alpha_frame_at_physical_index(start >> 12)
            .is_err());
        // Mix large arenas with frame-directory nodes in the refillable pool.
        map(&mut memory, 0x200000000 + slot * 0x1000000, false);
    }
    assert!(memory.physical_memory_info().unwrap().free_pages < before);
    let after = memory.physical_memory_info().unwrap().free_pages;
    for (slot, descriptor) in descriptors.into_iter().enumerate() {
        memory.reset_process_arena(slot).unwrap();
        assert_eq!(memory.ensure_process_arena(slot).unwrap(), descriptor);
    }
    assert_eq!(memory.physical_memory_info().unwrap().free_pages, after);
}

#[test]
fn large_process_frame_metadata_grows_lazily_and_reuses_its_backing_on_reap() {
    let (mut memory, _) = boot(33, true);
    let primary = memory.ensure_process_arena(10).unwrap();
    let directory = make_root_slot_descriptor(12, 2000);
    arch::generic::convert(
        memory.kernel_object_generic(),
        CapabilityType::Node,
        10,
        1,
        0,
        2000,
    )
    .unwrap();
    let mut expected = Vec::new();
    let mut plateau = None;
    for lifetime in 0..20 {
        for chunk in 0..100 {
            let source = memory.process_frame_arena(10, chunk, 9).unwrap();
            if chunk < 16 {
                assert_eq!(source, primary);
            } else {
                assert_ne!(source, primary);
            }
            arch::generic::convert(source, CapabilityType::Node, 9, 1, directory, chunk).unwrap();
            let descriptor = make_child_slot_descriptor(directory, 10, chunk);
            let address = FAKE.with_borrow(|fake| fake.caps[&descriptor].end);
            if lifetime == 0 {
                expected.push(address);
            } else {
                assert_eq!(address, expected[chunk]);
            }
        }
        let free = memory.physical_memory_info().unwrap().free_pages;
        assert_eq!(
            *plateau.get_or_insert(free),
            free,
            "metadata RAM leaked across root-slot reuse"
        );
        memory.reset_process_arena(10).unwrap();
    }
    assert!(memory.process_frame_arena(10, 0, usize::MAX).is_err());
}

#[test]
fn process_frame_metadata_revoke_keeps_other_processes_alive_and_reports_failure() {
    let (mut memory, _) = boot(33, true);
    memory.ensure_process_arena(10).unwrap();
    memory.ensure_process_arena(11).unwrap();
    let source = memory.process_frame_arena(10, 16, 9).unwrap();
    let peer = memory.process_frame_arena(11, 16, 9).unwrap();
    assert_ne!(source, peer);
    let directory = make_root_slot_descriptor(12, 2000);
    arch::generic::convert(
        memory.kernel_object_generic(),
        CapabilityType::Node,
        10,
        1,
        0,
        2000,
    )
    .unwrap();
    arch::generic::convert(peer, CapabilityType::Node, 9, 1, directory, 0).unwrap();
    let node = make_child_slot_descriptor(directory, 10, 0);
    memory.reset_process_arena(10).unwrap();
    FAKE.with_borrow(|fake| assert!(fake.caps.contains_key(&node)));
    FAKE.with_borrow_mut(|fake| fake.fail_revoke = true);
    assert!(memory.reset_process_arena(11).is_err());
    FAKE.with_borrow(|fake| assert!(fake.caps.contains_key(&node)));
    FAKE.with_borrow_mut(|fake| fake.fail_revoke = false);
    memory.reset_process_arena(11).unwrap();
    FAKE.with_borrow(|fake| assert!(!fake.caps.contains_key(&node)));
}

#[test]
fn fragmented_unaligned_sources_preserve_every_page() {
    let (mut memory, _) = boot(33, true);
    // The root's remainder starts at 1.5 MiB, not on an 8-MiB boundary.
    for page in (0x180000..0x200000).step_by(4096).rev() {
        map(&mut memory, page, false);
    }
    // Remaining heap source was delegated, not reserved with the heap.
    for address in [0x82000000, 0x83fff000] {
        map(&mut memory, address, false);
    }
}

#[test]
fn every_delegated_resource_prefix_is_excluded_from_ram_and_capability_allocation() {
    let (mut memory, _) = boot_with_reserved_prefix(33, true, 0x200000);
    for address in [0x200000000, 0x2001ff000] {
        assert!(memory.allocate_physical_at(address, 4096, false).is_err());
        assert!(memory
            .ensure_alpha_frames_for_range_from_initial_generic(address, 4096, false)
            .is_err());
    }
    map(&mut memory, 0x200200000, false);
    map(&mut memory, 0x200201000, false);
}

#[test]
fn prefix_split_failure_keeps_progress_for_retry_and_lower_addresses() {
    let (mut memory, _) = boot(40, true);
    FAKE.with(|fake| fake.borrow_mut().fail_kind = Some((CapabilityType::Generic, 3)));
    assert!(memory
        .ensure_alpha_frames_for_range_from_initial_generic(0x800000000, 4096, true)
        .is_err());
    for address in [0x800000000, 0x90000000, 0xa1220000, 0xfed00000] {
        map(&mut memory, address, true);
    }
}

#[test]
fn directory_node_failure_can_be_retried_without_losing_the_source() {
    let (mut memory, _) = boot(40, true);
    FAKE.with(|fake| fake.borrow_mut().fail_kind = Some((CapabilityType::Node, 1)));
    assert!(memory
        .ensure_alpha_frames_for_range_from_initial_generic(0x800000000, 4096, true)
        .is_err());
    map(&mut memory, 0x800000000, true);
}

#[test]
fn failed_frame_batch_is_not_converted_again() {
    let (mut memory, _) = boot(40, true);
    FAKE.with(|fake| fake.borrow_mut().fail_kind = Some((CapabilityType::Frame, 0)));
    assert!(memory
        .ensure_alpha_frames_for_range_from_initial_generic(0xa1220000, 4096, true)
        .is_err());
    let calls = FAKE.with(|fake| fake.borrow().calls);
    assert!(memory
        .ensure_alpha_frames_for_range_from_initial_generic(0xa1220000, 4096, true)
        .is_err());
    assert_eq!(FAKE.with(|fake| fake.borrow().calls), calls);
    map(&mut memory, 0x90000000, true);
}

#[test]
fn invalid_requests_do_not_allocate_metadata() {
    let (mut memory, _) = boot(40, true);
    let calls = FAKE.with(|fake| fake.borrow().calls);
    for (address, bytes) in [(0, 0), (usize::MAX - 1, 4096), (usize::MAX, usize::MAX)] {
        assert!(memory
            .ensure_alpha_frames_for_range_from_initial_generic(address, bytes, true)
            .is_err());
    }
    assert_eq!(FAKE.with(|fake| fake.borrow().calls), calls);
}

#[test]
fn mapping_flags_use_the_existing_attribute_word_and_keep_default_normal() {
    use capability_call::address_space::Attribute;
    struct Vm;
    impl nanami_core::vm_space::VmTracker for Vm {
        fn page_tables_ready(&self, _: usize) -> bool {
            true
        }
        fn record_frame(&mut self, _: usize, _: usize) -> Result<(), ()> {
            Ok(())
        }
        fn record_page_table(&mut self, _: usize, _: usize) -> Result<(), ()> {
            Ok(())
        }
    }
    let (mut memory, _) = boot(33, true);
    let mut vm = Vm;
    memory.map_frame(1, 2, 0x1000, &mut vm).unwrap();
    for mapping in [Attribute::NORMAL, Attribute::BUFFERED, Attribute::UNCACHED] {
        memory
            .map_frame_with_attribute(1, 2, 0x2000, Attribute::ALL | mapping, &mut vm)
            .unwrap();
    }
    memory.map_frame_strict(1, 2, 0x3000, &mut vm).unwrap();
    FAKE.with(|fake| assert_eq!(fake.borrow().map_attributes, [7, 7, 0x17, 0x27, 7]));
}

#[derive(Default)]
struct PageTableVm {
    slots: Vec<usize>,
}

impl nanami_core::vm_space::VmTracker for PageTableVm {
    fn page_tables_ready(&self, _: usize) -> bool {
        false
    }
    fn record_frame(&mut self, _: usize, _: usize) -> Result<(), ()> {
        Ok(())
    }
    fn record_page_table(&mut self, _: usize, slot: usize) -> Result<(), ()> {
        self.slots.push(slot);
        Ok(())
    }
}

fn destroy_test_address_space(
    memory: &mut MemoryManager,
    address_space: usize,
) -> CapabilityResult {
    // Model destroying the stopped PCB/root's last address-space alias first.
    FAKE.with_borrow_mut(|fake| {
        fake.tables
            .retain(|&(owner, _, _), _| owner != address_space)
    });
    memory.release_page_tables(address_space)
}

#[test]
fn page_tables_reuse_backing_pages_and_slots_across_process_exit_and_exec() {
    let (mut memory, _) = boot(33, true);
    let mut live = PageTableVm::default();
    memory.ensure_page_tables(1, 0x1000, &mut live).unwrap();
    let permanent: Vec<_> = FAKE.with_borrow(|fake| fake.tables.values().copied().collect());
    let mut plateau = None;
    // 2,000 address-space lifetimes and 8,000 tables exceed the old 1,024
    // lifetime allocations. Reuse both identical (exec) and new descriptors.
    for cycle in 0..1000 {
        for address_space in [2, cycle + 3] {
            let mut vm = PageTableVm::default();
            memory
                .ensure_page_tables(address_space, 0x1000, &mut vm)
                .unwrap();
            memory
                .ensure_page_tables(address_space, 0x40_0000, &mut vm)
                .unwrap();
            assert_eq!(vm.slots.len(), 4);
            destroy_test_address_space(&mut memory, address_space).unwrap();
        }
        let cursor = FAKE.with_borrow(|fake| fake.caps[&memory.kernel_object_generic()].cursor);
        assert_eq!(*plateau.get_or_insert(cursor), cursor, "backing RAM leaked");
        memory.ensure_page_tables(1, 0x1000, &mut live).unwrap();
        FAKE.with_borrow(|fake| {
            assert_eq!(fake.tables.values().copied().collect::<Vec<_>>(), permanent);
        });
    }
}

#[test]
fn exhausted_page_table_pool_recovers_without_reclaiming_live_tables() {
    let (mut memory, _) = boot(33, true);
    memory
        .ensure_page_tables(1, 0, &mut PageTableVm::default())
        .unwrap();
    let mut vm = PageTableVm::default();
    for region in 0..1018 {
        memory.ensure_page_tables(2, region << 21, &mut vm).unwrap();
    }
    assert_eq!(
        memory.ensure_page_tables(2, 1018 << 21, &mut vm),
        Err(CapabilityError::InvalidArgument)
    );
    destroy_test_address_space(&mut memory, 2).unwrap();
    memory
        .ensure_page_tables(3, 0, &mut PageTableVm::default())
        .unwrap();
    memory
        .ensure_page_tables(1, 0, &mut PageTableVm::default())
        .unwrap();
}

#[test]
fn page_table_conversion_and_revoke_failures_do_not_recycle_owned_pages() {
    let (mut memory, _) = boot(33, true);
    FAKE.with_borrow_mut(|fake| fake.fail_kind = Some((CapabilityType::PageTable, 1)));
    assert_eq!(
        memory.ensure_page_tables(1, 0, &mut PageTableVm::default()),
        Err(CapabilityError::Fatal)
    );
    FAKE.with_borrow_mut(|fake| fake.fail_revoke = true);
    assert!(destroy_test_address_space(&mut memory, 1).is_err());
    let mut vm = PageTableVm::default();
    memory.ensure_page_tables(2, 0, &mut vm).unwrap();
    assert!(vm.slots.iter().all(|&slot| slot >= 2));
    FAKE.with_borrow_mut(|fake| fake.fail_revoke = false);
    destroy_test_address_space(&mut memory, 1).unwrap();
    // The other, still-live address space must not have lost any tables.
    memory.ensure_page_tables(2, 0, &mut vm).unwrap();
    destroy_test_address_space(&mut memory, 2).unwrap();
}

#[test]
fn bootstrap_page_tables_remain_permanent_without_heap_owned_metadata() {
    let (mut memory, _) = boot(33, false);
    let mut vm = PageTableVm::default();
    memory.ensure_page_tables(1, 0, &mut vm).unwrap();
    // Bootstrap Alpha tables are not owned by a process teardown path.
    memory.release_page_tables(1).unwrap();
    assert_eq!(vm.slots.len(), 3);
    memory.initialize_physical_allocator().unwrap();
    memory.ensure_page_tables(1, 0, &mut vm).unwrap();
    assert_eq!(vm.slots.len(), 3);
}

#[test]
fn real_generic_partition_excludes_parent_allocation_and_can_be_revoked_and_reused() {
    let (mut memory, _) = boot(33, true);
    let before = memory.physical_memory_info().unwrap().free_pages;
    let id = memory.reserve_alpha_memory(29).unwrap();
    let info = memory.delegation_info(id).unwrap();
    assert_eq!(info.bytes, 1 << 29);
    assert_eq!(info.address % info.bytes, 0);
    assert_eq!(
        before - memory.physical_memory_info().unwrap().free_pages,
        info.bytes / 4096
    );
    assert!(memory
        .allocate_physical_at(info.address, 4096, false)
        .is_err());
    assert!(memory.free_physical(info.address, info.bytes).is_err());
    assert!(memory
        .ensure_alpha_frame_at_physical_index(info.address / 4096)
        .is_err());
    let resources = memory.take_delegated_resources(id).unwrap();
    assert!(memory.take_delegated_resources(id).is_err());
    for (half, resource) in resources.iter().enumerate() {
        assert_eq!(resource.address, info.address + half * (info.bytes / 2));
        assert_eq!(resource.size_bytes(), Some(info.bytes / 2));
        assert!(!resource.is_device());
        FAKE.with_borrow(|fake| {
            let cap = fake.caps[&resource.capability];
            assert_eq!(cap.cursor, resource.address);
            assert_eq!(cap.end, resource.address + info.bytes / 2);
        });
    }
    // No process or external mapping was created from this test partition.
    unsafe {
        memory.reclaim_delegation(id).unwrap();
    }
    assert_eq!(
        memory.delegation_info(id).unwrap().state,
        memory::DelegationState::Reclaimed
    );
    assert_eq!(memory.physical_memory_info().unwrap().free_pages, before);
    assert!(unsafe { memory.reclaim_delegation(id) }.is_err());
    FAKE.with_borrow(|fake| {
        assert!(resources
            .iter()
            .all(|resource| !fake.caps.contains_key(&resource.capability)));
    });
    map(&mut memory, info.address, false);
}

#[test]
fn failed_revoke_never_releases_the_partition() {
    let (mut memory, _) = boot(33, true);
    let id = memory.reserve_alpha_memory(29).unwrap();
    let info = memory.delegation_info(id).unwrap();
    let free = memory.physical_memory_info().unwrap().free_pages;
    FAKE.with_borrow_mut(|fake| fake.fail_revoke = true);
    assert!(unsafe { memory.reclaim_delegation(id) }.is_err());
    assert_eq!(memory.physical_memory_info().unwrap().free_pages, free);
    assert_eq!(
        memory.delegation_info(id).unwrap().state,
        memory::DelegationState::Prepared
    );
    assert!(memory
        .allocate_physical_at(info.address, 4096, false)
        .is_err());
}

#[test]
fn child_bootstrap_preserves_a_whole_generic_for_recursive_delegation() {
    let (mut parent, _) = boot(33, true);
    let child_id = parent.reserve_alpha_memory(30).unwrap();
    let child_info = parent.delegation_info(child_id).unwrap();
    let mut resources = parent.take_delegated_resources(child_id).unwrap();
    assert_eq!(resources.len(), 3);
    let mut address = child_info.address;
    for (resource, radix) in resources.iter().zip([28, 28, 29]) {
        assert_eq!(resource.address, address);
        assert_eq!(resource.size_radix, radix);
        address += 1 << radix;
    }
    assert_eq!(address, child_info.address + child_info.bytes);

    // Descriptors are receiver-local. Switch the syscall mock to the child's
    // namespace rather than treating a parent root descriptor as a global ID.
    FAKE.with_borrow_mut(|fake| {
        let caps: Vec<_> = resources
            .iter()
            .map(|resource| fake.caps[&resource.capability])
            .collect();
        *fake = Fake::default();
        fake.caps.insert(
            0,
            Cap {
                kind: CapabilityType::Node,
                cursor: 0,
                end: 0,
                radix: 12,
                device: false,
            },
        );
        let node = make_root_slot_descriptor(12, 117);
        fake.caps.insert(
            node,
            Cap {
                kind: CapabilityType::Node,
                cursor: 0,
                end: 0,
                radix: 7,
                device: false,
            },
        );
        for (index, (resource, cap)) in resources.iter_mut().zip(caps).enumerate() {
            resource.capability = make_child_slot_descriptor(node, 7, 127 - index);
            fake.caps.insert(resource.capability, cap);
        }
    });
    let mut environment = AlphaEnvironment {
        resource_count: resources.len(),
        bootstrap_resource: 0,
        ..AlphaEnvironment::default()
    };
    environment.capabilities.root_radix = 12;
    environment.resources[..resources.len()].copy_from_slice(&resources);
    let mut child = MemoryManager::bootstrap(&environment).unwrap();
    // Exercise the same two bootstrap consumers as a live Alpha: object pools
    // in resource 0 and the 32-MiB heap in the smallest non-bootstrap extent.
    child
        .prepare_bootstrap_frames(1, resources[1].address, 8192)
        .unwrap();
    child.initialize_physical_allocator().unwrap();
    child
        .allocate_physical_at(resources[1].address, 1 << 25, false)
        .unwrap();
    let free = child.physical_memory_info().unwrap().free_pages;
    let grandchild_id = child.reserve_alpha_memory(29).unwrap();
    let grandchild = child.delegation_info(grandchild_id).unwrap();
    assert_eq!(grandchild.address, resources[2].address);
    assert_eq!(grandchild.bytes, 1 << 29);
    assert_eq!(
        free - child.physical_memory_info().unwrap().free_pages,
        (1 << 29) / 4096
    );
    assert!(child
        .allocate_physical_at(grandchild.address, 4096, false)
        .is_err());
    assert!(parent
        .allocate_physical_at(grandchild.address, 4096, false)
        .is_err());
    assert!(child
        .free_physical(grandchild.address, grandchild.bytes)
        .is_err());
    let free = child.physical_memory_info().unwrap().free_pages;
    assert!(child.reserve_alpha_memory(29).is_err());
    assert_eq!(child.physical_memory_info().unwrap().free_pages, free);
}

#[test]
fn failed_partition_preparation_is_accounted_and_quarantined() {
    let (mut memory, _) = boot(33, true);
    let before = memory.physical_memory_info().unwrap().free_pages;
    FAKE.with_borrow_mut(|fake| fake.fail_kind = Some((CapabilityType::Generic, 0)));
    assert!(memory.reserve_alpha_memory(29).is_err());
    let records: Vec<_> = memory.delegation_records().collect();
    assert_eq!(records.len(), 1);
    let (id, info) = records[0];
    assert_eq!(info.state, memory::DelegationState::Reserved);
    assert_eq!(
        before - memory.physical_memory_info().unwrap().free_pages,
        info.bytes / 4096
    );
    assert!(memory.free_physical(info.address, info.bytes).is_err());
    assert!(memory.take_delegated_resources(id).is_err());
}

#[test]
fn aligned_partition_search_uses_free_ram_not_device_or_materialized_frames() {
    let (mut memory, _) = boot(33, true);
    let before = memory.physical_memory_info().unwrap().free_pages;
    for radix in [0, 12, 28, usize::BITS as usize, usize::MAX] {
        assert!(memory.reserve_alpha_memory(radix).is_err());
    }
    assert_eq!(memory.physical_memory_info().unwrap().free_pages, before);
    let first = memory.reserve_alpha_memory(29).unwrap();
    let first = memory.delegation_info(first).unwrap();
    let second = memory.reserve_alpha_memory(29).unwrap();
    let second = memory.delegation_info(second).unwrap();
    assert!(
        first.address + first.bytes <= second.address
            || second.address + second.bytes <= first.address
    );
    for range in [first, second] {
        assert!(
            (0x200000000..0x400000000).contains(&range.address)
                || (0x100000000..0x140000000).contains(&range.address)
        );
    }
}

#[test]
fn partition_exhaustion_is_bounded_and_does_not_fall_back_to_device_authority() {
    let (mut memory, _) = boot(33, true);
    let mut partitions = 0;
    for _ in 0..32 {
        if memory.reserve_alpha_memory(29).is_err() {
            break;
        }
        partitions += 1;
    }
    assert!(partitions > 1 && partitions < 32);
    let free = memory.physical_memory_info().unwrap().free_pages;
    let records = memory.delegation_records().count();
    for _ in 0..4 {
        assert!(memory.reserve_alpha_memory(29).is_err());
        assert_eq!(memory.physical_memory_info().unwrap().free_pages, free);
        assert_eq!(memory.delegation_records().count(), records);
    }
}
