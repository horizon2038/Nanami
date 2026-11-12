//! Actual shared-exec transaction, with Kernel calls faulted at every boundary.
use super::*;
use a9n_abi::CapabilityType;
use std::cell::RefCell;
const PROCESS_SLOT_ADDRESS_SPACE: usize = 2;
const PROCESS_SLOT_L3_NODE: usize = 3;
const PROCESS_SLOT_L2_NODE: usize = 4;
const PROCESS_SLOT_L1_NODE: usize = 5;
const PROCESS_SLOT_FRAME_NODE: usize = 6;
const PAGE_TABLE_NODE_RADIX: usize = 8;
const PROCESS_FRAME_DIRECTORY_RADIX: usize = 8;
const OS_REQUEST_PROCESS_REAP: usize = 99;
struct OsRequestEvent { identifier: usize, code: usize, arg0: usize, arg1: usize, arg2: usize, arg3: usize }
#[derive(Debug, PartialEq, Eq)]
enum Call { Invalidate(usize), Node(usize), Remove(usize), AddressSpace, Configure, Tables(usize), Reap(usize) }
#[derive(Default)]
struct Kernel { calls: Vec<Call>, fail: Option<usize> }
thread_local! { static KERNEL: RefCell<Kernel> = RefCell::new(Kernel::default()); }
fn call(event: Call) -> Result<(), CapabilityError> {
    KERNEL.with(|k| {
        let mut k = k.borrow_mut();
        let index = k.calls.len(); k.calls.push(event);
        if k.fail == Some(index) { Err(CapabilityError::Fatal) } else { Ok(()) }
    })
}
mod arch {
    use super::*;
    pub mod node {
        use super::*;
        pub fn remove(root: usize, slot: usize) -> Result<(), CapabilityError> {
            assert_eq!(root, 203); call(Call::Remove(slot))
        }
    }
    pub mod generic {
        use super::*;
        pub fn convert(arena: usize, kind: CapabilityType, _: usize, count: usize, root: usize, slot: usize)
            -> Result<(), CapabilityError> {
            assert_eq!((arena, count, root), (999, 1, 203));
            call(if matches!(kind, CapabilityType::Node) { Call::Node(slot) } else {
                assert!(matches!(kind, CapabilityType::AddressSpace));
                assert_eq!(slot, PROCESS_SLOT_ADDRESS_SPACE); Call::AddressSpace
            })
        }
    }
    pub mod process_control_block {
        use super::*;
        pub fn configure(pcb: usize, _: capability_call::process_control_block::ConfigurationInfo,
            space: usize, _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
            _: usize, _: usize, _: usize) -> Result<(), CapabilityError> {
            assert_eq!((pcb, space), (204, 205)); call(Call::Configure)
        }
    }
}
struct Memory;
impl Memory {
    fn release_page_tables(&mut self, space: usize) -> Result<(), CapabilityError> {
        // The parent's AS descriptor (204) must never be retired here.
        assert_eq!(space, 205); call(Call::Tables(space))
    }
}
struct Alpha { processes: ProcessManager, memory: Memory }
impl Alpha {
    fn new(fail: Option<usize>) -> Self {
        KERNEL.with(|k| *k.borrow_mut() = Kernel { fail, ..Kernel::default() });
        let mut processes = manager();
        install(&mut processes, 1, 201); install(&mut processes, 2, 202);
        processes.ensure_vm_space_for_pid(1).unwrap();
        processes.vm_space_mut(1).unwrap().record_frame(0x1000, 700).unwrap();
        processes.vm_space_mut(1).unwrap().set_permissions(0x1000, 3);
        processes.register_physical_allocation(1, 0x1000, 0, 800, 1).unwrap();
        processes.seal_capabilities(2).unwrap();
        *processes.root_types_mut(2).unwrap() = nanami_core::process::RootSlotTypes::empty();
        processes.share_address_space(2, 1).unwrap();
        Self { processes, memory: Memory }
    }
    fn process_arena_for_root(&self, root: usize) -> Result<usize, CapabilityError> {
        assert_eq!(root, 203); Ok(999)
    }
    fn invalidate_process_copy_mappings(&mut self, pid: usize) -> Result<(), CapabilityError> {
        assert_eq!(self.processes.memory_owner_pid(pid), 1); call(Call::Invalidate(pid))
    }
    fn handle_process_reap_request(&mut self, request: OsRequestEvent) -> Result<(), CapabilityError> {
        assert_eq!(request.arg0, 1);
        assert!(!self.processes.has_memory_peers(1));
        call(Call::Reap(request.arg0))
    }
    fn check_parent(&mut self) {
        assert_eq!(self.processes.memory_owner_pid(1), 1);
        assert_eq!(self.processes.vm_space_mut(1).unwrap().find_frame(0x1000), Some(700));
        assert_eq!(self.processes.vm_space_mut(1).unwrap().permissions(0x1000), 3);
        assert!(self.processes.find_active_physical_allocation_reference(1, 0x1000, 1).is_some());
    }
}
#[path = "../../nanami/src/nanami_core/alpha/exec_space.rs"]
mod production;

