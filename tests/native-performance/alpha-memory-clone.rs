//! Real VM-clone traversal; page construction/copy/protection are modeled.
use super::*;
use std::collections::BTreeMap;
const PAGE_SIZE: usize = 4096;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PageSource {
    pid: usize,
    address: usize,
    bytes: usize,
}
struct Alpha {
    processes: ProcessManager,
    data: BTreeMap<(usize, usize), Box<[u8; PAGE_SIZE]>>,
    allocations: Vec<(usize, usize, usize, PageSource)>,
    copies: Vec<(usize, usize)>,
    protections: Vec<(usize, usize)>,
    split_calls: usize,
    fail_allocate: bool,
    fail_protect: Option<usize>,
    next_frame: usize,
}
impl Alpha {
    fn split_private_large_mappings(&mut self, _: usize, _: usize, _: usize, _: bool) -> Result<(), CapabilityError> {
        self.split_calls += 1;
        Ok(())
    }
    fn new() -> Self {
        let mut processes = manager();
        for pid in 1..=3 {
            install(&mut processes, pid, 200 + pid);
            processes.ensure_vm_space_for_pid(pid).unwrap();
        }
        Self {
            processes,
            data: BTreeMap::new(),
            allocations: Vec::new(),
            copies: Vec::new(),
            protections: Vec::new(),
            split_calls: 0,
            fail_allocate: false,
            fail_protect: None,
            next_frame: 100,
        }
    }
    fn page(&mut self, pid: usize, va: usize, value: u8, permissions: usize) {
        let owner = self.processes.memory_owner_pid(pid);
        self.data.insert((owner, va), Box::new([value; PAGE_SIZE]));
        self.next_frame += 1;
        let vm = self.processes.vm_space_mut(pid).unwrap();
        vm.record_frame(va, self.next_frame).unwrap();
        vm.set_permissions(va, permissions);
    }
    fn map_process_initialized_pages(
        &mut self,
        pid: usize,
        base: Option<usize>,
        pages: usize,
        source: Option<PageSource>,
    ) -> Result<usize, CapabilityError> {
        self.map_process_initialized_pages_protected(pid, base, pages, source, 7)
    }
    fn map_process_initialized_pages_protected(
        &mut self, pid: usize, base: Option<usize>, pages: usize,
        source: Option<PageSource>, permissions: usize,
    ) -> Result<usize, CapabilityError> {
        let base = base.unwrap();
        let source = source.unwrap();
        assert_eq!(source.bytes, pages * PAGE_SIZE);
        self.allocations.push((pid, base, pages, source));
        if self.fail_allocate {
            return Err(CapabilityError::Fatal);
        }
        let source_owner = self.processes.memory_owner_pid(source.pid);
        for offset in (0..source.bytes).step_by(PAGE_SIZE) {
            let value = *self.data[&(source_owner, source.address + offset)];
            self.page(pid, base + offset, 0, permissions);
            let owner = self.processes.memory_owner_pid(pid);
            *self.data.get_mut(&(owner, base + offset)).unwrap() = Box::new(value);
        }
        Ok(base)
    }
    fn copy_process_memory(
        &mut self,
        source: usize,
        address: usize,
        destination: usize,
        target: usize,
        bytes: usize,
    ) -> Result<(), CapabilityError> {
        self.copies.push((address, target));
        let owner = self.processes.memory_owner_pid(source);
        let values: Vec<_> = (0..bytes).step_by(PAGE_SIZE)
            .map(|offset| self.data[&(owner, address + offset)].clone()).collect();
        let owner = self.processes.memory_owner_pid(destination);
        for (index, value) in values.into_iter().enumerate() {
            self.data.insert((owner, target + index * PAGE_SIZE), value);
        }
        Ok(())
    }
    fn process_frame_for_page(&mut self, pid: usize, va: usize) -> Result<usize, CapabilityError> {
        self.processes
            .vm_space_mut(pid)
            .and_then(|vm| vm.find_frame(va))
            .ok_or(CapabilityError::InvalidArgument)
    }
    fn protect_process_page(
        &mut self,
        pid: usize,
        _: usize,
        va: usize,
        _: usize,
        permissions: usize,
    ) -> Result<(), CapabilityError> {
        if self.fail_protect == Some(self.protections.len()) {
            return Err(CapabilityError::Fatal);
        }
        self.protections.push((va, permissions));
        self.processes
            .vm_space_mut(pid)
            .unwrap()
            .set_permissions(va, permissions);
        Ok(())
    }
}
#[path = "../../nanami/src/nanami_core/alpha/memory_clone.rs"]
mod production;

