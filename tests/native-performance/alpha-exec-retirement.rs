//! Production AS switch, real table-pool retirement and RAM lifetime transaction.
use super::*;
use a9n_abi::CapabilityType;
use std::{cell::RefCell, collections::{BTreeSet, BTreeMap}};
type CapabilityResult = Result<(), CapabilityError>;
const PAGE_SIZE: usize = 4096;
const PAGE_BITS: usize = 12;
const PROCESS_FRAME_CHUNK_PAGES: usize = 4;
const USER_HEAP_LIMIT: usize = 0x4000_0000;
const PROCESS_SLOT_ADDRESS_SPACE: usize = 2;
const PROCESS_SLOT_L3_NODE: usize = 3;
const PROCESS_SLOT_L2_NODE: usize = 4;
const PROCESS_SLOT_L1_NODE: usize = 5;
const PROCESS_SLOT_FRAME_NODE: usize = 6;
const PAGE_TABLE_NODE_RADIX: usize = 8;
const PROCESS_FRAME_DIRECTORY_RADIX: usize = 8;
const PAGE_TABLE_POOL_RADIX: usize = 10;
const PAGE_TABLE_POOL_SLOTS: usize = 1024;
const TABLE_NODE: usize = 7000;
const TABLE_GENERICS: usize = 8000;
const OS_REQUEST_PROCESS_REAP: usize = 99;
struct OsRequestEvent { identifier: usize, code: usize, arg0: usize, arg1: usize, arg2: usize, arg3: usize }
fn make_child_slot_descriptor(node: usize, _: usize, slot: usize) -> usize { node * 10000 + slot }
fn process_frame_directory_descriptor(root: usize) -> usize { root + 1000 }
fn process_frame_chunk_descriptor(root: usize, chunk: usize) -> usize { root + chunk }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Call { Invalidate, RemoveAs, NewAs, Configure, Table(usize), Revoke(usize), Free(usize) }
struct Calls {
    events: Vec<Call>, leaves: BTreeSet<usize>, caps: BTreeSet<usize>, tables: BTreeSet<usize>,
    old_as_reachable: bool, fail: Option<Call>, fail_after: bool,
    first_remove_allocations: Option<usize>, fail_free: Option<usize>,
    free_attempts: usize, freed: BTreeSet<usize>,
}
impl Default for Calls {
    fn default() -> Self {
        Self { events: Vec::new(), leaves: BTreeSet::new(), caps: BTreeSet::new(),
            tables: BTreeSet::new(), old_as_reachable: true, fail: None, fail_after: false,
            first_remove_allocations: None, fail_free: None, free_attempts: 0, freed: BTreeSet::new() }
    }
}
thread_local! { static CALLS: RefCell<Calls> = RefCell::new(Calls::default()); }
fn kernel(event: Call, effect: impl FnOnce(&mut Calls)) -> CapabilityResult {
    CALLS.with_borrow_mut(|calls| {
        calls.events.push(event);
        if calls.fail == Some(event) && !calls.fail_after { return Err(CapabilityError::Fatal); }
        effect(calls);
        if calls.fail == Some(event) { return Err(CapabilityError::Fatal); }
        Ok(())
    })
}
mod arch {
    use super::*;
    pub mod address_space {
        use super::*;
        pub fn unmap(_: usize, _: usize, _: usize) -> CapabilityResult {
            panic!("unreachable exec must not walk or unmap individual leaves")
        }
    }
    pub mod node {
        use super::*;
        pub fn remove(root: usize, slot: usize) -> CapabilityResult {
            assert_eq!((root, slot), (202, PROCESS_SLOT_ADDRESS_SPACE));
            CALLS.with_borrow_mut(|calls| {
                calls.first_remove_allocations = PROTECT_ALLOCATIONS.with(|count| count.get());
            });
            // The PCB still holds the old AS after this root capability removal.
            kernel(Call::RemoveAs, |_| {})
        }
        pub fn revoke(node: usize, slot: usize) -> CapabilityResult {
            if node == TABLE_GENERICS {
                return kernel(Call::Table(slot), |calls| {
                    assert!(!calls.old_as_reachable, "table retired while PCB still holds old AS");
                    calls.tables.remove(&slot);
                    if calls.tables.is_empty() { calls.leaves.clear(); }
                });
            }
            assert_eq!(node, process_frame_directory_descriptor(202));
            kernel(Call::Revoke(slot), |calls| {
                assert!(!calls.old_as_reachable && calls.tables.is_empty());
                calls.caps.remove(&slot);
            })
        }
    }
    pub mod generic {
        use super::*;
        pub fn convert(_: usize, kind: CapabilityType, _: usize, _: usize, node: usize, slot: usize)
            -> CapabilityResult {
            if matches!(kind, CapabilityType::AddressSpace) {
                assert_eq!((node, slot), (202, PROCESS_SLOT_ADDRESS_SPACE));
                return kernel(Call::NewAs, |_| {});
            }
            // The other conversions set up actual PageTables pool ownership.
            if matches!(kind, CapabilityType::PageTable) {
                assert_eq!(node, TABLE_NODE);
                CALLS.with_borrow_mut(|calls| { calls.tables.insert(slot); });
            } else { assert!(matches!(kind, CapabilityType::Generic)); }
            Ok(())
        }
    }
    pub mod process_control_block {
        use super::*;
        pub fn configure(pcb: usize, _: capability_call::process_control_block::ConfigurationInfo,
            space: usize, _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
            _: usize, _: usize, _: usize) -> CapabilityResult {
            assert_eq!((pcb, space), (203, 204));
            kernel(Call::Configure, |calls| calls.old_as_reachable = false)
        }
    }
}
#[path = "../../nanami/src/nanami_core/memory/page_tables.rs"]
mod page_tables;
struct Memory { tables: page_tables::PageTables }
impl Memory {
    fn release_page_tables(&mut self, space: usize) -> CapabilityResult { self.tables.release(space) }
    fn free_physical(&mut self, address: usize, _: usize) -> CapabilityResult {
        CALLS.with_borrow_mut(|calls| {
            assert!(!calls.old_as_reachable, "RAM freed while PCB still holds old AS");
            assert!(calls.tables.is_empty(), "RAM freed before all old tables retired");
            assert!(calls.leaves.is_empty() && calls.caps.is_empty());
            let index = calls.free_attempts; calls.free_attempts += 1;
            if calls.fail_free == Some(index) { return Err(CapabilityError::Fatal); }
            assert!(calls.freed.insert(address), "RAM released twice");
            calls.events.push(Call::Free(address)); Ok(())
        })
    }
}
struct Alpha { processes: ProcessManager, memory: Memory }
impl Alpha {
    fn new() -> Self {
        CALLS.with(|calls| *calls.borrow_mut() = Calls::default());
        let mut processes = manager(); install(&mut processes, 1, 201); install(&mut processes, 2, 202);
        processes.ensure_vm_space_for_pid(1).unwrap(); processes.seal_capabilities(1).unwrap();
        let mut memory = Memory { tables: page_tables::PageTables::new(TABLE_NODE, TABLE_GENERICS) };
        for _ in 0..4 { memory.tables.allocate(1, 204, 1, true).unwrap(); }
        Self { processes, memory }
    }
    fn process_arena_for_root(&self, root: usize) -> Result<usize, CapabilityError> {
        assert_eq!(root, 202); Ok(999)
    }
    fn handle_process_reap_request(&mut self, _: OsRequestEvent) -> CapabilityResult {
        panic!("private exec cannot have a deferred shared owner")
    }
    fn invalidate_process_copy_mappings(&mut self, _: usize) -> CapabilityResult {
        kernel(Call::Invalidate, |_| {})
    }
    fn page(&mut self, slot: usize, physical: usize, permissions: usize) {
        let va = (slot + 1) * PAGE_SIZE;
        self.processes.register_physical_allocation(1, va, slot, physical, 1).unwrap();
        self.processes.register_frame_chunk(1, slot / PROCESS_FRAME_CHUNK_PAGES).unwrap();
        let vm = self.processes.vm_space_mut(1).unwrap();
        vm.record_frame(va, slot + 1000).unwrap(); vm.set_permissions(va, permissions);
        CALLS.with_borrow_mut(|calls| {
            if permissions != 0 { calls.leaves.insert(va); }
            calls.caps.insert(slot / PROCESS_FRAME_CHUNK_PAGES);
        });
    }
    fn retire(&mut self) -> CapabilityResult {
        self.drop_sealed_process_runtime_mappings(1, self.processes.find_entry_by_pid(1).unwrap())
    }
}
#[path = "../../nanami/src/nanami_core/alpha/exec_space.rs"]
mod space;
#[path = "../../nanami/src/nanami_core/alpha/memory_retirement.rs"]
mod deferred;
#[path = "../../nanami/src/nanami_core/alpha/memory_exec.rs"]
mod production;

