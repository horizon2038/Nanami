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
#[derive(Clone, Copy, Default)]
pub struct GenericDescriptor {
    address: usize,
    size_radix: u8,
    is_device: bool,
}
pub struct InitInfo {
    generic_list_count: usize,
    generic_list: [GenericDescriptor; 128],
}
pub enum InitSlotOffset {
    GenericNode = 6,
}
pub mod capability_call {
    pub mod address_space {
        #[derive(Clone, Copy)]
        pub struct Attribute;
        impl Attribute {
            pub const ALL: Self = Self;
        }
    }
}
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
                    assert_eq!(start, original.cursor, "untyped alignment prefix lost");
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
        pub fn revoke(_: usize, _: usize) -> CapabilityResult {
            panic!("live source revoked")
        }
    }
    pub mod address_space {
        use super::super::*;
        pub fn map(
            _: usize,
            _: usize,
            _: usize,
            _: capability_call::address_space::Attribute,
        ) -> CapabilityResult {
            Ok(())
        }
        pub fn get_unset_depth(_: usize, _: usize, _: usize) -> Result<usize, CapabilityError> {
            Ok(0)
        }
    }
}

fn boot(ram_radix: u8, online: bool) -> (MemoryManager, InitInfo) {
    let mut info = InitInfo {
        generic_list_count: 5,
        generic_list: [GenericDescriptor::default(); 128],
    };
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
        info.generic_list[index] = GenericDescriptor {
            address,
            size_radix: radix,
            is_device: device,
        };
    }
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
        let node = make_root_slot_descriptor(12, InitSlotOffset::GenericNode as usize);
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
        for (index, generic) in info.generic_list[..5].iter().enumerate() {
            fake.caps.insert(
                make_child_slot_descriptor(node, 7, index),
                Cap {
                    kind: CapabilityType::Generic,
                    cursor: generic.address + if index == 0 { 0x80000 } else { 0 },
                    end: generic.address + (1 << generic.size_radix),
                    radix: generic.size_radix as usize,
                    device: generic.is_device,
                },
            );
        }
    });
    let mut memory = MemoryManager::bootstrap(&info, 0, 12, 1, 0, 0x80000).unwrap();
    memory
        .prepare_bootstrap_frames(2, 0x80000000, 8192)
        .unwrap();
    if online {
        memory.initialize_physical_allocator(&info).unwrap();
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
