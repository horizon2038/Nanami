//! Production VM metadata, with no mocked indexing or permission rules.
#![allow(dead_code)]
extern crate alloc;

#[test]
fn snapshot_page_cursor_replaces_repeated_dense_queries_without_allocating() {
    let base = 0x800000;
    let mut vm = VmSpace::new();
    for page in 0..1024 {
        let va = base + page * 4096;
        vm.record_frame(va, page + 5).unwrap();
        vm.mark_private_page_shared(va, true);
    }
    VmSpace::take_snapshot_point_queries();
    for page in 0..1024 {
        let va = base + page * 4096;
        assert!(vm.private_page_shared(va));
        assert_eq!(vm.find_frame(va), Some(page + 5));
    }
    assert_eq!(VmSpace::take_snapshot_point_queries(), (1024, 1024));
    for references in [1, 2] {
        VmSpace::take_snapshot_shared_word_queries();
        DENIED.with(|count| count.set(0));
        DENY_AT_OR_ABOVE.with(|limit| limit.set(0));
        let mut cursor = vm.snapshot_page_cursor();
        for page in (0..1024).rev() {
            assert!(cursor.page_matches(base + page * 4096, page + 5, usize::MAX, references));
        }
        DENY_AT_OR_ABOVE.with(|limit| limit.set(usize::MAX));
        assert_eq!(cursor.diagnostics(), (16, 16, 0));
        assert_eq!(VmSpace::take_snapshot_point_queries(), (0, 0));
        assert_eq!(VmSpace::take_snapshot_shared_word_queries(), if references == 1 { 0 } else { 16 });
        assert_eq!(DENIED.with(Cell::get), 0);
    }
    println!("SNAPSHOT-VM-CURSOR pages=1024 frame+sharing-points=2048->0 selected-words=16 allocations=0");
}

#[test]
fn snapshot_page_cursor_matches_sparse_dense_holes_and_fresh_views_after_mutation() {
    let base = 0x800000;
    let top = usize::MAX & !4095;
    let mut vm = VmSpace::new();
    for page in 0..130 {
        if page < 64 || page % 17 == 0 {
            vm.record_frame(base + page * 4096, page).unwrap();
            vm.mark_private_page_shared(base + page * 4096, page % 3 != 0);
        }
    }
    vm.record_frame(base + 3, 77).unwrap();
    vm.mark_private_page_shared(base + 3, true);
    vm.record_frame(top, usize::MAX).unwrap();
    vm.mark_private_page_shared(top, true);
    for phase in 0..3 {
        let mut cursor = vm.snapshot_page_cursor();
        for page in (0..130).rev().chain(0..130) {
            for va in [base + page * 4096, base + 3, top, top - 4096] {
                for references in [0, 1, 2, 7] {
                    for (pool, child) in [(page, 999), (999, page), (0, 0), (usize::MAX, 999)] {
                        let expected = references != 0 && (references == 1 || vm.private_page_shared(va))
                            && vm.find_frame(va).is_some_and(|frame| frame == pool || frame == child);
                        assert_eq!(cursor.page_matches(va, pool, child, references), expected,
                            "phase={phase} va={va:x} references={references} pool={pool} child={child}");
                    }
                }
            }
        }
        drop(cursor);
        vm.forget_frame(base + (phase + 5) * 4096);
        vm.record_frame(base + 17 * 4096, 999).unwrap();
        vm.mark_private_page_shared(base + 17 * 4096, phase % 2 == 0);
        vm.mark_private_page_shared(base + 3, phase % 2 != 0);
        vm.forget_frame(top);
    }
}

fn scalar_immutable_source(vm: &VmSpace, start: usize, end: usize) -> bool {
    vm.fully_backed_aligned_range(start, end)
        && !vm.has_uncertain_mappings(start, end)
        && (start..end).step_by(4096).all(|va| matches!(vm.permissions(va), 0 | 1))
}

#[test]
fn immutable_source_words_match_scalar_rights_backing_and_uncertainty_at_boundaries() {
    const PAGE: usize = 4096;
    let base = 3 * PAGE;
    let mut vm = VmSpace::new();
    for i in 0..160 {
        vm.record_frame(base + i * PAGE, i + 17).unwrap();
        vm.set_permissions(base + i * PAGE, usize::from(i % 7 == 0));
    }
    // Unaligned metadata is not queried by the legacy page-key walk.
    vm.record_frame(base + 1, 77).unwrap();
    vm.set_permissions(base + 1, usize::MAX);
    for rights in [0, 1, 2, 3, 4, 5, 6, 7, 8, 17, usize::MAX] {
        for i in [0, 1, 60, 61, 62, 125, 159] {
            let va = base + i * PAGE;
            vm.set_permissions(va, rights);
            for (first, last) in [(0, 1), (0, 60), (1, 61), (60, 62), (61, 64), (64, 128), (125, 160), (0, 160)] {
                let start = base + first * PAGE; let end = base + last * PAGE;
                assert_eq!(vm.immutable_source_backed_range(start, end), scalar_immutable_source(&vm, start, end),
                    "rights={rights} page={i} range={first}..{last}");
            }
            vm.set_permissions(va, usize::from(i % 7 == 0));
        }
    }
    for key in [base + 1, base + 61 * PAGE, base + 159 * PAGE] {
        vm.mark_mapping_uncertain(key);
        assert!(!vm.immutable_source_backed_range(base, base + 160 * PAGE));
        assert!(!scalar_immutable_source(&vm, base, base + 160 * PAGE));
        vm.confirm_unmapped(key);
        vm.set_permissions(key, if key & 4095 == 0 { 0 } else { usize::MAX });
    }
    let frame = vm.forget_frame(base + 61 * PAGE).unwrap();
    assert!(!vm.immutable_source_backed_range(base, base + 160 * PAGE));
    vm.record_frame(base + 61 * PAGE, frame).unwrap();
    vm.set_permissions(base + 61 * PAGE, 0);
    assert!(vm.immutable_source_backed_range(base, base + 160 * PAGE));
    for (start, end) in [(0, 0), (base + PAGE, base), (base + 1, base + PAGE), (base, base + PAGE + 1), (base, base + 161 * PAGE)] {
        assert!(!vm.immutable_source_backed_range(start, end));
    }
    // Sparse permission entries and a top-of-address-space partial word.
    let top = usize::MAX & !4095;
    let mut edge = VmSpace::new();
    for i in 1..=3 {
        edge.record_frame(top - i * PAGE, i).unwrap();
        edge.set_permissions(top - i * PAGE, i & 1);
    }
    assert!(edge.immutable_source_backed_range(top - 3 * PAGE, top));
    assert!(!edge.immutable_source_backed_range(top - 4 * PAGE, top));
}

#[test]
fn immutable_source_permission_words_replace_point_queries_without_allocating() {
    let base = 0x800000;
    let end = base + 1024 * 4096;
    let mut vm = VmSpace::new();
    for i in 0..1024 {
        vm.record_frame(base + i * 4096, i + 1).unwrap();
        vm.set_permissions(base + i * 4096, i & 1);
    }
    VmSpace::take_immutable_permission_counts();
    assert!(scalar_immutable_source(&vm, base, end));
    assert_eq!(VmSpace::take_immutable_permission_counts(), (1024, 0));
    DENIED.with(|count| count.set(0));
    DENY_AT_OR_ABOVE.with(|limit| limit.set(0));
    let valid = vm.immutable_source_backed_range(base, end);
    DENY_AT_OR_ABOVE.with(|limit| limit.set(usize::MAX));
    assert!(valid);
    assert_eq!(DENIED.with(Cell::get), 0);
    assert_eq!(VmSpace::take_immutable_permission_counts(), (0, 16));
    println!("IMMUTABLE-SOURCE-WORDS pages=1024 point_before=1024 point_after=0 permission_words=16 allocations=0");
}

#[test]
fn unmapped_frame_words_keep_slice_identity_and_flags_without_claiming_a_leaf_table() {
    use vm_space::{FRAME_WORD_BYTES,FRAME_WORD_PAGES};
    let base=0x800000;
    let frames=core::array::from_fn::<_,FRAME_WORD_PAGES,_>(|page| 0x80000000+page*17);
    let mut vm=VmSpace::new();
    vm.record_frame(base+1,77).unwrap();
    vm.mark_private_page_shared(base+4096,true);
    vm.mark_private_page_cow_write(base+4096,true);
    vm.record_unmapped_frame_word(base,&frames).unwrap();
    assert!(!vm.page_tables_ready(base));
    assert_eq!(vm.find_frame(base+1),Some(77));
    for (page,&frame) in frames.iter().enumerate() {
        let va=base+page*4096;
        assert_eq!(vm.find_frame(va),Some(frame));assert_eq!(vm.permissions(va),0);
        assert_eq!(vm.hardware_permissions(va),0);assert!(!vm.mapping_deferred(va));
        assert!(!vm.mapping_is_uncertain(va));assert!(!vm.readonly_backed_range(va,va+4096));
    }
    assert!(vm.private_page_shared(base+4096));assert!(vm.private_page_cow_write(base+4096));
    assert!(vm.record_unmapped_frame_word(base,&frames).is_err());
    assert!(vm.record_unmapped_frame_word(base+4096,&frames).is_err());
    assert!(vm.record_unmapped_frame_word(usize::MAX&!(FRAME_WORD_BYTES-1),&frames).is_err());
    let mut uncertain=VmSpace::new();uncertain.mark_mapping_uncertain(base+7*4096);
    assert!(uncertain.record_unmapped_frame_word(base,&frames).is_err());
    assert_eq!(uncertain.find_frame(base),None);
    assert!(uncertain.mapping_is_uncertain(base+7*4096));
}

#[test]
fn aligned_frame_walk_stops_at_a_hole_and_ignores_unaligned_records() {
    let mut vm = VmSpace::new();
    let base = 0x240000;
    for index in 0..130 {
        let address = base + index * 4096;
        vm.record_frame(address + 1, 100 + index).unwrap();
        if index != 99 {
            vm.record_frame(address, 1000 + index).unwrap();
        }
    }
    let mut seen = Vec::new();
    assert!(vm.visit_aligned_frame_run(base, base + 99 * 4096, |frame| seen.push(frame)));
    assert_eq!(seen, (1000..1099).collect::<Vec<_>>());
    seen.clear();
    assert!(!vm.visit_aligned_frame_run(base, base + 130 * 4096, |frame| seen.push(frame)));
    assert_eq!(seen, (1000..1099).collect::<Vec<_>>());
    assert!(!vm.visit_aligned_frame_run(base + 1, base + 4096, |_| panic!("unaligned source")));
}

#[test]
fn deferred_frame_words_match_point_records_and_keep_hardware_absent() {
    use vm_space::{FRAME_WORD_BYTES, FRAME_WORD_PAGES};
    const PAGE: usize = 4096;
    let base = 0x240000;
    let end = base + FRAME_WORD_BYTES;
    let frames = core::array::from_fn::<_, FRAME_WORD_PAGES, _>(|page| 1000 + 3 * page);
    for (shared, uncertain) in [(false, false), (true, false), (true, true)] {
        let mut scalar = VmSpace::new();
        let mut bulk = VmSpace::new();
        for vm in [&mut scalar, &mut bulk] {
            vm.record_frame(base - PAGE, 99).unwrap();
            vm.mark_private_page_cow_write(base - PAGE, true);
            vm.set_permissions(base + 7 * PAGE, 17);
            if uncertain { vm.mark_mapping_uncertain(base + 22 * PAGE); }
        }
        scalar.prepare_frame_word(base, frames.len());
        for (index, &frame) in frames.iter().enumerate() {
            let va = base + index * PAGE;
            if shared { scalar.mark_private_page_shared(va, true); }
            scalar.mark_mapping_deferred(va);
            scalar.record_frame(va, frame).unwrap();
            scalar.set_permissions(va, 1);
        }
        bulk.record_deferred_frame_word(base, &frames, shared).unwrap();
        assert!(bulk.record_deferred_frame_word(base, &frames, false).is_err());
        assert!(bulk.record_deferred_frame_word(base + PAGE, &frames, false).is_err());
        assert!(bulk.readonly_backed_range(base, end) == !uncertain);
        assert_eq!(bulk.page_tables_ready(base), scalar.page_tables_ready(base));
        for va in (base - PAGE..end + PAGE).step_by(PAGE) {
            assert_eq!(bulk.find_frame(va), scalar.find_frame(va));
            assert_eq!(bulk.permissions(va), scalar.permissions(va));
            assert_eq!(bulk.hardware_permissions(va), scalar.hardware_permissions(va));
            assert_eq!(bulk.private_page_shared(va), scalar.private_page_shared(va));
            assert_eq!(bulk.private_page_cow_write(va), scalar.private_page_cow_write(va));
            assert_eq!(bulk.mapping_deferred(va), scalar.mapping_deferred(va));
            assert_eq!(bulk.snapshot_page_managed(va), scalar.snapshot_page_managed(va));
            assert_eq!(bulk.frame_may_be_present(va), scalar.frame_may_be_present(va));
            assert_eq!(bulk.readonly_backed_range(va, va + PAGE), scalar.readonly_backed_range(va, va + PAGE));
        }
        bulk.complete_deferred_mapping(base + PAGE);
        assert_eq!(bulk.hardware_permissions(base + PAGE), 1);
        assert_eq!(bulk.hardware_permissions(base + 2 * PAGE), 0);
        assert_eq!(bulk.forget_frame(base + 2 * PAGE), Some(frames[2]));
        assert!(!bulk.snapshot_page_managed(base + 2 * PAGE));
    }
}