#[test]
fn sealed_exec_switches_pcb_and_retires_all_tables_before_caps_and_ram() {
    let mut a = Alpha::new();
    for (slot, permissions) in [(0, 5), (1, 0), (4, 3), (9, 1)] { a.page(slot, 100 + slot, permissions); }
    a.retire().unwrap();
    CALLS.with_borrow(|calls| {
        assert_eq!(&calls.events[..11], &[Call::Invalidate, Call::RemoveAs, Call::NewAs,
            Call::Configure, Call::Table(3), Call::Table(2), Call::Table(1), Call::Table(0),
            Call::Revoke(0), Call::Revoke(1), Call::Revoke(2)]);
        assert_eq!(calls.freed.len(), 4);
    });
    assert!(a.processes.vm_space_mut(1).unwrap().backed_pages(0, usize::MAX).is_empty());
    assert_eq!(a.processes.deferred_physical_allocations_for_pid(1).count(), 0);
}

#[test]
fn sealed_exec_eliminates_leaf_walk_and_unmaps_for_small_and_large_images() {
    for pages in [1, 31, 64, 513, 4096] {
        let mut a = Alpha::new();
        for slot in 0..pages { a.page(slot, 10000 + slot, if slot % 7 == 6 { 0 } else { 5 }); }
        let mut old_unmaps = 0;
        a.processes.vm_space_mut(1).unwrap().try_for_each_present_frame(0, usize::MAX, |_, _| {
            old_unmaps += 1; Ok::<_, ()>(())
        }).unwrap();
        CALLS.with_borrow_mut(|calls| calls.events.reserve(20000));
        PROTECT_ALLOCATIONS.with(|count| count.set(Some(0)));
        let result = a.retire(); PROTECT_ALLOCATIONS.with(|count| count.set(None));
        assert_eq!(result, Ok(()));
        CALLS.with_borrow(|calls| {
            assert_eq!(calls.first_remove_allocations, Some(0));
            assert_eq!(calls.freed.len(), pages);
            assert_eq!(calls.events.iter().filter(|event| matches!(event, Call::Table(_))).count(), 4);
        });
        println!("EXEC-RETIRE frames={pages} old-unmaps={old_unmaps} new-unmaps=0 table-retirements=4");
    }
}

