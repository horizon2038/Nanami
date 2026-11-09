//! Production remap guard and real generation table; model hardware effects.
#![allow(dead_code)]
extern crate alloc;
type Word = usize;
const LINUX_MAP_FIXED: Word = 16;
const LINUX_PAGE_SIZE: Word = 4096;
const ALTER_PROCESS_MAPPING_MAX: usize = 128;
#[derive(Clone, Copy)]
pub struct ProcessMapping { base: Word, size: Word, prot: Word }
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/mapping_table.rs"]
mod mapping_table;
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/readonly_mapping.rs"]
mod readonly_mapping;
mod state { pub use super::readonly_mapping::ReadonlyMapping; }
struct Process { mappings: mapping_table::MappingTable }
struct Runtime {
    process: Process,
    readonly_mapping: std::cell::Cell<state::ReadonlyMapping>,
    protections: Vec<(Word, Word, Word)>,
    releases: Vec<(Word, Word)>,
    bytes: Vec<u8>,
    can_protect: bool,
    fail_protect: bool,
}
impl Runtime {
    fn managed_process(&self, pid: Word) -> Option<&Process> { (pid == 7).then_some(&self.process) }
    fn can_protect_mapping(&self, _: Word, _: Word, _: Word) -> bool { self.can_protect }
}
fn sys_mprotect(runtime: &mut Runtime, pid: Word, base: Word, bytes: Word, prot: Word) -> Result<Word, i32> {
    let origin = production::before_protect(runtime, pid, prot);
    runtime.protections.push((base, bytes, prot));
    runtime.process.mappings.invalidate_views();
    if runtime.fail_protect { Err(5) } else {
        production::after_protect(runtime, pid, origin);
        Ok(0)
    }
}
fn sys_munmap(runtime: &mut Runtime, _: Word, base: Word, bytes: Word) -> Result<Word, i32> {
    runtime.releases.push((base, bytes));
    runtime.process.mappings.invalidate_views();
    Ok(0)
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/file_remap.rs"]
mod production;
const BASE: usize = 0x100000;
const ID: usize = (17 << 32) | 9;
const SIZE: usize = 16 * 4096 - 37;
fn fixture() -> Runtime {
    let mut mappings = mapping_table::MappingTable::EMPTY;
    assert!(mappings.push(ProcessMapping { base: BASE, size: 16 * 4096, prot: 1 }));
    let runtime = Runtime {
        process: Process { mappings },
        readonly_mapping: std::cell::Cell::new(state::ReadonlyMapping::EMPTY),
        protections: Vec::new(), releases: Vec::new(),
        bytes: (0..16 * 4096).map(|i| if i < SIZE { (i % 239) as u8 } else { 0 }).collect(),
        can_protect: true, fail_protect: false,
    };
    production::remember(&runtime, 7, ID, SIZE, BASE, 16 * 4096, 0);
    runtime
}
impl Drop for Runtime { fn drop(&mut self) { self.process.mappings.release(); } }

#[test]
fn snapshot_write_gate_requires_current_owned_extent_and_valid_write_permissions() {
    let mut r = fixture();
    for prot in 0..16 {
        assert_eq!(production::snapshot_write(&r, 7, BASE + 4096, 8192, prot), matches!(prot, 2 | 3 | 6 | 7));
    }
    for (pid, base, bytes) in [(8, BASE, 4096), (7, BASE - 4096, 4096),
        (7, BASE, 0), (7, BASE + 15 * 4096, 8192), (7, usize::MAX - 4095, 8192)] {
        assert!(!production::snapshot_write(&r, pid, base, bytes, 3));
    }
    r.process.mappings.invalidate_views();
    assert!(!production::snapshot_write(&r, 7, BASE, 4096, 3));
    production::remember(&r, 7, ID, SIZE, BASE, 16 * 4096, 0);
    assert!(production::snapshot_write(&r, 7, BASE, 4096, 3));
    production::before_protect(&r, 7, 3);
    assert!(!production::snapshot_write(&r, 7, BASE, 4096, 3));
    production::remember(&r, 7, ID, 8192 + 17, BASE, 16 * 4096, 4096);
    assert!(production::snapshot_write(&r, 7, BASE + 4096, 8192, 3));
    assert!(!production::snapshot_write(&r, 7, BASE + 8192, 4096, 3));
}

#[test]
fn consecutive_read_execute_segments_reuse_original_extent_and_leave_all_bytes_intact() {
    let mut runtime = fixture();
    let before = runtime.bytes.clone();
    for (page, pages, prot) in [(1, 8, 5), (9, 5, 1), (15, 1, 1), (0, 1, 1)] {
        assert_eq!(production::reuse(&mut runtime, 7, ID, SIZE,
            BASE + page * 4096, pages * 4096, page * 4096, prot, 18), Ok(true));
    }
    assert_eq!(runtime.protections.len(), 4);
    assert!(runtime.releases.is_empty());
    assert_eq!(runtime.bytes, before);
    assert!(runtime.bytes[SIZE..].iter().all(|byte| *byte == 0));
}

#[test]
fn all_subranges_require_exact_file_offset_translation_and_fresh_version_and_size() {
    let mut runtime = fixture();
    for first in 0..16 {
        for pages in 1..=16-first {
            for shift in [0, 4096] {
                let result = production::reuse(&mut runtime, 7, ID, SIZE,
                    BASE + first * 4096, pages * 4096, first * 4096 + shift, 5, 18);
                assert_eq!(result, Ok(shift == 0));
            }
        }
    }
    for (id, size) in [(ID + (1 << 32), SIZE), (ID, SIZE + 1), (ID & 0xffff_ffff, SIZE)] {
        assert_eq!(production::reuse(&mut runtime, 7, id, size, BASE, 4096, 0, 1, 18), Ok(false));
    }
}

#[test]
fn generation_changes_fork_exec_unmap_and_pid_reuse_decline_without_effects() {
    for mutation in 0..5 {
        let mut runtime = fixture();
        match mutation {
            0 => runtime.process.mappings.invalidate_views(),
            1 => { assert!(runtime.process.mappings.push(ProcessMapping { base: BASE + 0x20000, size: 4096, prot: 3 })); },
            2 => runtime.process.mappings.clear(),
            3 => {
                let old = runtime.process.mappings;
                let mut fork = mapping_table::MappingTable::EMPTY;
                assert!(fork.initialize_from(old));
                runtime.process.mappings = fork;
                let mut old = old; old.release();
            },
            _ => { runtime.process.mappings.release(); assert!(runtime.process.mappings.push(ProcessMapping { base: BASE, size: 4096, prot: 1 })); },
        }
        assert_eq!(production::reuse(&mut runtime, 7, ID, SIZE, BASE, 4096, 0, 1, 18), Ok(false));
        assert!(runtime.protections.is_empty() && runtime.releases.is_empty());
    }
}

#[test]
fn unsupported_flags_rights_bounds_and_metadata_capacity_keep_original_path() {
    for (base, bytes, prot, flags) in [
        (BASE, 4096, 3, 18), (BASE, 4096, 7, 18), (BASE, 4096, 0, 18),
        (BASE, 4096, 1, 2), (BASE, 4096, 1, 17), (BASE+1, 4096, 1, 18),
        (BASE, 0, 1, 18), (BASE-4096, 4096, 1, 18), (BASE, 17*4096, 1, 18),
        (usize::MAX-4095, 8192, 1, 18),
    ] {
        let mut runtime = fixture();
        assert_eq!(production::reuse(&mut runtime, 7, ID, SIZE, base, bytes, 0, prot, flags), Ok(false));
        assert!(runtime.protections.is_empty() && runtime.releases.is_empty());
    }
    let mut runtime = fixture(); runtime.can_protect = false;
    assert_eq!(production::reuse(&mut runtime, 7, ID, SIZE, BASE, 4096, 0, 1, 18), Ok(false));
    assert!(runtime.protections.is_empty());
}

#[test]
fn protection_failure_cleans_requested_range_once_and_never_reuses_old_proof() {
    let mut runtime = fixture(); runtime.fail_protect = true;
    assert_eq!(production::reuse(&mut runtime, 7, ID, SIZE, BASE+4096, 8192, 4096, 5, 18), Err(5));
    assert_eq!(runtime.releases, [(BASE+4096, 8192)]);
    assert_eq!(runtime.protections.len(), 1);
    assert_eq!(runtime.readonly_mapping.get().generation, 0);
    runtime.fail_protect = false;
    assert_eq!(production::reuse(&mut runtime, 7, ID, SIZE, BASE, 4096, 0, 1, 18), Ok(false));
}

#[test]
fn unversioned_missing_process_overflow_and_exhausted_generations_never_admit() {
    let runtime = fixture();
    for (pid, id, base, size, offset) in [
        (7, 9, BASE, 4096, 0), (8, ID, BASE, 4096, 0),
        (7, ID, usize::MAX-4095, 4096, 0), (7, ID, BASE, 4096, usize::MAX-4095),
    ] {
        production::remember(&runtime, pid, id, SIZE, base, size, offset);
        assert_eq!(runtime.readonly_mapping.get().generation, 0);
    }
    assert_eq!(state::ReadonlyMapping::new(0, ID, SIZE, BASE, 4096, 0).generation, 0);
}

#[test]
fn readonly_permission_changes_retain_original_extent_for_later_fixed_segments() {
    for prot in [0, 1, 4, 5] {
        let mut runtime = fixture();
        let bytes = runtime.bytes.clone();
        let original = runtime.readonly_mapping.get().generation;
        assert_eq!(sys_mprotect(&mut runtime, 7, BASE + 4096, 15 * 4096, prot), Ok(0));
        let updated = runtime.readonly_mapping.get().generation;
        assert_ne!(original, updated);
        assert_eq!(updated, runtime.process.mappings.generation());
        assert_eq!(production::reuse(&mut runtime, 7, ID, SIZE, BASE + 4096, 12 * 4096, 4096, 5, 18), Ok(true));
        assert_eq!(production::reuse(&mut runtime, 7, ID, SIZE, BASE, 4096, 0, 1, 18), Ok(true));
        assert_eq!(runtime.bytes, bytes);
        assert!(runtime.releases.is_empty());
    }
}

#[test]
fn write_permission_then_readonly_cannot_resurrect_changed_contents() {
    for prot in [2, 3, 6, 7] {
        let mut runtime = fixture();
        assert_eq!(sys_mprotect(&mut runtime, 7, BASE, 4096, prot), Ok(0));
        runtime.bytes[0] ^= 0x5a;
        assert_eq!(sys_mprotect(&mut runtime, 7, BASE, 4096, 1), Ok(0));
        assert_eq!(production::reuse(&mut runtime, 7, ID, SIZE, BASE, 4096, 0, 1, 18), Ok(false));
    }
}

#[test]
fn failed_readonly_protection_and_preexisting_stale_generations_never_restore_proof() {
    for stale in [false, true] {
        let mut runtime = fixture();
        if stale { runtime.process.mappings.invalidate_views(); }
        else { runtime.fail_protect = true; }
        let _ = sys_mprotect(&mut runtime, 7, BASE, 4096, 0);
        runtime.fail_protect = false;
        assert_eq!(sys_mprotect(&mut runtime, 7, BASE, 4096, 1), Ok(0));
        assert_eq!(production::reuse(&mut runtime, 7, ID, SIZE, BASE, 4096, 0, 1, 18), Ok(false));
    }
}

#[test]
fn another_vm_protection_does_not_take_or_replace_unrelated_origin() {
    let runtime = fixture();
    let before = runtime.readonly_mapping.get().generation;
    let proof = production::before_protect(&runtime, 8, 0);
    production::after_protect(&runtime, 8, proof);
    assert_eq!(runtime.readonly_mapping.get().generation, before);
    assert!(runtime.readonly_mapping.get().covers(runtime.process.mappings.generation(), ID, SIZE, BASE, 4096, 0));
}