#[test]
fn large_copy_with_identical_current_rights_needs_no_shape_or_protection_changes() {
    for permissions in 0..=7 {
        let mut a = Alpha::new();
        for i in 0..512 {
            a.page(1, 0x400000 + i * PAGE_SIZE, (i % 239) as u8, permissions);
            a.page(2, 0x800000 + i * PAGE_SIZE, 0, permissions);
        }
        a.clone_backed_memory(1, 0x400000, 2, 0x800000, 1 << 21).unwrap();
        assert_eq!(a.split_calls, 0);
        assert_eq!(a.copies, [(0x400000, 0x800000)]);
        assert!(a.allocations.is_empty());
        assert!(a.protections.is_empty());
        for i in 0..512 {
            assert_eq!(a.data[&(1, 0x400000 + i * PAGE_SIZE)], a.data[&(2, 0x800000 + i * PAGE_SIZE)]);
        }
    }
}

#[test]
fn large_copy_still_applies_different_destination_rights() {
    let mut a = Alpha::new();
    for i in 0..512 {
        a.page(1, 0x400000 + i * PAGE_SIZE, (i % 239) as u8, 5);
        a.page(2, 0x800000 + i * PAGE_SIZE, 0, if i == 511 { 3 } else { 5 });
    }
    a.clone_backed_memory(1, 0x400000, 2, 0x800000, 1 << 21).unwrap();
    assert_eq!(a.split_calls, 1);
    assert_eq!(a.copies.len(), 512);
    assert_eq!(a.protections.len(), 512);
    assert_eq!(a.processes.vm_space(2).unwrap().permissions(0x9ff000), 5);
    for i in 0..512 {
        assert_eq!(a.data[&(1, 0x400000 + i * PAGE_SIZE)], a.data[&(2, 0x800000 + i * PAGE_SIZE)]);
    }
}

#[test]
fn large_overlapping_copy_through_shared_vm_pids_keeps_owner_aware_order() {
    let mut a = Alpha::new();
    for i in 0..513 {
        a.page(1, 0x400000 + i * PAGE_SIZE, (i % 239) as u8, 3);
    }
    a.processes.share_address_space(3, 1).unwrap();
    a.clone_backed_memory(1, 0x400000, 3, 0x401000, 1 << 21).unwrap();
    assert_eq!(a.split_calls, 1);
    assert_eq!(a.copies.len(), 512);
    for i in 0..512 {
        assert_eq!(a.data[&(1, 0x401000 + i * PAGE_SIZE)][0], (i % 239) as u8);
    }
}

#[test]
fn cloning_an_uncertain_source_fails_before_copying_or_allocating() {
    let mut a = Alpha::new();
    a.page(1, 0x1000, 11, 3);
    a.processes
        .vm_space_mut(1)
        .unwrap()
        .mark_mapping_uncertain(0x1000);
    assert_eq!(a.clone_process_vm(1, 2), Err(CapabilityError::Fatal));
    assert_eq!(
        a.clone_backed_memory(1, 0x1000, 2, 0x5000, PAGE_SIZE),
        Err(CapabilityError::Fatal)
    );
    assert!(a.allocations.is_empty());
    assert!(a.copies.is_empty());
    assert!(a.protections.is_empty());
    assert!(a
        .processes
        .vm_space_mut(2)
        .unwrap()
        .backed_pages(0, usize::MAX)
        .is_empty());
}

#[test]
fn bounded_clone_does_not_quarantine_an_unrelated_source_range() {
    let mut a = Alpha::new();
    a.page(1, 0x1000, 11, 3);
    a.page(1, 0x2000, 22, 3);
    a.processes
        .vm_space_mut(1)
        .unwrap()
        .mark_mapping_uncertain(0x2000);
    a.clone_backed_memory(1, 0x1000, 2, 0x5000, PAGE_SIZE)
        .unwrap();
    assert_eq!(a.data[&(2, 0x5000)][0], 11);
    assert!(a
        .processes
        .vm_space_mut(1)
        .unwrap()
        .mapping_is_uncertain(0x2000));
}

