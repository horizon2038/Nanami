#![allow(dead_code)]
extern crate alloc;
extern crate self as nanami_runtime;
pub type Word = usize;
#[path = "../../nanami/servers/apps/alter/shared/src/common/abi.rs"]
mod abi;
use abi::ALTER_PROCESS_MAPPING_MAX;

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
}
impl Runtime {
    fn new() -> Self {
        Self {
            process: ManagedProcess {
                mappings: MappingTable::EMPTY,
            },
        }
    }
    fn managed_process(&self, pid: Word) -> Option<&ManagedProcess> {
        if pid == 1 {
            Some(&self.process)
        } else {
            None
        }
    }
    fn managed_process_mut(&mut self, pid: Word) -> Option<&mut ManagedProcess> {
        if pid == 1 {
            Some(&mut self.process)
        } else {
            None
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.process.mappings.release();
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/mappings.rs"]
mod mappings;

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
    assert!(!runtime.add_mapping(2, 0x1000, 4096, 3));
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
        for sample in 0..3 {
            let begin = Instant::now();
            for index in 0..500_000 {
                let address = 0x10000 + (index % count) * 0x2000;
                assert!(black_box(&runtime).memory_access_allowed(
                    black_box(1),
                    black_box(address),
                    4,
                    2
                ));
            }
            println!(
                "ALTER-VMA-PERF mappings={} sample={} checks=500000 ns={}",
                count,
                sample,
                begin.elapsed().as_nanos()
            );
        }
    }
}