#[test]
fn shared_exec_changes_only_the_child_after_configuration_and_all_failure_prefixes_keep_parent_ram() {
    for fail in (0..9).map(Some).chain([None]) {
        let mut a = Alpha::new(fail);
        let target = a.processes.find_entry_by_pid(2).unwrap();
        a.prepare_shared_exec(target).unwrap();
        assert_eq!(a.processes.memory_owner_pid(2), 1);
        let result = a.detach_shared_exec(target);
        a.check_parent();
        if fail.is_some() {
            assert_eq!(result, Err(CapabilityError::Fatal));
            assert_eq!(a.processes.memory_owner_pid(2), 1);
        } else {
            result.unwrap();
            assert_eq!(a.processes.memory_owner_pid(2), 2);
            assert!(a.processes.vm_space_mut(2).unwrap().find_frame(0x1000).is_none());
            assert!(!a.processes.has_memory_peers(1));
            a.processes.reset_runtime_memory_for_exec(2, 50, 0x200000, 0x400000).unwrap();
            a.check_parent();
            KERNEL.with(|k| assert_eq!(k.borrow().calls, [Call::Invalidate(2), Call::Node(3),
                Call::Node(4), Call::Node(5), Call::Node(6), Call::Remove(2),
                Call::AddressSpace, Call::Configure, Call::Tables(205)]));
        }
    }
}

#[test]
fn shared_exec_rejects_native_or_populated_private_roots_before_kernel_calls() {
    for case in 0..4 {
        let mut a = Alpha::new(None);
        match case {
            0 => a.processes.root_types_mut(2).unwrap().other(PROCESS_SLOT_L1_NODE),
            1 => { let entry = a.processes.find_entry_by_pid(1).unwrap();
                assert_eq!(a.prepare_shared_exec(entry), Err(CapabilityError::IllegalOperation)); continue; }
            2 => { a.processes.commit_private_exec(2).unwrap_err();
                a.processes.ensure_vm_space_for_pid(2).unwrap();
                a.processes.commit_private_exec(2).unwrap();
                a.processes.register_physical_allocation(2, 0x2000, 0, 900, 1).unwrap();
                a.processes.share_address_space(2, 1).unwrap(); }
            _ => { a.processes.ensure_vm_space_for_pid(2).unwrap();
                a.processes.commit_private_exec(2).unwrap(); }
        }
        let target = a.processes.find_entry_by_pid(2).unwrap();
        assert_eq!(a.prepare_shared_exec(target), Err(CapabilityError::IllegalOperation));
        a.check_parent();
        KERNEL.with(|k| assert!(k.borrow().calls.is_empty()));
    }
}

#[test]
fn last_detaching_child_releases_a_deferred_owner_only_after_switching_away() {
    let mut a = Alpha::new(None);
    a.processes.defer_vm_reap(1);
    let target = a.processes.find_entry_by_pid(2).unwrap();
    a.prepare_shared_exec(target).unwrap(); a.detach_shared_exec(target).unwrap();
    KERNEL.with(|k| assert_eq!(k.borrow().calls.last(), Some(&Call::Reap(1))));
}