#[test]
fn contiguous_private_snapshot_groups_final_permissions_and_preserves_every_byte() {
    let mut a = Alpha::new();
    for (i, permission) in [5, 3, 0, 7].into_iter().enumerate() {
        a.page(1, (i + 1) * PAGE_SIZE, i as u8 + 10, permission);
    }
    a.clone_process_vm(1, 2).unwrap();
    assert_eq!(a.allocations.len(), 4);
    for i in 1..=4 {
        assert_eq!(a.data[&(1, i * PAGE_SIZE)], a.data[&(2, i * PAGE_SIZE)]);
    }
    assert_eq!(
        a.protections,
        [(0x3000, 0)]
    );
    a.data.get_mut(&(1, 0x1000)).unwrap()[0] = 90;
    a.data.get_mut(&(2, 0x2000)).unwrap()[0] = 91;
    assert_eq!(a.data[&(2, 0x1000)][0], 10);
    assert_eq!(a.data[&(1, 0x2000)][0], 11);
}

#[test]
fn sparse_holes_split_runs_without_materializing_reservations() {
    let mut a = Alpha::new();
    for va in [0x1000, 0x2000, 0x100_0000_0000, 0x100_0000_1000] {
        a.page(1, va, 21, 0);
    }
    a.clone_process_vm(1, 2).unwrap();
    assert_eq!(a.allocations.len(), 2);
    assert!(a.allocations.iter().all(|run| run.2 == 2));
    assert_eq!(
        a.processes
            .vm_space_mut(2)
            .unwrap()
            .backed_pages(0, usize::MAX)
            .len(),
        4
    );
}

#[test]
fn later_write_upgrades_and_supervisor_copies_preserve_private_fork_isolation() {
    let mut a = Alpha::new();
    a.page(1, 0x1000, 0x51, 5);
    a.page(3, 0x2000, 0xc7, 3);
    a.clone_process_vm(1, 2).unwrap();
    let child = a.processes.memory_entry(2).unwrap();
    let frame = a.process_frame_for_page(2, 0x1000).unwrap();
    a.protect_process_page(2, child.address_space, 0x1000, frame, 3)
        .unwrap();
    a.copy_process_memory(3, 0x2000, 2, 0x1000, PAGE_SIZE)
        .unwrap();
    assert_eq!(a.data[&(1, 0x1000)][0], 0x51);
    assert_eq!(a.data[&(2, 0x1000)][0], 0xc7);
    assert_eq!(a.processes.vm_space_mut(1).unwrap().permissions(0x1000), 5);
    assert_eq!(a.processes.vm_space_mut(2).unwrap().permissions(0x1000), 3);

    let parent = a.processes.memory_entry(1).unwrap();
    let frame = a.process_frame_for_page(1, 0x1000).unwrap();
    a.protect_process_page(1, parent.address_space, 0x1000, frame, 3)
        .unwrap();
    a.data.get_mut(&(3, 0x2000)).unwrap().fill(0x83);
    a.copy_process_memory(3, 0x2000, 1, 0x1000, PAGE_SIZE)
        .unwrap();
    assert_eq!(a.data[&(1, 0x1000)][0], 0x83);
    assert_eq!(a.data[&(2, 0x1000)][0], 0xc7);
}

#[test]
fn source_mapping_replacement_preserves_existing_private_descendants() {
    let mut a = Alpha::new();
    a.page(1, 0x1000, 0x62, 1);
    a.clone_process_vm(1, 2).unwrap();
    a.processes.vm_space_mut(1).unwrap().forget_frame(0x1000);
    a.data.remove(&(1, 0x1000));
    a.page(1, 0x1000, 0xa4, 5);
    a.clone_process_vm(2, 3).unwrap();
    assert_eq!(a.data[&(1, 0x1000)][0], 0xa4);
    assert_eq!(a.data[&(2, 0x1000)][0], 0x62);
    assert_eq!(a.data[&(3, 0x1000)][0], 0x62);
    assert_eq!(a.processes.vm_space_mut(2).unwrap().permissions(0x1000), 1);
    assert_eq!(a.processes.vm_space_mut(3).unwrap().permissions(0x1000), 1);
}

