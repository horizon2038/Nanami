//! Actual protection and release handlers share real ownership/VM metadata.
//! Only capability calls are mocked, including failure after PTE mutation.
use super::*;
use nanami_core::process::{ProcessEntry, SharedMemoryReservation};
use std::{cell::RefCell, collections::BTreeMap};

const PAGE_SIZE: usize = 4096;
const VA: usize = 0x10000;
#[derive(Clone, Copy)]
struct OsRequestEvent {
    identifier: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
}
#[derive(Clone, Copy)]
enum Failure {
    Before,
    After,
}
#[derive(Default)]
struct Kernel {
    ptes: BTreeMap<(usize, usize), (usize, usize)>,
    map_failure: Option<Failure>,
    map_failure_at: Option<usize>,
    unmap_failure: Option<Failure>,
    missing_table: bool,
    maps: usize,
    unmaps: usize,
}
thread_local! { static KERNEL: RefCell<Kernel> = RefCell::new(Kernel::default()); }
mod arch {
    pub mod address_space {
        use super::super::*;
        pub fn map(
            space: usize,
            frame: usize,
            va: usize,
            attribute: capability_call::address_space::Attribute,
        ) -> Result<(), CapabilityError> {
            KERNEL.with(|kernel| {
                let mut kernel = kernel.borrow_mut();
                let attempt = kernel.maps;
                kernel.maps += 1;
                let failure = if kernel.map_failure_at.is_none_or(|index| index == attempt) {
                    kernel.map_failure.take()
                } else {
                    None
                };
                if matches!(failure, Some(Failure::Before)) {
                    return Err(CapabilityError::Fatal);
                }
                if kernel.ptes.contains_key(&(space, va)) {
                    return Err(CapabilityError::IllegalOperation);
                }
                kernel.ptes.insert((space, va), (frame, attribute.bits()));
                if matches!(failure, Some(Failure::After)) {
                    return Err(CapabilityError::Fatal);
                }
                Ok(())
            })
        }
        pub fn unmap(space: usize, frame: usize, va: usize) -> Result<(), CapabilityError> {
            KERNEL.with(|kernel| {
                let mut kernel = kernel.borrow_mut();
                kernel.unmaps += 1;
                if kernel.missing_table {
                    return Err(CapabilityError::InvalidArgument);
                }
                if matches!(kernel.unmap_failure, Some(Failure::Before)) {
                    return Err(CapabilityError::Fatal);
                }
                // Empty-leaf UNMAP succeeds in both current HALs; Kernel still
                // performs fallible remote shootdown after that success.
                if let Some(pte) = kernel.ptes.get(&(space, va)) {
                    assert_eq!(pte.0, frame);
                }
                kernel.ptes.remove(&(space, va));
                if matches!(kernel.unmap_failure, Some(Failure::After)) {
                    return Err(CapabilityError::Fatal);
                }
                Ok(())
            })
        }
    }
}
fn process_frame_descriptor(root: usize, slot: usize) -> usize {
    root * 100000 + slot
}
struct Alpha {
    processes: ProcessManager,
}
impl Alpha {
    fn new(permissions: usize) -> Self {
        KERNEL.with(|kernel| *kernel.borrow_mut() = Kernel::default());
        let mut processes = manager();
        install(&mut processes, 1, 201);
        processes.ensure_vm_space_for_pid(1).unwrap();
        let mut alpha = Self { processes };
        alpha.page(1, VA, permissions);
        alpha
    }
    fn page(&mut self, pid: usize, va: usize, permissions: usize) {
        let (root, space, slot) = self
            .processes
            .reserve_process_heap_at(pid, va, 1, PAGE_SIZE, 1 << 20)
            .unwrap();
        self.processes
            .register_physical_allocation(pid, va, slot, 1000 + slot, 1)
            .unwrap();
        let frame = process_frame_descriptor(root, slot);
        let vm = self.processes.vm_space_mut(pid).unwrap();
        vm.record_frame(va, frame).unwrap();
        vm.set_permissions(va, permissions);
        if permissions != 0 {
            KERNEL.with(|kernel| {
                kernel
                    .borrow_mut()
                    .ptes
                    .insert((space, va), (frame, permissions));
            });
        }
    }
    fn map_process_heap_pages_at(
        &mut self,
        pid: usize,
        va: usize,
        count: usize,
    ) -> Result<usize, CapabilityError> {
        assert_eq!(count, 1);
        self.page(pid, va, 7);
        Ok(va)
    }
    fn process_frame_for_page(&mut self, pid: usize, va: usize) -> Result<usize, CapabilityError> {
        self.processes
            .vm_space_mut(pid)
            .and_then(|vm| vm.find_frame(va))
            .ok_or(CapabilityError::InvalidArgument)
    }
    fn invalidate_process_copy_mappings(&mut self, _: usize) -> Result<(), CapabilityError> {
        Ok(())
    }
    fn release_shared_memory(
        &mut self,
        _: ProcessEntry,
        _: SharedMemoryReservation,
    ) -> Result<(), CapabilityError> {
        panic!("these tests use private pages, not shared reservations");
    }
    fn protect(&mut self, permissions: usize) -> Result<(), CapabilityError> {
        self.handle_mapping_protect_request(OsRequestEvent {
            identifier: 1,
            arg0: 1,
            arg1: VA,
            arg2: PAGE_SIZE,
            arg3: permissions,
        })
    }
    fn release(&mut self) -> Result<(), CapabilityError> {
        self.handle_mapping_release_request(OsRequestEvent {
            identifier: 1,
            arg0: VA,
            arg1: PAGE_SIZE,
            arg2: 1,
            arg3: nanami_uapi::os::OS_MAPPING_RELEASE_ALLOW_HOLES,
        })
    }
    fn has_pte(&self) -> bool {
        let space = self.processes.memory_entry(1).unwrap().address_space;
        KERNEL.with(|kernel| kernel.borrow().ptes.contains_key(&(space, VA)))
    }
    fn owns_page(&self) -> bool {
        self.processes
            .find_active_physical_allocation_reference(1, VA, 1)
            .is_some()
    }
}
#[path = "../../nanami/src/nanami_core/alpha/mapping_protection.rs"]
mod protection;
#[path = "../../nanami/src/nanami_core/alpha/mapping_release.rs"]
mod release;