#[test]
fn every_switch_and_table_failure_keeps_all_backing_owned_before_and_after_kernel_effects() {
    let boundaries = [Call::Invalidate, Call::RemoveAs, Call::NewAs, Call::Configure,
        Call::Table(3), Call::Table(2), Call::Table(1), Call::Table(0)];
    for after in [false, true] { for (failed, &event) in boundaries.iter().enumerate() {
        let mut a = Alpha::new(); for slot in 0..513 { a.page(slot, 1000 + slot, 3); }
        CALLS.with_borrow_mut(|calls| { calls.fail = Some(event); calls.fail_after = after; });
        assert_eq!(a.retire(), Err(CapabilityError::Fatal));
        assert_eq!(a.processes.vm_space_mut(1).unwrap().backed_pages(0, usize::MAX).len(), 513);
        assert_eq!(a.processes.releasable_physical_allocations_for_pid(1).len(), 513);
        CALLS.with_borrow(|calls| {
            assert_eq!(calls.events.len(), failed + 1);
            assert!(calls.freed.is_empty()); assert_eq!(calls.caps.len(), 129);
        });
        CALLS.with_borrow_mut(|calls| calls.fail = None);
        if failed >= 4 { a.memory.release_page_tables(204).unwrap(); }
    }}
}

#[test]
fn uncertain_old_ptes_are_retired_by_the_same_unreachable_table_fence() {
    let mut a = Alpha::new(); a.page(0, 100, 3);
    a.processes.vm_space_mut(1).unwrap().mark_mapping_uncertain(PAGE_SIZE);
    assert_eq!(a.processes.vm_space_mut(1).unwrap().permissions(PAGE_SIZE), 0);
    a.retire().unwrap(); CALLS.with_borrow(|calls| assert_eq!(calls.freed.len(), 1));
}

