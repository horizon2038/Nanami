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
            fn record_bootstrap_frame(&mut self, address: usize, frame: usize, leaf_bits: usize) -> Result<(), ()> {
                if leaf_bits == 12 { self.record_frame(address, frame) } else { Err(()) }
            }
            fn page_tables_ready(&self, address: usize) -> bool;
            fn record_frame(&mut self, address: usize, descriptor: usize) -> Result<(), ()>;
            fn record_page_table(&mut self, address: usize, slot: usize) -> Result<(), ()>;
        }
    }
}
#[path = "../../nanami/src/nanami_core"]
mod production {
    pub mod memory;

    pub fn pool_frame(memory: &memory::MemoryManager, frame: usize) -> bool {
        memory.is_pool_frame_descriptor(frame)
    }
}
use descriptor::{make_child_slot_descriptor, make_root_slot_descriptor};
use memory::MemoryManager;
use production::memory;

#[test]
fn reserved_large_initialization_keeps_one_reservation_and_exports_only_small_frames_after_demotion() {
    let (mut memory, _) = boot(30, true);
    let address = 0x200400000;
    let first = address / 4096;
    let before = memory.physical_memory_info().unwrap().free_pages;
    memory.allocate_physical_at(address - 4096, (2 << 20) + 8192, false).unwrap();
    let reserved = memory.physical_memory_info().unwrap().free_pages;
    let allocation = memory.prepare_reserved_large_frame(first).unwrap().unwrap();
    assert!(allocation.large);
    assert_eq!(allocation.first, first);
    assert_eq!(memory.physical_memory_info().unwrap().free_pages, reserved);
    memory.ensure_alpha_frames_for_allocation(first - 1, 1).unwrap();
    let key = memory.physical_frame_descriptor_from_index(first).unwrap();
    let maps = FAKE.with_borrow(|f| f.frame_maps.len());
    assert_eq!(memory.map_private_large_alias(key, None).unwrap().unwrap().1, 512);
    FAKE.with_borrow(|f| assert_eq!(f.frame_maps[maps..].iter().map(|&(_,bits)|bits).collect::<Vec<_>>(), vec![21]));
    memory.demote_private_large_pool(first).unwrap();
    for page in first..first + 512 {
        let frame = memory.physical_frame_descriptor_from_index(page).unwrap();
        FAKE.with_borrow(|f| {
            let (&node, &(base, count)) = f.frames.iter().find(|(_, (base, count))|
                page * 4096 >= *base && page * 4096 - *base < count * 4096).unwrap();
            assert_eq!(count, 512);
            assert_eq!(frame, make_child_slot_descriptor(node, f.caps[&node].radix, page - base / 4096));
            assert!(!f.caps.contains_key(&frame)); // 4K caps are modeled as batches.
        });
        assert!(production::pool_frame(&memory, frame));
    }
    memory.free_physical(address - 4096, (2 << 20) + 8192).unwrap();
    assert_eq!(memory.physical_memory_info().unwrap().free_pages, before);
}

#[test]
fn reserved_large_initialization_preserves_warm_frames_and_handles_unsupported_huge_conversion() {
    let (mut memory, _) = boot(30, true);
    let address = 0x200400000;
    let first = address / 4096;
    memory.allocate_physical_at(address, 2 << 20, false).unwrap();
    memory.ensure_alpha_frames_for_allocation(first, 512).unwrap();
    let frame = memory.physical_frame_descriptor_from_index(first).unwrap();
    let calls = FAKE.with_borrow(|f| f.calls);
    assert!(memory.prepare_reserved_large_frame(first).unwrap().is_none());
    assert_eq!(FAKE.with_borrow(|f| f.calls), calls);
    assert_eq!(memory.physical_frame_descriptor_from_index(first), Some(frame));
    memory.free_physical(address, 2 << 20).unwrap();

    let (mut memory, _) = boot(30, true);
    memory.allocate_physical_at(address, 2 << 20, false).unwrap();
    FAKE.with_borrow_mut(|f| f.huge_failure = Some((1, CapabilityError::InvalidDepth)));
    assert!(!memory.prepare_reserved_large_frame(first).unwrap().unwrap().large);
    for page in first..first + 512 {
        let frame = memory.physical_frame_descriptor_from_index(page).unwrap();
        FAKE.with_borrow(|f| {
            let (&node, &(base, count)) = f.frames.iter().find(|(_, (base, count))|
                page * 4096 >= *base && page * 4096 - *base < count * 4096).unwrap();
            assert_eq!(count, 512);
            assert_eq!(frame, make_child_slot_descriptor(node, f.caps[&node].radix, page - base / 4096));
            assert!(!f.caps.contains_key(&frame));
        });
    }
    memory.free_physical(address, 2 << 20).unwrap();
}

#[test]
fn interior_large_preparation_failure_prevents_releasing_the_containing_owner() {
    for kind in [CapabilityType::Node, CapabilityType::Generic] {
        let (mut memory, _) = boot(30, true);
        let address = 0x200400000;
        memory.allocate_physical_at(address - 4096, (2 << 20) + 8192, false).unwrap();
        let free = memory.physical_memory_info().unwrap().free_pages;
        FAKE.with_borrow_mut(|f| f.fail_kind = Some((kind, 0)));
        assert!(memory.prepare_reserved_large_frame(address / 4096).is_err());
        assert_eq!(memory.free_physical(address - 4096, (2 << 20) + 8192), Err(CapabilityError::IllegalOperation));
        assert_eq!(memory.free_physical(address + 4096, 4096), Err(CapabilityError::IllegalOperation));
        assert_eq!(memory.physical_memory_info().unwrap().free_pages, free);
    }
}

#[test]
fn failed_temporary_huge_unmap_retains_the_entire_owned_reservation_until_ack() {
    let (mut memory, _) = boot(30, true);
    let address = 0x200400000;
    memory.allocate_physical_at(address - 4096, (2 << 20) + 8192, false).unwrap();
    memory.prepare_reserved_large_frame(address / 4096).unwrap().unwrap();
    let key = memory.physical_frame_descriptor_from_index(address / 4096).unwrap();
    memory.map_private_large_alias(key, None).unwrap().unwrap();
    let free = memory.physical_memory_info().unwrap().free_pages;
    FAKE.with_borrow_mut(|f| f.fail_unmap = true);
    assert!(memory.free_physical(address - 4096, (2 << 20) + 8192).is_err());
    assert_eq!(memory.physical_memory_info().unwrap().free_pages, free);
    FAKE.with_borrow_mut(|f| f.fail_unmap = false);
    memory.free_physical(address - 4096, (2 << 20) + 8192).unwrap();
    assert_eq!(memory.physical_memory_info().unwrap().free_pages, free + 514);
}

#[test]
fn namespace_checks_use_prefix_and_depth_not_rights_or_numeric_nearness() {
    use descriptor::is_within_node_namespace as within;
    let pool = make_root_slot_descriptor(12, 1024);
    let other = make_root_slot_descriptor(12, 1025);
    for index in [0, 1, 127, 255] {
        let node = make_child_slot_descriptor(pool, 8, index);
        let frame = make_child_slot_descriptor(node, 11, 2047);
        assert!(within(node, pool));
        assert!(within(frame, pool));
        assert!(within(frame, node));
        assert!(!within(frame, other));
        assert!(!within(pool, node));
    }
    assert!(!within(pool, pool));
    assert!(!within(usize::MAX, pool));
    assert!(!within(pool, usize::MAX));
    for slot in [200, 999, 1023, 1025, 2000] {
        let root = make_root_slot_descriptor(12, slot);
        let child_frame = make_child_slot_descriptor(root, 9, 3);
        assert!(!within(child_frame, pool));
    }
}