#[test]
fn existing_destination_keeps_the_generic_copy_and_protect_path() {
    let mut a = Alpha::new();
    for va in [0x1000, 0x2000, 0x3000] {
        a.page(1, va, 24, 5);
    }
    a.page(2, 0x2000, 90, 7);
    a.clone_process_vm(1, 2).unwrap();
    assert_eq!(a.copies, [(0x2000, 0x2000)]);
    assert_eq!(a.allocations.len(), 2);
    assert!(a.allocations.iter().all(|run| run.2 == 1));
    assert_eq!(a.data[&(2, 0x2000)][0], 24);
}

#[test]
fn shared_vm_source_resolves_the_owner_but_child_storage_is_independent() {
    let mut a = Alpha::new();
    a.page(1, 0x1000, 44, 3);
    a.page(1, 0x2000, 45, 1);
    a.processes.share_address_space(3, 1).unwrap();
    a.clone_process_vm(3, 2).unwrap();
    assert_eq!(a.allocations.len(), 2);
    a.data.get_mut(&(1, 0x1000)).unwrap()[0] = 99;
    assert_eq!(a.data[&(2, 0x1000)][0], 44);
}

#[test]
fn empty_snapshot_and_missing_destination_do_not_allocate() {
    let mut a = Alpha::new();
    assert_eq!(a.clone_process_vm(1, 2), Ok(()));
    assert_eq!(
        a.clone_process_vm(1, 99),
        Err(CapabilityError::InvalidArgument)
    );
    assert!(a.allocations.is_empty());
}

#[test]
fn constructor_failure_stops_before_protection_or_source_mutation() {
    let mut a = Alpha::new();
    a.page(1, 0x1000, 21, 5);
    a.page(1, 0x2000, 22, 0);
    a.fail_allocate = true;
    assert_eq!(a.clone_process_vm(1, 2), Err(CapabilityError::Fatal));
    assert_eq!(a.allocations.len(), 1);
    assert!(a.protections.is_empty());
    assert_eq!(a.data[&(1, 0x2000)][0], 22);
}

#[test]
fn protection_failure_keeps_copied_child_pages_accounted_for_task_rollback() {
    let mut a = Alpha::new();
    for va in [0x1000, 0x2000, 0x3000] {
        a.page(1, va, 31, 0);
    }
    a.fail_protect = Some(1);
    assert_eq!(a.clone_process_vm(1, 2), Err(CapabilityError::Fatal));
    assert_eq!(
        a.processes
            .vm_space_mut(2)
            .unwrap()
            .backed_pages(0, usize::MAX)
            .len(),
        3
    );
    assert_eq!(a.protections, [(0x1000, 0)]);
    assert_eq!(a.data[&(1, 0x3000)][0], 31);
}

#[test]
fn contiguous_snapshot_can_cross_frame_node_boundaries() {
    let mut a = Alpha::new();
    for i in 1..=520 {
        a.page(1, i * PAGE_SIZE, (i % 239) as u8, 7);
    }
    a.clone_process_vm(1, 2).unwrap();
    assert_eq!(a.allocations.len(), 1);
    assert_eq!(a.allocations[0].2, 520);
    assert_eq!(a.data[&(2, 520 * PAGE_SIZE)][0], (520 % 239) as u8);
}

#[test]
fn generic_overlapping_copy_keeps_memmove_order() {
    for (source, target, expected) in [
        (0x1000, 0x2000, [1, 1, 2, 3]),
        (0x2000, 0x1000, [2, 3, 4, 4]),
    ] {
        let mut a = Alpha::new();
        for i in 1..=4 {
            a.page(1, i * PAGE_SIZE, i as u8, 3);
        }
        a.clone_backed_memory(1, source, 1, target, 3 * PAGE_SIZE)
            .unwrap();
        for (i, value) in expected.into_iter().enumerate() {
            assert_eq!(a.data[&(1, (i + 1) * PAGE_SIZE)][0], value);
        }
        assert!(a.allocations.is_empty());
    }
}