#[test]
fn whole_permission_words_match_point_updates_for_packed_sparse_and_uncertain_backing() {
    const WORD: usize = 64 * 4096;
    fn fixture() -> VmSpace {
        let mut vm = VmSpace::new();
        for word in [0, 1, 3] { vm.prepare_frame_word(word * WORD, 64); }
        for page in 0..256 {
            let va = page * 4096;
            if page < 64 || (page < 128 && page % 2 == 0) || page >= 192 {
                vm.record_frame(va, page + 1000).unwrap();
            }
        }
        vm.record_frame(2 * WORD, 777).unwrap();
        vm.mark_mapping_uncertain(64 * 4096);
        vm.mark_mapping_deferred(2 * WORD);
        vm.mark_mapping_deferred(4096);
        vm.mark_private_page_cow_write(4 * 4096, true);
        vm.set_permissions(3 * WORD, 17);
        vm.record_frame(4097, 555).unwrap();
        vm.set_permissions(4097, 19);
        vm
    }
    let mut bulk = fixture();
    let mut scalar = fixture();
    for permissions in [1, 0, 3, 5, 7, 1, 2, 4, 6] {
        bulk.set_permission_words(0, 4 * WORD, permissions);
        for va in (0..4 * WORD).step_by(4096) { scalar.set_permissions(va, permissions); }
        for va in (0..4 * WORD).step_by(4096).chain([4097]) {
            assert_eq!(bulk.permissions(va), scalar.permissions(va));
            assert_eq!(bulk.hardware_permissions(va), scalar.hardware_permissions(va));
            assert_eq!(bulk.find_frame(va), scalar.find_frame(va));
            assert_eq!(bulk.frame_may_be_present(va), scalar.frame_may_be_present(va));
            assert_eq!(bulk.snapshot_page_managed(va), scalar.snapshot_page_managed(va));
            assert_eq!(bulk.mapping_deferred(va), scalar.mapping_deferred(va));
            assert_eq!(bulk.readonly_backed_range(va, va + 4096), scalar.readonly_backed_range(va, va + 4096));
        }
        for base in (0..4 * WORD).step_by(WORD) {
            assert_eq!(bulk.readonly_backed_range(base, base + WORD), scalar.readonly_backed_range(base, base + WORD));
        }
    }
}

#[test]
fn bootstrap_large_leaf_never_claims_a_four_kib_leaf_table_or_process_leaf() {
    std::thread::Builder::new().stack_size(8*1024*1024).spawn(|| {
    let mut boot=BootstrapVmSpace::new();
    assert!(boot.record_bootstrap_frame(0x58000000,123,21).is_ok());
    assert_eq!(boot.find_frame(0x58000000),Some(123));
    assert_eq!(boot.find_frame(0x58001000),None);
    assert!(!boot.page_tables_ready(0x58000000));
    assert!(boot.record_bootstrap_frame(0x58201000,124,21).is_err());
    assert!(boot.record_bootstrap_frame(0x58200000,124,30).is_err());
    let mut process=VmSpace::new();
    assert!(process.record_bootstrap_frame(0x58000000,123,21).is_err());
    assert_eq!(process.find_frame(0x58000000),None);
    }).unwrap().join().unwrap();
}

#[test]
fn aligned_collision_range_matches_point_walk_with_scalar_and_packed_frames() {
    let mut vm = VmSpace::new();
    for page in 0..192 {
        vm.record_frame(page * 4096 + 1, 5000 + page).unwrap();
        if page % 3 != 0 {
            vm.record_frame(page * 4096, 1000 + page).unwrap();
            vm.set_permissions(page * 4096, page % 8);
            if page % 7 == 0 { vm.mark_mapping_uncertain(page * 4096); }
        }
    }
    for start in 0..=195 {
        for length in [0, 1, 2, 3, 63, 64, 65, 128, 192] {
            let low = start * 4096;
            let high = low + length * 4096;
            let expected = (low..high).step_by(4096).any(|va| vm.find_frame(va).is_some());
            assert_eq!(vm.has_backed_aligned_pages(low, high), expected, "{start}/{length}");
        }
    }
    let mut scalar = VmSpace::new();
    scalar.record_frame(4097, 1).unwrap();
    assert!(!scalar.has_backed_aligned_pages(4096, 8192));
    assert!(!scalar.has_backed_aligned_pages(8192, 4096));
    scalar.record_frame(8192, 2).unwrap();
    scalar.set_permissions(8192, 0);
    scalar.mark_mapping_uncertain(8192);
    assert!(!scalar.has_backed_aligned_pages(4096, 8192));
    assert!(scalar.has_backed_aligned_pages(8192, 12288));
    let high = usize::MAX & !4095;
    scalar.record_frame(high - 4096, 3).unwrap();
    assert!(scalar.has_backed_aligned_pages(high - 8192, high));
}

#[test]
fn full_backing_preflight_and_ordered_metadata_match_aligned_point_lookups() {
    for hole in [None, Some(0), Some(63), Some(64), Some(127)] {
        let mut vm = VmSpace::new();
        for page in 0..128 {
            let va = page * 4096;
            if hole != Some(page) { vm.record_frame(va, 1000 + page).unwrap(); }
            vm.set_permissions(va, page % 8);
            if page % 17 == 0 { vm.mark_mapping_uncertain(va); }
            // Extra scalar keys are not part of the aligned protection walk.
            vm.record_frame(va + 1, 2000 + page).unwrap();
            vm.set_permissions(va + 1, 8);
        }
        for start in [0, 1, 63, 64, 127] {
            for end in [0, 1, 64, 65, 128, 129] {
                let first = start * 4096;
                let last = end * 4096;
                let expected = first < last && (first..last).step_by(4096).all(|va| vm.find_frame(va).is_some());
                assert_eq!(vm.fully_backed_aligned_range(first, last), expected);
                if expected {
                    let mut streamed = Vec::new();
                    vm.visit_backed_metadata(first, last, |va, frame, rights, uncertain| {
                        if va & 4095 == 0 { streamed.push((va, frame, if uncertain { 0 } else { rights })); }
                    });
                    let points: Vec<_> = (first..last).step_by(4096)
                        .map(|va| (va, vm.find_frame(va).unwrap(), vm.permissions(va))).collect();
                    assert_eq!(streamed, points);
                }
            }
        }
        assert!(!vm.fully_backed_aligned_range(1, 4096));
        assert!(!vm.fully_backed_aligned_range(0, 4095));
    }
}

#[test]
fn confirmed_none_ranges_match_scalar_metadata_with_holes_and_uncertainty() {
    let mut vm = VmSpace::new();
    for page in 0..513 {
        let va = page * 4096;
        vm.set_permissions(va, 0);
        if page % 13 == 0 { vm.record_frame(va, page + 1).unwrap(); }
    }
    for changed in [0, 1, 63, 64, 65, 511, 512] {
        for rights in 0..=8 {
            let va = changed * 4096;
            vm.set_permissions(va, rights);
            for uncertain in [false, true] {
                if uncertain { vm.mark_mapping_uncertain(va); }
                for start in [0, 4096, 63 * 4096, 64 * 4096, 511 * 4096] {
                    for end in [start, start + 4096, 65 * 4096, 512 * 4096, 513 * 4096, 514 * 4096] {
                        let expected = start < end && (start..end).step_by(4096)
                            .all(|p| !vm.mapping_is_uncertain(p) && vm.permissions(p) == 0);
                        assert_eq!(vm.confirmed_zero_permissions(start, end), expected);
                        // Backing adds a requirement; it cannot turn a failed
                        // confirmed-zero proof into a successful zero no-op.
                        if vm.backed_range_with_permissions(start, end, 0) {
                            assert!(vm.confirmed_zero_permissions(start, end));
                        }
                    }
                }
                assert!(!vm.confirmed_zero_permissions(1, 4096));
                assert!(!vm.confirmed_zero_permissions(0, 4095));
                if uncertain { vm.confirm_unmapped(va); vm.set_permissions(va, rights); }
            }
            vm.set_permissions(va, 0);
        }
    }
    vm.mark_mapping_uncertain(1);
    assert!(vm.confirmed_zero_permissions(0, 513 * 4096));
}
extern crate self as nun;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeMap;
pub type CapabilityDescriptor = usize;
pub mod avl {
    include!(env!("NANAMI_AVL_SOURCE"));
}
#[path = "../../nanami/src/nanami_utils/static_avl.rs"]
pub mod static_avl;
mod nanami_utils {
    pub use crate::{avl, static_avl};
}
mod vm_space {
    include!(env!("NANAMI_VM_SPACE_SOURCE"));

    pub fn unreserved_backed_pages(vm: &VmSpace, start: usize, end: usize) -> Vec<(usize, usize)> {
        let mut pages = Vec::new();
        vm.visit_backed_metadata(start, end, |va, _, rights, uncertain| {
            pages.push((va, if uncertain { 0 } else { rights }));
        });
        pages
    }

    pub fn dense_count(vm: &VmSpace, start: usize, end: usize) -> usize {
        vm.frame_by_va.dense_range_count(start, end)
    }

    // Previous registration path, including its redundant presence lookup
    // and ineligible-bit clear. Keep it independent of record_frame below.
    fn old_record_frame(vm: &mut VmSpace, va: usize, frame: usize) {
        vm.frame_by_va.insert(va, frame).unwrap();
        let permissions = vm.permissions.get(va).unwrap_or(7);
        vm.update_readonly_backing(va, permissions);
        let region = va >> PAGE_TABLE_REGION_SHIFT;
        if !vm.page_table_regions.contains(&region) {
            vm.page_table_regions[vm.next_page_table_region] = region;
            vm.next_page_table_region = (vm.next_page_table_region + 1) % BOOTSTRAP_PT_REGIONS;
        }
    }

    #[test]
    fn known_inserted_frame_presence_preserves_readonly_metadata_after_all_mutations() {
        let mut vm = VmSpace::new();
        let mut old = VmSpace::new();
        let mut random = 91u64;
        for turn in 0..12000 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let page = (random >> 32) as usize % 192;
            let va = match page {
                0 => usize::MAX,
                1 => usize::MAX & !4095,
                2 => 1,
                _ => page * 4096,
            };
            match turn % 7 {
                0 | 1 | 2 => {
                    let frame = if turn % 2 == 0 { 0 } else { usize::MAX };
                    vm.record_frame(va, frame).unwrap();
                    old_record_frame(&mut old, va, frame);
                }
                3 => {
                    let rights = (random >> 48) as usize % 10;
                    vm.set_permissions(va, rights); old.set_permissions(va, rights);
                }
                4 => { vm.mark_mapping_uncertain(va); old.mark_mapping_uncertain(va); }
                5 => { vm.confirm_unmapped(va); old.confirm_unmapped(va); }
                _ => { assert_eq!(vm.forget_frame(va), old.forget_frame(va)); }
            }
            assert_eq!(vm.readonly_backing, old.readonly_backing);
            assert_eq!(vm.find_frame(va), old.find_frame(va));
            assert_eq!(vm.permissions(va), old.permissions(va));
            assert_eq!(vm.page_tables_ready(va), old.page_tables_ready(va));
        }
        assert_eq!(vm.backed_pages(0,usize::MAX), old.backed_pages(0,usize::MAX));
        // Rights-before-Frame, replacement, downgrade, uncertainty and removal
        // all retain exact bitmap semantics, including descriptor zero.
        for rights in 0..=9 {
            for page in 0..128 {
                let va = page * 4096;
                vm.set_permissions(va,rights); old.set_permissions(va,rights);
                vm.record_frame(va,0).unwrap(); old_record_frame(&mut old,va,0);
                vm.record_frame(va,usize::MAX).unwrap(); old_record_frame(&mut old,va,usize::MAX);
                assert_eq!(vm.readonly_backing,old.readonly_backing);
            }
        }
        println!("FRAME-RECORD known-presence oracle-mutations=12000 rights=0..9 descriptors=0/MAX");
    }

    #[test]
    fn fallible_frame_walk_stops_at_the_failed_row_and_keeps_exact_error_prefixes() {
        use super::*;
        let mut vm = VmSpace::new();
        for page in 0..4096 {
            vm.record_frame(page * 4096, usize::MAX - page).unwrap();
            vm.set_permissions(page * 4096, if page % 3 == 0 { 0 } else { 3 });
            if page % 17 == 0 { vm.mark_mapping_uncertain(page * 4096); }
        }
        vm.record_frame(1, 0).unwrap();
        vm.record_frame(usize::MAX, 9).unwrap();
        let mut old_visits = 0;
        vm.frame_by_va.visit_range(0, usize::MAX, |_, _| old_visits += 1);
        let mut visits = 0;
        DENY_AT_OR_ABOVE.with(|limit| limit.set(0));
        let result = vm.frame_by_va.try_visit_range(0, usize::MAX, |va, _| {
            visits += 1;
            Err(va)
        });
        DENY_AT_OR_ABOVE.with(|limit| limit.set(usize::MAX));
        assert_eq!(result, Err(0));
        assert_eq!((old_visits, visits), (4097, 1));
        println!("FRAME-WALK first-error old-frame-visits={old_visits} new-frame-visits={visits}");
        let all = vm.backed_pages(0, usize::MAX);
        for fail in [0, 1, 63 * 4096, 2048 * 4096, 4095 * 4096, usize::MAX] {
            let expected: Vec<_> = all.iter().filter(|&&(va,_)| va <= fail).map(|&(va,_)| va).collect();
            let mut actual = Vec::new();
            let valid = vm.all_backed_pages(0, usize::MAX, |va| { actual.push(va); va != fail });
            assert_eq!(actual, expected);
            assert_eq!(valid, fail == usize::MAX);
        }
        let mut present = Vec::new();
        vm.visit_backed_metadata(0,usize::MAX,|va, frame, rights, uncertain| {
            if rights != 0 || uncertain { present.push((va, frame)); }
        });
        for index in [0,1,31,present.len()/2,present.len()-1] {
            let mut actual = Vec::new();
            let result = vm.try_for_each_present_frame(0,usize::MAX,|va, frame| {
                actual.push((va, frame));
                if va == present[index].0 { Err(71) } else { Ok(()) }
            });
            assert_eq!(result, Err((present[index].0, 71)));
            assert_eq!(actual, present[..=index]);
        }
        for (start,end) in [(0,0),(10,1),(usize::MAX,usize::MAX)] {
            assert!(vm.all_backed_pages(start,end,|_| panic!("empty")));
            vm.try_for_each_present_frame(start,end,|_,_| -> Result<(), ()> { panic!("empty") }).unwrap();
        }
        assert_eq!(vm.backed_pages(0,usize::MAX),all);
    }

    // Independent reconstruction of the previous per-Frame coverage check.
    // Keep permission/uncertainty gates and return its actual visit count.
    pub fn scalar_coverage(vm: &VmSpace, start: usize, end: usize, rights: usize) -> (bool, usize) {
        if start >= end || (start | end) & 4095 != 0 || rights > 7 { return (false, 0); }
        if end - start == 4096 {
            return (vm.permissions(start) == rights
                && (rights != 0 || !vm.mapping_is_uncertain(start))
                && vm.find_frame(start).is_some(), 1);
        }
        if vm.uncertain_mappings.range(start..end).any(|va| va & 4095 == 0)
            || !vm.permissions.aligned_range_is(start, end, rights)
        { return (false, 0); }
        let mut frames = 0;
        vm.frame_by_va.visit_range(start, end, |va, _| { if va & 4095 == 0 { frames += 1; } });
        (frames == (end - start) / 4096, frames)
    }
}
use vm_space::{BootstrapVmSpace, VmSpace, VmTracker};

