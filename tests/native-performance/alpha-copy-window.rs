#![allow(dead_code)]
use std::collections::BTreeMap;

type CapabilityDescriptor = usize;
const PAGE_SIZE: usize = 4096;
const PROCESS_COPY_TEMP_BASE: usize = 0x6800_0000;
#[derive(Debug, PartialEq, Eq)]
enum CapabilityError {
    InvalidArgument,
    IllegalOperation,
}

const POOL_FRAME: usize = 0x4000_0000;
struct Memory;
impl Memory {
    fn map_private_large_alias(&mut self, _: usize, _: Option<usize>) -> Result<Option<(usize, usize)>, CapabilityError> { Ok(None) }
    fn physical_frame_descriptor_from_index(&self, page: usize) -> Option<usize> {
        (page < 0x10000).then_some(POOL_FRAME + page)
    }
    fn is_pool_frame_descriptor(&self, frame: usize) -> bool {
        frame & 0xffff_0000 == POOL_FRAME
    }
}

struct Alpha {
    memory: Memory,
    processes: Processes,
    copy_window: memory_copy::CopyWindow,
    initialization_window: memory_init::InitializationWindow,
    mappings: BTreeMap<usize, usize>,
    maps: usize,
    unmaps: usize,
    fail_map: bool,
    fail_unmap: bool,
    fail_after_unmaps: Option<usize>,
    unmap_attempts: Vec<(usize, usize)>,
}
impl Alpha {
    fn new() -> Self {
        Self {
            memory: Memory,
            processes: Processes::default(),
            copy_window: memory_copy::CopyWindow::new(),
            initialization_window: memory_init::InitializationWindow::new(),
            mappings: BTreeMap::new(),
            maps: 0,
            unmaps: 0,
            fail_map: false,
            fail_unmap: false,
            fail_after_unmaps: None,
            unmap_attempts: Vec::new(),
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
        self.unmap_attempts.push((frame, va));
        if self.fail_unmap || self.fail_after_unmaps == Some(self.unmaps) {
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
#[path = "../../nanami/src/nanami_core/alpha/memory_init.rs"]
mod memory_init;

#[test]
fn pool_copy_probe_requires_exact_ready_permanent_frame_without_mutating_windows() {
    let mut a = Alpha::new();
    assert_eq!(a.cached_pool_copy_frame(POOL_FRAME + 19), None);
    let alias = a.map_process_copy_frame(1, POOL_FRAME + 19, None).unwrap();
    a.map_process_copy_frame(2, 1234, None).unwrap();
    let before = (a.maps, a.unmaps, a.unmap_attempts.len());
    for _ in 0..10_000 {
        assert_eq!(a.cached_pool_copy_frame(POOL_FRAME + 19), Some(alias));
        assert_eq!(a.cached_pool_copy_frame(POOL_FRAME + 20), None);
        assert_eq!(a.cached_pool_copy_frame(1234), None);
    }
    assert_eq!((a.maps, a.unmaps, a.unmap_attempts.len()), before);
    a.fail_unmap = true;
    assert!(a.invalidate_process_copy_mappings(0).is_err());
    assert_eq!(a.cached_pool_copy_frame(POOL_FRAME + 19), None);
    assert_eq!(a.mappings[&alias], POOL_FRAME + 19);
    let mut a = Alpha::new();
    a.fail_map = true;
    assert!(a.map_process_copy_frame(1, POOL_FRAME + 20, None).is_err());
    assert_eq!(a.cached_pool_copy_frame(POOL_FRAME + 20), None);
    assert_eq!(a.mappings.len(), 1);
}

#[test]
fn destination_borrow_eligibility_preserves_ready_initializer_and_pending_cleanup() {
    let mut a = Alpha::new();
    assert!(a.initialization_page_needs_mapping(19));
    a.map_initialization_page(19).unwrap();
    assert!(!a.initialization_page_needs_mapping(19));
    assert!(a.initialization_page_needs_mapping(19 + memory_init::PAGES));
    a.fail_unmap = true;
    assert!(a.map_initialization_page(19 + memory_init::PAGES).is_err());
    assert!(!a.initialization_page_needs_mapping(19));
    assert!(!a.initialization_page_needs_mapping(19 + memory_init::PAGES));
    a.fail_unmap = false;
    a.map_initialization_page(19 + memory_init::PAGES).unwrap();
    assert!(!a.initialization_page_needs_mapping(19 + memory_init::PAGES));
    assert!(a.initialization_page_needs_mapping(19));
    a.fail_map = true;
    assert!(a.map_initialization_page(20).is_err());
    assert!(!a.initialization_page_needs_mapping(20));
    assert!(!a.initialization_page_needs_mapping(0x10000));
}

#[test]
fn copy_uses_exact_ready_initialization_alias_without_another_mapping_or_eviction() {
    let mut a = Alpha::new();
    let source = a.map_initialization_page(19).unwrap();
    let frame = POOL_FRAME + 19;
    for pid in [1, 2, 256, 999] {
        assert_eq!(a.map_process_transfer_frame(pid, frame, None).unwrap(), source);
        a.invalidate_process_copy_mappings(pid).unwrap();
    }
    assert_eq!((a.maps, a.unmaps), (1, 0));
    let destination = a.map_process_transfer_frame(2, 123, Some(source)).unwrap();
    assert_ne!(destination, source);
    assert_eq!(a.mappings[&source], frame);
    assert_eq!(a.map_process_transfer_frame(1, frame, Some(destination)).unwrap(), source);
    assert_eq!((a.maps, a.unmaps), (2, 0));
    a.invalidate_process_copy_mappings(2).unwrap();
    assert_eq!(a.mappings[&source], frame);
    assert!(!a.mappings.contains_key(&destination));
}

#[test]
fn initialization_slot_replacement_and_pending_errors_fall_back_to_tracked_copy_aliases() {
    let mut a = Alpha::new();
    let old = a.map_initialization_page(19).unwrap();
    let replacement = a.map_initialization_page(19 + memory_init::PAGES).unwrap();
    assert_eq!(old, replacement);
    let fallback = a.map_process_transfer_frame(1, POOL_FRAME + 19, Some(replacement)).unwrap();
    assert_ne!(fallback, replacement);
    assert_eq!(a.mappings[&replacement], POOL_FRAME + 19 + memory_init::PAGES);
    assert_eq!(a.mappings[&fallback], POOL_FRAME + 19);
    a.fail_map = true;
    assert!(a.map_initialization_page(20).is_err());
    a.fail_map = false;
    assert!(a.cached_initialization_frame(POOL_FRAME + 20).is_none());
    let pending = a.initialization_alias_address(20);
    let fallback = a.map_process_transfer_frame(2, POOL_FRAME + 20, Some(pending)).unwrap();
    assert_ne!(fallback, pending);
    assert_eq!(a.mappings[&pending], POOL_FRAME + 20);
    assert_eq!(a.mappings[&fallback], POOL_FRAME + 20);
    a.fail_unmap = true;
    assert!(a.map_initialization_page(20 + memory_init::PAGES).is_err());
    assert!(a.cached_initialization_frame(POOL_FRAME + 20).is_none());
    assert_eq!(a.map_process_transfer_frame(2, POOL_FRAME + 20, None).unwrap(), fallback);
}

#[test]
fn retained_copy_alias_survives_initialization_window_replacement() {
    let mut a = Alpha::new();
    let initial = a.map_initialization_page(19).unwrap();
    // COW detach obtains its source before mapping a new initialization page;
    // it must retain the stable CopyWindow contract rather than transfer reuse.
    let retained = a.map_process_copy_frame(1, POOL_FRAME + 19, None).unwrap();
    assert_ne!(retained, initial);
    a.map_initialization_page(19 + memory_init::PAGES).unwrap();
    assert_eq!(a.mappings[&retained], POOL_FRAME + 19);
    assert_eq!(a.mappings[&initial], POOL_FRAME + 19 + memory_init::PAGES);
}

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
fn source_fallback_cannot_evict_a_pinned_permanent_pool_destination() {
    let mut a = Alpha::new();
    let destination = a.map_process_copy_frame(0, POOL_FRAME + 100, None).unwrap();
    for frame in 200..10_200 {
        let source = a.map_process_copy_frame(17, frame, Some(destination)).unwrap();
        assert_ne!(source, destination);
        assert_eq!(a.mappings[&destination], POOL_FRAME + 100);
    }
    a.invalidate_process_copy_mappings(17).unwrap();
    assert_eq!(a.mappings.len(), 1);
    assert_eq!(a.mappings[&destination], POOL_FRAME + 100);
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
    assert_eq!(std::mem::size_of::<memory_copy::CopyWindow>(), 50176);
}

#[test]
fn owner_lists_preserve_scalar_unmap_order_and_failure_prefix_after_evictions() {
    for fail_prefix in [0, 1, 5, 31] {
        let mut a = Alpha::new();
        let mut owners = BTreeMap::new();
        for frame in 1..=3500 {
            let pid = [1, 257, 513, 2][frame % 4];
            let va = a.map_process_copy_frame(pid, frame, None).unwrap();
            owners.retain(|va, _| a.mappings.contains_key(va));
            owners.insert(va, pid);
            if frame % 97 == 0 { a.copy_window.assert_consistent(); }
        }
        // Pool aliases share bucket zero with PID256 but outlive its teardown.
        let pool = a.map_process_copy_frame(256, POOL_FRAME + 7, None).unwrap();
        owners.retain(|va, _| a.mappings.contains_key(va));
        owners.insert(pool, 0);
        let expected: Vec<_> = owners.iter().filter(|(_, pid)| **pid == 1)
            .map(|(va, _)| (a.mappings[va], *va)).collect();
        assert!(expected.len() > fail_prefix);
        let before = a.mappings.clone();
        a.unmap_attempts.clear();
        a.fail_after_unmaps = Some(a.unmaps + fail_prefix);
        assert!(a.invalidate_process_copy_mappings(1).is_err());
        assert_eq!(a.unmap_attempts, expected[..=fail_prefix]);
        let mut remaining = before;
        for &(_, va) in &expected[..fail_prefix] { remaining.remove(&va); }
        assert_eq!(a.mappings, remaining);
        a.copy_window.assert_consistent();
        a.fail_after_unmaps = None;
        a.unmap_attempts.clear();
        a.invalidate_process_copy_mappings(1).unwrap();
        assert_eq!(a.unmap_attempts, expected[fail_prefix..]);
        for &(_, va) in &expected[fail_prefix..] { remaining.remove(&va); }
        assert_eq!(a.mappings, remaining);
        for pid in [257, 513, 2, 256, 999] {
            a.invalidate_process_copy_mappings(pid).unwrap();
            a.copy_window.assert_consistent();
        }
        assert_eq!(a.mappings.len(), 1);
        assert_eq!(a.mappings.get(&pool), Some(&(POOL_FRAME + 7)));
    }
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