#[test]
fn normal_permission_transitions_and_confirmed_release_keep_original_semantics() {
    let mut alpha = Alpha::new(0);
    for permissions in [3, 1, 5, 0, 3, 0] {
        assert_eq!(alpha.protect(permissions), Ok(()));
        assert_eq!(alpha.has_pte(), permissions != 0);
        assert_eq!(
            alpha.processes.vm_space_mut(1).unwrap().permissions(VA),
            permissions
        );
        assert!(alpha.owns_page());
    }
    assert_eq!(alpha.release(), Ok(()));
    assert!(!alpha.has_pte());
    assert!(!alpha.owns_page());
}

#[test]
fn failed_map_after_install_must_unmap_before_backing_can_be_deferred() {
    let mut alpha = Alpha::new(0);
    KERNEL.with(|kernel| kernel.borrow_mut().map_failure = Some(Failure::After));
    assert_eq!(alpha.protect(3), Err(CapabilityError::Fatal));
    assert!(alpha.owns_page());
    assert_eq!(alpha.release(), Ok(()));
    assert!(
        !alpha.has_pte(),
        "release skipped a PTE installed by the failed MAP"
    );
    assert!(!alpha.owns_page());
}

#[test]
fn failed_map_and_unconfirmed_cleanup_cannot_turn_into_a_successful_none_noop() {
    let mut alpha = Alpha::new(0);
    KERNEL.with(|kernel| {
        let mut kernel = kernel.borrow_mut();
        kernel.map_failure = Some(Failure::After);
        kernel.unmap_failure = Some(Failure::Before);
    });
    assert_eq!(alpha.protect(3), Err(CapabilityError::Fatal));
    assert!(
        alpha.protect(0).is_err(),
        "uncertain MAP must not be treated as absent"
    );
    assert!(alpha.release().is_err());
    assert!(alpha.owns_page());
    assert!(alpha.has_pte());
}