#[test]
fn packed_frame_snapshot_reserves_once_without_changing_collected_metadata() {
    let mut vm = VmSpace::new();
    for page in 0..4096 {
        vm.record_frame(page * 4096, page).unwrap();
        vm.set_permissions(page * 4096, page % 8);
        if page % 17 == 0 { vm.mark_mapping_uncertain(page * 4096); }
    }
    COUNTS.with(|counts| counts.set(Allocations::default()));
    TRACK.with(|track| track.set(true));
    let old = vm_space::unreserved_backed_pages(&vm, 0, 4096 * 4096);
    TRACK.with(|track| track.set(false));
    let before = COUNTS.with(Cell::get);
    COUNTS.with(|counts| counts.set(Allocations::default()));
    TRACK.with(|track| track.set(true));
    let new = vm.backed_pages(0, 4096 * 4096);
    TRACK.with(|track| track.set(false));
    let after = COUNTS.with(Cell::get);
    assert_eq!(new, old);
    assert_eq!(new.len(), 4096);
    assert_eq!(after.calls, 1);
    assert!(before.calls > after.calls);
    assert_eq!(after.live, 4096 * core::mem::size_of::<(usize, usize)>());
    assert!(after.peak < before.peak);
    println!("FRAME-SNAPSHOT pages=4096 old-allocations={} new-allocations={} old-peak={} new-peak={}", before.calls, after.calls, before.peak, after.peak);
    FAIL_ONCE_BYTES.with(|bytes| bytes.set(after.live));
    let fallback = vm.backed_pages(0, 4096 * 4096);
    assert_eq!(FAIL_ONCE_BYTES.with(Cell::get), 0);
    assert_eq!(fallback, old);
}

#[test]
fn packed_snapshot_count_handles_edges_holes_empty_words_and_scalar_rows() {
    let mut vm = VmSpace::new();
    let top = usize::MAX & !4095;
    for base in [0, top - 63 * 4096] {
        vm.prepare_frame_word(base, 64);
        for page in 0..64 {
            if page % 3 != 0 { vm.record_frame(base + page * 4096, usize::MAX - page).unwrap(); }
        }
    }
    vm.prepare_frame_word(1 << 18, 64); // Empty prepared word contributes zero.
    for va in [1, 4095, (1 << 20) + 4096, usize::MAX] { vm.record_frame(va, 0).unwrap(); }
    let all = vm_space::unreserved_backed_pages(&vm, 0, usize::MAX);
    for (start, end) in [(0,usize::MAX),(0,0),(99,1),(0,1),(1,4096),(4095,8193),
        (63*4096,65*4096),(1<<18,2<<18),(top-63*4096+1,usize::MAX),(top,usize::MAX),
        (usize::MAX,usize::MAX)] {
        let expected: Vec<_> = all.iter().copied().filter(|&(va,_)| start <= va && va < end).collect();
        DENY_AT_OR_ABOVE.with(|limit| limit.set(0));
        let count = vm_space::dense_count(&vm, start, end);
        DENY_AT_OR_ABOVE.with(|limit| limit.set(usize::MAX));
        assert!(count <= expected.len());
        let actual = vm.backed_pages(start,end);
        assert_eq!(actual, expected);
        if count != 0 { assert_eq!(actual.capacity(), expected.len()); }
    }
}

#[test]
fn bitmap_frame_coverage_matches_scalar_oracle_for_dense_scalar_holes_and_permissions() {
    for rights in 0..=7 {
        let mut vm = VmSpace::new();
        for page in 0..160 {
            let va = page * 4096;
            if page < 128 || page % 3 != 0 { vm.record_frame(va, usize::MAX - page).unwrap(); }
            vm.set_permissions(va, rights);
        }
        vm.record_frame(1, 0).unwrap();
        vm.record_frame(4095, usize::MAX).unwrap();
        for turn in 0..6 {
            match turn {
                1 => { vm.forget_frame(31 * 4096); }
                2 => { vm.record_frame(31 * 4096, 0).unwrap(); vm.set_permissions(31 * 4096, rights); }
                3 => vm.mark_mapping_uncertain(63 * 4096),
                4 => vm.confirm_unmapped(63 * 4096),
                5 => { vm.record_frame(63 * 4096, usize::MAX).unwrap(); vm.set_permissions(63 * 4096, rights); },
                _ => {}
            }
            for (first, last) in [(0, 128), (1, 31), (31, 65), (63, 127), (64, 129), (128, 160), (0, 160), (160, 161)] {
                for requested in 0..=7 {
                    assert_eq!(vm.backed_range_with_permissions(first * 4096, last * 4096, requested),
                        vm_space::scalar_coverage(&vm, first * 4096, last * 4096, requested).0);
                }
            }
        }
    }
}

#[test]
fn bitmap_frame_coverage_reads_words_without_allocating_or_inventing_prepared_frames() {
    let mut vm = VmSpace::new();
    for page in 0..4096 { vm.record_frame(page * 4096, page).unwrap(); }
    let (old_result, old_visits) = vm_space::scalar_coverage(&vm, 0, 4096 * 4096, 7);
    assert!(old_result);
    assert_eq!(old_visits, 4096);
    DENIED.with(|count| count.set(0));
    DENY_AT_OR_ABOVE.with(|limit| limit.set(0));
    let covered = vm.backed_range_with_permissions(0, 4096 * 4096, 7);
    DENY_AT_OR_ABOVE.with(|limit| limit.set(usize::MAX));
    assert!(covered);
    assert_eq!(DENIED.with(Cell::get), 0);
    let top = usize::MAX & !((1 << 18) - 1);
    for page in 0..63 { vm.record_frame(top | (page << 12), page).unwrap(); }
    for (start, end) in [(top, usize::MAX - 4095), (0, usize::MAX - 4095), (0, 0), (8192, 4096), (1, 8192)] {
        assert_eq!(vm.backed_range_with_permissions(start, end, 7), vm_space::scalar_coverage(&vm, start, end, 7).0);
    }
    let mut empty = VmSpace::new();
    empty.prepare_frame_word(0, 64);
    assert!(!empty.backed_range_with_permissions(0, 64 * 4096, 7));
    println!("FRAME-COVERAGE pages=4096 old-frame-visits=4096 dense-word-comparisons=64 (word count derived from current index layout)");
}

#[test]
fn preparing_bulk_frame_words_avoids_transient_scalar_allocation_and_promotion() {
    let mut baseline = VmSpace::new();
    COUNTS.with(|counts| counts.set(Allocations::default()));
    TRACK.with(|track| track.set(true));
    for page in 0..4096 { baseline.record_frame(page * 4096, page).unwrap(); }
    TRACK.with(|track| track.set(false));
    let before = COUNTS.with(Cell::get);
    let mut prepared = VmSpace::new();
    COUNTS.with(|counts| counts.set(Allocations::default()));
    TRACK.with(|track| track.set(true));
    for page in 0..4096 {
        if page % 64 == 0 { prepared.prepare_frame_word(page * 4096, 4096 - page); }
        prepared.record_frame(page * 4096, page).unwrap();
    }
    TRACK.with(|track| track.set(false));
    let after = COUNTS.with(Cell::get);
    assert!(after.calls * 3 < before.calls);
    assert!(after.live <= before.live);
    for page in 0..4096 { assert_eq!(prepared.find_frame(page * 4096), baseline.find_frame(page * 4096)); }
    assert_eq!(prepared.backed_pages(0, 4096 * 4096), baseline.backed_pages(0, 4096 * 4096));
    println!("FRAME-WORD-PREPARE pages=4096 old-allocations={} new-allocations={} old-live={} new-live={}", before.calls, after.calls, before.live, after.live);
}

#[test]
fn prepared_frame_word_publishes_no_backing_and_failed_first_map_cleanup_releases_it() {
    let mut vm = VmSpace::new();
    COUNTS.with(|counts| counts.set(Allocations::default()));
    TRACK.with(|track| track.set(true));
    let mut empty_live = None;
    for word in 0..128 {
        let va = word << 18;
        vm.prepare_frame_word(va, 64);
        assert_eq!(vm.find_frame(va), None);
        assert!(!vm.frame_may_be_present(va));
        assert!(!vm.has_backed_pages(va, va + (1 << 18)));
        assert!(!vm.page_tables_ready(va));
        assert_eq!(vm.forget_frame(va), None);
        let live = COUNTS.with(Cell::get).live;
        if let Some(expected) = empty_live { assert_eq!(live, expected); } else { empty_live = Some(live); }
    }
    TRACK.with(|track| track.set(false));
    // A hint cannot make a short/unaligned run allocate or add any Frame.
    DENIED.with(|count| count.set(0));
    DENY_AT_OR_ABOVE.with(|limit| limit.set(0));
    vm.prepare_frame_word(0, 31);
    vm.prepare_frame_word(1, 64);
    vm.prepare_frame_word(usize::MAX - 4095, usize::MAX);
    let short_denied = DENIED.with(Cell::get);
    DENY_AT_OR_ABOVE.with(|limit| limit.set(512));
    vm.prepare_frame_word(0, 64); // Optional array allocation is rejected.
    let mut correct = true;
    for page in 0..32 { correct &= vm.record_frame(page * 4096, page).is_ok(); }
    DENY_AT_OR_ABOVE.with(|limit| limit.set(usize::MAX));
    assert!(correct);
    assert_eq!(short_denied, 0);
    assert!(DENIED.with(Cell::get) > 0);
    for page in 0..32 { assert_eq!(vm.find_frame(page * 4096), Some(page)); }
}

#[test]
fn packed_frame_index_reduces_page_allocations_and_preserves_avl_order_and_churn() {
    const PAGES: usize = 4096;
    let mut old = avl::AvlTree::new();
    COUNTS.with(|counts| counts.set(Allocations::default()));
    TRACK.with(|track| track.set(true));
    for page in 0..PAGES { old.insert(page * 4096, page).unwrap(); }
    TRACK.with(|track| track.set(false));
    let baseline = COUNTS.with(Cell::get);
    let mut vm = VmSpace::new();
    COUNTS.with(|counts| counts.set(Allocations::default()));
    TRACK.with(|track| track.set(true));
    for page in 0..PAGES { vm.record_frame(page * 4096, page).unwrap(); }
    TRACK.with(|track| track.set(false));
    let packed = COUNTS.with(Cell::get);
    println!("PACKED-FRAME-INDEX pages={PAGES} old-allocations={} new-allocations={} old-live-bytes={} new-live-bytes={}",
        baseline.calls, packed.calls, baseline.live, packed.live);
    assert!(packed.calls * 4 < baseline.calls);
    assert!(packed.live < baseline.live);
    // Opaque descriptor values and exact unaligned/full-width keys stay exact.
    for (key, value) in [(1, 0), (usize::MAX - 4095, usize::MAX), (usize::MAX, 0)] {
        old.insert(key, value).unwrap();
        vm.record_frame(key, value).unwrap();
        assert_eq!(vm.find_frame(key), old.find(key));
    }
    DENIED.with(|count| count.set(0));
    DENY_AT_OR_ABOVE.with(|limit| limit.set(0));
    let mut correct = true;
    for page in 0..PAGES {
        let key = page * 4096;
        old.insert(key, usize::MAX - page).unwrap();
        correct &= vm.record_frame(key, usize::MAX - page).is_ok();
        if page % 2 == 0 { correct &= vm.forget_frame(key) == old.remove(key); }
        correct &= vm.find_frame(key) == old.find(key);
    }
    DENY_AT_OR_ABOVE.with(|limit| limit.set(usize::MAX));
    assert!(correct);
    assert_eq!(DENIED.with(Cell::get), 0);
    for (start, end) in [(0, usize::MAX), (1, 4096), (4096, 4096), (8192, 1), (usize::MAX - 4095, usize::MAX)] {
        let mut expected = Vec::new();
        old.visit_range(start, end, |key, value| expected.push((key, value)));
        let mut actual = Vec::new();
        vm.try_for_each_present_frame(start, end, |key, value| { actual.push((key, value)); Ok::<(), ()>(()) }).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(vm.has_backed_pages(start, end), old.has_range(start, end));
    }
}

