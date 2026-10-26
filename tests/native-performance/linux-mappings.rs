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
    assert_eq!(runtime.process.mappings.len(), ALTER_PROCESS_MAPPING_MAX);
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