#[test]
fn private_large_frame_words_match_every_scalar_key_with_one_range_probe_per_word() {
    let (mut memory, _) = boot(30, true);
    let allocation = memory.allocate_private_large_frame().unwrap().unwrap();
    let before = memory.frame_range_probe_count();
    let expected: Vec<_> = (0..512).map(|page|
        memory.physical_frame_descriptor_from_index(allocation.first + page).unwrap()).collect();
    let scalar_probes = memory.frame_range_probe_count() - before;
    let before = memory.frame_range_probe_count();
    let mut actual = [0;512];
    for (word, frames) in actual.chunks_mut(64).enumerate() {
        memory.read_private_large_frames(allocation.first + word * 64, frames).unwrap();
    }
    let word_probes = memory.frame_range_probe_count() - before;
    assert_eq!(actual.as_slice(), expected);
    println!("HUGE-FRAME-KEYS pages=512 range_probes_before={scalar_probes} range_probes_after={word_probes}");
    assert_eq!((scalar_probes,word_probes),(512,8));
    for page in [0,1,63,64,127,511] {
        let mut key=[0];memory.read_private_large_frames(allocation.first+page,&mut key).unwrap();
        assert_eq!(key[0],expected[page]);
    }
    for (first,count) in [(allocation.first+511,2),(usize::MAX,1),(usize::MAX/4096,2)] {
        let mut output=vec![0xaa;count];
        assert!(memory.read_private_large_frames(first,&mut output).is_err());
        assert!(output.iter().all(|&key|key==0xaa));
    }
    memory.demote_private_large_pool(allocation.first).unwrap();
    let mut output=[0xaa;64];
    assert!(memory.read_private_large_frames(allocation.first,&mut output).is_err());
    assert_eq!(output,[0xaa;64]);
}

#[test]
fn private_large_slices_cannot_be_kernel_capabilities_and_alias_exact_subpages() {
    let (mut memory, _) = boot(30, true);
    let before = memory.physical_memory_info().unwrap().free_pages;
    let allocation = memory.allocate_private_large_frame().unwrap().unwrap();
    assert!(allocation.large);
    assert_eq!(allocation.first & 511, 0);
    assert_eq!(memory.physical_memory_info().unwrap().free_pages, before - 512);
    let first = memory.physical_frame_descriptor_from_index(allocation.first).unwrap();
    let last = memory.physical_frame_descriptor_from_index(allocation.first + 511).unwrap();
    assert!(descriptor::descriptor_depth(first) > WORD_BITS);
    assert!(descriptor::descriptor_depth(last) > WORD_BITS);
    memory.map_private_large_frame(allocation.first, 500, 0x20000000, 3).unwrap();
    assert_eq!(memory.ensure_alpha_frame_at_physical_index(allocation.first), Err(CapabilityError::IllegalOperation));
    let mut vm = HeapVm::default();
    assert_eq!(memory.map_frame_strict(500, first, 0x20000000, &mut vm), Err(CapabilityError::InvalidArgument));
    let (low, count) = memory.map_private_large_alias(first, None).unwrap().unwrap();
    let (high, tail) = memory.map_private_large_alias(last, None).unwrap().unwrap();
    assert_eq!((count, tail, high - low), (512, 1, 511 * 4096));
    assert_eq!(FAKE.with_borrow(|f| f.frame_maps.len()), 2, "every subpage remapped the huge alias");
    let second = memory.allocate_private_large_frame().unwrap().unwrap();
    let key = memory.physical_frame_descriptor_from_index(second.first).unwrap();
    let (other, _) = memory.map_private_large_alias(key, Some(high)).unwrap().unwrap();
    assert_eq!(other - low, 1 << 21);
    assert_eq!(memory.map_private_large_alias(0x1234, None), Ok(None));
    assert!(memory.map_private_large_alias(first ^ (1 << 30), None).is_err());
}

#[test]
fn private_large_demotes_same_ram_only_after_guest_and_alias_unmap_ack() {
    let (mut memory, _) = boot(30, true);
    let allocation = memory.allocate_private_large_frame().unwrap().unwrap();
    let key = memory.physical_frame_descriptor_from_index(allocation.first).unwrap();
    memory.map_private_large_alias(key, None).unwrap();
    memory.map_private_large_frame(allocation.first, 500, 0x20000000, 3).unwrap();
    assert_eq!(memory.free_physical(allocation.first * 4096, 2 << 20), Err(CapabilityError::IllegalOperation));
    assert_eq!(memory.demote_private_large_pool(allocation.first), Err(CapabilityError::IllegalOperation));
    memory.unmap_private_large_frame(allocation.first).unwrap();
    let revokes = FAKE.with_borrow(|f| f.heap_events.iter().filter(|&&x| x == "revoke").count());
    FAKE.with_borrow_mut(|f| f.fail_unmap = true);
    assert_eq!(memory.demote_private_large_pool(allocation.first), Err(CapabilityError::Fatal));
    assert_eq!(FAKE.with_borrow(|f| f.heap_events.iter().filter(|&&x| x == "revoke").count()), revokes);
    FAKE.with_borrow_mut(|f| f.fail_unmap = false);
    memory.demote_private_large_pool(allocation.first).unwrap();
    assert!(memory.private_large_mapping(500, 0x20000000, 0x20200000).unwrap().small);
    for page in 0..512 {
        let frame = memory.physical_frame_descriptor_from_index(allocation.first + page).unwrap();
        assert!(descriptor::descriptor_depth(frame) <= WORD_BITS);
        memory.ensure_alpha_frame_at_physical_index(allocation.first + page).unwrap();
    }
    memory.finish_private_large_split(allocation.first).unwrap();
    memory.free_physical(allocation.first * 4096, 2 << 20).unwrap();
}

#[test]
fn whole_large_protection_keeps_the_capability_ram_and_only_maps_large_leaves() {
    let (mut memory, _) = boot(30, true);
    let allocation = memory.allocate_private_large_frame().unwrap().unwrap();
    memory.map_private_large_frame(allocation.first, 500, 0x20000000, 3).unwrap();
    let key = memory.physical_frame_descriptor_from_index(allocation.first + 511).unwrap();
    let free = memory.physical_memory_info().unwrap().free_pages;
    let initial = FAKE.with_borrow_mut(|f| { f.heap_events.clear(); f.frame_maps.len() });
    for rights in [1, 0, 3, 5, 2, 7, 4, 3] {
        memory.protect_private_large_frame(allocation.first, rights).unwrap();
        assert_eq!(memory.private_large_protection(500, 0x20000000).unwrap().permissions, rights);
        assert_eq!(memory.physical_frame_descriptor_from_index(allocation.first + 511), Some(key));
        assert_eq!(memory.physical_memory_info().unwrap().free_pages, free);
    }
    FAKE.with_borrow(|f| {
        assert_eq!(f.frame_maps.len() - initial, 7);
        assert!(f.frame_maps[initial..].iter().all(|&(_, bits)| bits == 21));
        assert_eq!(f.heap_events.iter().filter(|&&event| event == "unmap").count(), 7);
        assert!(!f.heap_events.contains(&"revoke"));
        assert!(!f.heap_events.contains(&"large-attempt"));
    });
}