#[test]
fn active_and_deferred_aliases_release_once_but_never_free_a_peers_backing() {
    for peer in [false, true] {
        let mut a = Alpha::new(); a.page(0, 77, 1); a.page(1, 77, 0);
        a.processes.defer_active_physical_allocation(1, 8192, 1).unwrap();
        a.processes.vm_space_mut(1).unwrap().forget_frame(8192);
        if peer { a.processes.register_physical_allocation(2, 4096, 0, 77, 1).unwrap(); }
        a.retire().unwrap();
        CALLS.with_borrow(|calls| assert_eq!(calls.freed.len(), usize::from(!peer)));
        assert_eq!(a.processes.deferred_physical_allocations_for_pid(1).count(), 0);
        if peer { assert_eq!(a.processes.releasable_physical_allocations_for_pid(2).len(), 1); }
    }
}

#[test]
fn chunk_failures_after_the_space_fence_still_keep_all_ram_owned_for_reap() {
    for after in [false, true] { for chunk in [0, 1] {
        let mut a = Alpha::new(); a.page(0, 100, 3); a.page(4, 104, 3);
        CALLS.with_borrow_mut(|calls| { calls.fail = Some(Call::Revoke(chunk)); calls.fail_after = after; });
        assert_eq!(a.retire(), Err(CapabilityError::Fatal));
        assert_eq!(a.processes.releasable_physical_allocations_for_pid(1).len(), 2);
        CALLS.with_borrow(|calls| assert!(calls.freed.is_empty()));
    }}
}

#[test]
fn failed_ram_release_commits_prefix_and_retains_suffix_without_double_free() {
    let mut a = Alpha::new(); for slot in 0..3 { a.page(slot, 100 + slot, 3); }
    CALLS.with_borrow_mut(|calls| calls.fail_free = Some(1));
    assert_eq!(a.retire(), Err(CapabilityError::Fatal));
    assert_eq!(a.processes.deferred_physical_allocations_for_pid(1).count(), 2);
    assert!(a.processes.releasable_physical_allocations_for_pid(1).is_empty());
    CALLS.with_borrow_mut(|calls| calls.fail_free = None);
    a.free_deferred_process_allocations(1, None).unwrap();
    CALLS.with_borrow(|calls| assert_eq!(calls.freed.len(), 3));
}

#[test]
fn only_sealed_exclusive_vms_can_use_bulk_exec_retirement() {
    let mut a = Alpha::new(); a.processes.share_address_space(2, 1).unwrap();
    assert_eq!(a.retire(), Err(CapabilityError::IllegalOperation));
    CALLS.with_borrow(|calls| assert!(calls.events.is_empty()));
}
