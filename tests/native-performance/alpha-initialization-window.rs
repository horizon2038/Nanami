#![allow(dead_code)]
use std::collections::BTreeMap;
type CapabilityDescriptor = usize;
const PAGE_SIZE: usize = 4096;
const PROCESS_COPY_TEMP_BASE: usize = 0x6800_0000;
const INITIALIZATION_PAGES: usize = memory_init::PAGES;
#[derive(Debug, PartialEq, Eq)]
enum CapabilityError {
    InvalidArgument,
    IllegalOperation,
}
struct Memory { large_alias: bool }
impl Memory {
    fn map_private_large_alias(&mut self, frame: usize, _: Option<usize>) -> Result<Option<(usize, usize)>, CapabilityError> {
        let page = frame - 100_000;
        if self.large_alias && (512..1024).contains(&page) {
            return Ok(Some((0x6a00_0000 + (page - 512) * PAGE_SIZE, 1024 - page)));
        }
        Ok(None)
    }
    fn physical_frame_descriptor_from_index(&self, page: usize) -> Option<usize> {
        (page != 0).then_some(page + 100_000)
    }
}
struct Alpha {
    memory: Memory,
    initialization_window: memory_init::InitializationWindow,
    mappings: BTreeMap<usize, usize>,
    maps: usize,
    unmaps: usize,
    fail_map: bool,
    fail_unmap: bool,
}
impl Alpha {
    fn new() -> Self {
        Self {
            memory: Memory { large_alias: false },
            initialization_window: memory_init::InitializationWindow::new(),
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
        if self.fail_map {
            return Err(CapabilityError::IllegalOperation);
        }
        Ok(())
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
#[path = "../../nanami/src/nanami_core/alpha/memory_init.rs"]
mod memory_init;

#[test]
fn ready_slot_index_preserves_zero_slot_and_declines_evicted_or_pending_frames() {
    let mut a = Alpha::new();
    let old = INITIALIZATION_PAGES + 100_000;
    assert_eq!(a.cached_initialization_slot(old), None);
    a.map_initialization_page(INITIALIZATION_PAGES).unwrap();
    assert_eq!(a.cached_initialization_slot(old), Some(0));
    a.map_initialization_page(2 * INITIALIZATION_PAGES).unwrap();
    assert_eq!(a.cached_initialization_slot(old), None);
    assert_eq!(a.cached_initialization_slot(2 * INITIALIZATION_PAGES + 100_000), Some(0));
    a.fail_map = true;
    assert!(a.map_initialization_page(3 * INITIALIZATION_PAGES).is_err());
    assert_eq!(a.cached_initialization_slot(2 * INITIALIZATION_PAGES + 100_000), None);
    assert_eq!(a.cached_initialization_slot(3 * INITIALIZATION_PAGES + 100_000), None);
}

#[test]
fn ready_runs_stop_at_cold_pending_mismatched_and_wrapped_aliases() {
    let mut a = Alpha::new();
    assert!(a.map_initialization_run(1, 0).is_err());
    assert_eq!(a.maps, 0);
    for page in 1..=9 { a.map_initialization_page(page).unwrap(); }
    assert_eq!(a.map_initialization_run(1, 20).unwrap().1, 9);
    // Every successful probe refreshes the same hints as sequential mapping.
    let mut scalar = Alpha::new();
    for page in 1..=9 { scalar.map_initialization_page(page).unwrap(); }
    for page in 1..=9 { scalar.map_initialization_page(page).unwrap(); }
    for page in 1..=9 {
        assert_eq!(a.cached_initialization_frame(page + 100_000),
                   scalar.cached_initialization_frame(page + 100_000));
    }
    a.map_initialization_page(4 + INITIALIZATION_PAGES).unwrap();
    assert_eq!(a.map_initialization_run(1, 9).unwrap().1, 3);
    a.fail_map = true;
    assert!(a.map_initialization_page(4).is_err());
    assert_eq!(a.map_initialization_run(1, 9).unwrap().1, 3);
    a.fail_map = false;
    a.map_initialization_page(4).unwrap();
    assert_eq!(a.map_initialization_run(1, 9).unwrap().1, 9);
    for page in INITIALIZATION_PAGES - 2..=INITIALIZATION_PAGES + 2 {
        a.map_initialization_page(page).unwrap();
    }
    assert_eq!(a.map_initialization_run(INITIALIZATION_PAGES - 2, 5).unwrap().1, 2);
    assert_eq!(a.map_initialization_run(INITIALIZATION_PAGES, 3).unwrap().1, 3);
}

#[test]
fn repeated_process_initialization_uses_pool_capabilities_without_mapping_churn() {
    let mut a = Alpha::new();
    for _ in 0..50 {
        for page in 1..=512 {
            let va = a.map_initialization_page(page).unwrap();
            assert_eq!(a.mappings[&va], page + 100_000);
        }
    }
    assert_eq!((a.maps, a.unmaps), (512, 0));
}
#[test]
fn collisions_evict_before_mapping_and_have_a_fixed_space_bound() {
    let mut a = Alpha::new();
    for page in 1..5000 {
        a.map_initialization_page(page).unwrap();
    }
    assert_eq!(a.mappings.len(), INITIALIZATION_PAGES);
    assert_eq!(a.unmaps, a.maps - INITIALIZATION_PAGES);
}
#[test]
fn failed_map_after_installation_is_never_a_hit() {
    let mut a = Alpha::new();
    a.fail_map = true;
    assert!(a.map_initialization_page(1).is_err());
    a.fail_map = false;
    a.map_initialization_page(1).unwrap();
    assert_eq!((a.maps, a.unmaps), (2, 1));
}
#[test]
fn failed_unmap_keeps_the_original_mapping_tracked_and_prevents_replacement() {
    let mut a = Alpha::new();
    let va = a.map_initialization_page(1).unwrap();
    a.fail_unmap = true;
    let collision = INITIALIZATION_PAGES + 1;
    assert!(a.map_initialization_page(collision).is_err());
    assert_eq!(a.mappings[&va], 100_001);
    assert_eq!(a.maps, 1);
    a.fail_unmap = false;
    assert_eq!(a.map_initialization_page(collision).unwrap(), va);
    assert_eq!(a.mappings[&va], 100_000 + collision);
}
#[test]
fn unowned_physical_page_is_rejected_without_mapping() {
    let mut a = Alpha::new();
    assert!(a.map_initialization_page(0).is_err());
    assert_eq!(a.maps, 0);
}

#[test]
fn source_probe_is_read_only_and_never_accepts_collisions_or_failed_maps() {
    let mut a = Alpha::new();
    assert_eq!(a.cached_initialization_page(1), None);
    let address = a.map_initialization_page(1).unwrap();
    assert_eq!(a.cached_initialization_page(1), Some(address));
    assert_eq!(a.cached_initialization_page(1 + INITIALIZATION_PAGES), None);
    assert_eq!((a.maps, a.unmaps), (1, 0));
    a.fail_unmap = true;
    assert!(a.map_initialization_page(1 + INITIALIZATION_PAGES).is_err());
    assert_eq!(a.cached_initialization_page(1), None);
    assert_eq!((a.maps, a.unmaps), (1, 0));
    a.fail_unmap = false;
    a.fail_map = true;
    assert!(a.map_initialization_page(1 + INITIALIZATION_PAGES).is_err());
    assert_eq!(a.cached_initialization_page(1 + INITIALIZATION_PAGES), None);
    assert_eq!(a.cached_initialization_page(0), None);
}

#[test]
fn resident_span_above_old_capacity_does_not_churn() {
    let mut a = Alpha::new();
    for _ in 0..3 {
        for page in 1..=1500 {
            let va = a.map_initialization_page(page).unwrap();
            assert_eq!(a.mappings[&va], page + 100_000);
        }
    }
    assert_eq!((a.maps, a.unmaps), (1500, 0));
}

#[test]
fn cache_metadata_and_alias_addresses_remain_bounded() {
    assert_eq!(
        core::mem::size_of::<memory_init::InitializationWindow>(),
        INITIALIZATION_PAGES * 16
    );
    let mut a = Alpha::new();
    for page in 1..=INITIALIZATION_PAGES {
        let va = a.map_initialization_page(page).unwrap();
        assert!((0x6700_0000..PROCESS_COPY_TEMP_BASE).contains(&va));
        assert!(va + PAGE_SIZE <= PROCESS_COPY_TEMP_BASE);
    }
    assert_eq!(a.mappings.len(), INITIALIZATION_PAGES);
}

#[test]
fn failed_map_retains_cleanup_ownership_through_a_failed_unmap() {
    let mut a = Alpha::new();
    a.fail_map = true;
    assert!(a.map_initialization_page(1).is_err());
    a.fail_map = false;
    a.fail_unmap = true;
    assert!(a.map_initialization_page(1).is_err());
    assert!(a.map_initialization_page(INITIALIZATION_PAGES + 1).is_err());
    assert_eq!(a.maps, 1);
    assert_eq!(a.unmaps, 0);
    a.fail_unmap = false;
    let va = a.map_initialization_page(1).unwrap();
    assert_eq!(a.mappings[&va], 100_001);
    assert_eq!((a.maps, a.unmaps), (2, 1));
}

#[test]
fn frame_probe_reuses_only_the_exact_ready_alias_and_never_changes_mappings() {
    let mut a=Alpha::new();let frame=100_001;
    assert_eq!(a.cached_initialization_frame(frame),None);
    let va=a.map_initialization_page(1).unwrap();
    assert_eq!(a.cached_initialization_frame(frame),Some(va));
    assert_eq!(a.cached_initialization_frame(0),None);
    assert_eq!(a.cached_initialization_frame(999_999),None);
    assert_eq!((a.maps,a.unmaps),(1,0));
    a.map_initialization_page(1+INITIALIZATION_PAGES).unwrap();
    assert_eq!(a.cached_initialization_frame(frame),None);
    assert_eq!(a.cached_initialization_frame(frame+INITIALIZATION_PAGES),Some(va));
    assert_eq!((a.maps,a.unmaps),(2,1));
}

#[test]
fn frame_hints_decline_after_failed_unmap_and_post_install_map_failure() {
    let mut a=Alpha::new();a.map_initialization_page(1).unwrap();
    a.fail_unmap=true;
    assert!(a.map_initialization_page(1+INITIALIZATION_PAGES).is_err());
    assert_eq!(a.cached_initialization_frame(100_001),None);
    a.fail_unmap=false;a.fail_map=true;
    assert!(a.map_initialization_page(1+INITIALIZATION_PAGES).is_err());
    assert_eq!(a.cached_initialization_frame(100_001+INITIALIZATION_PAGES),None);
    a.fail_map=false;
    let va=a.map_initialization_page(1+INITIALIZATION_PAGES).unwrap();
    assert_eq!(a.cached_initialization_frame(100_001+INITIALIZATION_PAGES),Some(va));
    assert_eq!((a.maps,a.unmaps),(3,2));
}

#[test]
fn frame_hash_collisions_keep_every_ready_alias_without_mapping() {
    let mut a=Alpha::new();
    let mut old_hints = [0usize; INITIALIZATION_PAGES];
    for page in 1..=1500 {
        a.map_initialization_page(page).unwrap();
        old_hints[original_bucket(page + 100_000)] = page;
    }
    let mut old_misses=0;
    for page in 1..=1500 {
        let expected=a.cached_initialization_page(page).unwrap();
        if old_hints[original_bucket(page + 100_000)] != page { old_misses += 1; }
        assert_eq!(a.cached_initialization_frame(page+100_000), Some(expected));
    }
    assert!(old_misses>0);assert_eq!((a.maps,a.unmaps),(1500,0));
    println!("resident=1500 original_hint_misses={old_misses} indexed_misses=0 maps=1500 unmaps=0");
    a.initialization_window.assert_consistent();
    assert_eq!(core::mem::size_of::<memory_init::InitializationWindow>(),INITIALIZATION_PAGES*16);
}

#[test]
fn twelve_mib_resident_working_set_avoids_compact_window_remapping() {
    let mut a = Alpha::new();
    for _ in 0..3 {
        for page in 1..=3072 {
            let address = a.map_initialization_page(page).unwrap();
            assert_eq!(a.mappings[&address], page + 100_000);
        }
    }
    // The compact layout retains the middle 1024 pages between passes; the
    // two colliding ends each remap another 1024 pages on each later pass.
    let expected = if INITIALIZATION_PAGES >= 3072 { (3072, 0) } else { (7168, 5120) };
    assert_eq!((a.maps, a.unmaps), expected);
    assert!(core::mem::size_of::<memory_init::InitializationWindow>() <= 64 * 1024);
    a.initialization_window.assert_consistent();
}

#[test]
fn available_huge_alias_precedes_ready_small_alias_and_covers_the_remaining_span() {
    let mut a = Alpha::new();
    let small = a.map_initialization_page(519).unwrap();
    a.memory.large_alias = true;
    a.fail_map = true;
    a.fail_unmap = true;
    assert_eq!(a.map_initialization_page(519).unwrap(), 0x6a00_7000);
    assert_eq!(a.map_initialization_run(519, 700).unwrap(), (0x6a00_7000, 505));
    assert_eq!(a.map_initialization_run(512, 2).unwrap(), (0x6a00_0000, 2));
    assert_eq!((a.maps, a.unmaps), (1, 0));
    assert_eq!(a.mappings[&small], 519 + 100_000);
    a.initialization_window.assert_consistent();
}

fn original_bucket(frame: usize) -> usize {
    let mut hash = frame as u64;
    hash ^= hash >> 30; hash = hash.wrapping_mul(0xbf58476d1ce4e5b9);
    hash ^= hash >> 27; hash = hash.wrapping_mul(0x94d049bb133111eb);
    (hash ^ (hash >> 31)) as usize & (INITIALIZATION_PAGES - 1)
}

#[test]
fn bucket_heads_survive_their_physical_rows_becoming_ready_pending_and_replaced() {
    let bucket = (original_bucket(INITIALIZATION_PAGES + 100_000) + 1) % INITIALIZATION_PAGES;
    let mut pages = Vec::new();
    for page in 1usize.. {
        if page % INITIALIZATION_PAGES != bucket && original_bucket(page + 100_000) == bucket
            && !pages.iter().any(|old| old % INITIALIZATION_PAGES == page % INITIALIZATION_PAGES) {
            pages.push(page);
            if pages.len() == 32 { break; }
        }
    }
    let mut a = Alpha::new();
    for &page in &pages { a.map_initialization_page(page).unwrap(); }
    let row = if bucket == 0 { INITIALIZATION_PAGES } else { bucket };
    a.map_initialization_page(row).unwrap();
    a.fail_unmap = true; assert!(a.map_initialization_page(row + INITIALIZATION_PAGES).is_err());
    a.initialization_window.assert_consistent();
    for &page in &pages { assert_eq!(a.cached_initialization_frame(page + 100_000), a.cached_initialization_page(page)); }
    a.fail_unmap = false; a.fail_map = true;
    assert!(a.map_initialization_page(row + INITIALIZATION_PAGES).is_err());
    a.initialization_window.assert_consistent();
    for &page in &pages { assert_eq!(a.cached_initialization_frame(page + 100_000), a.cached_initialization_page(page)); }
    a.fail_map = false; a.map_initialization_page(row + INITIALIZATION_PAGES).unwrap();
    for &index in &[31, 16, 0, 15, 17] {
        let page = pages[index]; a.map_initialization_page(page + INITIALIZATION_PAGES).unwrap();
        assert_eq!(a.cached_initialization_frame(page + 100_000), None);
        a.initialization_window.assert_consistent();
    }
}

#[test]
fn indexed_initialization_keeps_original_map_unmap_and_failure_state_through_churn() {
    let mut a = Alpha::new();
    let mut tags = [None::<(usize, bool)>; INITIALIZATION_PAGES];
    let mut backend = BTreeMap::new(); let mut maps = 0; let mut unmaps = 0;
    let mut rng = 0x19c7_ab3du32;
    for step in 0..5000 {
        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        let page = 1 + rng as usize % (6 * INITIALIZATION_PAGES);
        let index = page % INITIALIZATION_PAGES; let frame = page + 100_000;
        let va = 0x6700_0000 + index * PAGE_SIZE;
        a.fail_map = step % 29 == 0; a.fail_unmap = step % 31 == 0;
        let expected = (|| {
            if let Some((old, ready)) = tags[index] {
                if ready && old == frame { return Ok(va); }
                tags[index] = Some((old, false));
                if a.fail_unmap { return Err(CapabilityError::IllegalOperation); }
                assert_eq!(backend.remove(&va), Some(old)); unmaps += 1; tags[index] = None;
            }
            tags[index] = Some((frame, false));
            assert!(backend.insert(va, frame).is_none()); maps += 1;
            if a.fail_map { return Err(CapabilityError::IllegalOperation); }
            tags[index] = Some((frame, true)); Ok(va)
        })();
        assert_eq!(a.map_initialization_page(page), expected, "step={step}");
        assert_eq!((a.maps, a.unmaps), (maps, unmaps)); assert_eq!(a.mappings, backend);
        a.initialization_window.assert_consistent();
        for (slot, tag) in tags.iter().enumerate() {
            if let Some((frame, ready)) = *tag {
                assert_eq!(a.cached_initialization_frame(frame), ready.then_some(0x6700_0000 + slot * PAGE_SIZE));
            }
        }
    }
}