#[test]
fn inaccessible_large_backing_defers_all_guest_tables_until_first_visible_protection() {
    let (mut memory, _) = boot(30, true);
    let allocation = memory.allocate_private_large_frame().unwrap().unwrap();
    let free = memory.physical_memory_info().unwrap().free_pages;
    let before = FAKE.with_borrow(|f| (f.calls, f.frame_maps.len(), f.tables.len()));
    memory.map_private_large_frame(allocation.first, 500, 0x20000000, 0).unwrap();
    assert_eq!(FAKE.with_borrow(|f| (f.calls, f.frame_maps.len(), f.tables.len())), before);
    assert_eq!(memory.private_large_protection(500, 0x20000000).unwrap().permissions, 0);
    memory.protect_private_large_frame(allocation.first, 3).unwrap();
    FAKE.with_borrow(|f| {
        assert_eq!(&f.frame_maps[before.1..], &[(0x20000000, 21)]);
        assert!(f.tables.contains_key(&(500, 0x20000000 >> 30, 2)));
        assert!(f.tables.contains_key(&(500, 0x20000000 >> 39, 3)));
        assert!(!f.tables.contains_key(&(500, 0x20000000 >> 21, 1)));
        assert!(!f.map_attributes.contains(&0));
    });
    // Page tables use their existing reserved pool; the huge RAM stays owned.
    assert_eq!(memory.physical_memory_info().unwrap().free_pages, free);
}

#[test]
fn large_protection_map_failures_restore_prior_rights_or_absence_and_quarantine_failed_repairs() {
    for old in [0, 3] {
        let (mut memory, _) = boot(30, true);
        let allocation = memory.allocate_private_large_frame().unwrap().unwrap();
        memory.map_private_large_frame(allocation.first, 500, 0x20000000, 3).unwrap();
        if old == 0 { memory.protect_private_large_frame(allocation.first, 0).unwrap(); }
        let free = memory.physical_memory_info().unwrap().free_pages;
        FAKE.with_borrow_mut(|f| { f.fail_frame_map_at = Some((f.frame_maps.len() + 1, true)); });
        assert!(memory.protect_private_large_frame(allocation.first, 1).is_err());
        assert_eq!(memory.private_large_protection(500, 0x20000000).unwrap().permissions, old);
        assert_eq!(memory.physical_memory_info().unwrap().free_pages, free);
        FAKE.with_borrow(|f| { assert!(!f.map_attributes.contains(&0)); });
    }
    for unmap in [false, true] {
        let (mut memory, _) = boot(30, true);
        let allocation = memory.allocate_private_large_frame().unwrap().unwrap();
        memory.map_private_large_frame(allocation.first, 500, 0x20000000, 3).unwrap();
        FAKE.with_borrow_mut(|f| {
            if unmap { f.fail_unmap = true; }
            else { f.fail_frame_map_at = Some((f.frame_maps.len() + 1, false)); }
        });
        assert!(memory.protect_private_large_frame(allocation.first, 1).is_err());
        assert!(memory.private_large_protection(500, 0x20000000).is_none());
        assert!(memory.private_large_mapping(500, 0x20000000, 0x20200000).unwrap().quarantined);
        assert!(memory.free_physical(allocation.first * 4096, 1 << 21).is_err());
    }
}

#[test]
fn first_partial_protection_can_split_an_inaccessible_huge_frame_without_upper_tables() {
    let (mut memory, _) = boot(30, true);
    let allocation = memory.allocate_private_large_frame().unwrap().unwrap();
    let base = 0x20000000;
    memory.map_private_large_frame(allocation.first, 500, base, 0).unwrap();
    assert_eq!(arch::address_space::get_unset_depth(500, base, 12).unwrap(), 3);
    memory.demote_private_large_pool(allocation.first).unwrap();
    memory.ensure_private_large_split_table(allocation.first, &mut HeapVm::default()).unwrap();
    assert_eq!(arch::address_space::get_unset_depth(500, base, 12).unwrap(), 0);
    memory.restore_private_large_frame(allocation.first).unwrap();
    assert_eq!(arch::address_space::get_unset_depth(500, base, 12).unwrap(), 1);
    assert_eq!(memory.private_large_protection(500, base).unwrap().permissions, 0);
    memory.protect_private_large_frame(allocation.first, 3).unwrap();
    assert_eq!(memory.private_large_protection(500, base).unwrap().permissions, 3);
}

#[test]
fn restoring_a_zero_rights_large_frame_unlinks_small_table_without_mapping_zero_attributes() {
    let (mut memory, _) = boot(30, true);
    let allocation = memory.allocate_private_large_frame().unwrap().unwrap();
    memory.map_private_large_frame(allocation.first, 500, 0x20000000, 3).unwrap();
    memory.protect_private_large_frame(allocation.first, 0).unwrap();
    memory.demote_private_large_pool(allocation.first).unwrap();
    memory.ensure_private_large_split_table(allocation.first, &mut HeapVm::default()).unwrap();
    let before = FAKE.with_borrow(|f| f.frame_maps.len());
    memory.restore_private_large_frame(allocation.first).unwrap();
    assert_eq!(memory.private_large_protection(500, 0x20000000).unwrap().permissions, 0);
    FAKE.with_borrow(|f| {
        assert_eq!(f.frame_maps.len(), before);
        assert!(!f.map_attributes.contains(&0));
        assert!(!f.tables.contains_key(&(500, 0x20000000 >> 21, 1)));
    });
    memory.protect_private_large_frame(allocation.first, 3).unwrap();
}

#[test]
fn private_large_unsupported_partial_conversion_reuses_the_reserved_ram_as_small() {
    let (mut memory, _) = boot(30, true);
    let before = memory.physical_memory_info().unwrap().free_pages;
    FAKE.with_borrow_mut(|f| f.huge_failure = Some((1, CapabilityError::InvalidDepth)));
    let allocation = memory.allocate_private_large_frame().unwrap().unwrap();
    assert!(!allocation.large);
    assert_eq!(memory.physical_memory_info().unwrap().free_pages, before - 512);
    for page in 0..512 {
        let frame = memory.physical_frame_descriptor_from_index(allocation.first + page).unwrap();
        assert!(descriptor::descriptor_depth(frame) <= WORD_BITS);
    }
    assert!(memory.allocate_private_large_frame().unwrap().is_none());
    memory.free_physical(allocation.first * 4096, 2 << 20).unwrap();
    assert_eq!(memory.physical_memory_info().unwrap().free_pages, before);
}

#[test]
fn private_large_virtual_fallback_recognizes_retained_empty_small_leaf_table() {
    let (mut memory, _) = boot(30, true);
    assert!(memory.private_large_virtual_available(500, 0x20000000).unwrap());
    let mut vm = HeapVm::default();
    memory.ensure_page_tables(500, 0x20001000, &mut vm).unwrap();
    assert!(!memory.private_large_virtual_available(500, 0x20000000).unwrap());
    assert!(memory.private_large_virtual_available(500, 0x20200000).unwrap());
}

#[test]
fn private_large_failed_revoke_is_not_a_ready_slice_and_can_restore_original_huge_leaf() {
    let (mut memory, _) = boot(30, true);
    let allocation = memory.allocate_private_large_frame().unwrap().unwrap();
    memory.map_private_large_frame(allocation.first, 500, 0x20000000, 5).unwrap();
    memory.unmap_private_large_frame(allocation.first).unwrap();
    FAKE.with_borrow_mut(|f| f.fail_revoke_after = true);
    assert!(memory.demote_private_large_pool(allocation.first).is_err());
    assert!(memory.physical_frame_descriptor_from_index(allocation.first).is_none());
    FAKE.with_borrow_mut(|f| f.fail_revoke_after = false);
    memory.restore_private_large_frame(allocation.first).unwrap();
    let mapping = memory.private_large_mapping(500, 0x20000000, 0x20200000).unwrap();
    assert!(!mapping.small);
    assert_eq!(mapping.permissions, 5);
    assert!(memory.physical_frame_descriptor_from_index(allocation.first).is_some());
    assert!(memory.ensure_alpha_frame_at_physical_index(allocation.first).is_err());
}