#[test]
fn dense_frame_promotion_failure_keeps_scalar_rows_and_later_promotion_preserves_holes() {
    let mut vm = VmSpace::new();
    DENIED.with(|count| count.set(0));
    DENY_AT_OR_ABOVE.with(|limit| limit.set(512));
    let mut correct = true;
    for page in 0..32 {
        correct &= vm.record_frame(page * 4096, usize::MAX - page).is_ok();
    }
    DENY_AT_OR_ABOVE.with(|limit| limit.set(usize::MAX));
    assert!(correct);
    assert!(DENIED.with(Cell::get) > 0);
    for page in 0..32 { assert_eq!(vm.find_frame(page * 4096), Some(usize::MAX - page)); }
    // Overwriting the failed promotion's scalar row does not need to allocate.
    DENY_AT_OR_ABOVE.with(|limit| limit.set(0));
    correct = vm.record_frame(0, 0).is_ok();
    DENY_AT_OR_ABOVE.with(|limit| limit.set(usize::MAX));
    assert!(correct);
    vm.record_frame(32 * 4096, 32).unwrap(); // New key retries optional packing.
    DENIED.with(|count| count.set(0));
    DENY_AT_OR_ABOVE.with(|limit| limit.set(0));
    correct = vm.record_frame(40 * 4096, 0).is_ok();
    correct &= vm.forget_frame(2 * 4096) == Some(usize::MAX - 2);
    correct &= vm.find_frame(2 * 4096).is_none();
    correct &= vm.find_frame(0) == Some(0);
    correct &= vm.find_frame(40 * 4096) == Some(0);
    DENY_AT_OR_ABOVE.with(|limit| limit.set(usize::MAX));
    assert!(correct);
    assert_eq!(DENIED.with(Cell::get), 0);
}

#[test]
fn dense_frame_words_merge_unaligned_keys_and_preserve_top_address_boundaries() {
    let mut vm = VmSpace::new();
    let mut expected = BTreeMap::new();
    let top = usize::MAX & !((1 << 18) - 1);
    for base in [0usize, 1 << 20, top] {
        for page in 0..64 {
            let va = base | (page << 12);
            vm.record_frame(va, usize::MAX - page).unwrap();
            expected.insert(va, usize::MAX - page);
        }
        for offset in [1, 4095, 4097, (1 << 18) - 1] {
            let va = base | offset;
            vm.record_frame(va, 0).unwrap();
            expected.insert(va, 0);
        }
        for page in [0, 1, 31, 63] {
            let va = base | (page << 12);
            assert_eq!(vm.forget_frame(va), expected.remove(&va));
        }
        for (start, end) in [(base, base | ((1 << 18) - 1)), (base | 4095, base | (33 << 12))] {
            let mut actual = Vec::new();
            vm.try_for_each_present_frame(start, end, |key, value| { actual.push((key, value)); Ok::<(), ()>(()) }).unwrap();
            assert_eq!(actual, expected.range(start..end).map(|(&key, &value)| (key, value)).collect::<Vec<_>>());
        }
    }
    for (&key, &value) in &expected { assert_eq!(vm.find_frame(key), Some(value)); }
    for key in expected.keys().copied().collect::<Vec<_>>() { assert_eq!(vm.forget_frame(key), expected.remove(&key)); }
    assert!(!vm.has_backed_pages(0, usize::MAX));
    assert_eq!(vm.find_frame(usize::MAX), None);
}

#[test]
fn permission_bitplanes_preserve_old_map_access_backing_and_ordered_frame_views() {
    let mut vm = VmSpace::new();
    let mut rights = BTreeMap::new();
    let mut frames = BTreeMap::new();
    let mut pending = std::collections::BTreeSet::new();
    let mut random = 0x581ac839u64;
    for turn in 0..8000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let mut va = ((random >> 23) as usize % 256) * 4096;
        if turn % 11 == 0 { va += 1; }
        if turn % 41 == 0 { va = usize::MAX - 4095; }
        if turn % 43 == 0 { va = usize::MAX; }
        let value = match (random >> 32) % 10 { 8 => 8, 9 => usize::MAX, n => n as usize };
        match turn % 8 {
            0..=3 => {
                vm.set_permissions(va, value);
                if value == 7 { rights.remove(&va); } else { rights.insert(va, value); }
            }
            4 => { vm.forget_frame(va); frames.remove(&va); rights.remove(&va); pending.remove(&va); }
            5 => { vm.record_frame(va, turn + 1).unwrap(); frames.insert(va, turn + 1); }
            6 => { vm.mark_mapping_uncertain(va); pending.insert(va); }
            _ => { vm.confirm_unmapped(va); pending.remove(&va); rights.insert(va, 0); }
        }
        for address in [va, va ^ 4096, 0, usize::MAX] {
            assert_eq!(vm.permissions(address), if pending.contains(&address) { 0 }
                else { rights.get(&address).copied().unwrap_or(7) });
        }
        if turn % 31 != 0 { continue; }
        for (start, end) in [(0, usize::MAX), (1, 256 * 4096 - 1), (0, 1),
            (63 * 4096 + 1, 65 * 4096 - 1), (64 * 4096, 128 * 4096),
            (usize::MAX - 8192, usize::MAX), (4096, 4096), (8192, 4096)] {
            for permission in [1, 2, 4] {
                let first = start & !4095;
                let allowed = start < end && matches!(permission, 1 | 2)
                    && !pending.iter().any(|&va| va & 4095 == 0 && va >= first && va < end)
                    && rights.iter().all(|(&va, &r)| va & 4095 != 0 || va < first || va >= end || r & permission != 0);
                assert_eq!(vm.user_range_allowed(start, end, permission), allowed);
            }
            for permission in 0..=7 {
                let selected: Vec<_> = frames.keys().copied().filter(|va|
                    va & 4095 == 0 && *va >= start && *va < end).collect();
                let backed = start < end && (start | end) & 4095 == 0
                    && selected.len() == (end - start) / 4096
                    && selected.iter().all(|va| !pending.contains(va)
                        && rights.get(va).copied().unwrap_or(7) == permission);
                assert_eq!(vm.backed_range_with_permissions(start, end, permission), backed);
            }
            let expected: Vec<_> = frames.iter().filter(|(&va, _)| va >= start && va < end)
                .map(|(&va, _)| (va, if pending.contains(&va) { 0 } else { rights.get(&va).copied().unwrap_or(7) })).collect();
            assert_eq!(vm.backed_pages(start, end), expected);
            let present: Vec<_> = frames.iter().filter(|(&va, _)| va >= start && va < end
                && (pending.contains(&va) || rights.get(&va).copied().unwrap_or(7) != 0))
                .map(|(&va, &frame)| (va, frame)).collect();
            let mut actual = Vec::new();
            assert_eq!(vm.try_for_each_present_frame(start, end, |va, frame| {
                actual.push((va, frame)); Ok::<(), ()>(())
            }), Ok(()));
            assert_eq!(actual, present);
        }
    }
}

#[test]
fn permission_bitplanes_reduce_dense_heap_and_keep_rollback_allocation_free() {
    let pages = 4096;
    COUNTS.with(|counts| counts.set(Allocations::default()));
    TRACK.with(|track| track.set(true));
    let mut old = BTreeMap::new();
    for page in 0..pages { old.insert(page * 4096, 3usize); }
    TRACK.with(|track| track.set(false));
    let old_counts = COUNTS.with(Cell::get);
    drop(old);
    let mut vm = VmSpace::new();
    COUNTS.with(|counts| counts.set(Allocations::default()));
    TRACK.with(|track| track.set(true));
    for page in 0..pages { vm.set_permissions(page * 4096, 3); }
    TRACK.with(|track| track.set(false));
    let packed = COUNTS.with(Cell::get);
    println!("PERMISSION-BITPLANES pages={pages} old-allocations={} packed-allocations={} old-live-bytes={} packed-live-bytes={}",
        old_counts.calls, packed.calls, old_counts.live, packed.live);
    assert!(packed.calls * 8 < old_counts.calls);
    assert!(packed.live * 8 < old_counts.live);
    DENIED.with(|count| count.set(0));
    DENY_AT_OR_ABOVE.with(|limit| limit.set(0));
    let mut correct = true;
    for page in (0..pages).step_by(2) {
        vm.set_permissions(page * 4096, 7);
        correct &= vm.permissions(page * 4096) == 7;
        vm.set_permissions(page * 4096, 5);
        correct &= vm.permissions(page * 4096) == 5;
        vm.set_permissions(page * 4096, 3);
    }
    correct &= vm.user_range_allowed(0, pages * 4096, 1);
    correct &= vm.user_range_allowed(0, pages * 4096, 2);
    for page in 0..pages { vm.set_permissions(page * 4096, 7); }
    correct &= vm.user_range_allowed(0, pages * 4096, 1);
    DENY_AT_OR_ABOVE.with(|limit| limit.set(usize::MAX));
    assert!(correct);
    assert_eq!(DENIED.with(Cell::get), 0);
}

#[test]
fn permission_bitplanes_keep_isolated_page_heap_at_old_scalar_footprint() {
    for stride in [8, 64] {
        COUNTS.with(|counts| counts.set(Allocations::default()));
        TRACK.with(|track| track.set(true));
        let mut old = BTreeMap::new();
        for page in 0..4096 { old.insert(page * stride * 4096, 3usize); }
        TRACK.with(|track| track.set(false));
        let baseline = COUNTS.with(Cell::get);
        drop(old);
        let mut vm = VmSpace::new();
        COUNTS.with(|counts| counts.set(Allocations::default()));
        TRACK.with(|track| track.set(true));
        for page in 0..4096 { vm.set_permissions(page * stride * 4096, 3); }
        TRACK.with(|track| track.set(false));
        let actual = COUNTS.with(Cell::get);
        println!("PERMISSION-ADAPTIVE stride={stride} pages=4096 old-allocations={} new-allocations={} old-live-bytes={} new-live-bytes={}",
            baseline.calls, actual.calls, baseline.live, actual.live);
        if stride == 64 {
            assert_eq!(actual.calls, baseline.calls);
            assert_eq!(actual.live, baseline.live);
        } else {
            assert!(actual.calls < baseline.calls);
            assert!(actual.live < baseline.live);
        }
        for page in 0..4096 { assert_eq!(vm.permissions(page * stride * 4096), 3); }
    }
}

#[test]
fn shared_page_bitmap_matches_exact_set_through_mixed_vm_mutations_and_ranges() {
    let mut vm = VmSpace::new();
    let mut expected = std::collections::BTreeSet::new();
    let mut random = 0x29a773d9u64;
    for turn in 0..5000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let mut va = ((random >> 24) as usize % 192) * 4096;
        if turn % 11 == 0 { va += 1; }
        if turn % 41 == 0 { va = usize::MAX - 4095; }
        if turn % 43 == 0 { va = usize::MAX; }
        match turn % 7 {
            0 | 1 => { vm.mark_private_page_shared(va, true); expected.insert(va); }
            2 => { vm.mark_private_page_shared(va, false); expected.remove(&va); }
            3 => { vm.forget_frame(va); expected.remove(&va); }
            4 => { vm.record_frame(va, turn + 1).unwrap(); }
            5 => { vm.set_permissions(va, turn & 7); }
            _ => { vm.mark_mapping_uncertain(va); }
        }
        assert_eq!(vm.private_page_shared(va), expected.contains(&va));
        if turn % 17 == 0 {
            for (start, end) in [(0, usize::MAX), (0, 1), (4095, 4097), (4096, 4096),
                (63 * 4096 + 1, 64 * 4096 + 1), (64 * 4096, 128 * 4096),
                (128 * 4096 - 1, 191 * 4096 + 1),
                (usize::MAX - 8192, usize::MAX), (usize::MAX, usize::MAX)] {
                let wanted: Vec<_> = expected.range(start..end).copied().collect();
                assert_eq!(vm.shared_private_pages(start, end), wanted);
                assert_eq!(vm.private_range_shared(start, end), !wanted.is_empty());
            }
        }
    }
}

#[test]
fn shared_page_bitmap_reduces_dense_tree_allocations_and_updates_without_allocation() {
    let pages = 4096;
    COUNTS.with(|counts| counts.set(Allocations::default()));
    TRACK.with(|track| track.set(true));
    let mut old = std::collections::BTreeSet::new();
    for page in 0..pages { old.insert(page * 4096); }
    TRACK.with(|track| track.set(false));
    let old_counts = COUNTS.with(Cell::get);
    drop(old);
    let mut vm = VmSpace::new();
    COUNTS.with(|counts| counts.set(Allocations::default()));
    TRACK.with(|track| track.set(true));
    for page in 0..pages { vm.mark_private_page_shared(page * 4096, true); }
    TRACK.with(|track| track.set(false));
    let bitmap_counts = COUNTS.with(Cell::get);
    println!("SHARED-PAGE-BITMAP pages={pages} old-allocations={} bitmap-allocations={} old-live-bytes={} bitmap-live-bytes={}",
        old_counts.calls, bitmap_counts.calls, old_counts.live, bitmap_counts.live);
    assert!(bitmap_counts.calls * 8 < old_counts.calls);
    assert!(bitmap_counts.live * 8 < old_counts.live);
    assert_eq!(vm.shared_private_pages(0, pages * 4096).len(), pages);

    DENIED.with(|count| count.set(0));
    DENY_AT_OR_ABOVE.with(|limit| limit.set(0));
    let mut correct = true;
    for page in (0..pages).step_by(2) {
        vm.mark_private_page_shared(page * 4096, false);
        correct &= !vm.private_page_shared(page * 4096);
        correct &= vm.private_range_shared(0, pages * 4096);
        vm.mark_private_page_shared(page * 4096, true);
        correct &= vm.private_page_shared(page * 4096);
    }
    for page in 0..pages { vm.mark_private_page_shared(page * 4096, false); }
    correct &= !vm.private_range_shared(0, pages * 4096);
    DENY_AT_OR_ABOVE.with(|limit| limit.set(usize::MAX));
    assert!(correct);
    assert_eq!(DENIED.with(Cell::get), 0);
}

