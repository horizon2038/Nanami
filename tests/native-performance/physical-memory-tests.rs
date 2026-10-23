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
                    _ => radix,
                };
                let bytes = 1usize << bits;
                let start = (original.cursor + bytes - 1) & !(bytes - 1);
                let end = start + bytes * count;
                assert!(end <= original.end, "generic exhausted");
                if kind != CapabilityType::Node {
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
            _: usize,
            _: usize,
            _: usize,
            attribute: capability_call::address_space::Attribute,
        ) -> CapabilityResult {
            FAKE.with(|fake| fake.borrow_mut().map_attributes.push(attribute.bits()));
            Ok(())
        }
        pub fn get_unset_depth(_: usize, _: usize, _: usize) -> Result<usize, CapabilityError> {
            Ok(0)
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