#[test]
fn private_large_rollback_unlinks_split_table_before_recreating_huge_frame_and_clears_ready_hints() {
    let (mut memory, _) = boot(30, true);
    let allocation = memory.allocate_private_large_frame().unwrap().unwrap();
    memory.map_private_large_frame(allocation.first, 500, 0x20000000, 3).unwrap();
    memory.unmap_private_large_frame(allocation.first).unwrap();
    memory.demote_private_large_pool(allocation.first).unwrap();
    let mut vm = HeapVm::default();
    memory.ensure_private_large_split_table(allocation.first, &mut vm).unwrap();
    let frame = memory.physical_frame_descriptor_from_index(allocation.first).unwrap();
    memory.map_frame_strict(500, frame, 0x20000000, &mut vm).unwrap();
    memory.restore_private_large_frame(allocation.first).unwrap();
    assert!(!memory.private_large_mapping(500, 0x20000000, 0x20200000).unwrap().small);
    assert!(descriptor::descriptor_depth(memory.physical_frame_descriptor_from_index(allocation.first).unwrap()) > WORD_BITS);
    assert_eq!(memory.ensure_alpha_frame_at_physical_index(allocation.first), Err(CapabilityError::IllegalOperation));
    FAKE.with_borrow(|f| {
        assert!(!f.tables.contains_key(&(500, 0x20000000 >> 21, 1)));
        let events = &f.heap_events;
        let last_large = events.iter().rposition(|&x| x == "large-attempt").unwrap();
        assert_eq!(events[last_large - 1], "revoke");
        assert_eq!(events[last_large - 2], "unmap");
    });
}

#[test]
fn private_large_ambiguous_map_and_fatal_rollback_hold_ram_until_address_space_retirement() {
    let (mut memory, _) = boot(30, true);
    let allocation = memory.allocate_private_large_frame().unwrap().unwrap();
    FAKE.with_borrow_mut(|f| f.fail_frame_map_at = Some((f.frame_maps.len() + 1, true)));
    assert!(memory.map_private_large_frame(allocation.first, 500, 0x20000000, 3).is_err());
    assert!(memory.free_physical(allocation.first * 4096, 2 << 20).is_err());
    memory.quarantine_private_large_mapping(allocation.first);
    assert!(memory.private_large_mapping(500, 0x20000000, 0x20200000).unwrap().quarantined);
    FAKE.with_borrow_mut(|f| f.fail_frame_map_at = None);
    // The test supplies the same unreachable-AS precondition as normal reap.
    memory.release_page_tables(500).unwrap();
    memory.free_physical(allocation.first * 4096, 2 << 20).unwrap();
}

