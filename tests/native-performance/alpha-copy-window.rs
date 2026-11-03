#![allow(dead_code)]
use std::collections::BTreeMap;

type CapabilityDescriptor = usize;
const PAGE_SIZE: usize = 4096;
#[derive(Debug, PartialEq, Eq)]
enum CapabilityError {
    InvalidArgument,
    IllegalOperation,
}

const POOL_FRAME: usize = 0x4000_0000;
struct Memory;
impl Memory {
    fn is_pool_frame_descriptor(&self, frame: usize) -> bool {
        frame & 0xffff_0000 == POOL_FRAME
    }
}

struct Alpha {
    memory: Memory,
    processes: Processes,
    copy_window: memory_copy::CopyWindow,
    mappings: BTreeMap<usize, usize>,
    maps: usize,
    unmaps: usize,
    fail_map: bool,
    fail_unmap: bool,
}
impl Alpha {
    fn new() -> Self {
        Self {
            memory: Memory,
            processes: Processes::default(),
            copy_window: memory_copy::CopyWindow::new(),
            mappings: BTreeMap::new(),
            maps: 0,
            unmaps: 0,
            fail_map: false,
            fail_unmap: false,
        }
    }
    fn map_alpha_temporary_frame(
        &mut self,
        frame: usize,
        va: usize,
    ) -> Result<(), CapabilityError> {
        assert!(self.mappings.insert(va, frame).is_none());
        self.maps += 1;
        // Fail after the PTE was installed, not only before MAP.
        if self.fail_map {
            Err(CapabilityError::IllegalOperation)
        } else {
            Ok(())
        }
    }
    fn unmap_alpha_frame(&mut self, frame: usize, va: usize) -> Result<(), CapabilityError> {
        if self.fail_unmap {
            return Err(CapabilityError::IllegalOperation);
        }
        assert_eq!(self.mappings.remove(&va), Some(frame));
        self.unmaps += 1;
        Ok(())
    }
}
#[derive(Default)]
struct Processes(BTreeMap<usize, usize>);
impl Processes {
    fn memory_owner_pid(&self, pid: usize) -> usize {
        self.0.get(&pid).copied().unwrap_or(pid)
    }
}
#[path = "../../nanami/src/nanami_core/alpha/memory_copy.rs"]
mod memory_copy;

#[test]
fn copy_alias_and_metadata_budgets_remain_bounded() {
    assert_eq!(memory_copy::COPY_WINDOW_PAGES, 2048);
    // At most 32 bytes per tracked alias, one round-robin byte per original
    // set, and 256 u16 owner counters. Higher associativity must not allocate
    // a larger alias region or another metadata layer.
    assert!(std::mem::size_of::<memory_copy::CopyWindow>() <= 2048 * 32 + 512 + 256 * 2);
}

#[test]
fn warm_clock_and_poll_transfers_do_not_map_or_unmap_in_either_direction() {
    let mut a = Alpha::new();
    let guest = a.map_process_copy_frame(1, 100, None).unwrap();
    let scratch = a.map_process_copy_frame(2, 200, Some(guest)).unwrap();
    for _ in 0..10_000 {
        assert_eq!(a.map_process_copy_frame(2, 200, None).unwrap(), scratch);
        assert_eq!(
            a.map_process_copy_frame(1, 100, Some(scratch)).unwrap(),
            guest
        );
        assert_eq!(a.map_process_copy_frame(1, 100, None).unwrap(), guest);
        assert_eq!(
            a.map_process_copy_frame(2, 200, Some(guest)).unwrap(),
            scratch
        );
    }
    assert_eq!((a.maps, a.unmaps), (2, 0));
}

#[test]
fn eviction_never_overwrites_the_pinned_source() {
    let mut a = Alpha::new();
    let source = a.map_process_copy_frame(1, 100, None).unwrap();
    for frame in 200..10_200 {
        let destination = a.map_process_copy_frame(2, frame, Some(source)).unwrap();
        assert_ne!(destination, source);
        assert_eq!(a.mappings[&source], 100);
        assert!(a.mappings.len() <= memory_copy::COPY_WINDOW_PAGES);
    }
    a.invalidate_process_copy_mappings(2).unwrap();
    assert_eq!(a.mappings.len(), 1);
    a.invalidate_process_copy_mappings(1).unwrap();
    assert!(a.mappings.is_empty());
    assert_eq!(a.maps, a.unmaps);
}