#[test]
fn shared_page_bitmap_keeps_actual_old_reversed_range_behavior_after_each_tree_history() {
    for history in 0..5 {
        let mut vm = VmSpace::new();
        let mut old = std::collections::BTreeSet::new();
        let va = if history % 2 == 0 { 4097 } else { 4096 };
        if history != 0 { vm.mark_private_page_shared(va, true); old.insert(va); }
        if history >= 3 { vm.mark_private_page_shared(va, false); old.remove(&va); }
        let old_panics = std::panic::catch_unwind(std::panic::AssertUnwindSafe(||
            old.range(8192..4096).next().is_some())).is_err();
        assert_eq!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(||
            vm.private_range_shared(8192, 4096))).is_err(), old_panics);
        assert_eq!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(||
            vm.shared_private_pages(8192, 4096))).is_err(), old_panics);
    }
}

#[test]
fn readonly_bitmap_tracks_every_mutation_against_current_tree_metadata() {
    let mut vm=VmSpace::new();let mut seed=0x98137_u64;
    for turn in 0..20000 {
        seed=seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let page=(seed>>19) as usize%1024;
        let base=if seed&16==0 {usize::MAX&!4095} else {page*4096};
        let va=if seed&32==0 {base.saturating_add(3)} else {base};
        match seed%7 {
            0=>{vm.record_frame(va,0).unwrap();},
            1=>{vm.record_frame(va,seed as usize).unwrap();},
            2=>vm.set_permissions(va,1),
            3=>vm.set_permissions(va,(seed>>33) as usize%8),
            4=>vm.mark_mapping_uncertain(va),
            5=>vm.confirm_unmapped(va),
            _=>{if seed&64==0 {VmTracker::forget_frame(&mut vm,va);} else {vm.forget_frame(va);}},
        }
        for (start,end) in [(0,1024*4096),(base,base.saturating_add(4096)),
            (page/64*64*4096,(page/64+1)*64*4096),(base,base),
            (base.saturating_add(1),base.saturating_add(8192))] {
            assert_eq!(vm.readonly_backed_range(start,end),vm.backed_range_with_permissions(start,end,1),
                "turn={turn} range={start:x}..{end:x}");
        }
    }
    let fresh=VmSpace::new();assert!(!fresh.readonly_backed_range(0,4096));
}

#[test]
fn readonly_bitmap_handles_word_boundaries_holes_and_rights_before_backing() {
    for base in [0,0x60000000,(usize::MAX&!4095)-192*4096] {
        let mut vm=VmSpace::new();
        for page in 0..192 {
            let va=base+page*4096;vm.set_permissions(va,1);
            assert!(!vm.readonly_backed_range(va,va+4096));
            vm.record_frame(va,0).unwrap();
        }
        for start in [0,1,31,63,64,65,127,128,191] {
            for end in [1,32,63,64,65,127,128,129,192] {
                let lo=base+start*4096;let hi=base+end*4096;
                assert_eq!(vm.readonly_backed_range(lo,hi),start<end);
            }
        }
        for hole in [0,31,63,64,127,128,191] {
            let va=base+hole*4096;vm.set_permissions(va,7);
            assert!(!vm.readonly_backed_range(base,base+192*4096));
            assert_eq!(vm.readonly_backed_range(base,va),hole!=0);
            vm.set_permissions(va,1);assert!(vm.readonly_backed_range(base,base+192*4096));
        }
    }
}

#[test]
fn readonly_bitmap_queries_downgrades_and_retirement_do_not_allocate() {
    let mut vm=VmSpace::new();
    for page in 0..512 {vm.record_frame(page*4096,page).unwrap();vm.set_permissions(page*4096,1);}
    DENIED.with(|count|count.set(0));DENY_AT_OR_ABOVE.with(|limit|limit.set(0));
    let before=vm.readonly_backed_range(0,512*4096);
    for page in 0..512 {
        if page%2==0 {vm.set_permissions(page*4096,7);} else {vm.forget_frame(page*4096);}
    }
    let after=vm.readonly_backed_range(0,512*4096);
    DENY_AT_OR_ABOVE.with(|limit|limit.set(usize::MAX));
    assert!(before && !after);assert_eq!(DENIED.with(Cell::get),0);
    for page in 0..512 {vm.record_frame(page*4096,page).unwrap();vm.set_permissions(page*4096,1);}
    assert!(vm.readonly_backed_range(0,512*4096));
}

#[test]
#[ignore = "host readonly metadata query work; excludes A9N and application launch"]
fn readonly_bitmap_query_work() {
    use std::{hint::black_box,time::Instant};
    for pages in [64,394,2048,16384] {
        let mut vm=VmSpace::new();
        for page in 0..pages {vm.record_frame(page*4096,page).unwrap();vm.set_permissions(page*4096,1);}
        let begin=Instant::now();
        for _ in 0..1000 {assert!(black_box(vm.backed_range_with_permissions(0,black_box(pages*4096),1)));}
        let old_ns=begin.elapsed().as_nanos();let begin=Instant::now();
        for _ in 0..1000 {assert!(black_box(vm.readonly_backed_range(0,black_box(pages*4096))));}
        let bitmap_ns=begin.elapsed().as_nanos();
        println!("READONLY-BACKING pages={pages} bitmap-words={} queries=1000 tree-ns={old_ns} bitmap-ns={bitmap_ns}",pages.div_ceil(64));
    }
}

#[test]
fn merged_backing_metadata_matches_point_queries_with_dense_unbacked_exceptions() {
    let mut vm = VmSpace::new();
    let mut frames = BTreeMap::new();
    let mut rights = BTreeMap::new();
    let mut pending = std::collections::BTreeSet::new();
    for i in 0..10000 {
        let va = i * 13;
        let permission = i % 16;
        vm.set_permissions(va, permission);
        rights.insert(va, permission);
        if i % 3 == 0 {
            vm.mark_mapping_uncertain(va);
            pending.insert(va);
        }
    }
    for i in 0..1024 {
        let va = i * 127;
        vm.record_frame(va, i + 100).unwrap();
        frames.insert(va, i + 100);
    }
    for va in [usize::MAX - 1, usize::MAX] {
        vm.record_frame(va, 99999).unwrap();
        frames.insert(va, 99999);
        vm.set_permissions(va, 0);
        rights.insert(va, 0);
        vm.mark_mapping_uncertain(va);
        pending.insert(va);
    }
    for (start, end) in [(0, usize::MAX), (1, 127), (13, 128), (255, 128000),
        (127, 127), (1024, 512), (usize::MAX - 2, usize::MAX), (usize::MAX, usize::MAX)]
    {
        let expected: Vec<_> = frames.iter().filter(|(va, _)| **va >= start && **va < end)
            .map(|(&va, _)| (va, vm.permissions(va))).collect();
        assert_eq!(vm.backed_pages(start, end), expected);
        let present: Vec<_> = frames.iter().filter(|(va, _)| **va >= start && **va < end)
            .filter(|(va, _)| rights.get(va).copied().unwrap_or(7) != 0 || pending.contains(va))
            .map(|(&va, &frame)| (va, frame)).collect();
        let mut actual = Vec::new();
        let result: Result<(), (usize, ())> = vm.try_for_each_present_frame(start, end, |va, frame| {
            actual.push((va, frame));
            Ok(())
        });
        assert_eq!(result, Ok(()));
        assert_eq!(actual, present);
    }
    for (va, permission) in rights {
        assert_eq!(vm.permissions(va), if pending.contains(&va) { 0 } else { permission });
    }
}

#[test]
fn range_existence_matches_ordered_map_through_insert_replace_and_remove() {
    let mut tree = avl::AvlTree::new();
    let mut oracle = BTreeMap::new();
    let mut seed = 0x12345678u64;
    for step in 0..4000usize {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let key = match step % 31 { 0 => 0, 1 => usize::MAX, _ => (seed as usize) % 2048 };
        if step % 3 == 0 {
            assert_eq!(tree.remove(key), oracle.remove(&key));
        } else {
            tree.insert(key, step).unwrap();
            oracle.insert(key, step);
        }
        for (start, end) in [(0, usize::MAX), (key, key), (key, key.saturating_add(1)),
            (key.saturating_sub(1), key), (key, usize::MAX), (usize::MAX, 0),
            (700, 900), (2500, 3000)] {
            assert_eq!(tree.has_range(start, end), oracle.keys().any(|&k| start <= k && k < end));
        }
    }
}

#[test]
fn backing_existence_matches_snapshot_without_permissions_or_heap_work() {
    let mut vm = VmSpace::new();
    vm.set_permissions(4096, 0);
    vm.mark_mapping_uncertain(8192);
    assert!(!vm.has_backed_pages(0, usize::MAX));
    for va in [0, 17, 4096, 8192, 513 * 4096, usize::MAX] {
        vm.record_frame(va, va.wrapping_add(10)).unwrap();
    }
    for (start, end) in [(0, usize::MAX), (0, 1), (1, 17), (17, 18), (4096, 8192),
        (8192, 12288), (12288, 513 * 4096), (usize::MAX, usize::MAX), (8192, 4096)] {
        let expected = !vm.backed_pages(start, end).is_empty();
        COUNTS.with(|c| c.set(Allocations::default()));
        TRACK.with(|c| c.set(true));
        let actual = vm.has_backed_pages(start, end);
        TRACK.with(|c| c.set(false));
        assert_eq!(actual, expected);
        assert_eq!(COUNTS.with(|c| c.get().calls), 0);
    }
    vm.forget_frame(4096);
    assert!(!vm.has_backed_pages(4096, 8192));
    assert!(vm.has_backed_pages(8192, 8193));
}

#[test]
fn readonly_extent_matches_point_checks_with_holes_rights_and_uncertain_pages() {
    let mut vm = VmSpace::new();
    for page in 1..=513 {
        vm.record_frame(page * 4096, page).unwrap();
        vm.set_permissions(page * 4096, 1);
    }
    for page in [1, 2, 256, 512, 513] {
        let va = page * 4096;
        for rights in [0, 1, 2, 3, 4, 5, 7, usize::MAX] {
            vm.set_permissions(va, rights);
            for (start, end) in [(4096, 514 * 4096), (va, va + 4096), (4096, va + 4096)] {
                let expected = (start..end).step_by(4096)
                    .all(|p| vm.permissions(p) == 1 && vm.find_frame(p).is_some());
                assert_eq!(vm.readonly_backed_range(start, end), expected);
            }
        }
        vm.set_permissions(va, 1);
        vm.mark_mapping_uncertain(va);
        assert!(!vm.readonly_backed_range(4096, 514 * 4096));
        vm.confirm_unmapped(va);
        vm.set_permissions(va, 1);
        vm.forget_frame(va);
        vm.set_permissions(va, 1);
        assert!(!vm.readonly_backed_range(4096, 514 * 4096));
        vm.record_frame(va, page).unwrap();
        assert!(vm.readonly_backed_range(4096, 514 * 4096));
    }
    vm.set_permissions(4101, 0); vm.mark_mapping_uncertain(4103);
    vm.record_frame(4107, 42).unwrap();
    assert!(vm.readonly_backed_range(4096, 514 * 4096));
}

#[test]
fn readonly_extent_requires_all_frames_and_exact_rights_including_default_gaps() {
    let mut vm = VmSpace::new();
    for start in [4096, (usize::MAX & !4095) - 8192] {
        for offset in 0..2 {
            let va = start + offset * 4096;
            vm.record_frame(va, 0).unwrap();
            vm.set_permissions(va, 1);
        }
        assert!(vm.readonly_backed_range(start, start + 8192));
        vm.set_permissions(start + 4096, 7);
        assert!(!vm.readonly_backed_range(start, start + 8192));
        vm.set_permissions(start + 4096, 1);
        vm.forget_frame(start);
        vm.record_frame(start + 3, 99).unwrap();
        vm.set_permissions(start, 1);
        assert!(!vm.readonly_backed_range(start, start + 8192));
    }
    for (start, end) in [(4096, 4096), (8192, 4096), (4097, 8192), (4096, 8193)] {
        assert!(!vm.readonly_backed_range(start, end));
    }
}

