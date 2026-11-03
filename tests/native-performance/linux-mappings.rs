#![allow(dead_code)]
extern crate alloc;
extern crate self as nanami_runtime;
pub type Word = usize;
#[path = "../../nanami/servers/apps/alter/shared/src/common/abi.rs"]
mod abi;
use abi::ALTER_PROCESS_MAPPING_MAX;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

thread_local! {
    static FAIL_NEXT_ZEROED: Cell<bool> = const { Cell::new(false) };
    static ZEROED_CALLS: Cell<usize> = const { Cell::new(0) };
}
struct TestAllocator;
unsafe impl GlobalAlloc for TestAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ZEROED_CALLS.with(|calls| calls.set(calls.get() + 1));
        if FAIL_NEXT_ZEROED.with(|fail| fail.replace(false)) {
            return core::ptr::null_mut();
        }
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: TestAllocator = TestAllocator;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcessMapping {
    base: Word,
    size: Word,
    prot: Word,
}
impl ProcessMapping {
    const EMPTY: Self = Self {
        base: 0,
        size: 0,
        prot: 0,
    };
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/mapping_table.rs"]
mod mapping_table;
use mapping_table::MappingTable;
struct ManagedProcess {
    mappings: MappingTable,
}
struct Runtime {
    process: ManagedProcess,
    child: ManagedProcess,
}
impl Runtime {
    fn new() -> Self {
        Self {
            process: ManagedProcess {
                mappings: MappingTable::EMPTY,
            },
            child: ManagedProcess {
                mappings: MappingTable::EMPTY,
            },
        }
    }
    fn managed_process(&self, pid: Word) -> Option<&ManagedProcess> {
        match pid {
            1 => Some(&self.process),
            2 => Some(&self.child),
            _ => None,
        }
    }
    fn managed_process_mut(&mut self, pid: Word) -> Option<&mut ManagedProcess> {
        match pid {
            1 => Some(&mut self.process),
            2 => Some(&mut self.child),
            _ => None,
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        if !self.child.mappings.same_storage(self.process.mappings) {
            self.child.mappings.release();
        }
        self.process.mappings.release();
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/mappings.rs"]
mod mappings;

#[test]
fn ordinary_fork_copies_only_live_vmas_into_independent_storage() {
    let mut r = Runtime::new();
    for index in 0..ALTER_PROCESS_MAPPING_MAX {
        assert!(r.add_mapping(1, 0x10000 + index * 8192, 4096, index % 8));
    }
    let before = ZEROED_CALLS.with(Cell::get);
    assert!(r.inherit_mappings(1, 2));
    assert_eq!(ZEROED_CALLS.with(Cell::get) - before, 1);
    assert_eq!(&*r.process.mappings, &*r.child.mappings);
    assert!(!r.process.mappings.same_storage(r.child.mappings));
    assert_eq!(r.child.mappings.capacity(), ALTER_PROCESS_MAPPING_MAX);
    assert!(r.protect_mapping(2, 0x10000, 4096, 3));
    assert_eq!(r.mapping_prot(1, 0x10000, 4096), Some(0));
    assert_eq!(r.mapping_prot(2, 0x10000, 4096), Some(3));
    r.child.mappings.clear();
    assert_eq!(r.process.mappings.len(), ALTER_PROCESS_MAPPING_MAX);
    r.child.mappings.release();
    assert!(r.inherit_mappings(1, 2));
    r.process.mappings.release();
    assert_eq!(r.child.mappings.len(), ALTER_PROCESS_MAPPING_MAX);
}

#[test]
fn fork_preserves_unsorted_overlapping_vma_policy_and_order() {
    let mut r = Runtime::new();
    for (base, size, prot) in [(0x9000, 4096, 3), (0x1000, 16384, 1), (0x3000, 4096, 0)] {
        assert!(r.process.mappings.push(ProcessMapping { base, size, prot }));
    }
    assert!(r.inherit_mappings(1, 2));
    assert_eq!(&*r.process.mappings, &*r.child.mappings);
    for base in (0x1000..0xb000).step_by(4096) {
        for permission in [1, 2, 3, 4] {
            assert_eq!(
                r.memory_access_allowed(1, base, 4096, permission),
                r.memory_access_allowed(2, base, 4096, permission)
            );
        }
    }
}

#[test]
fn empty_fork_stays_lazy_and_invalid_or_shared_destinations_are_unchanged() {
    let mut r = Runtime::new();
    let before = ZEROED_CALLS.with(Cell::get);
    assert!(r.inherit_mappings(1, 2));
    assert_eq!(ZEROED_CALLS.with(Cell::get), before);
    assert!(!r.inherit_mappings(999, 2));
    assert!(!r.inherit_mappings(1, 999));
    assert!(r.child.mappings.ensure());
    assert!(!r.inherit_mappings(1, 2));
    r.child.mappings.release();
    assert!(r.add_mapping(1, 0x1000, 4096, 0));
    assert!(!r.inherit_mappings(1, 1));
    assert!(r.add_mapping(2, 0x9000, 4096, 3));
    assert!(!r.inherit_mappings(1, 2));
    assert_eq!(r.mapping_prot(2, 0x9000, 4096), Some(3));
    r.child.mappings.release();
    r.child.mappings = r.process.mappings;
    r.child.mappings.clear();
    assert!(!r.inherit_mappings(1, 2));
    assert!(r.process.mappings.is_empty());
}

#[test]
fn failed_fork_table_allocation_preserves_parent_and_allows_retry() {
    let mut r = Runtime::new();
    assert!(r.add_mapping(1, 0x1000, 4096, 0));
    FAIL_NEXT_ZEROED.with(|fail| fail.set(true));
    let inherited = r.inherit_mappings(1, 2);
    FAIL_NEXT_ZEROED.with(|fail| fail.set(false));
    assert!(!inherited);
    assert!(r.child.mappings.is_empty());
    assert_eq!(r.mapping_prot(1, 0x1000, 4096), Some(0));
    assert!(r.inherit_mappings(1, 2));
    assert!(!r.process.mappings.same_storage(r.child.mappings));
    assert!(!r.memory_access_allowed(2, 0x1000, 1, 1));
}

#[test]
fn copyin_copyout_checks_cover_prot_none_and_crossing_vma_boundaries() {
    let mut runtime = Runtime::new();
    assert!(runtime.add_mapping(1, 0x1000, 0x3000, 3));
    assert!(runtime.protect_mapping(1, 0x2000, 0x1000, 0));
    assert!(runtime.memory_access_allowed(1, 0x1000, 0x1000, 2));
    assert!(!runtime.memory_access_allowed(1, 0x2000, 4, 1));
    assert!(!runtime.memory_access_allowed(1, 0x1fff, 2, 1));
    assert!(!runtime.memory_access_allowed(1, usize::MAX, 2, 1));
    assert!(runtime.protect_mapping(1, 0x2000, 0x1000, 1));
    assert!(runtime.memory_access_allowed(1, 0x2000, 4, 1));
    assert!(!runtime.memory_access_allowed(1, 0x2000, 4, 2));
}

#[test]
fn mapping_table_is_lazy_and_does_not_expand_process_snapshots_or_server_stack() {
    assert_eq!(
        core::mem::size_of::<MappingTable>(),
        core::mem::size_of::<Word>()
    );
    let mut runtime = Runtime::new();
    assert!(runtime.process.mappings.is_empty());
    assert!(runtime.add_mapping(1, 0x1000, 0x1000, 3));
    assert_eq!(runtime.process.mappings.len(), 1);
    assert_eq!(
        runtime.process.mappings.capacity(),
        ALTER_PROCESS_MAPPING_MAX
    );
    runtime.process.mappings.release();
    assert!(runtime.process.mappings.is_empty());
    runtime.process.mappings.release();
    assert!(runtime.add_mapping(1, 0x3000, 0x1000, 3));
}

#[test]
fn editor_sized_fragmented_vmas_survive_capacity_failure_and_reuse() {
    let mut runtime = Runtime::new();
    assert!(ALTER_PROCESS_MAPPING_MAX >= 128);
    for index in 0..ALTER_PROCESS_MAPPING_MAX {
        assert!(runtime.add_mapping(1, 0x1000 + index * 0x2000, 0x1000, 3));
    }
    assert!(!runtime.add_mapping(1, 0x1000000, 0x1000, 3));
    for index in 0..ALTER_PROCESS_MAPPING_MAX {
        assert!(runtime.has_mapping(1, 0x1000 + index * 0x2000, 0x1000));
    }
    assert!(runtime.remove_mapping(1, 0x1000, 0x1000));
    assert!(runtime.add_mapping(1, 0x1000000, 0x1000, 3));
    assert!(!runtime.has_mapping(1, 0x1000, 0x1000));
}

#[test]
fn adjacent_ranges_coalesce_and_partial_unmap_preserves_both_sides() {
    let mut runtime = Runtime::new();
    for base in [0x1000, 0x3000, 0x2000] {
        assert!(runtime.add_mapping(1, base, 0x1000, 3));
    }
    assert_eq!(
        runtime
            .process
            .mappings
            .iter()
            .filter(|mapping| mapping.base != 0)
            .count(),
        1
    );
    assert!(runtime.remove_mapping(1, 0x2000, 0x1000));
    assert!(runtime.has_mapping(1, 0x1000, 0x1000));
    assert!(runtime.has_mapping(1, 0x3000, 0x1000));
    assert!(!runtime.has_mapping(1, 0x2000, 0x1000));
    assert!(!runtime.add_mapping(1, Word::MAX - 1, 4096, 3));
    assert!(!runtime.add_mapping(999, 0x1000, 4096, 3));
}

#[test]
fn protection_capacity_failure_leaves_original_vma_unchanged() {
    let mut runtime = Runtime::new();
    assert!(runtime.add_mapping(1, 0x1000, 0x3000, 3));
    for index in 1..ALTER_PROCESS_MAPPING_MAX {
        assert!(runtime.add_mapping(1, 0x10000 + index * 0x2000, 0x1000, 3));
    }
    assert!(!runtime.can_protect_mapping(1, 0x2000, 0x1000));
    assert!(!runtime.protect_mapping(1, 0x2000, 0x1000, 0));
    assert_eq!(runtime.mapping_prot(1, 0x1000, 0x3000), Some(3));
    assert!(runtime.protect_mapping(1, 0x1000, 0x3000, 1));
    assert_eq!(runtime.mapping_prot(1, 0x1000, 0x3000), Some(1));
}

#[test]
fn compact_prefix_has_no_empty_entries_after_split_remove_merge_and_reset() {
    let mut r = Runtime::new();
    assert!(r.add_mapping(1, 0x1000, 0x5000, 3));
    assert!(r.protect_mapping(1, 0x3000, 0x1000, 1));
    assert_eq!(r.process.mappings.len(), 3);
    assert!(r.process.mappings.iter().all(|mapping| mapping.base != 0));
    assert!(r.remove_mapping(1, 0x3000, 0x1000));
    assert_eq!(r.process.mappings.len(), 2);
    assert!(r.add_mapping(1, 0x3000, 0x1000, 3));
    assert_eq!(r.process.mappings.len(), 1);
    assert_eq!(r.mapping_prot(1, 0x1000, 0x5000), Some(3));
    r.process.mappings.clear();
    assert!(r.process.mappings.is_empty());
    assert!(r.add_mapping(1, 0xa000, 0x1000, 1));
    assert_eq!(r.process.mappings.len(), 1);
    assert!(!r.has_mapping(1, 0x1000, 0x1000));
}

#[test]
fn clone_vm_alias_observes_length_changes_and_cannot_use_stale_permissions() {
    let mut r = Runtime::new();
    assert!(r.add_mapping(1, 0x1000, 0x3000, 3));
    let mut shared = r.process.mappings;
    assert!(shared.same_storage(r.process.mappings));
    // Verify access before changing shared VMA metadata.
    assert!(r.memory_access_allowed(1, 0x2000, 4, 1));
    assert!(r.memory_access_allowed(1, 0x2000, 4, 2));
    assert!(r.protect_mapping(1, 0x2000, 0x1000, 0));
    assert_eq!(shared.len(), 3);
    assert!(!r.memory_access_allowed(1, 0x2000, 4, 1));
    shared.clear();
    assert!(r.process.mappings.is_empty());
    assert!(shared.push(ProcessMapping {
        base: 0x8000,
        size: 0x1000,
        prot: 1
    }));
    assert_eq!(r.mapping_prot(1, 0x8000, 0x1000), Some(1));
    assert!(!r.memory_access_allowed(1, 0x8000, 4, 2));
}

#[test]
fn permission_checks_reject_neighboring_denied_vmas() {
    let mut r = Runtime::new();
    assert!(r.add_mapping(1, 0x1000, 0x1000, 0));
    assert!(r.add_mapping(1, 0x9000, 0x1000, 1));
    assert!(r.add_mapping(1, 0xb000, 0x1000, 0));
    for _ in 0..10 {
        assert!(r.memory_access_allowed(1, 0x2000, 0x7000, 2));
        assert!(!r.memory_access_allowed(1, 0x1fff, 2, 2));
        assert!(!r.memory_access_allowed(1, 0x8fff, 2, 2));
        assert!(r.memory_access_allowed(1, 0x9000, 0x2000, 1));
        assert!(!r.memory_access_allowed(1, 0xafff, 2, 1));
        // Read permission alone cannot authorize a combined read/write request.
        assert!(!r.memory_access_allowed(1, 0x9000, 4, 3));
    }
}

#[test]
fn new_unbacked_prot_none_reservations_revoke_prior_access() {
    let mut r = Runtime::new();
    assert!(r.add_mapping(1, 0x1000, 0x1000, 3));
    assert!(r.memory_access_allowed(1, 0x8000, 8, 1));
    assert!(r.memory_access_allowed(1, 0x8000, 8, 2));
    // No physical Frame is involved: personality policy must still reject it.
    assert!(r.add_mapping(1, 0x8000, 0x1000, 0));
    assert!(!r.memory_access_allowed(1, 0x8000, 8, 1));
    assert!(!r.memory_access_allowed(1, 0x8000, 8, 2));
    assert!(r.protect_mapping(1, 0x8000, 0x1000, 3));
    assert!(r.memory_access_allowed(1, 0x8000, 8, 2));
    assert!(r.protect_mapping(1, 0x8000, 0x1000, 1));
    assert!(!r.memory_access_allowed(1, 0x8000, 8, 2));
}

#[test]
fn shared_clear_push_and_release_cannot_retain_old_permissions() {
    let mut r = Runtime::new();
    assert!(r.add_mapping(1, 0x1000, 0x1000, 3));
    assert!(r.memory_access_allowed(1, 0x1000, 8, 2));
    let mut shared = r.process.mappings;
    shared.clear();
    assert!(shared.push(ProcessMapping {
        base: 0x1000,
        size: 0x1000,
        prot: 0
    }));
    assert!(!r.memory_access_allowed(1, 0x1000, 8, 2));
    assert!(r.remove_mapping(1, 0x1000, 0x1000));
    assert!(r.memory_access_allowed(1, 0x1000, 8, 2));
    r.process.mappings.release();
    assert!(r.add_mapping(1, 0x1000, 0x1000, 0));
    assert!(!r.memory_access_allowed(1, 0x1000, 8, 2));
}

#[test]
fn access_policy_is_vm_local() {
    let mut first = Runtime::new();
    let mut second = Runtime::new();
    assert!(first.add_mapping(1, 0x1000, 0x1000, 3));
    assert!(second.add_mapping(1, 0x1000, 0x1000, 0));
    for _ in 0..10 {
        assert!(first.memory_access_allowed(1, 0x1000, 8, 2));
        assert!(!second.memory_access_allowed(1, 0x1000, 8, 2));
    }
    assert!(!first.memory_access_allowed(999, 0x1000, 8, 2));
    assert!(first.memory_access_allowed(999, usize::MAX, 0, 2));
}

fn uncached_access_allowed(r: &Runtime, base: Word, bytes: Word, permission: Word) -> bool {
    let Some(end) = base.checked_add(bytes) else {
        return false;
    };
    if bytes == 0 {
        return true;
    }
    r.process.mappings.iter().all(|mapping| {
        mapping.base == 0
            || mapping.base >= end
            || mapping.base.saturating_add(mapping.size) <= base
            || mapping.prot & permission == permission
    })
}

#[test]
fn permission_checks_match_the_original_predicate_for_overlapping_unsorted_vmas() {
    let mut r = Runtime::new();
    let mut random = 0x1977u64;
    for step in 0..5000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        if step % 3 == 0 && !r.process.mappings.is_empty() {
            r.process
                .mappings
                .remove((random >> 32) as usize % r.process.mappings.len());
        }
        if step % 71 == 0 {
            r.process.mappings.clear();
        }
        if r.process.mappings.len() < 64 {
            assert!(r.process.mappings.push(ProcessMapping {
                base: 0x1000 + ((random >> 32) as usize % 32) * 4096,
                size: (1 + ((random >> 24) as usize % 8)) * 4096,
                prot: (random >> 16) as usize & 7,
            }));
        }
        for offset in 0..48 {
            let base = offset * 4096 + 4095;
            for permission in [1, 2, 3, 4, 0] {
                assert_eq!(
                    r.memory_access_allowed(1, base, 2, permission),
                    uncached_access_allowed(&r, base, 2, permission)
                );
            }
        }
    }
}

#[test]
fn mixed_vma_operations_match_an_independent_page_permission_model() {
    let mut r = Runtime::new();
    let mut pages = [None; 64];
    let mut random = 17u64;
    for _ in 0..5000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let page = (random >> 32) as usize % pages.len();
        let base = 0x10000 + page * 4096;
        match (random >> 8) % 3 {
            0 if pages[page].is_none() => {
                assert!(r.add_mapping(1, base, 4096, 3));
                pages[page] = Some(3);
            }
            1 if pages[page].is_some() => {
                let prot = [0, 1, 3][(random >> 16) as usize % 3];
                assert!(r.protect_mapping(1, base, 4096, prot));
                pages[page] = Some(prot);
            }
            2 if pages[page].is_some() => {
                assert!(r.remove_mapping(1, base, 4096));
                pages[page] = None;
            }
            _ => {}
        }
        assert!(r.process.mappings.iter().all(|mapping| mapping.base != 0));
        assert!(r.process.mappings.len() <= pages.len());
        for (page, expected) in pages.iter().enumerate() {
            let base = 0x10000 + page * 4096;
            assert_eq!(r.has_mapping(1, base, 4096), expected.is_some());
            assert_eq!(r.mapping_prot(1, base, 4096), *expected);
            for permission in [1, 2] {
                assert_eq!(
                    r.memory_access_allowed(1, base, 4, permission),
                    expected.is_none_or(|prot| prot & permission == permission)
                );
            }
        }
    }
}

#[test]
#[ignore = "host microbenchmark of Alter-only VMA bookkeeping; no IPC"]
fn permission_check_cost_at_normal_and_fragmented_vma_counts() {
    use std::{hint::black_box, time::Instant};
    for count in [8, 32, 128] {
        let mut runtime = Runtime::new();
        for index in 0..count {
            assert!(runtime.add_mapping(1, 0x10000 + index * 0x2000, 0x1000, 3));
        }
        for (use_runtime, label) in [(false, "reference"), (true, "runtime")] {
            for sample in 0..3 {
                let begin = Instant::now();
                for index in 0..500_000 {
                    let address = 0x10000 + (index % count) * 0x2000;
                    let allowed = if use_runtime {
                        black_box(&runtime).memory_access_allowed(
                            black_box(1),
                            black_box(address),
                            4,
                            2,
                        )
                    } else {
                        uncached_access_allowed(black_box(&runtime), black_box(address), 4, 2)
                    };
                    assert!(allowed);
                }
                println!(
                    "ALTER-VMA-PERF mode={} mappings={} sample={} checks=500000 ns={}",
                    label,
                    count,
                    sample,
                    begin.elapsed().as_nanos()
                );
            }
        }
    }
}

#[test]
#[ignore = "host microbenchmark of fork VMA inheritance; no IPC or page copying"]
fn fork_table_copy_versus_repeated_mapping_registration() {
    use std::{hint::black_box, time::Instant};
    for count in [8, 32, 128, ALTER_PROCESS_MAPPING_MAX] {
        let mut r = Runtime::new();
        for index in 0..count {
            assert!(r.add_mapping(1, 0x10000 + index * 8192, 4096, index % 8));
        }
        for use_copy in [false, true] {
            for sample in 0..3 {
                let before = Instant::now();
                for _ in 0..256 {
                    if use_copy {
                        assert!(black_box(&mut r).inherit_mappings(1, 2));
                    } else {
                        let parent = r.process.mappings;
                        for mapping in parent.iter() {
                            assert!(black_box(&mut r).add_mapping(
                                2,
                                mapping.base,
                                mapping.size,
                                mapping.prot
                            ));
                        }
                    }
                    assert_eq!(r.process.mappings.len(), r.child.mappings.len());
                    r.child.mappings.release();
                }
                println!(
                    "FORK-VMA-PERF copy={} mappings={} sample={} forks=256 ns={}",
                    use_copy,
                    count,
                    sample,
                    before.elapsed().as_nanos()
                );
            }
        }
    }
}