#[test]
fn unmap_exec_reap_invalidation_prevents_same_descriptor_reuse_hits() {
    let mut a = Alpha::new();
    for _ in 0..20 {
        a.map_process_copy_frame(1, 100, None).unwrap();
        a.map_process_copy_frame(2, 200, None).unwrap();
        let before = a.maps;
        a.invalidate_process_copy_mappings(1).unwrap();
        a.map_process_copy_frame(1, 100, None).unwrap();
        a.map_process_copy_frame(2, 200, None).unwrap();
        assert_eq!(a.maps, before + 1);
    }
}

#[test]
fn failed_map_retains_cleanup_metadata_but_is_never_a_hit() {
    let mut a = Alpha::new();
    a.fail_map = true;
    assert!(a.map_process_copy_frame(1, 100, None).is_err());
    a.fail_map = false;
    a.map_process_copy_frame(1, 100, None).unwrap();
    assert_eq!((a.maps, a.unmaps), (2, 1));
    a.invalidate_process_copy_mappings(1).unwrap();
    assert!(a.mappings.is_empty());
}

#[test]
fn failed_unmap_does_not_forget_or_reuse_a_possibly_live_mapping() {
    let mut a = Alpha::new();
    a.map_process_copy_frame(1, 100, None).unwrap();
    a.fail_unmap = true;
    assert!(a.invalidate_process_copy_mappings(1).is_err());
    assert_eq!(a.mappings.len(), 1);
    a.fail_unmap = false;
    a.invalidate_process_copy_mappings(1).unwrap();
    assert!(a.mappings.is_empty());
    a.map_process_copy_frame(1, 100, None).unwrap();
    assert_eq!(a.maps, 2);
}

#[test]
fn working_set_larger_than_sixteen_reuses_all_resident_aliases() {
    let mut a = Alpha::new();
    let mut aliases = Vec::new();
    for frame in 1..=64 {
        aliases.push(a.map_process_copy_frame(1, frame, None).unwrap());
    }
    for _ in 0..20 {
        for (frame, &va) in (1..=64).zip(&aliases) {
            assert_eq!(a.map_process_copy_frame(1, frame, None).unwrap(), va);
        }
    }
    assert_eq!((a.maps, a.unmaps), (64, 0));
}

#[test]
fn shared_vm_peers_reuse_and_invalidate_the_memory_owners_alias() {
    let mut a = Alpha::new();
    a.processes.0.insert(2, 1);
    let owner = a.map_process_copy_frame(1, 100, None).unwrap();
    assert_eq!(a.map_process_copy_frame(2, 100, None).unwrap(), owner);
    assert_eq!(a.maps, 1);
    a.invalidate_process_copy_mappings(2).unwrap();
    assert!(a.mappings.is_empty());
    a.map_process_copy_frame(1, 100, None).unwrap();
    assert_eq!(a.maps, 2);
}

#[test]
fn identical_descriptors_in_independent_spaces_do_not_alias_cache_keys() {
    let mut a = Alpha::new();
    let first = a.map_process_copy_frame(1, 100, None).unwrap();
    let second = a.map_process_copy_frame(2, 100, None).unwrap();
    assert_ne!(first, second);
    a.invalidate_process_copy_mappings(1).unwrap();
    assert_eq!(a.mappings.get(&second), Some(&100));
    assert_eq!(a.map_process_copy_frame(2, 100, None).unwrap(), second);
    assert_eq!(a.maps, 2);
}

