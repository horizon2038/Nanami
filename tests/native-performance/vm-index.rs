//! Production VM metadata, with no mocked indexing or permission rules.
#![allow(dead_code)]
extern crate alloc;
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
}
use vm_space::{VmSpace, VmTracker};

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
}
struct CountingAllocator;
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
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
fn bootstrap_readiness_eviction_is_conservative_and_guest_policy_is_unchanged() {
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
        assert!(!guest.page_tables_ready(0));
        assert!(guest.page_tables_ready(REGION));
    });
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
