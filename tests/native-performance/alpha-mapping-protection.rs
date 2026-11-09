//! Actual protection and release handlers share real ownership/VM metadata.
//! Only capability calls are mocked, including failure after PTE mutation.
use super::*;
use nanami_core::process::{ProcessEntry, SharedMemoryReservation};
use std::{cell::RefCell, collections::BTreeMap};

const PAGE_SIZE: usize = 4096;
const PROCESS_FRAME_TOTAL_PAGES: usize = 524288;
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
struct Memory;
impl Memory {
    fn physical_frame_descriptor_from_index(&self, page: usize) -> Option<usize> { Some(1_000_000_000 + page) }
}
struct Alpha {
    memory: Memory,
    processes: ProcessManager,
    invalidations: usize,
    fail_invalidation: bool,
    fail_zero_construction: bool,
}
impl Alpha {
    fn new(permissions: usize) -> Self {
        KERNEL.with(|kernel| *kernel.borrow_mut() = Kernel::default());
        let mut processes = manager();
        install(&mut processes, 1, 201);
        processes.ensure_vm_space_for_pid(1).unwrap();
        let mut alpha = Self { memory: Memory, processes, invalidations:0, fail_invalidation:false, fail_zero_construction:false };
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
    fn map_process_initialized_pages_with_stack_slot(
        &mut self, pid: usize, base: Option<usize>, count: usize, source: Option<()>, permissions: usize,
        reserved: Option<crate::nanami_core::process::FreshZeroFrameSlot>,
    ) -> Result<usize, CapabilityError> {
        assert_eq!(count, 1);
        assert!(source.is_none());
        if self.fail_zero_construction { return Err(CapabilityError::Fatal); }
        let base = base.unwrap();
        if let Some(reserved) = reserved {
            let owner = self.processes.memory_owner_pid(pid);
            let (root, space, slot) = reserved.into_parts(owner, base).unwrap();
            let entry = self.processes.memory_entry(pid).unwrap();
            assert_eq!((root, space), (entry.root_node, entry.address_space));
            self.processes.register_physical_allocation(pid, base, slot, 1000 + slot, 1).unwrap();
            let frame = process_frame_descriptor(root, slot);
            let vm = self.processes.vm_space_mut(pid).unwrap();
            vm.record_frame(base, frame).unwrap();
            vm.set_permissions(base, permissions);
            KERNEL.with_borrow_mut(|k| {k.ptes.insert((space, base), (frame, permissions));});
        } else { self.page(pid, base, permissions); }
        Ok(base)
    }
    fn invalidate_process_copy_mappings(&mut self, _: usize) -> Result<(), CapabilityError> {
        self.invalidations+=1;
        if self.fail_invalidation {Err(CapabilityError::Fatal)} else {Ok(())}
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
fn bulk_zero_protection_keeps_cold_pages_absent_until_single_page_demand() {
    for rights in [1, 2, 3, 4, 5, 6, 7] {
        let mut a = Alpha::new(7);
        a.processes.register_lazy_mapping(1, VA, 64, 700,
            crate::nanami_core::process::ProcessLazyMappingKind::Zero).unwrap();
        a.fail_zero_construction = true;
        let request = OsRequestEvent {identifier:1,arg0:1,arg1:VA,arg2:64*PAGE_SIZE,arg3:rights};
        a.handle_mapping_protect_request(request).unwrap();
        for page in 1..64 {
            let vm=a.processes.vm_space_mut(1).unwrap();
            assert!(vm.find_frame(VA+page*PAGE_SIZE).is_none());
            assert_eq!(vm.permissions(VA+page*PAGE_SIZE),rights);
            assert!(a.processes.find_active_physical_allocation_reference(1,VA+page*PAGE_SIZE,1).is_none());
        }
        let fault=OsRequestEvent {arg1:VA+PAGE_SIZE,arg2:PAGE_SIZE,..request};
        assert_eq!(a.handle_mapping_protect_request(fault),Err(CapabilityError::Fatal));
        a.fail_zero_construction=false;
        a.handle_mapping_protect_request(fault).unwrap();
        assert_eq!(a.processes.vm_space_mut(1).unwrap().permissions(VA+PAGE_SIZE),rights);
        assert!(a.processes.vm_space_mut(1).unwrap().find_frame(VA+PAGE_SIZE).is_some());
    }
}

#[test]
fn sparse_zero_rights_rollback_and_invalidation_failure_keep_original_metadata() {
    for fail_invalidation in [false,true] {
        let mut a=Alpha::new(7);a.page(1,VA+2*PAGE_SIZE,7);
        a.processes.register_lazy_mapping(1,VA,3,700,
            crate::nanami_core::process::ProcessLazyMappingKind::Zero).unwrap();
        a.fail_zero_construction=true;a.fail_invalidation=fail_invalidation;
        if !fail_invalidation {KERNEL.with_borrow_mut(|k| {k.map_failure=Some(Failure::After);k.map_failure_at=Some(k.maps+1);});}
        let request=OsRequestEvent {identifier:1,arg0:1,arg1:VA,arg2:3*PAGE_SIZE,arg3:3};
        assert!(a.handle_mapping_protect_request(request).is_err());
        for page in 0..3 {
            let vm=a.processes.vm_space_mut(1).unwrap();assert_eq!(vm.permissions(VA+page*PAGE_SIZE),7);
        }
        assert!(a.processes.vm_space_mut(1).unwrap().find_frame(VA+PAGE_SIZE).is_none());
    }
}

#[test]
fn zero_lazy_protection_uses_initialized_final_rights_and_keeps_constructor_failure_unpublished() {
    for permissions in [1, 2, 3, 4, 5, 6, 7] {
        let mut a = Alpha::new(0);
        a.release().unwrap();
        let counts = KERNEL.with_borrow(|k| (k.maps, k.unmaps));
        a.processes.register_lazy_mapping(1, VA, 1, 700,
            crate::nanami_core::process::ProcessLazyMappingKind::Zero).unwrap();
        a.fail_zero_construction = true;
        assert_eq!(a.protect(permissions), Err(CapabilityError::Fatal));
        assert!(!a.has_pte());
        assert!(!a.owns_page());
        a.fail_zero_construction = false;
        a.protect(permissions).unwrap();
        assert!(a.owns_page());
        KERNEL.with_borrow(|k| {
            assert!(k.ptes.values().all(|(_, rights)| *rights == permissions));
            assert_eq!((k.maps, k.unmaps), counts);
        });
    }
}

#[test]
fn unchanged_backed_range_preserves_cleanup_failure_and_never_changes_user_ptes() {
    for permissions in 0..=7 {
        let mut a=Alpha::new(permissions);
        for offset in 1..64 {a.page(1,VA+offset*PAGE_SIZE,permissions);}
        let request=OsRequestEvent {identifier:1,arg0:1,arg1:VA,arg2:64*PAGE_SIZE,arg3:permissions};
        PROTECT_ALLOCATIONS.with(|count|count.set(Some(0)));
        let result=a.handle_mapping_protect_request(request);
        let allocations=PROTECT_ALLOCATIONS.with(|count|count.replace(None).unwrap());
        assert_eq!(result,Ok(()));assert_eq!(allocations,0);assert_eq!(a.invalidations,1);
        a.fail_invalidation=true;
        PROTECT_ALLOCATIONS.with(|count|count.set(Some(0)));
        let result=a.handle_mapping_protect_request(request);
        let allocations=PROTECT_ALLOCATIONS.with(|count|count.replace(None).unwrap());
        assert_eq!(result,Err(CapabilityError::Fatal));assert_eq!(allocations,0);
        assert_eq!(a.invalidations,2);assert!(a.owns_page());
        KERNEL.with_borrow(|k|{assert_eq!((k.maps,k.unmaps),(0,0));assert!(k.ptes.values().all(|(_,r)|*r==permissions));});
    }
}

#[test]
fn unchanged_none_range_with_unbacked_pages_preserves_cleanup_and_ownership() {
    for mixed in [false, true] {
        let mut a = Alpha::new(0);
        for page in 1..512 {
            let va = VA + page * PAGE_SIZE;
            if mixed && page % 17 == 0 { a.page(1, va, 0); }
            else { a.processes.vm_space_mut(1).unwrap().set_permissions(va, 0); }
        }
        let request = OsRequestEvent { identifier: 1, arg0: 1, arg1: VA, arg2: 512 * PAGE_SIZE, arg3: 0 };
        for fail in [false, true] {
            a.fail_invalidation = fail;
            protection::take_protection_page_visits();
            PROTECT_ALLOCATIONS.with(|count| count.set(Some(0)));
            let result = a.handle_mapping_protect_request(request);
            let allocations = PROTECT_ALLOCATIONS.with(|count| count.replace(None).unwrap());
            let visits = protection::take_protection_page_visits();
            assert_eq!(result, if fail { Err(CapabilityError::Fatal) } else { Ok(()) });
            println!("NONE-PROTECT mixed={mixed} cleanup_failure={fail} page_visits={visits}");
            assert_eq!(visits, 0);
            assert_eq!(allocations, 0);
            for page in 0..512 {
                let vm = a.processes.vm_space_mut(1).unwrap();
                let va = VA + page * PAGE_SIZE;
                assert_eq!(vm.permissions(va), 0);
                assert_eq!(vm.find_frame(va).is_some(), page == 0 || (mixed && page % 17 == 0));
            }
            KERNEL.with_borrow(|k| { assert!(k.ptes.is_empty()); assert_eq!((k.maps, k.unmaps), (0, 0)); });
        }
        assert_eq!(a.invalidations, 2);
    }
}

#[test]
fn backed_protection_ranges_keep_preflight_failure_and_large_prefix_rollback() {
    let request = OsRequestEvent { identifier: 1, arg0: 1, arg1: VA, arg2: 64 * PAGE_SIZE, arg3: 1 };
    let mut a = Alpha::new(3);
    for page in 1..64 { a.page(1, VA + page * PAGE_SIZE, 3); }
    a.fail_invalidation = true;
    protection::take_protection_page_visits();
    PROTECT_ALLOCATIONS.with(|count| count.set(Some(0)));
    let result = a.handle_mapping_protect_request(request);
    let allocations = PROTECT_ALLOCATIONS.with(|count| count.replace(None).unwrap());
    let visits = protection::take_protection_page_visits();
    assert_eq!(result, Err(CapabilityError::Fatal));
    println!("BACKED-PROTECT pages=64 point_loop={visits} preflight_allocations={allocations}");
    assert_eq!(visits, 0);
    assert_eq!(allocations, 1);
    for page in 0..64 { assert_eq!(a.processes.vm_space_mut(1).unwrap().permissions(VA + page * PAGE_SIZE), 3); }
    KERNEL.with_borrow(|k| { assert_eq!((k.maps, k.unmaps), (0, 0)); assert_eq!(k.ptes.len(), 64); });

    for failure in [Failure::Before, Failure::After] {
        for index in [0, 31, 63] {
            let mut a = Alpha::new(3);
            for page in 1..64 { a.page(1, VA + page * PAGE_SIZE, 3); }
            KERNEL.with_borrow_mut(|k| { k.map_failure = Some(failure); k.map_failure_at = Some(index); });
            protection::take_protection_vm_resolutions();
            assert_eq!(a.handle_mapping_protect_request(request), Err(CapabilityError::Fatal));
            assert_eq!(protection::take_protection_vm_resolutions(), 1);
            for page in 0..64 {
                let va = VA + page * PAGE_SIZE;
                let vm = a.processes.vm_space_mut(1).unwrap();
                assert_eq!(vm.permissions(va), 3);
                assert!(!vm.mapping_is_uncertain(va));
                assert!(vm.find_frame(va).is_some());
                assert!(a.processes.find_active_physical_allocation_reference(1, va, 1).is_some());
            }
            KERNEL.with_borrow(|k| { assert_eq!(k.ptes.len(), 64); assert!(k.ptes.values().all(|(_, rights)| *rights == 3)); });
        }
    }
    let mut a = Alpha::new(3);
    for page in 1..64 { a.page(1, VA + page * PAGE_SIZE, 3); }
    a.processes.vm_space_mut(1).unwrap().mark_mapping_uncertain(VA + 17 * PAGE_SIZE);
    protection::take_protection_vm_resolutions();
    assert_eq!(a.handle_mapping_protect_request(request), Ok(()));
    let resolutions = protection::take_protection_vm_resolutions();
    println!("PROTECT-UPDATE pages=64 vm_resolutions={resolutions}");
    assert_eq!(resolutions, 1);
    let vm = a.processes.vm_space_mut(1).unwrap();
    assert!(!vm.has_uncertain_mappings(VA, VA + 64 * PAGE_SIZE));
    for page in 0..64 { assert_eq!(vm.permissions(VA + page * PAGE_SIZE), 1); }
    KERNEL.with_borrow(|k| { assert_eq!(k.ptes.len(), 64); assert!(k.ptes.values().all(|(_, rights)| *rights == 1)); });
    let mut a = Alpha::new(1);
    for page in 1..64 { a.page(1, VA + page * PAGE_SIZE, 1); }
    a.processes.vm_space_mut(1).unwrap().mark_private_page_shared(VA + 32 * PAGE_SIZE, true);
    protection::take_protection_vm_resolutions();
    assert_eq!(a.handle_mapping_protect_request(OsRequestEvent { arg3: 3, ..request }), Err(CapabilityError::IllegalOperation));
    assert_eq!(protection::take_protection_vm_resolutions(), 1);
    let vm = a.processes.vm_space_mut(1).unwrap();
    assert!(vm.private_page_shared(VA + 32 * PAGE_SIZE));
    assert!(!vm.has_uncertain_mappings(VA, VA + 64 * PAGE_SIZE));
    for page in 0..64 { assert_eq!(vm.permissions(VA + page * PAGE_SIZE), 1); }
    KERNEL.with_borrow(|k| { assert_eq!(k.ptes.len(), 64); assert!(k.ptes.values().all(|(_, rights)| *rights == 1)); });
    let mut a = Alpha::new(3);
    let empty_base = VA + 128 * PAGE_SIZE;
    protection::take_protection_vm_resolutions();
    assert_eq!(a.handle_mapping_protect_request(OsRequestEvent { arg1: empty_base, arg3: 0, ..request }), Ok(()));
    assert_eq!(protection::take_protection_vm_resolutions(), 0);
    assert_eq!(a.invalidations, 1);
    let vm = a.processes.vm_space_mut(1).unwrap();
    assert_eq!(vm.permissions(VA), 3);
    assert!(vm.find_frame(empty_base).is_none());
    assert_eq!(vm.permissions(empty_base), 7);
}

#[test]
fn equal_rights_do_not_bypass_cow_write_or_uncertain_unmap_guards() {
    let mut a=Alpha::new(3);a.processes.vm_space_mut(1).unwrap().mark_private_page_shared(VA,true);
    assert!(a.protect(3).is_err());assert!(a.owns_page());
    let mut a=Alpha::new(1);a.processes.vm_space_mut(1).unwrap().mark_private_page_shared(VA,true);
    assert_eq!(a.protect(1),Ok(()));
    let mut a=Alpha::new(0);a.processes.vm_space_mut(1).unwrap().mark_mapping_uncertain(VA);
    KERNEL.with_borrow_mut(|k|k.unmap_failure=Some(Failure::Before));
    assert!(a.protect(0).is_err());assert!(a.owns_page());
    assert!(a.processes.vm_space_mut(1).unwrap().mapping_is_uncertain(VA));
    KERNEL.with_borrow(|k|assert!(k.unmaps>0));
}

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
fn one_protection_transaction_restores_heterogeneous_rights_on_every_failure_page() {
    for after in [false, true] {
        for failed_page in 0..64 {
            let rights = |page: usize| [0, 1, 3, 5][page % 4];
            let mut a = Alpha::new(rights(0));
            for page in 1..64 { a.page(1, VA + page * PAGE_SIZE, rights(page)); }
            let before = KERNEL.with_borrow(|kernel| kernel.ptes.clone());
            KERNEL.with_borrow_mut(|kernel| {
                kernel.map_failure = Some(if after { Failure::After } else { Failure::Before });
                kernel.map_failure_at = Some(failed_page);
            });
            assert_eq!(a.handle_mapping_protect_request(OsRequestEvent {
                identifier: 1, arg0: 1, arg1: VA, arg2: 64 * PAGE_SIZE, arg3: 7,
            }), Err(CapabilityError::Fatal));
            KERNEL.with_borrow(|kernel| assert_eq!(kernel.ptes, before));
            for page in 0..64 {
                let va = VA + page * PAGE_SIZE;
                let vm = a.processes.vm_space_mut(1).unwrap();
                assert_eq!(vm.permissions(va), rights(page));
                assert!(!vm.mapping_is_uncertain(va));
                assert!(vm.find_frame(va).is_some());
                assert!(a.processes.find_active_physical_allocation_reference(1, va, 1).is_some());
            }
        }
    }
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