#[test]
fn owner_bucket_collisions_and_failed_maps_remain_independently_tracked() {
    let mut a = Alpha::new();
    let first = a.map_process_copy_frame(1, 100, None).unwrap();
    a.fail_map = true;
    assert!(a.map_process_copy_frame(257, 200, None).is_err());
    a.fail_map = false;
    a.invalidate_process_copy_mappings(1).unwrap();
    assert!(!a.mappings.contains_key(&first));
    assert_eq!(a.mappings.len(), 1);
    a.fail_unmap = true;
    assert!(a.invalidate_process_copy_mappings(257).is_err());
    assert_eq!(a.mappings.len(), 1);
    a.fail_unmap = false;
    a.invalidate_process_copy_mappings(257).unwrap();
    assert!(a.mappings.is_empty());
    a.invalidate_process_copy_mappings(1).unwrap();
    a.invalidate_process_copy_mappings(257).unwrap();
    assert_eq!(a.maps, a.unmaps);
}

#[test]
fn eviction_unmap_failure_stops_replacement_and_is_recoverable() {
    let mut a = Alpha::new();
    a.fail_unmap = true;
    let mut rejected = None;
    for frame in 1..=memory_copy::COPY_WINDOW_PAGES + 1 {
        let before = a.mappings.clone();
        if a.map_process_copy_frame(1, frame, None).is_err() {
            assert_eq!(a.mappings, before);
            rejected = Some(frame);
            break;
        }
    }
    let frame = rejected.expect("a bounded cache must eventually attempt eviction");
    a.fail_unmap = false;
    a.map_process_copy_frame(1, frame, None).unwrap();
    a.invalidate_process_copy_mappings(1).unwrap();
    assert!(a.mappings.is_empty());
    assert_eq!(a.maps, a.unmaps);
}

#[test]
fn cache_window_stays_below_the_initialization_window() {
    assert!(0x6600_0000 + memory_copy::COPY_WINDOW_PAGES * PAGE_SIZE <= 0x6700_0000);
    assert!(std::mem::size_of::<memory_copy::CopyWindow>() <= 52 * 1024);
}

#[test]
fn permanent_ram_alias_is_reused_across_process_teardown() {
    let mut a = Alpha::new();
    let alias = a.map_process_copy_frame(1, POOL_FRAME + 100, None).unwrap();
    for pid in 1..=1000 {
        assert_eq!(
            a.map_process_copy_frame(pid, POOL_FRAME + 100, None)
                .unwrap(),
            alias
        );
        a.invalidate_process_copy_mappings(pid).unwrap();
        assert_eq!(a.mappings.get(&alias), Some(&(POOL_FRAME + 100)));
    }
    assert_eq!((a.maps, a.unmaps), (1, 0));
}

#[test]
fn transient_alias_cleanup_does_not_remove_pool_aliases_in_the_same_owner_bucket() {
    let mut a = Alpha::new();
    let permanent = a.map_process_copy_frame(1, POOL_FRAME + 100, None).unwrap();
    let transient = a.map_process_copy_frame(256, 200, None).unwrap();
    a.invalidate_process_copy_mappings(256).unwrap();
    assert_eq!(a.mappings.get(&permanent), Some(&(POOL_FRAME + 100)));
    assert!(!a.mappings.contains_key(&transient));
    a.fail_map = true;
    assert!(a.map_process_copy_frame(256, 300, None).is_err());
    a.fail_map = false;
    a.fail_unmap = true;
    assert!(a.invalidate_process_copy_mappings(256).is_err());
    a.fail_unmap = false;
    a.invalidate_process_copy_mappings(256).unwrap();
    assert_eq!(a.mappings.len(), 1);
}

#[test]
fn namespace_origin_not_numeric_magnitude_controls_alias_lifetime() {
    let mut a = Alpha::new();
    let permanent = a.map_process_copy_frame(1, POOL_FRAME + 1, None).unwrap();
    let transient = a
        .map_process_copy_frame(2, POOL_FRAME + 0x10000 + 1, None)
        .unwrap();
    a.invalidate_process_copy_mappings(1).unwrap();
    a.invalidate_process_copy_mappings(2).unwrap();
    assert_eq!(a.mappings.get(&permanent), Some(&(POOL_FRAME + 1)));
    assert!(!a.mappings.contains_key(&transient));
}