#[test]
fn exact_backed_rights_match_point_queries_for_default_none_and_explicit_permissions() {
    for desired in 0..=7 {
        let mut vm=VmSpace::new();
        for page in 1..=513 {vm.record_frame(page*4096,page).unwrap();vm.set_permissions(page*4096,desired);}
        for page in [1,256,513] {
            let va=page*4096;
            for changed in 0..=7 {
                vm.set_permissions(va,changed);
                for (start,end) in [(4096,514*4096),(va,va+4096),(4096,va+4096)] {
                    let expected=(start..end).step_by(4096).all(|p|!vm.mapping_is_uncertain(p)
                        && vm.permissions(p)==desired && vm.find_frame(p).is_some());
                    assert_eq!(vm.backed_range_with_permissions(start,end,desired),expected);
                }
            }
            vm.set_permissions(va,desired);vm.mark_mapping_uncertain(va);
            assert!(!vm.backed_range_with_permissions(va,va+4096,desired));
            assert!(!vm.backed_range_with_permissions(4096,514*4096,desired));
            vm.confirm_unmapped(va);vm.set_permissions(va,desired);vm.forget_frame(va);
            assert!(!vm.backed_range_with_permissions(4096,514*4096,desired));
            vm.record_frame(va,page).unwrap();vm.set_permissions(va,desired);
        }
        vm.record_frame(4107,99).unwrap();vm.set_permissions(4103,0);vm.mark_mapping_uncertain(4105);
        assert!(vm.backed_range_with_permissions(4096,514*4096,desired));
    }
}

#[test]
fn exact_backed_rights_require_valid_aligned_ranges_and_current_default_gaps() {
    let mut vm=VmSpace::new();let base=(usize::MAX&!4095)-8192;
    vm.record_frame(base,1).unwrap();vm.record_frame(base+4096,2).unwrap();
    assert!(vm.backed_range_with_permissions(base,base+8192,7));
    vm.set_permissions(base+4096,3);
    assert!(!vm.backed_range_with_permissions(base,base+8192,7));
    assert!(!vm.backed_range_with_permissions(base,base+8192,3));
    vm.set_permissions(base,3);assert!(vm.backed_range_with_permissions(base,base+8192,3));
    for (start,end,rights) in [(4096,4096,3),(8192,4096,3),(base+1,base+8192,3),
        (base,base+8191,3),(base,base+8192,8)] {
        assert!(!vm.backed_range_with_permissions(start,end,rights));
    }
}

#[test]
fn exact_backed_range_proof_does_not_allocate() {
    let mut vm=VmSpace::new();for p in 1..=513 {vm.record_frame(p*4096,p).unwrap();vm.set_permissions(p*4096,3);}
    COUNTS.with(|c|c.set(Allocations::default()));TRACK.with(|c|c.set(true));
    let matched=vm.backed_range_with_permissions(4096,514*4096,3);
    TRACK.with(|c|c.set(false));let calls=COUNTS.with(|c|c.get().calls);
    assert!(matched);assert_eq!(calls,0);
}

#[test]
fn present_frame_visits_match_logical_presence_in_va_order_without_changing_metadata() {
    let mut vm=VmSpace::new();let mut records=Vec::new();
    for i in (0..513).rev() {
        let va=(i+1)*4096;let rights=i%8;let uncertain=i%11==0;
        vm.record_frame(va,i+1000).unwrap();vm.set_permissions(va,rights);
        if uncertain {vm.mark_mapping_uncertain(va);}
        records.push((va,i+1000,rights,uncertain));
    }
    vm.record_frame(4107,9999).unwrap();records.push((4107,9999,7,false));
    vm.record_frame(4111,9998).unwrap();vm.set_permissions(4111,0);records.push((4111,9998,0,false));
    vm.set_permissions(514*4096,3); // No Frame, so no UNMAP callback.
    records.sort_unstable_by_key(|r|r.0);
    for (start,end) in [(0,usize::MAX),(8192,257*4096),(4096,4096),(8192,4096)] {
        let expected:Vec<_>=records.iter().filter(|r|r.0>=start && r.0<end && (r.2!=0||r.3)).map(|r|(r.0,r.1)).collect();
        let mut actual=Vec::new();
        let result:Result<(),(usize,())>=vm.try_for_each_present_frame(start,end,|va,frame|{actual.push((va,frame));Ok(())});
        assert_eq!(result,Ok(()));assert_eq!(actual,expected);
    }
    for (va,frame,rights,uncertain) in records {
        assert_eq!(vm.find_frame(va),Some(frame));assert_eq!(vm.mapping_is_uncertain(va),uncertain);
        assert_eq!(vm.permissions(va),if uncertain {0}else{rights});
    }
}

#[test]
fn present_frame_failure_returns_exact_va_and_stops_callbacks_after_the_failed_prefix() {
    let mut vm=VmSpace::new();for i in 0..513 {vm.record_frame((i+1)*4096,1000+i).unwrap();}
    for failed in [0,1,256,512] {
        let mut calls=Vec::new();
        let result=vm.try_for_each_present_frame(0,usize::MAX,|va,frame|{
            calls.push((va,frame));if calls.len()==failed+1 {Err(73)} else {Ok(())}
        });
        assert_eq!(result,Err(((failed+1)*4096,73)));assert_eq!(calls.len(),failed+1);
        for (i,(va,frame)) in calls.into_iter().enumerate() {assert_eq!((va,frame),((i+1)*4096,1000+i));}
        assert_eq!(vm.backed_pages(0,usize::MAX).len(),513);
    }
}

#[test]
fn present_frame_visit_does_not_allocate_or_build_a_snapshot() {
    let mut vm=VmSpace::new();for i in 0..513 {vm.record_frame((i+1)*4096,1000+i).unwrap();}
    let mut visited=0;
    COUNTS.with(|c|c.set(Allocations::default()));TRACK.with(|c|c.set(true));
    let result:Result<(),(usize,())>=vm.try_for_each_present_frame(0,usize::MAX,|_,_|{visited+=1;Ok(())});
    TRACK.with(|c|c.set(false));let calls=COUNTS.with(|c|c.get().calls);
    assert_eq!(result,Ok(()));assert_eq!(visited,513);assert_eq!(calls,0);
}

#[test]
fn range_access_matches_page_queries_across_sparse_permissions_and_uncertain_updates() {
    let mut vm = VmSpace::new();
    let mut state = 0x73a9_d021u64;
    for round in 0..512 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let page = (state as usize % 256) * 4096;
        vm.set_permissions(page, (state >> 32) as usize & 15);
        if round % 7 == 0 { vm.mark_mapping_uncertain(page); }
        if round % 11 == 0 { vm.confirm_unmapped(page); }
        // Old page-wise access never consults an unaligned metadata key.
        vm.set_permissions(page + 19, 0);
        vm.mark_mapping_uncertain(page + 31);
        let start = ((state >> 16) as usize % (128 * 4096)) + 1;
        let end = start + ((state >> 40) as usize % (128 * 4096)) + 1;
        for permission in [1, 2] {
            let expected = ((start & !4095)..end).step_by(4096)
                .all(|va| vm.permissions(va) & permission != 0);
            assert_eq!(vm.user_range_allowed(start, end, permission), expected,
                "round={round} range={start:#x}..{end:#x} permission={permission}");
        }
    }
}

#[test]
fn range_access_handles_partial_endpoints_high_addresses_and_invalid_inputs() {
    let mut vm = VmSpace::new();
    vm.set_permissions(0x2000, 0);
    assert!(vm.user_range_allowed(0x1001, 0x2000, 1));
    assert!(!vm.user_range_allowed(0x1001, 0x2001, 1));
    vm.set_permissions(0x2000, 1);
    assert!(vm.user_range_allowed(0x1001, 0x2001, 1));
    assert!(!vm.user_range_allowed(0x1001, 0x2001, 2));
    vm.mark_mapping_uncertain(0x2000);
    vm.set_permissions(0x2000, 7);
    assert!(!vm.user_range_allowed(0x1001, 0x2001, 1));
    vm.confirm_unmapped(0x2000);
    assert!(!vm.user_range_allowed(0x1001, 0x2001, 1));
    vm.set_permissions(0x2000, 7);
    assert!(vm.user_range_allowed(0x1001, 0x2001, 1));
    let last = usize::MAX & !4095;
    assert!(vm.user_range_allowed(last + 1, usize::MAX, 2));
    vm.mark_mapping_uncertain(last);
    assert!(!vm.user_range_allowed(last - 4096, usize::MAX, 1));
    for (start, end, permission) in [(1, 1, 1), (2, 1, 1), (1, 8192, 0), (1, 8192, 3), (1, 8192, 8)] {
        assert!(!vm.user_range_allowed(start, end, permission));
    }
}

#[test]
fn dense_read_write_permissions_and_implicit_defaults_follow_live_exceptions() {
    let mut vm = VmSpace::new();
    let start = 0x1000;
    let end = start + 4096 * 4096;
    for permission in [1, 2] {
        assert!(vm.user_range_allowed(start, end, permission));
    }
    for va in (start..end).step_by(4096) { vm.set_permissions(va, 3); }
    for permission in [1, 2] {
        assert!(vm.user_range_allowed(start + 17, end - 19, permission));
    }
    let middle = start + 2048 * 4096;
    vm.set_permissions(middle, 1);
    assert!(vm.user_range_allowed(start, end, 1));
    assert!(!vm.user_range_allowed(start, end, 2));
    vm.set_permissions(middle, 7);
    assert!(vm.user_range_allowed(start, end, 2));
    vm.mark_mapping_uncertain(middle);
    assert!(!vm.user_range_allowed(start, end, 1));
    vm.confirm_unmapped(middle);
    assert!(!vm.user_range_allowed(start, end, 1));
    vm.set_permissions(middle, 3);
    assert!(vm.user_range_allowed(start, end, 1));
}

#[derive(Clone, Copy, Default)]
struct Allocations {
    calls: usize,
    live: usize,
    peak: usize,
}
thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<Allocations> = const { Cell::new(Allocations { calls: 0, live: 0, peak: 0 }) };
    static DENY_AT_OR_ABOVE: Cell<usize> = const { Cell::new(usize::MAX) };
    static DENIED: Cell<usize> = const { Cell::new(0) };
    static FAIL_ONCE_BYTES: Cell<usize> = const { Cell::new(0) };
}
struct CountingAllocator;
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if FAIL_ONCE_BYTES.with(|bytes| {
            if bytes.get() != 0 && bytes.get() == layout.size() { bytes.set(0); true } else { false }
        }) { return core::ptr::null_mut(); }
        if DENY_AT_OR_ABOVE.with(|limit| layout.size() >= limit.get()) {
            DENIED.with(|count| count.set(count.get() + 1));
            return core::ptr::null_mut();
        }
        let pointer = System.alloc(layout);
        if !pointer.is_null() && TRACK.with(Cell::get) {
            COUNTS.with(|counts| {
                let mut value = counts.get();
                value.calls += 1;
                value.live += layout.size();
                value.peak = value.peak.max(value.live);
                counts.set(value);
            });
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if TRACK.with(Cell::get) {
            COUNTS.with(|counts| {
                let mut value = counts.get();
                value.live -= layout.size();
                counts.set(value);
            });
        }
        System.dealloc(pointer, layout);
    }
}

#[test]
fn tree_preserves_owned_send_and_shared_read_bounds() {
    fn require_send_sync<T: Send + Sync>() {}
    require_send_sync::<avl::AvlTree>();
    let mut tree = avl::AvlTree::new();
    for key in 0..2048 {
        tree.insert(key, key + 1).unwrap();
    }
    let tree = std::thread::spawn(move || {
        for key in (0..2048).step_by(2) {
            assert_eq!(tree.remove(key), Some(key + 1));
        }
        tree
    })
    .join()
    .unwrap();
    let tree = std::sync::Arc::new(tree);
    let readers: Vec<_> = (0..2)
        .map(|_| {
            let tree = tree.clone();
            std::thread::spawn(move || {
                for key in 0..2048 {
                    assert_eq!(tree.find(key), (key % 2 == 1).then_some(key + 1));
                }
            })
        })
        .collect();
    for reader in readers {
        reader.join().unwrap();
    }
}

#[test]
fn small_node_insertions_survive_large_allocation_denial() {
    // 64 bytes rejects pool-directory growth; 512 permits its first entries
    // but rejects a complete node block. Original single-node Boxes still fit.
    for threshold in [64, 512] {
        let mut tree = avl::AvlTree::new();
        DENIED.with(|count| count.set(0));
        DENY_AT_OR_ABOVE.with(|limit| limit.set(threshold));
        let result = (|| {
            for key in 0..256 {
                tree.insert(key, usize::MAX - key)?;
            }
            Ok::<(), ()>(())
        })();
        DENY_AT_OR_ABOVE.with(|limit| limit.set(usize::MAX));
        assert_eq!(result, Ok(()));
        for key in 0..256 {
            assert_eq!(tree.find(key), Some(usize::MAX - key));
            assert_eq!(tree.remove(key), Some(usize::MAX - key));
        }
        println!(
            "AVL-FAILOVER threshold={threshold} denied={}",
            DENIED.with(Cell::get)
        );
    }
}

#[test]
fn avl_word_keys_updates_removals_and_ranges_match_an_independent_model() {
    let mut tree = avl::AvlTree::new();
    let mut expected = BTreeMap::new();
    let mut random = 0x51a75e_u64;
    for turn in 0..20000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let key = match random % 5 {
            0 => 0,
            1 => usize::MAX,
            2 => usize::MAX - ((random >> 17) as usize % 2048),
            _ => (random >> 32) as usize % 2048,
        };
        if random & 7 < 5 {
            let value = (random as usize).rotate_left(17);
            tree.insert(key, value).unwrap();
            expected.insert(key, value);
        } else {
            assert_eq!(tree.remove(key), expected.remove(&key));
        }
        for probe in [0, key, usize::MAX, 2048] {
            assert_eq!(tree.find(probe), expected.get(&probe).copied());
        }
        if turn % 31 == 0 {
            for (start, end) in [(0, usize::MAX), (key, key), (512, 1536), (2000, 1000)] {
                let mut visited = Vec::new();
                tree.visit_range(start, end, |key, value| visited.push((key, value)));
                let wanted: Vec<_> = expected
                    .iter()
                    .filter(|(&key, _)| key >= start && key < end)
                    .map(|(&key, &value)| (key, value))
                    .collect();
                assert_eq!(visited, wanted);
            }
        }
    }
}