#[test]
fn failed_unmap_after_removal_cannot_turn_into_a_successful_remap_noop() {
    let mut alpha = Alpha::new(3);
    KERNEL.with(|kernel| kernel.borrow_mut().unmap_failure = Some(Failure::After));
    assert_eq!(alpha.protect(0), Err(CapabilityError::Fatal));
    assert!(!alpha.has_pte());
    assert!(
        alpha.protect(3).is_err(),
        "uncertain UNMAP must not be treated as present"
    );
    assert!(alpha.release().is_err());
    assert!(
        alpha.owns_page(),
        "failed shootdown keeps physical ownership quarantined"
    );
}

#[test]
fn uncertain_mapping_is_shared_with_vm_peers_and_recovers_only_after_confirmed_cleanup() {
    let mut alpha = Alpha::new(0);
    install(&mut alpha.processes, 2, 202);
    alpha.processes.share_address_space(2, 1).unwrap();
    KERNEL.with(|kernel| {
        let mut kernel = kernel.borrow_mut();
        kernel.map_failure = Some(Failure::After);
        kernel.unmap_failure = Some(Failure::Before);
    });
    assert_eq!(alpha.protect(3), Err(CapabilityError::Fatal));
    let peer = alpha.processes.vm_space_mut(2).unwrap();
    assert!(peer.mapping_is_uncertain(VA));
    assert_eq!(peer.permissions(VA), 0);
    peer.set_permissions(VA, 7);
    assert_eq!(
        peer.permissions(VA),
        0,
        "metadata edits do not acknowledge a shootdown"
    );
    assert!(alpha.owns_page());
    KERNEL.with(|kernel| kernel.borrow_mut().unmap_failure = None);
    assert_eq!(alpha.protect(1), Ok(()));
    let peer = alpha.processes.vm_space_mut(2).unwrap();
    assert!(!peer.mapping_is_uncertain(VA));
    assert_eq!(peer.permissions(VA), 1);
    assert!(alpha.has_pte());
    assert_eq!(alpha.release(), Ok(()));
    assert!(!alpha.has_pte());
    assert!(!alpha.owns_page());
    assert_eq!(
        alpha.processes.vm_space_mut(2).unwrap().find_frame(VA),
        None
    );
}

#[test]
fn before_map_failure_is_resolved_by_an_acknowledged_idempotent_unmap() {
    let mut alpha = Alpha::new(0);
    KERNEL.with(|kernel| kernel.borrow_mut().map_failure = Some(Failure::Before));
    assert_eq!(alpha.protect(3), Err(CapabilityError::Fatal));
    assert!(!alpha.has_pte());
    assert!(!alpha
        .processes
        .vm_space_mut(1)
        .unwrap()
        .mapping_is_uncertain(VA));
    assert_eq!(alpha.processes.vm_space_mut(1).unwrap().permissions(VA), 0);
    KERNEL.with(|kernel| assert_eq!(kernel.borrow().unmaps, 1));
    assert!(
        alpha.owns_page(),
        "backing remains active until explicit release"
    );
    assert_eq!(alpha.release(), Ok(()));
    assert!(!alpha.owns_page());
}

#[test]
fn release_failure_marks_the_vm_uncertain_and_never_defers_unconfirmed_backing() {
    for failure in [Failure::Before, Failure::After] {
        let mut alpha = Alpha::new(3);
        KERNEL.with(|kernel| kernel.borrow_mut().unmap_failure = Some(failure));
        assert!(alpha.release().is_err());
        assert!(alpha.owns_page());
        assert_eq!(alpha.processes.vm_space_mut(1).unwrap().permissions(VA), 0);
        assert!(alpha
            .processes
            .vm_space_mut(1)
            .unwrap()
            .frame_may_be_present(VA));
        assert!(alpha
            .processes
            .deferred_physical_allocations_for_pid(1)
            .next()
            .is_none());
        KERNEL.with(|kernel| kernel.borrow_mut().unmap_failure = None);
        assert_eq!(alpha.release(), Ok(()));
        assert!(!alpha.owns_page());
        assert!(!alpha.has_pte());
        KERNEL.with(|kernel| assert_eq!(kernel.borrow().unmaps, 2));
    }
}

