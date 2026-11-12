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
    static FAIL_NEXT_ALLOCATION: Cell<bool> = const { Cell::new(false) };
    static ALLOCATION_CALLS: Cell<usize> = const { Cell::new(0) };
    static ZEROED_BYTES: Cell<usize> = const { Cell::new(0) };
}
struct TestAllocator;
unsafe impl GlobalAlloc for TestAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATION_CALLS.with(|calls| calls.set(calls.get() + 1));
        if FAIL_NEXT_ALLOCATION.with(|fail| fail.replace(false)) {
            return core::ptr::null_mut();
        }
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            unsafe { pointer.write_bytes(0xa5, layout.size()); }
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATION_CALLS.with(|calls| calls.set(calls.get() + 1));
        ZEROED_BYTES.with(|bytes| bytes.set(bytes.get() + layout.size()));
        if FAIL_NEXT_ALLOCATION.with(|fail| fail.replace(false)) {
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

#[test]
fn reservation_history_is_shared_monotonic_and_independent_after_fork() {
    let mut table = MappingTable::EMPTY;
    assert_eq!(table.reservation_cursor(), 0);
    table.advance_reservation_cursor(0x1000);
    assert_eq!(table.reservation_cursor(), 0);
    assert!(table.push(ProcessMapping { base: 0x1000, size: 0x1000, prot: 1 }));
    let generation = table.generation();
    table.advance_reservation_cursor(0x400000);
    assert_eq!(table.generation(), generation);
    let alias = table;
    alias.advance_reservation_cursor(0x600000);
    table.advance_reservation_cursor(0x200000);
    assert_eq!(table.reservation_cursor(), 0x600000);
    let mut fork = MappingTable::EMPTY;
    assert!(fork.initialize_from(table));
    assert_eq!(fork.reservation_cursor(), 0x600000);
    fork.advance_reservation_cursor(0x800000);
    assert_eq!(table.reservation_cursor(), 0x600000);
    table.remove(0);
    table.clear();
    assert_eq!(alias.reservation_cursor(), 0x600000);
    table.release();
    assert_eq!(table.reservation_cursor(), 0);
    assert!(table.ensure());
    assert_eq!(table.reservation_cursor(), 0);
    table.release();
    fork.release();
}

#[test]
fn permission_interval_proofs_skip_scans_and_stop_at_denied_neighbors() {
    let mut table = MappingTable::EMPTY;
    for index in 0..256 {
        assert!(table.push(ProcessMapping { base: 0x1000 + index * 0x1000, size: 0x1000, prot: 1 }));
    }
    assert!(table.push(ProcessMapping { base: 0xa00000, size: 0x1000, prot: 0 }));
    MappingTable::take_access_entry_visits();
    assert!(table.access_allowed(0x800000, 0x800010, 2));
    assert_eq!(MappingTable::take_access_entry_visits(), 257);
    for offset in 0..10_000 {
        let base = 0x700000 + offset;
        assert!(table.access_allowed(base, base + 64, 2));
    }
    assert_eq!(MappingTable::take_access_entry_visits(), 0);
    assert!(!table.access_allowed(0x100fff, 0x101001, 2));
    assert!(!table.access_allowed(0x9fffff, 0xa00001, 2));
    // Independent R/W proofs also serve combined requests. Acquiring a tighter
    // combined proof may reduce cache coverage, never widen permission.
    assert!(table.access_allowed(0x800000, 0x800008, 1));
    MappingTable::take_access_entry_visits();
    assert!(table.access_allowed(0x800000, 0x800008, 3));
    assert_eq!(MappingTable::take_access_entry_visits(), 0);
    assert!(!table.access_allowed(0x1000, 0x1008, 3));
    table.release();
}

#[test]
fn shared_permission_proofs_expire_on_mutation_and_do_not_cross_fork_or_reuse() {
    let mut table = MappingTable::EMPTY;
    assert!(table.push(ProcessMapping { base: 0x1000, size: 0x1000, prot: 3 }));
    assert!(table.push(ProcessMapping { base: 0x8000, size: 0x1000, prot: 0 }));
    let alias = table;
    assert!(alias.access_allowed(0x1000, 0x1008, 2));
    let mut fork = MappingTable::EMPTY;
    assert!(fork.initialize_from(table));
    table.remove(0);
    assert!(table.push(ProcessMapping { base: 0x1000, size: 0x1000, prot: 1 }));
    assert!(!alias.access_allowed(0x1000, 0x1008, 2));
    assert!(fork.access_allowed(0x1000, 0x1008, 2));
    table.clear();
    // Untracked holes retain their ordinary Alpha checked-copy validation.
    assert!(alias.access_allowed(0x8000, 0x8008, 7));
    assert!(table.push(ProcessMapping { base: 0x8000, size: 0x1000, prot: 0 }));
    assert!(!alias.access_allowed(0x8000, 0x8008, 7));
    table.release();
    assert!(table.push(ProcessMapping { base: 0x1000, size: 0x1000, prot: 0 }));
    assert!(!table.access_allowed(0x1000, 0x1008, 2));
    assert!(fork.access_allowed(0x1000, 0x1008, 2));
    table.release();
    fork.release();
}

#[test]
fn permission_index_matches_full_scan_through_shared_mutations_and_fork() {
    let mut table = MappingTable::EMPTY;
    let alias = table;
    assert!(alias.access_allowed(1, usize::MAX, 7));
    for index in 0..ALTER_PROCESS_MAPPING_MAX {
        assert!(table.push(ProcessMapping {
            base: if index % 29 == 0 { usize::MAX - 100 } else { 1 + (index % 91) * 31 },
            size: index % 137,
            prot: index % 16,
        }));
    }
    let alias = table;
    let mut fork = MappingTable::EMPTY;
    assert!(fork.initialize_from(table));
    let original = fork.to_vec();
    let mut seed = 0x13579bdu64;
    for step in 0..1200 {
        for permission in 0..18 {
            for (base, bytes) in [(1, 1), (33, 31), (1, 4096), (usize::MAX - 140, 139)] {
                let end = base + bytes;
                let expected = table.iter().all(|mapping| {
                    mapping.base == 0 || mapping.base >= end
                        || mapping.base.saturating_add(mapping.size) <= base
                        || mapping.prot & permission == permission
                });
                assert_eq!(alias.access_allowed(base, end, permission), expected,
                    "step={step} permission={permission} base={base}");
            }
        }
        assert_eq!(&*fork, original.as_slice());
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        if step % 197 == 0 {
            table.clear();
        } else if !table.is_empty() && (step % 3 != 0 || table.len() == ALTER_PROCESS_MAPPING_MAX) {
            table.remove(seed as usize % table.len());
        } else {
            assert!(table.push(ProcessMapping {
                base: 1 + seed as usize % 4096,
                size: (seed >> 13) as usize % 8192,
                prot: (seed >> 29) as usize % 16,
            }));
        }
    }
    table.release();
    fork.release();
}

#[test]
fn fresh_dirty_table_exposes_only_initialized_entries_and_empty_permissions() {
    let before = ZEROED_BYTES.with(Cell::get);
    let mut table = MappingTable::EMPTY;
    assert!(table.ensure());
    let zeroed = ZEROED_BYTES.with(Cell::get) - before;
    assert!(table.is_empty());
    assert_ne!(table.generation(), 0);
    for permission in 0..18 {
        assert!(table.access_allowed(1, usize::MAX, permission));
    }
    assert!(table.push(ProcessMapping { base: 4096, size: 4096, prot: 7 }));
    let alias = table;
    assert_eq!(&*alias, &[ProcessMapping { base: 4096, size: 4096, prot: 7 }]);
    assert!(alias.access_allowed(4096, 8192, 7));
    table.clear();
    assert!(alias.is_empty());
    assert!(table.push(ProcessMapping { base: 8192, size: 4096, prot: 0 }));
    assert!(!alias.access_allowed(8192, 12288, 1));
    table.remove(0);
    assert!(alias.is_empty());
    assert!(alias.access_allowed(1, usize::MAX, 7));
    table.release();
    println!("fresh-table allocator-zeroed-bytes={zeroed}");
}

fn legacy_add(entries: &mut Vec<ProcessMapping>, base: Word, size: Word, prot: Word) -> bool {
    if base == 0 || size == 0 { return false; }
    let Some(mut end) = base.checked_add(size) else { return false; };
    let mut base = base;
    let mut i = 0;
    while i < entries.len() {
        let mapping = entries[i];
        if mapping.base != 0 && mapping.prot == prot {
            let Some(mapping_end) = mapping.base.checked_add(mapping.size) else { return false; };
            if base <= mapping_end && mapping.base <= end {
                base = base.min(mapping.base);
                end = end.max(mapping_end);
                entries.remove(i);
                continue;
            }
        }
        i += 1;
    }
    if entries.len() == ALTER_PROCESS_MAPPING_MAX { return false; }
    entries.push(ProcessMapping { base, size: end - base, prot });
    true
}

fn legacy_reset(entries: &mut Vec<ProcessMapping>, base: Word, size: Word, prot: Word) -> bool {
    let mut i = 0;
    while i < entries.len() {
        let mapping = entries[i];
        if mapping.base != 0 {
            let Some(mapping_end) = mapping.base.checked_add(mapping.size) else { return false; };
            let Some(end) = base.checked_add(size) else { return false; };
            if base <= mapping_end && mapping.base <= end {
                entries.remove(i);
                continue;
            }
        }
        i += 1;
    }
    legacy_add(entries, base, size, prot)
}

#[test]
fn stack_reset_matches_legacy_order_errors_permissions_and_aliases() {
    let mut seed = 0x87654321u64;
    for case in 0..512 {
        let mut r = Runtime::new();
        let mut expected = Vec::new();
        for index in 0..(case % 73) {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let mapping = ProcessMapping {
                base: if index % 23 == 17 { usize::MAX - 10 } else { 1 + seed as usize % 64 * 4096 },
                size: if index % 23 == 17 { 100 } else { (seed >> 17) as usize % 8 * 4096 },
                prot: (seed >> 29) as usize % 8,
            };
            assert!(r.process.mappings.push(mapping)); expected.push(mapping);
        }
        let alias = r.process.mappings;
        let was_allocated = alias.generation() != 0;
        let base = if case % 31 == 0 { usize::MAX - 10 } else { case % 64 * 4096 };
        let size = if case % 31 == 0 { 100 } else { case % 16 * 4096 };
        let success = legacy_reset(&mut expected, base, size, case % 8);
        assert_eq!(r.reset_stack_mapping(1, base, size, case % 8), success, "case={case}");
        assert_eq!(&*r.process.mappings, expected.as_slice(), "case={case}");
        if was_allocated { assert_eq!(&*alias, expected.as_slice()); }
        for permission in 0..18 {
            let expected_access = expected.iter().all(|m| m.base >= 0x40000
                || m.base.saturating_add(m.size) <= 1 || m.prot & permission == permission);
            assert_eq!(r.process.mappings.access_allowed(1, 0x40000, permission), expected_access);
        }
        r.process.mappings.release();
    }
    // Earlier removals must survive a later malformed mapping; the suffix
    // must remain in place even when it also touches the replacement range.
    let mut r = Runtime::new();
    let entries = [ProcessMapping { base: 4096, size: 4096, prot: 1 },
        ProcessMapping { base: usize::MAX - 10, size: 100, prot: 0 },
        ProcessMapping { base: 8192, size: 4096, prot: 7 }];
    for mapping in entries { assert!(r.process.mappings.push(mapping)); }
    assert!(!r.reset_stack_mapping(1, 4096, 16384, 3));
    assert_eq!(&*r.process.mappings, &entries[1..]);
    r.process.mappings.release();
}

#[test]
fn full_stack_reset_avoids_repeated_tail_moves() {
    let mut r = Runtime::new();
    for index in 0..ALTER_PROCESS_MAPPING_MAX {
        assert!(r.process.mappings.push(ProcessMapping { base: 4096 * (index + 1), size: 4096, prot: index % 8 }));
    }
    MappingTable::take_entry_moves();
    assert!(r.reset_stack_mapping(1, 4096, 4096 * ALTER_PROCESS_MAPPING_MAX, 3));
    let moves = MappingTable::take_entry_moves();
    assert_eq!(&*r.process.mappings, &[ProcessMapping { base: 4096, size: 4096 * ALTER_PROCESS_MAPPING_MAX, prot: 3 }]);
    r.process.mappings.release();
    println!("full-stack-reset entry-moves={moves}");
}

#[test]
fn coalesce_matches_legacy_order_partial_failure_permissions_and_shared_alias() {
    let mut seed = 0x12345678u64;
    for case in 0..512 {
        let mut r = Runtime::new();
        let mut expected = Vec::new();
        for index in 0..(case % 73) {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let entry = ProcessMapping {
                base: if index % 19 == 0 { usize::MAX - 10 } else { 1 + (seed as usize % 64) * 4096 },
                size: if index % 19 == 0 { 100 } else { ((seed >> 17) as usize % 8) * 4096 },
                prot: (seed >> 29) as usize % 8,
            };
            assert!(r.process.mappings.push(entry)); expected.push(entry);
        }
        let alias = r.process.mappings;
        let generation = alias.generation();
        let base = if case % 31 == 0 { usize::MAX - 10 } else { 1 + case % 64 * 4096 };
        let size = if case % 31 == 0 { 100 } else { (1 + case % 16) * 4096 };
        let prot = case % 8;
        let success = legacy_add(&mut expected, base, size, prot);
        assert_eq!(r.add_mapping(1, base, size, prot), success, "case={case}");
        assert_eq!(&*r.process.mappings, expected.as_slice(), "case={case}");
        if generation != 0 {
            assert_eq!(&*alias, expected.as_slice());
        }
        for permission in 0..18 {
            let expected_access = expected.iter().all(|m| m.base >= 0x40000
                || m.base.saturating_add(m.size) <= 1 || m.prot & permission == permission);
            assert_eq!(r.process.mappings.access_allowed(1, 0x40000, permission), expected_access);
        }
        r.process.mappings.release();
    }
}

#[test]
fn coalescing_a_full_table_preserves_extent_without_repeated_tail_moves() {
    let mut r = Runtime::new();
    for index in 0..ALTER_PROCESS_MAPPING_MAX {
        assert!(r.process.mappings.push(ProcessMapping { base: 4096 * (index + 1), size: 4096, prot: 3 }));
    }
    MappingTable::take_entry_moves();
    assert!(r.add_mapping(1, 4096, 4096 * ALTER_PROCESS_MAPPING_MAX, 3));
    let moves = MappingTable::take_entry_moves();
    assert_eq!(&*r.process.mappings, &[ProcessMapping { base: 4096, size: 4096 * ALTER_PROCESS_MAPPING_MAX, prot: 3 }]);
    r.process.mappings.release();
    println!("full-table coalesce entry-moves={moves}");
}

#[test]
fn permission_index_removes_every_position_across_word_boundaries() {
    for removed in 0..ALTER_PROCESS_MAPPING_MAX {
        let mut table = MappingTable::EMPTY;
        for index in 0..ALTER_PROCESS_MAPPING_MAX {
            assert!(table.push(ProcessMapping { base: 4096 * (index + 1), size: 4096, prot: index % 8 }));
        }
        table.remove(removed);
        assert!(table.push(ProcessMapping { base: 4096 * 1024, size: 4096, prot: 0 }));
        for mapping in table.iter() {
            for permission in 0..8 {
                assert_eq!(table.access_allowed(mapping.base, mapping.base + mapping.size, permission),
                    mapping.prot & permission == permission, "removed={removed}");
            }
        }
        table.release();
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/page_window.rs"]
mod read_window;

#[test]
fn mapping_generations_follow_shared_vm_mutations_but_not_fork_snapshots() {
    let mut source = MappingTable::EMPTY;
    assert_eq!(source.generation(), 0);
    assert!(source.push(ProcessMapping {
        base: 4096,
        size: 4096,
        prot: 3
    }));
    let first = source.generation();
    assert_ne!(first, 0);
    let alias = source;
    let mut fork = MappingTable::EMPTY;
    assert!(fork.initialize_from(source));
    assert_ne!(fork.generation(), first);
    source.invalidate_views();
    assert_ne!(source.generation(), first);
    assert_eq!(alias.generation(), source.generation());
    let second = source.generation();
    source.clear();
    assert_ne!(source.generation(), second);
    let old = source.generation();
    source.release();
    assert!(source.ensure());
    assert_ne!(source.generation(), old);
    source.release();
    fork.release();
}
struct ManagedProcess {
    mappings: MappingTable,
    vm_group: Word,
}
struct Runtime {
    process: ManagedProcess,
    child: ManagedProcess,
    read_window: read_window::ReadWindow,
}
impl Runtime {
    fn new() -> Self {
        Self {
            process: ManagedProcess {
                mappings: MappingTable::EMPTY,
                vm_group: 1,
            },
            child: ManagedProcess {
                mappings: MappingTable::EMPTY,
                vm_group: 2,
            },
            read_window: read_window::ReadWindow::new(),
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

#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/stack_fault.rs"]
mod stack_fault;
#[path = "../../nanami/src/nanami_core/alpha/stack_population.rs"]
mod stack_population;
thread_local! {
    static STACK_PROTECT_CALLS: std::cell::RefCell<Vec<(Word, Word, Word, Word)>> = const {
        std::cell::RefCell::new(Vec::new())
    };
    static STACK_PROTECT_FAIL: Cell<bool> = const { Cell::new(false) };
    static STACK_QUIESCE_FAIL: Cell<bool> = const { Cell::new(false) };
    static STACK_QUIESCED: Cell<bool> = const { Cell::new(false) };
}
const EIO: i32 = 5;
mod private_memory {
    use super::*;
    pub fn with_vm_quiescence<T>(_: &Runtime, _: Word, operation: impl FnOnce() -> Result<T, i32>)
        -> Result<T, i32>
    {
        if STACK_QUIESCE_FAIL.with(Cell::get) { return Err(EIO); }
        STACK_QUIESCED.with(|state| state.set(true));
        let result = operation();
        STACK_QUIESCED.with(|state| state.set(false));
        result
    }
}
pub fn request_process_mapping_protect(pid: Word, base: Word, bytes: Word, prot: Word)
    -> Result<(), ()>
{
    assert!(STACK_QUIESCED.with(Cell::get));
    STACK_PROTECT_CALLS.with(|calls| calls.borrow_mut().push((pid, base, bytes, prot)));
    if STACK_PROTECT_FAIL.with(Cell::get) { Err(()) } else { Ok(()) }
}

#[test]
fn demand_stack_faults_enforce_range_access_and_fault_kind_before_alpha() {
    let mut r = Runtime::new();
    for prot in 0..16 {
        r.process.mappings.clear();
        assert!(r.add_mapping(1, 0x4000000, 0x40000, prot));
        for address in [0x3ffffff, 0x4000000, 0x4001fff, 0x403ffff, 0x4040000] {
            for code in 0..128 {
                STACK_PROTECT_CALLS.with(|calls| calls.borrow_mut().clear());
                let generation = r.process.mappings.generation();
                let required = if code & 16 != 0 { 4 } else if code & 2 != 0 { 2 } else { 1 };
                let expected = cfg!(target_arch = "x86_64")
                    && (0x4000000..0x4040000).contains(&address)
                    && code & !0x16 == 0 && prot <= 7 && prot & required != 0;
                assert_eq!(stack_fault::materialize_stack_fault(&r, 1, address, code), expected);
                STACK_PROTECT_CALLS.with(|calls| {
                    let calls = calls.borrow();
                    assert_eq!(calls.len(), usize::from(expected));
                    if expected { assert_eq!(calls[0], (1, address & !4095, 4096, prot)); }
                });
                assert_eq!(r.process.mappings.generation() != generation, expected);
            }
        }
    }
}

#[test]
fn demand_stack_keeps_holes_denied_overlaps_unknown_owners_and_failed_requests_safe() {
    let mut r = Runtime::new();
    assert!(r.add_mapping(1, 0x4000000, 0x40000, 3));
    for pid in [2, 999] {
        assert!(!stack_fault::materialize_stack_fault(&r, pid, 0x4000000, 6));
    }
    assert!(r.remove_mapping(1, 0x4001000, 4096));
    assert!(!stack_fault::materialize_stack_fault(&r, 1, 0x4001001, 6));
    assert!(r.process.mappings.push(ProcessMapping { base: 0x4000000, size: 4096, prot: 0 }));
    assert!(!stack_fault::materialize_stack_fault(&r, 1, 0x4000001, 6));
    let before = r.process.mappings.to_vec();
    STACK_PROTECT_FAIL.with(|fail| fail.set(true));
    assert!(!stack_fault::materialize_stack_fault(&r, 1, 0x4002001, 6));
    assert_eq!(&*r.process.mappings, before.as_slice());
    STACK_PROTECT_FAIL.with(|fail| fail.set(false));
    assert!(stack_fault::materialize_stack_fault(&r, 1, 0x4002001, 6));
}

#[test]
fn raw_stack_population_keeps_reservation_and_initial_tail_with_legacy_optout() {
    for reserved in [0, 1, 3, 4, 31, 32, 33, 64, 512] {
        assert_eq!(stack_population::first_page(false, reserved), 0);
        let first = stack_population::first_page(true, reserved);
        let enabled = cfg!(target_arch = "x86_64") && option_env!("NANAMI_LAZY_RAW_STACK") != Some("0");
        assert_eq!(reserved - first, if enabled { reserved.min(32) } else { reserved });
        assert_eq!(stack_population::initial_permissions(true), if enabled { 3 } else { 7 });
        assert_eq!(stack_population::initial_permissions(false), 7);
    }
}

#[test]
fn exec_stack_tail_requires_sealed_raw_guest_and_keeps_full_reservation_policy() {
    for reserved in [0, 1, 3, 4, 31, 32, 33, 64, 512] {
        for sealed in [false, true] {
            for ipc in [false, true] {
                let first = stack_population::exec_first_page(sealed, ipc, reserved);
                let demand = cfg!(target_arch = "x86_64") && sealed && !ipc
                    && option_env!("NANAMI_LAZY_RAW_STACK") != Some("0");
                assert_eq!(reserved - first, if demand { reserved.min(32) } else { reserved });
            }
        }
    }
}

#[test]
fn demand_stack_cannot_publish_or_invalidate_views_when_peer_stop_fails() {
    let mut r = Runtime::new();
    assert!(r.add_mapping(1, 0x4000000, 0x40000, 3));
    STACK_PROTECT_CALLS.with(|calls| calls.borrow_mut().clear());
    let generation = r.process.mappings.generation();
    STACK_QUIESCE_FAIL.with(|fail| fail.set(true));
    assert!(!stack_fault::materialize_stack_fault(&r, 1, 0x4000000, 6));
    assert_eq!(r.process.mappings.generation(), generation);
    STACK_PROTECT_CALLS.with(|calls| assert!(calls.borrow().is_empty()));
    STACK_QUIESCE_FAIL.with(|fail| fail.set(false));
    assert!(stack_fault::materialize_stack_fault(&r, 1, 0x4000000, 6));
    assert!(!STACK_QUIESCED.with(Cell::get));
}

#[derive(Default)]
struct LeaseEffects {
    grants: usize,
}
impl read_window::LeaseEffects for LeaseEffects {
    fn grant(&mut self, _: Word, _: Word, _: Word) -> Result<(), read_window::GrantError> {
        self.grants += 1;
        Ok(())
    }
    fn release(&mut self, _: Word) -> Result<(), ()> {
        Ok(())
    }
}
impl read_window::Effects for LeaseEffects {
    fn read(&mut self, _: Word, _: Word, destination: &mut [core::mem::MaybeUninit<u8>]) {
        for byte in destination {
            byte.write(7);
        }
    }
}
fn cached_read(r: &Runtime, pid: Word, address: Word, effects: &mut LeaseEffects) -> bool {
    if !r.memory_access_allowed(pid, address, 1, 1) {
        return false;
    }
    let process = r.managed_process(pid).unwrap();
    r.read_window.copy_into(
        process.vm_group,
        process.mappings.generation(),
        pid,
        address,
        &mut [core::mem::MaybeUninit::uninit(); 1],
        effects,
    )
}

#[test]
fn metadata_edits_revalidate_cached_input_after_protection_and_replacement() {
    let mut r = Runtime::new();
    let mut effects = LeaseEffects::default();
    assert!(r.add_mapping(1, 0x10000, 8192, 3));
    assert!(cached_read(&r, 1, 0x10000, &mut effects));
    let generation = r.process.mappings.generation();
    for page in 0..128 {
        let base = 0x100000 + page * 8192;
        assert!(r.add_mapping(1, base, 4096, 3));
        assert!(r.protect_mapping(1, base, 4096, 1));
        assert!(r.remove_mapping(1, base, 4096));
        assert!(cached_read(&r, 1, 0x10000, &mut effects));
    }
    assert_ne!(r.process.mappings.generation(), generation);
    let previous = effects.grants;
    assert!(r.protect_mapping(1, 0x10000, 4096, 0));
    assert!(!cached_read(&r, 1, 0x10000, &mut effects));
    assert!(r.protect_mapping(1, 0x10000, 4096, 3));
    assert!(cached_read(&r, 1, 0x10000, &mut effects));
    assert_eq!(effects.grants, previous + 1);
    assert!(r.remove_mapping(1, 0x10000, 4096));
    assert!(r.add_mapping(1, 0x10000, 4096, 3));
    assert!(cached_read(&r, 1, 0x10000, &mut effects));
    assert_eq!(effects.grants, previous + 2);
}

#[test]
fn vm_peer_and_pre_pte_failure_invalidation_do_not_depend_on_metadata_changes() {
    let mut r = Runtime::new();
    let mut effects = LeaseEffects::default();
    assert!(r.add_mapping(1, 0x10000, 4096, 3));
    assert!(cached_read(&r, 1, 0x10000, &mut effects));
    r.child.mappings = r.process.mappings;
    r.child.vm_group = r.process.vm_group;
    assert!(cached_read(&r, 2, 0x10000, &mut effects));
    assert_eq!(effects.grants, 1);
    let generation = r.process.mappings.generation();
    // Called before the fallible hardware operation. Even if it fails and
    // leaves logical metadata unchanged, no old alias may satisfy the retry.
    r.child.mappings.invalidate_views();
    assert_ne!(r.process.mappings.generation(), generation);
    assert_eq!(r.mapping_prot(1, 0x10000, 4096), Some(3));
    assert!(cached_read(&r, 1, 0x10000, &mut effects));
    assert_eq!(effects.grants, 2);
    assert!(r.protect_mapping(2, 0x10000, 4096, 1));
    assert!(cached_read(&r, 1, 0x10000, &mut effects));
    assert_eq!(effects.grants, 3);
}

#[test]
fn resetting_a_touching_stack_invalidates_its_whole_removed_entry() {
    let mut r = Runtime::new();
    let mut effects = LeaseEffects::default();
    assert!(r.add_mapping(1, 0x10000, 0x3000, 3));
    assert!(cached_read(&r, 1, 0x10000, &mut effects));
    assert!(r.reset_stack_mapping(1, 0x13000, 4096, 3));
    assert_eq!(r.mapping_at(1, 0x10000), None);
    assert!(cached_read(&r, 1, 0x10000, &mut effects));
    assert_eq!(
        effects.grants, 2,
        "removed metadata requires fresh Alpha validation"
    );
}

#[test]
fn ordinary_fork_copies_only_live_vmas_into_independent_storage() {
    let mut r = Runtime::new();
    for index in 0..ALTER_PROCESS_MAPPING_MAX {
        assert!(r.add_mapping(1, 0x10000 + index * 8192, 4096, index % 8));
    }
    let before = ALLOCATION_CALLS.with(Cell::get);
    assert!(r.inherit_mappings(1, 2));
    assert_eq!(ALLOCATION_CALLS.with(Cell::get) - before, 1);
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
    let before = ALLOCATION_CALLS.with(Cell::get);
    assert!(r.inherit_mappings(1, 2));
    assert_eq!(ALLOCATION_CALLS.with(Cell::get), before);
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
    FAIL_NEXT_ALLOCATION.with(|fail| fail.set(true));
    let inherited = r.inherit_mappings(1, 2);
    FAIL_NEXT_ALLOCATION.with(|fail| fail.set(false));
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
fn removal_keeps_first_covering_entry_policy_for_unsorted_overlaps() {
    for protect in [false, true] {
        let mut r = Runtime::new();
        for (base, size, prot) in [
            (0x9000, 0x1000, 3),
            (0x3000, 0x2000, 1),
            (0x1000, 0x3000, 3),
            (0x4000, 0x3000, 4),
        ] {
            assert!(r.process.mappings.push(ProcessMapping { base, size, prot }));
        }
        // Removal follows the first covering entry at each cursor: first the
        // 0x1000 entry, then 0x3000, then 0x4000. The overlapping part of the
        // last entry before that cursor remains, as in the original algorithm.
        if protect {
            assert!(r.protect_mapping(1, 0x2000, 0x4000, 0));
        } else {
            assert!(r.remove_mapping(1, 0x2000, 0x4000));
        }
        let mut expected = vec![
            ProcessMapping { base: 0x9000, size: 0x1000, prot: 3 },
            ProcessMapping { base: 0x1000, size: 0x1000, prot: 3 },
            ProcessMapping { base: 0x3000, size: 0x1000, prot: 1 },
            ProcessMapping { base: 0x4000, size: 0x1000, prot: 4 },
            ProcessMapping { base: 0x6000, size: 0x1000, prot: 4 },
        ];
        if protect {
            expected.push(ProcessMapping { base: 0x2000, size: 0x4000, prot: 0 });
        }
        assert_eq!(&*r.process.mappings, expected.as_slice());
    }
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

fn scalar_remove_entries(entries: &mut Vec<ProcessMapping>, base: Word, end: Word) -> bool {
    let mut cursor = base;
    while cursor < end {
        let mut selected = None;
        for (index, mapping) in entries.iter().enumerate() {
            if mapping.base == 0 { continue; }
            let Some(stop) = mapping.base.checked_add(mapping.size) else { return false; };
            if mapping.base <= cursor && cursor < stop { selected = Some((index,*mapping,stop)); break; }
        }
        let Some((index,mapping,stop)) = selected else { return false; };
        let next = stop.min(end);
        let left = cursor-mapping.base;
        let right = stop-next;
        if left != 0 && right != 0 && entries.len() == ALTER_PROCESS_MAPPING_MAX { return false; }
        entries.remove(index);
        if left != 0 { entries.push(ProcessMapping {base:mapping.base,size:left,prot:mapping.prot}); }
        if right != 0 { entries.push(ProcessMapping {base:next,size:right,prot:mapping.prot}); }
        cursor=next;
    }
    true
}

#[test]
fn whole_vma_removal_compacts_once_and_keeps_shared_length_without_allocating() {
    for count in [3,8,64,128,ALTER_PROCESS_MAPPING_MAX] {
        let mut r=Runtime::new();
        for index in 0..count {
            assert!(r.process.mappings.push(ProcessMapping {base:4096*(index+1),size:4096,prot:index%8}));
        }
        let alias=r.process.mappings;
        let generation=alias.generation();
        MappingTable::take_entry_moves();
        let allocations=ALLOCATION_CALLS.with(Cell::get);
        assert!(r.remove_mapping(1,4096,count*4096));
        assert_eq!(ALLOCATION_CALLS.with(Cell::get),allocations);
        assert_eq!(MappingTable::take_entry_moves(),count-1);
        assert!(alias.is_empty());
        assert_ne!(alias.generation(),generation);
        assert!(!r.has_mapping(1,4096,4));
    }
}

#[test]
fn whole_vma_removal_keeps_unselected_overlaps_and_surviving_permission_bits() {
    let mut r=Runtime::new();
    let entries=[
        ProcessMapping {base:0x1000,size:4096,prot:3},
        ProcessMapping {base:0x1000,size:2048,prot:0},
        ProcessMapping {base:0x2000,size:4096,prot:1},
        ProcessMapping {base:0x9000,size:4096,prot:4},
        ProcessMapping {base:0x3000,size:4096,prot:5},
    ];
    for mapping in entries { assert!(r.process.mappings.push(mapping)); }
    let mut expected=entries.to_vec();
    assert!(scalar_remove_entries(&mut expected,0x1000,0x4000));
    assert!(r.remove_mapping(1,0x1000,0x3000));
    assert_eq!(&*r.process.mappings,expected.as_slice());
    assert!(!r.memory_access_allowed(1,0x1000,4,1));
    assert!(!r.memory_access_allowed(1,0x9000,4,2));
    assert!(r.memory_access_allowed(1,0x9000,4,4));
}

#[test]
fn whole_and_partial_vma_removal_match_scalar_order_for_shuffled_overlapping_tables() {
    let mut random=0x11335577u64;
    for case in 0..512 {
        let mut r=Runtime::new();
        let mut entries:Vec<_>=(0..16).map(|index| ProcessMapping {base:0x1000+index*4096,size:4096,prot:index%8}).collect();
        for index in (1..entries.len()).rev() {
            random=random.wrapping_mul(6364136223846793005).wrapping_add(1);
            entries.swap(index,(random>>32) as usize%(index+1));
        }
        if case%2==0 {
            for offset in 0..4 { entries.insert(offset*3,ProcessMapping {base:0x2000+offset*4096,size:8192,prot:offset}); }
        }
        for &mapping in &entries { assert!(r.process.mappings.push(mapping)); }
        let base=0x1000+(case%6)*4096+if case%3==0 {123} else {0};
        let end=0x1000+(10+case%7)*4096-if case%5==0 {77} else {0};
        let mut expected=entries;
        assert!(scalar_remove_entries(&mut expected,base,end));
        assert!(r.remove_mapping(1,base,end-base));
        assert_eq!(&*r.process.mappings,expected.as_slice());
        for page in 1..=16 { for permission in [1,2,4] {
            assert_eq!(r.memory_access_allowed(1,page*4096,4,permission),uncached_access_allowed(&r,page*4096,4,permission));
        }}
    }
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

fn reference_coverage(r: &Runtime, base: Word, size: Word, uniform: bool) -> (Option<Word>, usize) {
    let mut visits = 0;
    let result = (|| {
        if base == 0 || size == 0 { return None; }
        let end = base.checked_add(size)?;
        let mut cursor = base;
        let mut prot = None;
        while cursor < end {
            let mut found = false;
            for mapping in r.process.mappings.iter() {
                visits += 1;
                if mapping.base == 0 { continue; }
                let stop = mapping.base.checked_add(mapping.size)?;
                if mapping.base <= cursor && cursor < stop {
                    if uniform && prot.is_some_and(|old| old != mapping.prot) { return None; }
                    prot = Some(mapping.prot);
                    cursor = stop.min(end);
                    found = true;
                    break;
                }
            }
            if !found { return None; }
        }
        Some(if uniform { prot? } else { 0 })
    })();
    (result, visits)
}

#[test]
fn protection_preflight_walks_coverage_once_without_allocation_or_mutation() {
    for count in [1, 8, 64, 128, ALTER_PROCESS_MAPPING_MAX] {
        let mut r = Runtime::new();
        for index in 0..count { assert!(r.process.mappings.push(ProcessMapping { base: 0x1000 + index * 4096, size: 4096, prot: 3 })); }
        for prot in [1, 3] {
            mappings::take_coverage_visits();
            assert!(r.has_mapping(1, 0x1000, count * 4096));
            let old = if r.mapping_prot(1, 0x1000, count * 4096) == Some(prot) { Some(false) }
                else { r.can_protect_mapping(1, 0x1000, count * 4096).then_some(true) };
            let old_visits = mappings::take_coverage_visits();
            let generation = r.process.mappings.generation(); let allocations = ALLOCATION_CALLS.with(Cell::get);
            assert_eq!(r.mapping_protection_needed(1, 0x1000, count * 4096, prot), old);
            assert_eq!(ALLOCATION_CALLS.with(Cell::get), allocations);
            assert_eq!(r.process.mappings.generation(), generation);
            assert_eq!(mappings::take_coverage_visits(), count);
            assert_eq!(old_visits, count * if prot == 3 { 2 } else { 3 });
            println!("PROTECT-PREFLIGHT pages={count} prot={prot} visits={old_visits}->{count}");
        }
    }
}

#[test]
fn protection_preflight_matches_separate_queries_through_unsorted_overlaps_and_overflow() {
    let mut r = Runtime::new(); let mut random = 0x5fd92817u64;
    for step in 0..1000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        if step % 3 == 0 && !r.process.mappings.is_empty() { r.process.mappings.remove((random >> 32) as usize % r.process.mappings.len()); }
        if step % 71 == 0 { r.process.mappings.clear(); }
        if r.process.mappings.len() < 64 {
            let overflow = step % 37 == 0;
            assert!(r.process.mappings.push(ProcessMapping {
                base: if overflow { Word::MAX - 2 } else { 0x1000 + ((random >> 32) as usize % 32) * 4096 },
                size: if overflow { 4 } else { ((random >> 24) as usize % 8) * 4096 },
                prot: (random >> 16) as usize & 7,
            }));
        }
        for offset in 0..40 {
            let base = offset * 4096 + step % 3; let size = step % 9 * 4096 + step % 2;
            for prot in [0, 1, 3, 7] {
                let old = if !r.has_mapping(1, base, size) { None }
                    else if r.mapping_prot(1, base, size) == Some(prot) { Some(false) }
                    else { r.can_protect_mapping(1, base, size).then_some(true) };
                assert_eq!(r.mapping_protection_needed(1, base, size, prot), old);
            }
        }
    }
}

#[test]
fn protected_publication_matches_legacy_remove_add_and_keeps_noop_capacity_rules() {
    for sample in 0..160 {
        let mut actual = Runtime::new(); let mut expected = Runtime::new();
        for i in 0..12 {
            let mapping = ProcessMapping { base: 0x1000 + ((i * 7 + sample) % 13) * 4096, size: (i % 4 + 1) * 4096, prot: (i + sample) & 7 };
            assert!(actual.process.mappings.push(mapping)); assert!(expected.process.mappings.push(mapping));
        }
        let base = 0x1000 + sample % 14 * 4096; let size = (sample % 7 + 1) * 4096; let prot = sample & 7;
        let old = if expected.mapping_prot(1, base, size) == Some(prot) { true }
            else if !expected.can_protect_mapping(1, base, size) { false }
            else { expected.remove_mapping(1, base, size) && expected.add_mapping(1, base, size, prot) };
        assert_eq!(actual.protect_mapping(1, base, size, prot), old);
        assert_eq!(&*actual.process.mappings, &*expected.process.mappings);
    }
    let mut r = Runtime::new();
    assert!(r.process.mappings.push(ProcessMapping { base: 4096, size: 4096, prot: 3 }));
    assert!(r.process.mappings.push(ProcessMapping { base: Word::MAX - 2, size: 4, prot: 1 }));
    assert_eq!(r.mapping_protection_needed(1, 4096, 4096, 3), Some(false));
    assert_eq!(r.mapping_protection_needed(1, 4096, 4096, 1), None);
}

#[test]
fn coverage_cursor_eliminates_passed_vmas_without_allocating_or_changing_table() {
    for count in [1, 8, 64, 128, ALTER_PROCESS_MAPPING_MAX] {
        let mut r = Runtime::new();
        for index in 0..count {
            assert!(r.process.mappings.push(ProcessMapping { base: 0x1000 + index*4096, size: 4096, prot: 3 }));
        }
        let generation = r.process.mappings.generation();
        for uniform in [false, true] {
            let (expected, old_visits) = reference_coverage(&r, 0x1000, count*4096, uniform);
            assert_eq!(old_visits, count*(count+1)/2);
            mappings::take_coverage_visits();
            let allocations = ALLOCATION_CALLS.with(Cell::get);
            let result = if uniform { r.mapping_prot(1,0x1000,count*4096) } else { r.has_mapping(1,0x1000,count*4096).then_some(0) };
            assert_eq!(ALLOCATION_CALLS.with(Cell::get), allocations);
            assert_eq!(result, expected);
            assert_eq!(mappings::take_coverage_visits(), count);
            assert_eq!(r.process.mappings.generation(), generation);
            assert_eq!(r.process.mappings.len(), count);
        }
    }
}

#[test]
fn coverage_cursor_matches_old_walk_for_unsorted_overlaps_holes_and_mutations() {
    let mut r = Runtime::new();
    let mut random = 0x19111977u64;
    for step in 0..2000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        if step % 3 == 0 && !r.process.mappings.is_empty() { r.process.mappings.remove((random >> 32) as usize % r.process.mappings.len()); }
        if step % 71 == 0 { r.process.mappings.clear(); }
        if r.process.mappings.len() < 64 {
            assert!(r.process.mappings.push(ProcessMapping {
                base: 0x1000 + ((random >> 32) as usize % 32)*4096,
                size: ((random >> 24) as usize % 8)*4096,
                prot: (random >> 16) as usize & 7,
            }));
        }
        for offset in 0..40 {
            let base = offset*4096 + (step % 3);
            let size = (step % 9)*4096 + (step % 2);
            assert_eq!(r.has_mapping(1,base,size), reference_coverage(&r,base,size,false).0.is_some());
            assert_eq!(r.mapping_prot(1,base,size), reference_coverage(&r,base,size,true).0);
        }
    }
}

#[test]
fn coverage_cursor_keeps_legacy_overflow_and_first_covering_entry_order() {
    for entries in [
        [(usize::MAX-1,4,1),(0x1000,8192,3),(0x2000,8192,1)],
        [(0x1000,8192,3),(usize::MAX-1,4,1),(0x2000,8192,1)],
        [(0x2000,8192,1),(0x1000,8192,3),(usize::MAX-1,4,1)],
    ] {
        let mut r = Runtime::new();
        for (base,size,prot) in entries { assert!(r.process.mappings.push(ProcessMapping {base,size,prot})); }
        for (base,size) in [(0,1),(0x1000,0),(0x1000,4096),(0x1000,12288),(0x2000,8192),(usize::MAX,2)] {
            assert_eq!(r.has_mapping(1,base,size),reference_coverage(&r,base,size,false).0.is_some());
            assert_eq!(r.mapping_prot(1,base,size),reference_coverage(&r,base,size,true).0);
        }
    }
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