#[test]
fn only_stable_whole_large_mappings_can_be_omitted_from_a_split() {
    for state in 0..5 {
        let (mut memory, _) = boot(30, true);
        let allocation = memory.allocate_private_large_frame().unwrap().unwrap();
        if state == 4 {
            FAKE.with_borrow_mut(|f| f.fail_frame_map_at = Some((f.frame_maps.len() + 1, true)));
            assert!(memory.map_private_large_frame(allocation.first, 500, 0x20000000, 3).is_err());
        } else {
            memory.map_private_large_frame(allocation.first, 500, 0x20000000, if state == 0 { 3 } else { 0 }).unwrap();
            if state == 2 { memory.demote_private_large_pool(allocation.first).unwrap(); }
            if state == 3 { memory.quarantine_private_large_mapping(allocation.first); }
        }
        let before = FAKE.with_borrow(|f| (f.calls, f.frame_maps.len(), f.heap_events.len()));
        let mut inspected = 0;
        let result = memory.private_large_mapping_requiring_split(500, 0x20000000, 0x20200000, |base, _| {
            inspected += 1;
            assert_eq!(base, 0x20000000);
            true
        });
        assert_eq!(result.is_none(), state < 2);
        assert_eq!(inspected, usize::from(state < 2));
        assert!(memory.private_large_mapping(500, 0x20000000, 0x20200000).is_some());
        assert!(memory.private_large_mapping_requiring_split(501, 0x20000000, 0x20200000, |_, _| panic!()).is_none());
        assert_eq!(FAKE.with_borrow(|f| (f.calls, f.frame_maps.len(), f.heap_events.len())), before);
    }
}

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
    extra_frame_runs: Vec<(usize, usize, usize, usize)>,
    fail_kind: Option<(CapabilityType, usize)>,
    calls: usize,
    nodes: usize,
    batches: usize,
    map_attributes: Vec<usize>,
    tables: BTreeMap<(usize, usize, usize), usize>,
    parents: BTreeMap<usize, usize>,
    fail_revoke: bool,
    fail_revoke_after: bool,
    huge_failure: Option<(usize, CapabilityError)>,
    huge_successes_before_failure: usize,
    heap_events: Vec<&'static str>,
    frame_maps: Vec<(usize, usize)>,
    fail_frame_map_at: Option<(usize, bool)>,
    fail_unmap: bool,
    small_frame_owners: BTreeMap<(usize, usize), usize>,
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
                if kind == CapabilityType::Frame && radix == 21 {
                    fake.heap_events.push("large-attempt");
                }
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
                    if radix == 21 {
                        let failure = if fake.huge_successes_before_failure != 0 {
                            fake.huge_successes_before_failure -= 1;
                            None
                        } else { fake.huge_failure.take() };
                        let installed = failure.map_or(count, |(prefix,_)| prefix.min(count));
                        for i in 0..installed {
                            let descriptor = make_child_slot_descriptor(node, parent.radix, slot+i);
                            assert!(!fake.caps.contains_key(&descriptor));
                            fake.caps.insert(descriptor, Cap { kind, cursor: start+i*bytes,
                                end: start+(i+1)*bytes, radix, device: original.device });
                            fake.parents.insert(descriptor, source);
                        }
                        fake.caps.get_mut(&source).unwrap().cursor = start + installed*bytes;
                        if let Some((_,error)) = failure { return Err(error); }
                    } else {
                        assert_eq!(radix,12);
                        fake.small_frame_owners.insert((node, slot), source);
                        if slot == 0 { assert!(fake.frames.insert(node, (start, count)).is_none()); }
                        else { fake.extra_frame_runs.push((node, slot, start, count)); }
                        fake.heap_events.push("small-frames");
                    }
                    fake.batches += 1;
                } else {
                    assert_eq!(count, 1);
                    let descriptor = if node == 0 {
                        make_root_slot_descriptor(12, slot)
                    } else {
                        make_child_slot_descriptor(node, parent.radix, slot)
                    };
                    if fake.caps.contains_key(&descriptor) { return Err(CapabilityError::InvalidArgument); }
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
                fake.heap_events.push("revoke");
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
                let batches: Vec<_> = fake.small_frame_owners.iter().filter_map(|(&key, &owner)|
                    (owner == target).then_some(key)).collect();
                for (node, slot) in batches {
                    fake.small_frame_owners.remove(&(node, slot));
                    if slot == 0 { fake.frames.remove(&node); }
                    else { fake.extra_frame_runs.retain(|row| row.0 != node || row.1 != slot); }
                }
                if fake.fail_revoke_after { return Err(CapabilityError::IllegalOperation); }
                Ok(())
            })
        }
    }
    pub mod address_space {
        use super::super::*;
        pub fn unmap(space: usize, object: usize, address: usize) -> CapabilityResult {
            FAKE.with_borrow_mut(|fake| {
                fake.heap_events.push("unmap");
                if fake.fail_unmap { return Err(CapabilityError::Fatal); }
                if let Some(cap) = fake.caps.get(&object).copied() {
                    if cap.kind == CapabilityType::PageTable {
                        let key = (space, address >> (12 + 9 * cap.radix), cap.radix);
                        if let Some(table) = fake.tables.remove(&key) { assert_eq!(table, object); }
                    }
                }
                Ok(())
            })
        }
        pub fn map(
            address_space: usize,
            object: usize,
            address: usize,
            attribute: capability_call::address_space::Attribute,
        ) -> CapabilityResult {
            FAKE.with_borrow_mut(|fake| {
                let is_table = fake.caps.get(&object).is_some_and(|cap| cap.kind==CapabilityType::PageTable);
                let ordinal = fake.frame_maps.len()+1;
                if !is_table && fake.fail_frame_map_at==Some((ordinal,false)) {
                    return Err(CapabilityError::Fatal);
                }
                fake.map_attributes.push(attribute.bits());
                if !is_table {
                    if object & (1 << (usize::BITS - 1)) != 0 { return Err(CapabilityError::InvalidDepth); }
                    let bits = fake.caps.get(&object).map_or(12,|cap| cap.radix);
                    assert!(matches!(bits,12|21));
                    assert_eq!(address & ((1<<bits)-1),0);
                    if bits == 21 { assert!(!fake.tables.contains_key(&(address_space, address >> 21, 1))); }
                    fake.frame_maps.push((address,bits));
                    if fake.fail_frame_map_at==Some((ordinal,true)) { return Err(CapabilityError::Fatal); }
                }
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
            leaf_bits: usize,
        ) -> Result<usize, CapabilityError> {
            FAKE.with_borrow(|fake| {
                let minimum = match leaf_bits {12=>1,21=>2,_=>return Err(CapabilityError::InvalidDepth)};
                for depth in (minimum..=3).rev() {
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
    boot_with_heap_preparation(ram_radix, online, ram_consumed, true)
}

fn boot_with_heap_preparation(
    ram_radix: u8, online: bool, ram_consumed: usize, prepare_heap: bool,
) -> (MemoryManager, AlphaEnvironment) {
    boot_with_heap_prefix(ram_radix, online, ram_consumed, prepare_heap, 0)
}
fn boot_with_heap_prefix(
    ram_radix: u8, online: bool, ram_consumed: usize, prepare_heap: bool, heap_prefix: usize,
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
    info.resources[2].consumed_bytes = heap_prefix;
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
    if prepare_heap {
        memory.prepare_bootstrap_frames(2, 0x80000000, 8192).unwrap();
    }
    if online {
        memory.initialize_physical_allocator().unwrap();
        memory
            .allocate_physical_at(0x80000000, 0x2000000, false)
            .unwrap();
    }
    (memory, info)
}
#[derive(Default)]
struct HeapVm {
    frames: BTreeMap<usize,(usize,usize)>,
    ready: std::collections::BTreeSet<usize>,
}
impl nanami_core::vm_space::VmTracker for HeapVm {
    fn page_tables_ready(&self, address: usize) -> bool { self.ready.contains(&(address>>21)) }
    fn record_frame(&mut self, address: usize, descriptor: usize) -> Result<(),()> {
        self.frames.insert(address,(descriptor,12));self.ready.insert(address>>21);Ok(())
    }
    fn record_bootstrap_frame(&mut self, address:usize, descriptor:usize, bits:usize)->Result<(),()> {
        if bits==12 {return self.record_frame(address,descriptor);}
        assert_eq!(bits,21);assert_eq!(address&((1<<21)-1),0);
        self.frames.insert(address,(descriptor,bits));Ok(())
    }
    fn record_page_table(&mut self,_:usize,_:usize)->Result<(),()> {Ok(())}
}

#[test]
#[cfg(target_arch="x86_64")]
fn large_heap_preserves_alignment_prefix_as_reusable_small_frames() {
    for small in [false,true] {
        let (mut memory,_) = boot_with_heap_prefix(35,false,0,false,4096);
        if small { FAKE.with_borrow_mut(|f| f.huge_failure=Some((1,CapabilityError::InvalidArgument))); }
        let frames=memory.prepare_bootstrap_heap_frames(2,0x80200000,8192,true).unwrap();
        assert_eq!(frames.leaf_bits(),if small {12}else{21});
        for page in [0x80001,0x801ff] {
            let cap=memory.physical_frame_descriptor_from_index(page).expect("alignment prefix retained");
            memory.ensure_alpha_frame_at_physical_index(page).unwrap();
            assert_eq!(memory.physical_frame_descriptor_from_index(page),Some(cap));
        }
        assert!(memory.physical_frame_descriptor_from_index(0x80000).is_none());
        memory.initialize_physical_allocator().unwrap();
        memory.allocate_physical_at(0x80001000,4096,false).unwrap();
        memory.free_physical(0x80001000,4096).unwrap();
        FAKE.with_borrow(|f| assert!(f.extra_frame_runs.iter().all(|(_,slot,_,count)| slot+count<=8192)));
    }
}

#[test]
#[cfg(target_arch="x86_64")]
fn permanent_heap_uses_sixteen_large_leaves_and_only_two_upper_tables() {
    let (mut memory,_) = boot_with_heap_preparation(35,false,0,false);
    let frames = memory.prepare_bootstrap_heap_frames(2,0x80000000,8192,true).unwrap();
    assert_eq!((frames.leaf_bytes(),frames.leaves(),frames.physical_base()),(2*1024*1024,16,0x80000000));
    assert!(frames.frame(16).is_none());
    let mut vm=HeapVm::default();
    for leaf in 0..frames.leaves() {
        memory.map_bootstrap_heap_frame(3000,frames.frame(leaf).unwrap(),
            0x58000000+leaf*frames.leaf_bytes(),frames.leaf_bits(),&mut vm).unwrap();
    }
    assert_eq!(vm.frames.len(),16);assert!(vm.ready.is_empty());
    FAKE.with_borrow(|f| {
        assert_eq!(f.frame_maps.len(),16);assert_eq!(f.tables.len(),2);
        assert!(f.frame_maps.iter().all(|(_,bits)| *bits==21));
    });
    for page in [0,1,511,512,8191] {
        assert!(memory.physical_frame_descriptor_from_index((0x80000000>>12)+page).is_none(),
            "must not return a 2M cap as a 4K frame");
        assert!(memory.ensure_alpha_frame_at_physical_index((0x80000000>>12)+page).is_err());
    }
    assert!(memory.ensure_alpha_frames_for_range_from_initial_generic(0x80001000,4096,false).is_err());
    assert!(memory.prepare_bootstrap_heap_frames(2,0x82000000,8192,true).is_err());
    assert!(memory.prepare_bootstrap_frames(2,0x82000000,8192).is_err());
    memory.initialize_physical_allocator().unwrap();
    memory.allocate_physical_at(0x80000000,32*1024*1024,false).unwrap();
    assert!(memory.free_physical(0x80000000,32*1024*1024).is_err());
}

#[test]
#[cfg(target_arch="x86_64")]
fn permanent_stack_shares_large_frame_node_without_aliasing_heap_or_leaking_ram() {
    let (mut memory,_) = boot_with_heap_preparation(35,false,0,false);
    let all = memory.prepare_bootstrap_heap_frames(2,0x80000000,8192+1024,true).unwrap();
    let heap = all.slice_pages(0,8192).unwrap();
    let stack = memory.bootstrap_stack_frames(8192,1024).unwrap();
    assert_eq!((heap.leaves(),stack.leaves(),stack.leaf_bits(),stack.physical_base()),(16,2,21,0x82000000));
    let mut vm=HeapVm::default();
    for (frames,va) in [(heap,0x58000000),(stack,0x50000000)] {
        for i in 0..frames.leaves() {
            memory.map_bootstrap_heap_frame(3000,frames.frame(i).unwrap(),
                va+i*frames.leaf_bytes(),frames.leaf_bits(),&mut vm).unwrap();
        }
    }
    FAKE.with_borrow(|f| {
        assert_eq!(f.frame_maps.len(),18);
        assert_eq!(f.tables.len(),2,"heap and stack share the upper tables");
        let frame_node = make_root_slot_descriptor(12,1101);
        assert_eq!(f.caps[&frame_node].radix,5,"32 cap slots suffice for 18 large leaves");
        for i in 0..2 {
            assert_eq!(f.caps[&stack.frame(i).unwrap()].cursor,0x82000000+i*(1<<21));
            assert_ne!(stack.frame(i),heap.frame(i));
        }
    });
    memory.initialize_physical_allocator().unwrap();
    memory.allocate_physical_at(all.physical_base(),36*1024*1024,false).unwrap();
    for physical in [0x82000000,0x82001000,0x823ff000] {
        assert!(memory.allocate_physical_at(physical,4096,false).is_err());
        assert!(memory.free_physical(physical,4096).is_err());
        assert!(memory.frame_descriptor_from_physical(physical).is_none());
    }
    memory.allocate_physical_at(0x82400000,4096,false).unwrap();
    memory.free_physical(0x82400000,4096).unwrap();
    assert!(all.slice_pages(1,512).is_none());
    assert!(all.slice_pages(8192,513).is_none());
    assert!(all.slice_pages(9216,512).is_none());
    assert!(all.slice_pages(usize::MAX,512).is_none());
    assert!(all.slice_pages(0,0).is_none());
}

#[test]
#[cfg(target_arch="x86_64")]
fn unsupported_stack_leaf_revokes_whole_unpublished_heap_before_small_stack_mapping() {
    for successful_leaves in [0,16,17] {
        let (mut memory,_) = boot_with_heap_preparation(35,false,0,false);
        FAKE.with_borrow_mut(|f| {
            f.huge_successes_before_failure=successful_leaves;
            f.huge_failure=Some((1,CapabilityError::InvalidArgument));
        });
        let all=memory.prepare_bootstrap_heap_frames(2,0x80000000,9216,true).unwrap();
        let stack=memory.bootstrap_stack_frames(8192,1024).unwrap();
        assert_eq!((all.leaf_bits(),stack.leaf_bits(),stack.leaves(),stack.physical_base()),(12,12,1024,0x82000000));
        for i in [0,511,512,1023] {
            assert_eq!(stack.frame(i),memory.physical_frame_descriptor_from_index(0x82000+i));
        }
        let mut vm=HeapVm::default();
        for i in 0..stack.leaves() {
            memory.map_bootstrap_heap_frame(3000,stack.frame(i).unwrap(),0x50000000+i*4096,12,&mut vm).unwrap();
        }
        FAKE.with_borrow(|f| {
            assert_eq!(f.frame_maps.len(),1024);
            assert_eq!(f.tables.len(),4);
            assert!(!f.caps.values().any(|cap|cap.kind==CapabilityType::Frame&&cap.radix==21));
            let last_revoke=f.heap_events.iter().rposition(|e|*e=="revoke").unwrap();
            let first_small=f.heap_events.iter().position(|e|*e=="small-frames").unwrap();
            assert!(last_revoke<first_small);
        });
    }
}

#[test]
fn heap_only_reservation_keeps_original_separate_stack_fallback() {
    for prefer_large in [false,true] {
        let (mut memory,_) = boot_with_heap_preparation(35,false,0,false);
        let heap=memory.prepare_bootstrap_heap_frames(2,0x80000000,8192,prefer_large).unwrap();
        assert!(memory.bootstrap_stack_frames(8192,1024).is_none());
        assert!(heap.slice_pages(8192,1024).is_none());
    }
}

#[test]
#[cfg(target_arch="x86_64")]
fn unsupported_large_conversion_revokes_even_installed_prefix_before_small_fallback() {
    for prefix in [0,1] {
      for successes in [0,3,15] {
        let (mut memory,_) = boot_with_heap_preparation(35,false,0,false);
        FAKE.with_borrow_mut(|f| {f.huge_failure=Some((prefix,CapabilityError::InvalidArgument));f.huge_successes_before_failure=successes;f.heap_events.clear();});
        let frames=memory.prepare_bootstrap_heap_frames(2,0x80000000,8192,true).unwrap();
        assert_eq!((frames.leaf_bits(),frames.leaves()),(12,8192));
        FAKE.with_borrow(|f| {
            assert_eq!(f.heap_events.iter().filter(|e| **e=="revoke").count(),successes+1);
            let last_revoke=f.heap_events.iter().rposition(|e| *e=="revoke").unwrap();
            let first_small=f.heap_events.iter().position(|e| *e=="small-frames").unwrap();
            assert!(last_revoke < first_small);
            assert_eq!(f.extra_frame_runs.len(),15);
            assert!(!f.caps.values().any(|cap| cap.kind==CapabilityType::Frame&&cap.radix==21));
            assert!(f.frame_maps.is_empty());
        });
        for page in [0,1,511,512,8191] {
            assert_eq!(memory.physical_frame_descriptor_from_index((0x80000000>>12)+page),frames.frame(page));
        }
      }
    }
}

#[test]
#[cfg(target_arch="x86_64")]
fn failed_revoke_ack_never_falls_back_even_if_cleanup_may_have_completed() {
    for after in [false,true] {
        let (mut memory,_) = boot_with_heap_preparation(35,false,0,false);
        FAKE.with_borrow_mut(|f| {
            f.huge_failure=Some((1,CapabilityError::InvalidArgument));
            f.fail_revoke=!after;f.fail_revoke_after=after;f.heap_events.clear();
        });
        assert!(memory.prepare_bootstrap_heap_frames(2,0x80000000,8192,true).is_err());
        FAKE.with_borrow(|f| {
            assert_eq!(f.heap_events,vec!["large-attempt","revoke"]);
            assert!(f.frames.is_empty());assert!(f.frame_maps.is_empty());
            assert_eq!(f.caps.values().filter(|cap|cap.kind==CapabilityType::Frame&&cap.radix==21).count(),if after {0}else{1});
        });
    }
}

#[test]
#[cfg(target_arch="x86_64")]
fn unexpected_conversion_and_map_failures_hold_caps_and_never_retry_as_small() {
    let (mut memory,_) = boot_with_heap_preparation(35,false,0,false);
    FAKE.with_borrow_mut(|f|{f.huge_failure=Some((2,CapabilityError::Fatal));f.heap_events.clear();});
    assert!(memory.prepare_bootstrap_heap_frames(2,0x80000000,8192,true).is_err());
    FAKE.with_borrow(|f|assert_eq!(f.heap_events,vec!["large-attempt"]));
    for after in [false,true] {
        let (mut memory,_) = boot_with_heap_preparation(35,false,0,false);
        let frames=memory.prepare_bootstrap_heap_frames(2,0x80000000,8192,true).unwrap();
        FAKE.with_borrow_mut(|f|f.fail_frame_map_at=Some((3,after)));
        let mut vm=HeapVm::default();
        for leaf in 0..2 {
            memory.map_bootstrap_heap_frame(3000,frames.frame(leaf).unwrap(),0x58000000+leaf*(1<<21),21,&mut vm).unwrap();
        }
        assert!(memory.map_bootstrap_heap_frame(3000,frames.frame(2).unwrap(),0x58400000,21,&mut vm).is_err());
        assert_eq!(vm.frames.len(),3); // possible post-install failure remains recorded
        FAKE.with_borrow(|f| {
            assert_eq!(f.frame_maps.len(),if after {3}else{2});
            assert_eq!(f.caps.values().filter(|cap|cap.kind==CapabilityType::Frame&&cap.radix==21).count(),16);
            assert!(!f.heap_events.contains(&"revoke"));assert!(!f.heap_events.contains(&"small-frames"));
        });
    }
}

#[test]
fn explicit_small_heap_preserves_the_original_mapping_and_table_geometry() {
    let (mut memory,_) = boot_with_heap_preparation(35,false,0,false);
    let frames=memory.prepare_bootstrap_heap_frames(2,0x80000000,8192,false).unwrap();
    assert_eq!((frames.leaf_bits(),frames.leaves()),(12,8192));
    let mut vm=HeapVm::default();
    for leaf in 0..frames.leaves() {
        memory.map_bootstrap_heap_frame(3000,frames.frame(leaf).unwrap(),0x58000000+leaf*4096,12,&mut vm).unwrap();
    }
    FAKE.with_borrow(|f|{assert_eq!(f.frame_maps.len(),8192);assert_eq!(f.tables.len(),18);});
    for (address,pages) in [(0x80001000,8192),(0x80000000,0),(0x80000000,768),(0,usize::MAX)] {
        assert!(!MemoryManager::large_bootstrap_heap_eligible(address,pages));
    }
}

#[test]
#[cfg(target_arch="x86_64")]
fn ordinary_tracker_rejects_large_bootstrap_leaves_before_any_page_table_or_map() {
    let (mut memory,_) = boot_with_heap_preparation(35,false,0,false);
    let frames=memory.prepare_bootstrap_heap_frames(2,0x80000000,8192,true).unwrap();
    let mut vm=PageTableVm::default();
    assert!(memory.map_bootstrap_heap_frame(3000,frames.frame(0).unwrap(),0x58000000,21,&mut vm).is_err());
    FAKE.with_borrow(|f|{assert!(f.frame_maps.is_empty());assert!(f.tables.is_empty());});
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
fn pool_origin_is_distinct_from_child_aliases_and_survives_process_arena_reset() {
    let (mut memory, _) = boot(33, true);
    let bootstrap = memory
        .physical_frame_descriptor_from_index(0x80000000 >> 12)
        .unwrap();
    let frame = map(&mut memory, 0x200000000, false);
    assert!(production::pool_frame(&memory, bootstrap));
    assert!(production::pool_frame(&memory, frame));
    let child_root = make_root_slot_descriptor(12, 200);
    let child_alias = make_child_slot_descriptor(child_root, 9, 10);
    assert!(!production::pool_frame(&memory, child_alias));
    let before = FAKE.with(|fake| fake.borrow().frames.clone());
    memory.ensure_process_arena(200).unwrap();
    memory.reset_process_arena(200).unwrap();
    assert_eq!(
        memory.physical_frame_descriptor_from_index(0x200000000 >> 12),
        Some(frame)
    );
    assert!(production::pool_frame(&memory, frame));
    FAKE.with(|fake| assert_eq!(fake.borrow().frames, before));
}

#[test]
fn completed_frame_range_reuses_one_index_lookup_for_dense_page_resolution() {
    let (mut memory, _) = boot(40, true);
    let base = 0x200000000usize;
    let first = map(&mut memory, base, false);
    map(&mut memory, 0x208000000, false);
    map(&mut memory, 0x210000000, false); // Evict the first range even with two entries.
    let before = memory.frame_range_lookup_diagnostics().0;
    let calls = FAKE.with(|fake| fake.borrow().calls);
    for offset in 0..2048 {
        let address = base + offset * 4096;
        let page = address >> 12;
        memory.ensure_alpha_frame_at_physical_index(page).unwrap();
        let descriptor = memory.physical_frame_descriptor_from_index(page).unwrap();
        assert_eq!(memory.physical_frame_descriptor_from_index(page), Some(descriptor));
        FAKE.with(|fake| {
            let fake = fake.borrow();
            let (&node, &(start, _)) = fake.frames.iter().find(|(_, (start, pages))|
                address >= *start && address - *start < *pages * 4096).unwrap();
            assert_eq!(descriptor, make_child_slot_descriptor(node, fake.caps[&node].radix, (address - start) >> 12));
        });
    }
    assert_eq!(memory.frame_range_lookup_diagnostics().0 - before, 1);
    FAKE.with(|fake| assert_eq!(fake.borrow().calls, calls));
    for page in [0, 0x400000000 >> 12, usize::MAX] {
        assert!(memory.physical_frame_descriptor_from_index(page).is_none());
    }
    // Neither a miss nor the independent bootstrap range evicts this entry.
    assert!(memory.physical_frame_descriptor_from_index(0x80000000 >> 12).is_some());
    let before = memory.frame_range_lookup_diagnostics().0;
    assert_eq!(memory.physical_frame_descriptor_from_index(base >> 12), Some(first));
    assert_eq!(memory.frame_range_lookup_diagnostics().0, before);
    println!("READY-FRAME-RANGE pages=2048 resolutions=6144 index-lookups=1 metadata-bytes={}", memory.frame_range_lookup_diagnostics().1);
}

#[test]
fn large_allocation_hint_bounds_cold_frame_cap_creation_and_reuses_ready_ranges() {
    for pages in [256usize, 512, 775, 858, 1024, 1536, 2048] {
        for bounded in [false,true] {
            let (mut memory,_)=boot(40,true);let first=0x200000000usize>>12;
            let before=FAKE.with_borrow(|fake|fake.frames.values().map(|(_,n)|n).sum::<usize>());
            let hint=if bounded {memory.allocation_frame_hint(first,pages)}else{None};
            let mut descriptors=Vec::new();
            for page in first..first+pages {
                memory.ensure_alpha_frame_for_allocation(page,hint).unwrap();
                descriptors.push(memory.physical_frame_descriptor_from_index(page).unwrap());
            }
            let created=FAKE.with_borrow(|fake|fake.frames.values().map(|(_,n)|n).sum::<usize>())-before;
            assert_eq!(created,if bounded {pages.next_power_of_two().min(2048)}else{2048},"pages={pages} bounded={bounded}");
            let calls=FAKE.with_borrow(|fake|fake.calls);
            for (offset,descriptor) in descriptors.iter().enumerate() {
                memory.ensure_alpha_frame_for_allocation(first+offset,hint).unwrap();
                assert_eq!(memory.physical_frame_descriptor_from_index(first+offset),Some(*descriptor));
                FAKE.with_borrow(|fake|{
                    let address=(first+offset)*4096;
                    let (&node,&(start,_))=fake.frames.iter().find(|(_, (base,count))|address>=*base&&address-*base<*count*4096).unwrap();
                    assert_eq!(*descriptor,make_child_slot_descriptor(node,fake.caps[&node].radix,(address-start)>>12));
                });
            }
            assert_eq!(FAKE.with_borrow(|fake|fake.calls),calls);
        }
    }
}

#[test]
fn bounded_frame_ranges_coexist_with_later_ordinary_and_overlapping_requests() {
    let (mut memory,_)=boot(40,true);let first=0x200000000usize>>12;
    let hint=memory.allocation_frame_hint(first,256);assert!(hint.is_some());
    memory.ensure_alpha_frame_for_allocation(first,hint).unwrap();
    let original=memory.physical_frame_descriptor_from_index(first).unwrap();
    // Untyped source remainders, not an assumed old8M range, select the next
    // ordinary conversion. Neither it nor a later overlapping hint can replace
    // or duplicate an already Ready permanent Frame capability.
    for offset in [256,512,1536,2047,2048] {memory.ensure_alpha_frame_at_physical_index(first+offset).unwrap();}
    let calls=FAKE.with_borrow(|fake|fake.calls);
    let broad=memory.allocation_frame_hint(first,1024);
    for offset in 0..1024 {memory.ensure_alpha_frame_for_allocation(first+offset,broad).unwrap();}
    assert_eq!(FAKE.with_borrow(|fake|fake.calls),calls);
    assert_eq!(memory.physical_frame_descriptor_from_index(first),Some(original));
    let mut ranges=FAKE.with_borrow(|fake|fake.frames.values().map(|&(base,pages)|(base,base+pages*4096)).collect::<Vec<_>>());
    ranges.sort_unstable();assert!(ranges.windows(2).all(|pair|pair[0].1<=pair[1].0));
}

#[test]
fn shared_allocation_frame_runs_skip_ready_pages_and_preserve_conversion_failures() {
    let (mut memory, _) = boot(40, true);
    let first = 0x200000000usize >> 12;
    let pages = 2048;
    memory.ensure_alpha_frame_at_physical_index(first).unwrap();
    let hint = memory.allocation_frame_hint(first, pages);
    let probes = memory.frame_range_probe_count();
    let expected: Vec<_> = (first..first + pages).map(|page| {
        memory.ensure_alpha_frame_for_allocation(page, hint).unwrap();
        memory.physical_frame_descriptor_from_index(page).unwrap()
    }).collect();
    let old = memory.frame_range_probe_count() - probes;
    assert_eq!(old, 2 * pages);

    let calls = FAKE.with_borrow(|fake| fake.calls);
    let probes = memory.frame_range_probe_count();
    memory.ensure_alpha_frames_for_allocation(first, pages).unwrap();
    let new = memory.frame_range_probe_count() - probes;
    assert_eq!(new, 1);
    assert_eq!(FAKE.with_borrow(|fake| fake.calls), calls);
    let mut frames = vec![0; pages];
    let probes = memory.frame_range_probe_count();
    memory.read_ready_pool_frames(first, &mut frames).unwrap();
    let read_probes = memory.frame_range_probe_count() - probes;
    assert_eq!(read_probes, 1);
    assert_eq!(frames, expected);
    println!("SHARED-FRAME-RUNS pages={pages} prepare_old={old} prepare_new={new} collect_new={read_probes}");

    // The last Ready page is followed by a previously untyped range.
    FAKE.with_borrow_mut(|fake| fake.fail_kind = Some((CapabilityType::Frame, 0)));
    assert_eq!(memory.ensure_alpha_frames_for_allocation(first + pages - 1, 2),
        Err(CapabilityError::Fatal));
    assert!(memory.physical_frame_descriptor_from_index(first + pages - 1).is_some());
    assert!(memory.physical_frame_descriptor_from_index(first + pages).is_none());
    let mut spans = [0; 2];
    assert_eq!(memory.read_ready_pool_frames(first + pages - 1, &mut spans),
        Err(CapabilityError::InvalidArgument));
    let calls = FAKE.with_borrow(|fake| fake.calls);
    assert_eq!(memory.ensure_alpha_frames_for_allocation(first + pages - 1, 2),
        Err(CapabilityError::InvalidArgument));
    assert_eq!(FAKE.with_borrow(|fake| fake.calls), calls);
    assert_eq!(memory.ensure_alpha_frames_for_allocation(usize::MAX, 2),
        Err(CapabilityError::InvalidArgument));

    let (mut memory, _) = boot(40, true);
    memory.ensure_alpha_frames_for_allocation(first, 256).unwrap();
    memory.ensure_alpha_frames_for_allocation(first + 256, 256).unwrap();
    let boundary = first + 255;
    let probes = memory.frame_range_probe_count();
    memory.read_ready_pool_frames(boundary, &mut spans).unwrap();
    assert_eq!(memory.frame_range_probe_count() - probes, 2);
    for (offset, frame) in spans.into_iter().enumerate() {
        assert_eq!(memory.physical_frame_descriptor_from_index(boundary + offset), Some(frame));
    }
}

#[test]
fn invalid_allocation_hints_and_failed_bounded_conversion_never_publish_or_retry_frames() {
    let (mut memory,_)=boot(40,true);let first=0x200000000usize>>12;
    for (base,pages) in [(first,0),(first,255),(usize::MAX,256),(usize::MAX/4096,256)] {
        assert!(memory.allocation_frame_hint(base,pages).is_none());
    }
    let hint=memory.allocation_frame_hint(first,512);let calls=FAKE.with_borrow(|fake|fake.calls);
    for page in [first-1,first+512] {assert_eq!(memory.ensure_alpha_frame_for_allocation(page,hint),Err(CapabilityError::InvalidArgument));}
    assert_eq!(FAKE.with_borrow(|fake|fake.calls),calls);
    FAKE.with_borrow_mut(|fake|fake.fail_kind=Some((CapabilityType::Frame,0)));
    assert_eq!(memory.ensure_alpha_frame_for_allocation(first,hint),Err(CapabilityError::Fatal));
    assert!(memory.physical_frame_descriptor_from_index(first).is_none());
    let calls=FAKE.with_borrow(|fake|fake.calls);
    assert_eq!(memory.ensure_alpha_frame_for_allocation(first,hint),Err(CapabilityError::InvalidArgument));
    assert_eq!(FAKE.with_borrow(|fake|fake.calls),calls);
    memory.ensure_alpha_frame_at_physical_index(first+2048).unwrap();
    assert!(memory.physical_frame_descriptor_from_index(first+2048).is_some());
}

#[test]
fn ready_range_cache_does_not_publish_failed_conversion_or_retry_it() {
    let (mut memory, _) = boot(40, true);
    let ready = map(&mut memory, 0x200000000, false);
    FAKE.with(|fake| fake.borrow_mut().fail_kind = Some((CapabilityType::Frame, 0)));
    for _ in 0..2 {
        assert!(memory.ensure_alpha_frames_for_range_from_initial_generic(0xa1220000, 4096, true).is_err());
        assert!(memory.physical_frame_descriptor_from_index(0xa1220000 >> 12).is_none());
        assert_eq!(memory.physical_frame_descriptor_from_index(0x200000000 >> 12), Some(ready));
    }
    let calls = FAKE.with(|fake| fake.borrow().calls);
    assert!(memory.ensure_alpha_frame_at_physical_index(0xa1220000 >> 12).is_err());
    FAKE.with(|fake| assert_eq!(fake.borrow().calls, calls));
}

#[test]
fn alternating_ready_frame_ranges_avoid_repeated_index_searches() {
    let (mut memory, _) = boot(40, true);
    let bases = [0x200000000usize, 0x208000000];
    for base in bases { map(&mut memory, base, false); }
    let before = memory.frame_range_lookup_diagnostics().0;
    let calls = FAKE.with(|fake| fake.borrow().calls);
    for offset in 0..2048 {
        for base in bases {
            let address = base + offset * 4096;
            let descriptor = memory.physical_frame_descriptor_from_index(address >> 12).unwrap();
            FAKE.with(|fake| {
                let fake = fake.borrow();
                let (&node, &(start, _)) = fake.frames.iter().find(|(_, (start, pages))|
                    address >= *start && address - *start < *pages * 4096).unwrap();
                assert_eq!(descriptor, make_child_slot_descriptor(node, fake.caps[&node].radix,
                    (address - start) >> 12));
            });
        }
    }
    let lookups = memory.frame_range_lookup_diagnostics().0 - before;
    FAKE.with(|fake| assert_eq!(fake.borrow().calls, calls));
    println!("alternating frame ranges resolutions=4096 tree-lookups={lookups} cache-bytes={}",
        memory.frame_range_lookup_diagnostics().1);
    assert_eq!(lookups, 0);
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