#[test]
fn avl_sorted_insertion_deletion_rotations_and_reuse_preserve_contents() {
    let mut tree = avl::AvlTree::new();
    tree.insert(usize::MAX, usize::MAX).unwrap();
    for cycle in 0..8 {
        let base = cycle * 4096;
        for key in 0..2048 {
            tree.insert(base + key, key).unwrap();
        }
        for key in (0..2048).step_by(2) {
            assert_eq!(tree.remove(base + key), Some(key));
        }
        for key in (0..2048).rev() {
            assert_eq!(tree.find(base + key), (key % 2 == 1).then_some(key));
            assert_eq!(tree.remove(base + key), (key % 2 == 1).then_some(key));
        }
        assert_eq!(tree.find(usize::MAX), Some(usize::MAX));
    }
    assert_eq!(tree.remove(usize::MAX), Some(usize::MAX));
    for key in (0..4096).rev() {
        tree.insert(key, usize::MAX - key).unwrap();
    }
    for key in 0..4096 {
        assert_eq!(tree.remove(key), Some(usize::MAX - key));
    }
    assert_eq!(tree.remove(0), None);
}

#[test]
fn an_empty_tree_releases_its_node_storage_before_drop() {
    COUNTS.with(|counts| counts.set(Allocations::default()));
    TRACK.with(|track| track.set(true));
    let mut tree = avl::AvlTree::new();
    for key in 0..4096 {
        tree.insert(key, key).unwrap();
    }
    for key in 0..4096 {
        assert_eq!(tree.remove(key), Some(key));
    }
    let retained = COUNTS.with(Cell::get).live;
    drop(tree);
    TRACK.with(|track| track.set(false));
    assert_eq!(retained, 0);
    assert_eq!(COUNTS.with(Cell::get).live, 0);
}

#[test]
fn permission_only_entries_never_appear_as_backed_pages() {
    let mut vm = VmSpace::new();
    vm.set_permissions(0x1000, 0);
    assert_eq!(vm.find_frame(0x1000), None);
    assert_eq!(vm.permissions(0x1000), 0);
    assert!(vm.backed_pages(0, usize::MAX).is_empty());
    assert!(vm.all_backed_pages(0, usize::MAX, |_| panic!("no backing")));
    assert!(!vm.page_tables_ready(0x1000));
    vm.record_frame(0x1000, 0).unwrap();
    assert_eq!(vm.find_frame(0x1000), Some(0));
    assert_eq!(vm.backed_pages(0, usize::MAX), [(0x1000, 0)]);
    vm.record_frame(0x1000, 99).unwrap();
    assert_eq!(vm.permissions(0x1000), 0);
    assert_eq!(vm.forget_frame(0x1000), Some(99));
    assert_eq!(vm.permissions(0x1000), 7);
    vm.set_permissions(0x2000, 3);
    vm.set_permissions(0x2000, 7);
    assert_eq!(vm.find_frame(0x2000), None);
    assert_eq!(vm.permissions(0x2000), 7);
    assert_eq!(vm.forget_frame(0x2000), None);
}