#[test]
fn partial_range_failure_rolls_back_prior_pages_and_retains_all_backing() {
    let mut alpha = Alpha::new(0);
    alpha.page(1, VA + PAGE_SIZE, 0);
    KERNEL.with(|kernel| {
        let mut kernel = kernel.borrow_mut();
        kernel.map_failure = Some(Failure::After);
        kernel.map_failure_at = Some(1);
    });
    assert_eq!(
        alpha.handle_mapping_protect_request(OsRequestEvent {
            identifier: 1,
            arg0: 1,
            arg1: VA,
            arg2: PAGE_SIZE * 2,
            arg3: 3,
        }),
        Err(CapabilityError::Fatal)
    );
    KERNEL.with(|kernel| assert!(kernel.borrow().ptes.is_empty()));
    let vm = alpha.processes.vm_space_mut(1).unwrap();
    assert_eq!(vm.permissions(VA), 0);
    assert_eq!(vm.permissions(VA + PAGE_SIZE), 0);
    assert!(!vm.has_uncertain_mappings(VA, VA + PAGE_SIZE * 2));
    assert!(alpha.owns_page());
    assert!(alpha
        .processes
        .find_active_physical_allocation_reference(1, VA + PAGE_SIZE, 1)
        .is_some());
    assert!(alpha
        .processes
        .deferred_physical_allocations_for_pid(1)
        .next()
        .is_none());
}

#[test]
fn materialized_reservation_unmap_failure_is_tracked_before_transaction_preflight_finishes() {
    let mut alpha = Alpha::new(0);
    let address = VA + PAGE_SIZE;
    KERNEL.with(|kernel| kernel.borrow_mut().unmap_failure = Some(Failure::After));
    assert_eq!(
        alpha.handle_mapping_protect_request(OsRequestEvent {
            identifier: 1,
            arg0: 1,
            arg1: address,
            arg2: PAGE_SIZE,
            arg3: 3,
        }),
        Err(CapabilityError::Fatal)
    );
    let vm = alpha.processes.vm_space_mut(1).unwrap();
    assert!(vm.mapping_is_uncertain(address));
    assert_eq!(vm.permissions(address), 0);
    assert!(vm.find_frame(address).is_some());
    assert!(alpha
        .processes
        .find_active_physical_allocation_reference(1, address, 1)
        .is_some());
    assert!(alpha
        .handle_mapping_release_request(OsRequestEvent {
            identifier: 1,
            arg0: address,
            arg1: PAGE_SIZE,
            arg2: 1,
            arg3: nanami_uapi::os::OS_MAPPING_RELEASE_ALLOW_HOLES,
        })
        .is_err());
    assert!(alpha
        .processes
        .find_active_physical_allocation_reference(1, address, 1)
        .is_some());
}

#[test]
fn absent_leaf_is_not_cleanup_evidence_when_table_lookup_or_ack_fails() {
    let mut alpha = Alpha::new(3);
    KERNEL.with(|kernel| kernel.borrow_mut().unmap_failure = Some(Failure::After));
    assert!(alpha.release().is_err());
    KERNEL.with(|kernel| {
        let mut kernel = kernel.borrow_mut();
        kernel.unmap_failure = None;
        kernel.missing_table = true;
    });
    assert!(alpha.release().is_err());
    assert!(alpha.owns_page());
    assert!(alpha
        .processes
        .vm_space_mut(1)
        .unwrap()
        .mapping_is_uncertain(VA));
    KERNEL.with(|kernel| kernel.borrow_mut().missing_table = false);
    assert_eq!(alpha.release(), Ok(()));
    assert!(!alpha.owns_page());
}