#[test]
fn batched_snapshots_match_the_original_page_path_for_sparse_mixed_permissions() {
    let mut random = 0x42d6_8305u64;
    for _ in 0..64 {
        let mut batched = Alpha::new();
        let mut original = Alpha::new();
        for i in 1..=64 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            if random >> 32 & 3 == 0 {
                continue;
            }
            let value = (random >> 40) as u8;
            let permissions = (random >> 48) as usize & 7;
            for alpha in [&mut batched, &mut original] {
                alpha.page(1, i * PAGE_SIZE, value, permissions);
            }
        }
        batched.clone_process_vm(1, 2).unwrap();
        original
            .clone_backed_memory(1, PAGE_SIZE, 2, PAGE_SIZE, 64 * PAGE_SIZE)
            .unwrap();
        assert_eq!(batched.data, original.data);
        assert_eq!(
            batched
                .processes
                .vm_space_mut(2)
                .unwrap()
                .backed_pages(0, usize::MAX),
            original
                .processes
                .vm_space_mut(2)
                .unwrap()
                .backed_pages(0, usize::MAX)
        );
    }
}

#[test]
fn nested_fork_captures_current_child_bytes_and_permissions_not_the_grandparent() {
    let mut a = Alpha::new();
    for (i, permission) in [5, 3, 0, 7].into_iter().enumerate() {
        a.page(1, (i + 1) * PAGE_SIZE, 10 + i as u8, permission);
    }
    a.clone_process_vm(1, 2).unwrap();
    // A trusted copy may update retained PROT_NONE bytes without making a
    // hardware leaf accessible. A later fork must inherit that current data.
    a.data.get_mut(&(2, 0x3000)).unwrap()[0] = 42;
    a.data.get_mut(&(2, 0x2000)).unwrap()[PAGE_SIZE - 1] = 43;
    a.processes
        .vm_space_mut(2)
        .unwrap()
        .set_permissions(0x1000, 1);
    a.data.get_mut(&(1, 0x1000)).unwrap()[0] = 90;
    a.clone_process_vm(2, 3).unwrap();
    for va in [0x1000, 0x2000, 0x3000, 0x4000] {
        assert_eq!(a.data[&(2, va)], a.data[&(3, va)]);
        assert_eq!(
            a.processes.vm_space_mut(2).unwrap().permissions(va),
            a.processes.vm_space_mut(3).unwrap().permissions(va)
        );
    }
    a.data.get_mut(&(2, 0x3000)).unwrap()[0] = 91;
    a.data.get_mut(&(3, 0x2000)).unwrap()[PAGE_SIZE - 1] = 92;
    assert_eq!(a.data[&(3, 0x1000)][0], 10);
    assert_eq!(a.data[&(3, 0x3000)][0], 42);
    assert_eq!(a.data[&(2, 0x2000)][PAGE_SIZE - 1], 43);
    assert_eq!(a.data[&(1, 0x3000)][0], 12);
    assert_eq!(a.processes.vm_space_mut(3).unwrap().permissions(0x3000), 0);
}

#[test]
fn parent_exec_and_same_address_replacement_cannot_change_a_private_child() {
    let mut a = Alpha::new();
    for (va, byte, permission) in [(0x1000, 21, 5), (0x3000, 22, 0)] {
        a.page(1, va, byte, permission);
    }
    a.clone_process_vm(1, 2).unwrap();
    a.processes
        .reset_runtime_memory_for_exec(1, 0, 0x100000, 0x1000000)
        .unwrap();
    a.data.retain(|&(pid, _), _| pid != 1);
    a.page(1, 0x1000, 99, 7);
    a.page(1, 0x3000, 98, 7);
    a.clone_process_vm(2, 3).unwrap();
    for pid in [2, 3] {
        assert_eq!(a.data[&(pid, 0x1000)][0], 21);
        assert_eq!(a.data[&(pid, 0x3000)][PAGE_SIZE - 1], 22);
        assert_eq!(
            a.processes.vm_space_mut(pid).unwrap().permissions(0x1000),
            5
        );
        assert_eq!(
            a.processes.vm_space_mut(pid).unwrap().permissions(0x3000),
            0
        );
        assert!(a
            .processes
            .vm_space_mut(pid)
            .unwrap()
            .find_frame(0x2000)
            .is_none());
    }
}