#[test]
fn frame_and_permission_updates_match_an_independent_model() {
    let mut vm = VmSpace::new();
    let mut expected: BTreeMap<usize, (Option<usize>, usize)> = BTreeMap::new();
    let mut random = 0x47615_u64;
    for turn in 0..12000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let key = ((random >> 16) as usize % 128) * 4096;
        match random % 5 {
            0 | 1 => {
                let descriptor = (random >> 40) as usize;
                vm.record_frame(key, descriptor).unwrap();
                expected.entry(key).or_insert((None, 7)).0 = Some(descriptor);
            }
            2 | 3 => {
                let permissions = (random >> 32) as usize % 8;
                vm.set_permissions(key, permissions);
                let entry = expected.entry(key).or_insert((None, 7));
                entry.1 = permissions;
                if *entry == (None, 7) {
                    expected.remove(&key);
                }
            }
            _ => {
                assert_eq!(
                    vm.forget_frame(key),
                    expected.remove(&key).and_then(|entry| entry.0)
                );
            }
        }
        for key in [0, key, 4096 * 127, usize::MAX] {
            let entry = expected.get(&key).copied().unwrap_or((None, 7));
            assert_eq!(vm.find_frame(key), entry.0);
            assert_eq!(vm.permissions(key), entry.1);
        }
        if turn % 17 == 0 {
            let start = ((random >> 8) as usize % 128) * 4096;
            let end = start + 4096 * 19;
            let pages: Vec<_> = expected
                .range(start..end)
                .filter_map(|(&va, &(frame, permissions))| frame.map(|_| (va, permissions)))
                .collect();
            assert_eq!(vm.backed_pages(start, end), pages);
            let mut visited = Vec::new();
            let valid = vm.all_backed_pages(start, end, |va| {
                visited.push(va);
                expected[&va].1 != 0
            });
            assert_eq!(
                valid,
                pages.iter().all(|&(_, permissions)| permissions != 0)
            );
            let stop = pages
                .iter()
                .position(|&(_, permissions)| permissions == 0)
                .map_or(pages.len(), |index| index + 1);
            assert_eq!(
                visited,
                pages[..stop].iter().map(|&(va, _)| va).collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn permissions_keep_full_word_values_and_mapping_hints_stay_vm_local() {
    let mut first = VmSpace::new();
    let second = VmSpace::new();
    first.set_permissions(0x1000, usize::MAX);
    first.record_frame(0x1000, usize::MAX).unwrap();
    assert_eq!(first.permissions(0x1000), usize::MAX);
    assert_eq!(first.find_frame(0x1000), Some(usize::MAX));
    assert!(first.page_tables_ready(0x1000));
    assert!(!second.page_tables_ready(0x1000));
    first.record_page_table(0x2000, 42).unwrap();
    assert_eq!(first.find_page_table_slot(0x2000), Some(42));
    first.record_page_table(0x2000, 43).unwrap();
    assert_eq!(first.find_page_table_slot(0x2000), Some(43));
}

#[test]
fn uncertain_presence_is_separate_from_permission_bits_and_requires_confirmation() {
    let mut vm = VmSpace::new();
    vm.record_frame(0x1000, 42).unwrap();
    vm.set_permissions(0x1000, usize::MAX);
    vm.mark_mapping_uncertain(0x1000);
    assert_eq!(vm.permissions(0x1000), 0);
    assert_eq!(
        vm.find_frame(0x1000),
        Some(42),
        "ownership metadata is not discarded"
    );
    assert!(vm.frame_may_be_present(0x1000));
    assert!(vm.has_uncertain_mappings(0, 0x2000));
    assert!(!vm.has_uncertain_mappings(0x2000, 0x3000));
    vm.set_permissions(0x1000, 7);
    assert_eq!(vm.permissions(0x1000), 0);
    vm.confirm_unmapped(0x1000);
    assert_eq!(vm.permissions(0x1000), 0);
    assert!(!vm.frame_may_be_present(0x1000));
    assert!(!vm.mapping_is_uncertain(0x1000));
    vm.set_permissions(0x1000, usize::MAX);
    assert_eq!(vm.permissions(0x1000), usize::MAX);
    vm.mark_mapping_uncertain(0x1000);
    assert_eq!(vm.forget_frame(0x1000), Some(42));
    assert!(!vm.mapping_is_uncertain(0x1000));
}

#[test]
fn permission_keys_remain_exact_across_offsets_blocks_and_word_boundaries() {
    let mut vm = VmSpace::new();
    let addresses = [
        0,
        1,
        31,
        4095,
        4096,
        4097,
        8191,
        8192,
        32767,
        32768,
        32769,
        usize::MAX - 4096,
        usize::MAX - 1,
        usize::MAX,
    ];
    for (index, &address) in addresses.iter().enumerate() {
        vm.set_permissions(address, usize::MAX - index);
    }
    for (index, &address) in addresses.iter().enumerate() {
        assert_eq!(vm.permissions(address), usize::MAX - index);
    }
    for address in [2, 30, 4094, 4098, 8190, 8193, 32766, 32770, usize::MAX - 2] {
        assert_eq!(vm.permissions(address), 7);
    }
    for &address in &addresses {
        assert_eq!(vm.forget_frame(address), None);
        assert_eq!(vm.permissions(address), 7);
    }
    assert!(vm.backed_pages(0, usize::MAX).is_empty());
}

#[test]
fn permission_removal_never_allocates_including_interior_pages_and_empty_storage() {
    let mut vm = VmSpace::new();
    for page in 0..48 {
        vm.set_permissions(page * 4096, 3);
        vm.record_frame(page * 4096, page + 1).unwrap();
    }
    DENIED.with(|count| count.set(0));
    DENY_AT_OR_ABOVE.with(|limit| limit.set(0));
    // No assertions/reporting under allocation denial: only the production
    // metadata operations, which must also work during rollback/reclaim.
    vm.set_permissions(17 * 4096, 7);
    let removed = vm.forget_frame(23 * 4096);
    for page in (0..16).rev() {
        vm.set_permissions(page * 4096, 7);
    }
    DENY_AT_OR_ABOVE.with(|limit| limit.set(usize::MAX));
    assert_eq!(DENIED.with(Cell::get), 0);
    assert_eq!(removed, Some(24));
    for page in 0..48 {
        let cleared = page < 16 || page == 17 || page == 23;
        assert_eq!(vm.permissions(page * 4096), if cleared { 7 } else { 3 });
    }
}

#[test]
fn mixed_permission_updates_preserve_sparse_and_unaligned_independent_keys() {
    let mut vm = VmSpace::new();
    let mut expected = BTreeMap::new();
    let mut random = 0x90123a45_u64;
    for turn in 0..30000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let address = match random % 6 {
            0 => usize::MAX - ((random >> 21) as usize % 31),
            1 => ((random >> 17) as usize % 512) * 4096 + 1,
            2 => ((random >> 17) as usize % 512) * 4096 + 4095,
            _ => ((random >> 17) as usize % 512) * 4096,
        };
        let value = match (random >> 11) % 5 {
            0 => 7,
            1 => 0,
            2 => usize::MAX,
            _ => (random as usize).rotate_left(21),
        };
        if random & 7 == 0 {
            assert_eq!(vm.forget_frame(address), None);
            expected.remove(&address);
        } else {
            vm.set_permissions(address, value);
            if value == 7 {
                expected.remove(&address);
            } else {
                expected.insert(address, value);
            }
        }
        for probe in [0, address, address ^ 4096, usize::MAX] {
            assert_eq!(
                vm.permissions(probe),
                expected.get(&probe).copied().unwrap_or(7)
            );
        }
        if turn % 127 == 0 {
            for page in 0..512 {
                for offset in [0, 1, 4095] {
                    let address = page * 4096 + offset;
                    assert_eq!(
                        vm.permissions(address),
                        expected.get(&address).copied().unwrap_or(7)
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "host permission-only footprint, not application speed or total RAM"]
fn permission_metadata_density_and_release() {
    for stride in [1, 8, 64] {
        for count in [512, 4096, 16384] {
            COUNTS.with(|counts| counts.set(Allocations::default()));
            TRACK.with(|track| track.set(true));
            let mut vm = VmSpace::new();
            for page in 0..count {
                vm.set_permissions(page * stride * 4096, 3);
            }
            let footprint = COUNTS.with(Cell::get);
            for page in (0..count).rev() {
                assert_eq!(vm.permissions(page * stride * 4096), 3);
                vm.forget_frame(page * stride * 4096);
            }
            let empty = COUNTS.with(Cell::get).live;
            drop(vm);
            TRACK.with(|track| track.set(false));
            assert_eq!(COUNTS.with(Cell::get).live, 0);
            println!("PERMISSION-METADATA count={count} stride={stride} allocations={} bytes={} peak={} empty={empty}", footprint.calls, footprint.live, footprint.peak);
        }
    }
}

// Bootstrap's allocation-free tree is deliberately large. Keep test fixture
// construction off the small default harness stack, including sanitizer runs.
fn bootstrap_vm_test(test: impl FnOnce(&mut vm_space::BootstrapVmSpace) + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let mut vm = vm_space::BootstrapVmSpace::new();
            test(&mut vm);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn bootstrap_readiness_requires_frame_registration_and_survives_unmapping() {
    bootstrap_vm_test(|vm| {
        let base = 0x6600_0000;
        assert!(!vm.page_tables_ready(base));
        vm.record_page_table(base, 37).unwrap();
        assert!(!vm.page_tables_ready(base));
        assert_eq!(vm.find_page_table_slot(base), Some(37));
        vm.record_frame(base, usize::MAX).unwrap();
        assert!(vm.page_tables_ready(base + 0x1f_f000));
        assert!(!vm.page_tables_ready(base + 0x20_0000));
        assert_eq!(vm.forget_frame(base), Some(usize::MAX));
        assert!(vm.page_tables_ready(base));
        assert_eq!(vm.find_frame(base), None);
    });
}

#[test]
fn bootstrap_frame_updates_and_churn_preserve_full_width_keys_without_allocations() {
    bootstrap_vm_test(|vm| {
        fn address(index: usize) -> usize {
            match index {
                0 => 0,
                1 => usize::MAX,
                2 => usize::MAX - 4095,
                _ => 0x6600_0000 ^ (index * 4096),
            }
        }

        let mut expected = [None; 257];
        let mut random = 19u64;
        COUNTS.with(|counts| counts.set(Allocations::default()));
        TRACK.with(|track| track.set(true));
        for step in 0..25_000 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let index = (random >> 32) as usize % expected.len();
            if step % 3 == 0 {
                assert_eq!(vm.forget_frame(address(index)), expected[index].take());
            } else {
                let frame = match step % 5 {
                    0 => 0,
                    1 => usize::MAX,
                    _ => step,
                };
                vm.record_frame(address(index), frame).unwrap();
                expected[index] = Some(frame);
            }
            if step % 97 == 0 {
                for (index, &frame) in expected.iter().enumerate() {
                    assert_eq!(vm.find_frame(address(index)), frame);
                }
            }
        }
        for (index, &frame) in expected.iter().enumerate() {
            assert_eq!(vm.forget_frame(address(index)), frame);
            assert_eq!(vm.forget_frame(address(index)), None);
        }
        TRACK.with(|track| track.set(false));
        assert_eq!(COUNTS.with(Cell::get).calls, 0);
    });
}

#[test]
fn bootstrap_clone_keeps_frame_and_table_updates_independent() {
    bootstrap_vm_test(|vm| {
        for (address, frame) in [(0, 0), (usize::MAX, usize::MAX), (0x6600_0000, 91)] {
            vm.record_frame(address, frame).unwrap();
        }
        vm.record_page_table(0x6600_0000, 31).unwrap();
        let mut copied = *vm;
        assert_eq!(copied.forget_frame(0), Some(0));
        copied.record_frame(usize::MAX, 17).unwrap();
        copied.record_page_table(0x6600_0000, 48).unwrap();
        vm.record_frame(0x6600_0000, 92).unwrap();
        assert_eq!(vm.find_frame(0), Some(0));
        assert_eq!(vm.find_frame(usize::MAX), Some(usize::MAX));
        assert_eq!(vm.find_page_table_slot(0x6600_0000), Some(31));
        assert_eq!(copied.find_frame(0), None);
        assert_eq!(copied.find_frame(usize::MAX), Some(17));
        assert_eq!(copied.find_frame(0x6600_0000), Some(91));
        assert_eq!(copied.find_page_table_slot(0x6600_0000), Some(48));
    });
}

#[test]
fn bootstrap_readiness_stays_local_and_rejects_failed_registration() {
    bootstrap_vm_test(|vm| {
        const REGION: usize = 0x20_0000;
        for index in 0..(1 << 14) {
            vm.record_frame(index * REGION, index).unwrap();
        }
        let last = ((1 << 14) - 1) * REGION;
        let denied = (1 << 14) * REGION;
        assert!(vm.page_tables_ready(last));
        assert_eq!(vm.record_frame(denied, 99), Err(()));
        assert!(!vm.page_tables_ready(denied));
        assert!(vm.page_tables_ready(last));
        assert_eq!(vm.find_frame(denied), None);
        assert_eq!(vm.find_frame(last), Some((1 << 14) - 1));
        let fresh = vm_space::BootstrapVmSpace::new();
        assert!(!fresh.page_tables_ready(last));
        assert!(!fresh.page_tables_ready(usize::MAX & !4095));
    });
}

#[test]
fn bootstrap_readiness_reuses_eight_regions_without_heap_allocations() {
    bootstrap_vm_test(|vm| {
        const REGION: usize = 0x20_0000;
        let trees = core::mem::size_of::<static_avl::StaticAvlTree<{ 1 << 14 }>>()
            + core::mem::size_of::<static_avl::StaticAvlTree<256>>();
        assert_eq!(
            core::mem::size_of::<vm_space::BootstrapVmSpace>(),
            trees + 9 * core::mem::size_of::<usize>()
        );
        COUNTS.with(|counts| counts.set(Allocations::default()));
        TRACK.with(|track| track.set(true));
        for turn in 0..16 {
            for index in 0..8 {
                let address = 0x6600_0000 + index * REGION + turn * 4096;
                vm.record_frame(address, turn * 8 + index).unwrap();
                for ready in 0..=index {
                    assert!(vm.page_tables_ready(0x6600_0000 + ready * REGION));
                }
            }
        }
        TRACK.with(|track| track.set(false));
        assert_eq!(COUNTS.with(Cell::get).calls, 0);
        for index in 0..8 {
            assert!(vm.page_tables_ready(0x6600_0000 + index * REGION));
        }
        assert!(!vm.page_tables_ready(0x6600_0000 + 8 * REGION));
    });
}

#[test]
fn bootstrap_and_guest_readiness_eviction_is_conservative() {
    bootstrap_vm_test(|vm| {
        const REGION: usize = 0x20_0000;
        for index in 0..8 {
            vm.record_frame(index * REGION, index).unwrap();
        }
        // Repeated mappings in resident regions must not evict other regions.
        for index in 0..32 {
            vm.record_frame(0, index).unwrap();
        }
        for index in 0..8 {
            assert!(vm.page_tables_ready(index * REGION));
        }
        vm.record_frame(8 * REGION, 8).unwrap();
        assert!(!vm.page_tables_ready(0));
        for index in 1..=8 {
            assert!(vm.page_tables_ready(index * REGION));
        }
        assert_eq!(vm.find_frame(0), Some(31));
        let mut guest = VmSpace::new();
        guest.record_frame(0, 0).unwrap();
        guest.record_frame(REGION, 1).unwrap();
        assert!(guest.page_tables_ready(0));
        assert!(guest.page_tables_ready(REGION));
    });
}

#[test]
fn guest_readiness_reuses_regions_without_skipping_frame_or_permission_checks() {
    const REGION: usize = 0x20_0000;
    let mut vm = VmSpace::new();
    let mut previous_region = usize::MAX;
    let mut old_depth_queries = 0;
    let mut new_depth_queries = 0;
    for turn in 0..128 {
        for index in 0..8 {
            let address = index * REGION + turn * 4096;
            if previous_region != index { old_depth_queries += 1; }
            if !vm.page_tables_ready(address) { new_depth_queries += 1; }
            // Model the existing MemoryManager guard, followed by successful
            // map/record. Readiness never creates a Frame for another page.
            assert_eq!(vm.find_frame(address), None);
            vm.record_frame(address, turn * 8 + index).unwrap();
            previous_region = index;
            assert_eq!(vm.find_frame(address), Some(turn * 8 + index));
        }
    }
    assert_eq!((old_depth_queries, new_depth_queries), (1024, 8));
    vm.set_permissions(0, 0);
    vm.mark_mapping_uncertain(REGION);
    assert!(vm.page_tables_ready(0));
    assert!(vm.page_tables_ready(REGION));
    assert!(!vm.user_range_allowed(0, 4096, 1));
    assert!(!vm.user_range_allowed(REGION, REGION + 4096, 1));
    vm.forget_frame(0);
    assert!(vm.page_tables_ready(0)); // Leaf UNMAP leaves tables installed.
    assert_eq!(vm.find_frame(0), None);
}

#[test]
fn guest_readiness_fifo_eviction_and_replacement_never_reuse_another_vm() {
    const REGION: usize = 0x20_0000;
    let mut vm = VmSpace::new();
    for index in 0..8 { vm.record_frame(index * REGION, index).unwrap(); }
    // Updating a resident region must not advance the replacement cursor.
    DENY_AT_OR_ABOVE.with(|limit| limit.set(0));
    for index in 0..8 {
        vm.record_frame(index * REGION, 100 + index).unwrap();
        assert!(vm.page_tables_ready(index * REGION + REGION - 1));
    }
    DENY_AT_OR_ABOVE.with(|limit| limit.set(usize::MAX));
    vm.record_page_table(9 * REGION, 99).unwrap();
    assert!(!vm.page_tables_ready(9 * REGION));
    vm.record_frame(8 * REGION, 8).unwrap();
    assert!(!vm.page_tables_ready(0));
    assert_eq!(vm.find_frame(0), Some(100));
    for index in 1..=8 { assert!(vm.page_tables_ready(index * REGION)); }
    let other = VmSpace::new();
    assert!(!other.page_tables_ready(REGION));
    vm = VmSpace::new();
    for index in 0..10 { assert!(!vm.page_tables_ready(index * REGION)); }
    assert!(!vm.page_tables_ready(usize::MAX));
}

#[test]
#[ignore = "host VM metadata benchmark, excludes page copying and A9N calls"]
fn construction_protection_lookup_and_drop_cost() {
    use std::hint::black_box;
    use std::time::Instant;
    println!(
        "VM-INDEX tree-bytes={} vm-bytes={}",
        core::mem::size_of::<avl::AvlTree>(),
        core::mem::size_of::<VmSpace>()
    );
    for count in [32, 33, 512, 513, 3072, 4096, 4097, 16384] {
        for protected in [false, true] {
            for sample in 0..3 {
                COUNTS.with(|counts| counts.set(Allocations::default()));
                TRACK.with(|track| track.set(true));
                let mut vm = VmSpace::new();
                let begin = Instant::now();
                for page in 0..count {
                    let va = page * 4096;
                    vm.record_frame(va, page + 1).unwrap();
                    if protected {
                        vm.set_permissions(va, 3);
                    }
                }
                let constructed = begin.elapsed().as_nanos();
                let footprint = COUNTS.with(Cell::get);
                for page in (0..count).rev() {
                    let va = page * 4096;
                    assert_eq!(black_box(vm.find_frame(black_box(va))), Some(page + 1));
                    assert_eq!(
                        black_box(vm.permissions(black_box(va))),
                        if protected { 3 } else { 7 }
                    );
                }
                let looked_up = begin.elapsed().as_nanos();
                black_box(vm.backed_pages(0, count * 4096));
                let walked = begin.elapsed().as_nanos();
                drop(vm);
                let total = begin.elapsed().as_nanos();
                TRACK.with(|track| track.set(false));
                assert_eq!(COUNTS.with(Cell::get).live, 0);
                println!("VM-INDEX pages={count} protected={protected} sample={sample} allocations={} bytes={} peak-bytes={} construct-ns={constructed} lookup-ns={} walk-ns={} total-ns={total}", footprint.calls, footprint.live, footprint.peak, looked_up - constructed, walked - looked_up);
            }
        }
    }
}

#[test]
#[ignore = "host AVL churn and retention probe, excludes A9N and page copies"]
fn avl_node_churn_and_sparse_retention_cost() {
    use std::{hint::black_box, time::Instant};
    COUNTS.with(|counts| counts.set(Allocations::default()));
    TRACK.with(|track| track.set(true));
    let mut tree = avl::AvlTree::new();
    let before = Instant::now();
    for key in 0..4096 {
        tree.insert(key, key).unwrap();
    }
    let full = COUNTS.with(Cell::get);
    // Keep one entry per 64 original keys to expose slab fragmentation, rather
    // than only reporting perfectly packed or completely empty trees.
    for key in 0..4096 {
        if key % 64 != 0 {
            assert_eq!(tree.remove(key), Some(key));
        }
    }
    let sparse = COUNTS.with(Cell::get);
    for cycle in 0..8 {
        for key in 0..4096 {
            tree.insert(key, key + cycle).unwrap();
        }
        for key in 0..4096 {
            black_box(tree.find(key));
            if key % 64 != 0 {
                assert_eq!(tree.remove(key), Some(key + cycle));
            }
        }
    }
    let elapsed = before.elapsed().as_nanos();
    drop(tree);
    let total = COUNTS.with(Cell::get);
    TRACK.with(|track| track.set(false));
    assert_eq!(total.live, 0);
    println!("AVL-CHURN full-calls={} full-bytes={} sparse-bytes={} total-calls={} peak-bytes={} ns={elapsed}", full.calls, full.live, sparse.live, total.calls, total.peak);
}

#[test]
fn deferred_snapshot_keeps_logical_backing_but_never_invents_a_pte_or_table() {
    let mut vm=VmSpace::new();let base=0x800000;
    for page in 0..130 {let va=base+page*4096;vm.mark_mapping_deferred(va);vm.record_frame(va,1000+page).unwrap();vm.set_permissions(va,1);}
    assert!(vm.readonly_backed_range(base,base+130*4096));
    assert!(!vm.page_tables_ready(base));
    for page in 0..130 {let va=base+page*4096;assert_eq!(vm.permissions(va),1);assert_eq!(vm.hardware_permissions(va),0);assert!(!vm.frame_may_be_present(va));}
    let mut present=Vec::new();vm.try_for_each_present_frame(base,base+130*4096,|va,_|{present.push(va);Ok::<_,()>(())}).unwrap();assert!(present.is_empty());
    vm.mark_mapping_uncertain(base+4096);assert!(vm.frame_may_be_present(base+4096));
    vm.complete_deferred_mapping(base+64*4096);assert_eq!(vm.hardware_permissions(base+64*4096),1);
    assert!(vm.snapshot_page_managed(base+64*4096));assert!(vm.page_tables_ready(base+64*4096));
    vm.set_permissions(base+65*4096,0);assert!(!vm.mapping_deferred(base+65*4096));assert!(vm.snapshot_page_managed(base+65*4096));
    vm.forget_frame(base+66*4096);assert!(!vm.mapping_deferred(base+66*4096));assert!(!vm.snapshot_page_managed(base+66*4096));
    for first in [0,1,63,64,65,66,67,129,130] {for last in [0,1,63,64,65,66,67,129,130] {
        let a=base+first*4096;let b=base+last*4096;
        assert_eq!(vm.has_deferred_mappings(a,b),a<b && (a..b).step_by(4096).any(|va|vm.mapping_deferred(va)));}}
}
