#![allow(dead_code)]
extern crate alloc;
extern crate self as nun;

pub type Word = usize;
pub type CapabilityDescriptor = usize;
pub mod capability_call {
    pub mod address_space {
        pub use a9n_abi::capability_call::address_space::Attribute;
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityError {
    IllegalOperation,
    InvalidArgument,
    PermissionDenied,
    Fatal,
}
#[macro_export]
macro_rules! info {
    ($($args:tt)*) => {};
}
#[macro_export]
macro_rules! error {
    ($($args:tt)*) => {};
}

#[path = "../../nanami/src/nanami_utils/avl.rs"]
pub mod avl;
#[path = "../../nanami/src/nanami_utils/static_avl.rs"]
pub mod static_avl;
pub mod nanami_utils {
    pub use crate::{avl, static_avl};
}
#[path = "../../nanami/src/nanami_core/vm_space.rs"]
pub mod vm_space;
pub mod nanami_core {
    pub use crate::vm_space;
    pub mod elf_loader {
        #[derive(Clone, Copy)]
        pub struct ElfImage;
    }
    pub mod process {
        include!(env!("NANAMI_PROCESS_SOURCE"));
    }
}
use nanami_core::process::ProcessManager;
use vm_space::VmTracker;

#[path = "shared-memory-reservations.rs"]
mod shared_memory_reservations;

#[path = "allocation-index.rs"]
mod allocation_index;
#[path = "shared-memory-lifecycle.rs"]
mod shared_memory_lifecycle;

fn manager() -> ProcessManager {
    ProcessManager::new_alpha(1, 2, 99, 3, 4096, &[], 4).unwrap()
}

#[test]
fn alpha_uses_its_actual_pcb_capability() {
    assert_eq!(manager().alpha_entry().pcb, 99);
}

#[test]
fn tasks_share_vm_and_allocation_metadata_without_duplicating_physical_ownership() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    install(&mut manager, 2, 202);
    manager.ensure_vm_space_for_pid(1).unwrap();
    manager.share_address_space(2, 1).unwrap();
    manager
        .vm_space_mut(2)
        .unwrap()
        .record_frame(0x1000, 0xabcd)
        .unwrap();
    manager.vm_space_mut(2).unwrap().set_permissions(0x1000, 0);
    assert_eq!(
        manager.vm_space_mut(1).unwrap().find_frame(0x1000),
        Some(0xabcd)
    );
    assert_eq!(manager.vm_space_mut(1).unwrap().permissions(0x1000), 0);
    manager
        .register_physical_allocation(2, 0x1000, 7, 100, 1)
        .unwrap();
    assert!(manager
        .find_active_physical_allocation_reference(1, 0x1000, 1)
        .is_some());
    assert_eq!(manager.releasable_physical_allocations_for_pid(1).len(), 1);
    assert_eq!(manager.releasable_physical_allocations_for_pid(2).len(), 0);
    assert!(manager.has_memory_peers(1));
    manager.mark_exited(1, 1, 0).unwrap();
    manager.defer_vm_reap(1);
    assert!(manager.memory_entry(2).unwrap().deferred_vm_reap);
    manager.mark_exited(2, 1, 0).unwrap();
    manager.reap_process(2, true).unwrap();
    assert!(!manager.has_memory_peers(1));
    assert_eq!(
        manager.vm_space_mut(1).unwrap().find_frame(0x1000),
        Some(0xabcd)
    );
}

#[test]
fn sparse_vm_walk_includes_retained_prot_none_backing_not_unpopulated_holes() {
    let mut vm = vm_space::VmSpace::new();
    for va in [0x1000, 0x4000, 0x9000] {
        vm.record_frame(va, va + 1).unwrap();
    }
    vm.set_permissions(0x4000, 0);
    assert_eq!(vm.backed_pages(0x2000, 0x9000), [(0x4000, 0)]);
}

#[test]
fn native_root_provenance_never_treats_kernel_or_other_slots_as_notifications() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    let types = manager.find_entry_by_pid(1).unwrap().root_types;
    assert!(types.is_notification(21));
    assert!(types.is_ipc(20));
    for slot in [0, 1, 2, 3, 7, 22, 40, 64, 255, 256, usize::MAX] {
        assert!(!types.is_notification(slot));
    }
    assert!(manager.root_types_mut(0).is_err());
    assert!(manager.root_types_mut(999).is_err());
}

#[test]
fn root_slot_overwrite_clears_notification_type_without_touching_adjacent_slots() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    let types = manager.root_types_mut(1).unwrap();
    for slot in [31, 32, 63, 64, 127, 128, 255] {
        types.notification(slot);
        assert!(types.is_notification(slot));
        types.other(slot);
        assert!(types.occupied(slot));
        assert!(!types.is_notification(slot));
        types.ipc(slot);
        assert!(types.is_ipc(slot));
        types.clear(slot);
        assert!(!types.occupied(slot));
    }
    assert!(types.is_notification(21));
    assert!(types.is_ipc(20));
    types.notification(256);
    assert!(!types.occupied(256));
    assert_eq!(core::mem::size_of_val(types), 64);
}

#[test]
fn exited_process_cannot_acquire_new_root_provenance() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    manager.mark_exited(1, 1, 0).unwrap();
    assert!(manager.root_types_mut(1).is_err());
}

#[test]
fn foreign_revocation_can_empty_a_known_slot_without_requiring_a_remove() {
    use std::cell::RefCell;
    let operations = RefCell::new(Vec::new());
    for known in [false, true] {
        operations.borrow_mut().clear();
        nanami_core::process::install_notification_alias(
            known,
            || {
                operations.borrow_mut().push("copy");
                Ok(())
            },
            || {
                operations.borrow_mut().push("remove");
                Err(CapabilityError::PermissionDenied)
            },
        )
        .unwrap();
        assert_eq!(*operations.borrow(), ["copy"]);
    }
}

#[test]
fn only_a_known_live_notification_is_removed_before_replacement() {
    use std::cell::RefCell;
    let occupied = RefCell::new(true);
    let operations = RefCell::new(Vec::new());
    nanami_core::process::install_notification_alias(
        true,
        || {
            operations.borrow_mut().push("copy");
            if *occupied.borrow() {
                return Err(CapabilityError::Fatal); // A9N INIT_FIRST conversion.
            }
            *occupied.borrow_mut() = true;
            Ok(())
        },
        || {
            operations.borrow_mut().push("remove");
            *occupied.borrow_mut() = false;
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(*operations.borrow(), ["copy", "remove", "copy"]);
}

#[test]
fn occupied_unverified_or_ipc_slot_is_never_deleted_by_notification_transfer() {
    nanami_core::process::install_notification_alias(
        false,
        || Err(CapabilityError::Fatal),
        || {
            panic!("notification API deleted another capability type");
        },
    )
    .unwrap_err();
}

#[test]
fn failed_notification_withdrawal_is_not_followed_by_another_copy() {
    let mut copies = 0;
    let result = nanami_core::process::install_notification_alias(
        true,
        || {
            copies += 1;
            Err(CapabilityError::Fatal)
        },
        || Err(CapabilityError::PermissionDenied),
    );
    assert_eq!(result, Err(CapabilityError::PermissionDenied));
    assert_eq!(copies, 1);
}

#[test]
fn uncertain_authority_keeps_process_backing_and_metadata_quarantined() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    manager.quarantine_resources(1).unwrap();
    manager.mark_exited(1, 0, 5).unwrap();
    assert!(manager.reap_process(1, true).is_err());
    assert!(manager.find_entry_by_pid(1).unwrap().resource_quarantined);
}

#[test]
fn platform_access_is_per_process_and_defaults_to_denied() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    install(&mut manager, 2, 202);
    assert_eq!(
        manager.entries().map(|entry| entry.pid).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert!(
        !manager
            .find_entry_by_pid(1)
            .unwrap()
            .platform_information_access
    );
    manager.set_platform_information_access(1, true).unwrap();
    assert!(
        manager
            .find_entry_by_pid(1)
            .unwrap()
            .platform_information_access
    );
    assert!(
        !manager
            .find_entry_by_pid(2)
            .unwrap()
            .platform_information_access
    );
    assert!(manager.set_platform_information_access(999, true).is_err());
    manager.set_platform_information_access(1, false).unwrap();
    assert!(
        !manager
            .find_entry_by_pid(1)
            .unwrap()
            .platform_information_access
    );
}

#[test]
fn affinity_diagnostic_requires_explicit_one() {
    assert_eq!(
        nanami_core::process::SAME_CORE_DIAGNOSTIC,
        option_env!("NANAMI_SAME_CORE_DIAGNOSTIC") == Some("1")
    );
}

#[test]
fn affinity_allocation_preserves_topology_and_normal_round_robin() {
    for core_count in [1, 2, 4, 64] {
        let mut manager = ProcessManager::new_alpha(1, 2, 99, 3, 4096, &[], core_count).unwrap();
        for index in 0..core_count * 3 {
            let expected = if nanami_core::process::SAME_CORE_DIAGNOSTIC || core_count == 1 {
                0
            } else {
                1 + index % (core_count - 1)
            };
            assert_eq!(manager.allocate_affinity(), expected);
            assert_eq!(manager.online_core_count(), core_count);
        }
        assert_eq!(manager.alpha_entry().affinity, 0);
    }
}

#[test]
fn active_core_mask_uses_allocated_affinities_not_the_online_count() {
    let mut manager = manager();
    for pid in 1..=6 {
        let affinity = manager.allocate_affinity();
        let root = pid + 200;
        manager
            .install_process(
                pid,
                0,
                root,
                root + 1,
                root + 2,
                root + 3,
                root + 4,
                pid,
                affinity,
                0,
                0x200000,
                0x400000,
            )
            .unwrap();
    }
    let expected = if nanami_core::process::SAME_CORE_DIAGNOSTIC {
        1
    } else {
        0xf
    };
    assert_eq!(manager.active_core_mask(), expected);
    assert_eq!(manager.online_core_count(), 4);
}

fn install(manager: &mut ProcessManager, pid: usize, root: usize) {
    manager
        .install_process(
            pid,
            0,
            root,
            root + 1,
            root + 2,
            root + 3,
            root + 4,
            pid,
            pid % 4,
            0,
            0x200000,
            0x400000,
        )
        .unwrap();
}

#[test]
fn lookup_handles_out_of_order_install_replacement_missing_and_alpha() {
    let mut manager = manager();
    for pid in [55, 3, 901, 2, 16] {
        install(&mut manager, pid, pid + 200);
    }
    for pid in [2, 3, 16, 55, 901] {
        assert_eq!(manager.find_entry_by_pid(pid).unwrap().root_slot, pid + 200);
    }
    assert!(manager.find_entry_by_pid(54).is_none());
    assert!(manager.find_entry_by_pid(usize::MAX).is_none());
    assert_eq!(manager.find_entry_by_pid(0).unwrap().root_node, 1);
    install(&mut manager, 16, 987);
    assert_eq!(manager.find_entry_by_pid(16).unwrap().root_slot, 987);
    assert_eq!(manager.statistics().running, 5);
}

#[test]
fn reap_preserves_other_pid_and_vm_lookups_and_slot_reuse() {
    let mut manager = manager();
    for pid in [10, 7, 3, 8, 15] {
        install(&mut manager, pid, pid + 200);
        manager.ensure_vm_space_for_pid(pid).unwrap();
        manager
            .vm_space_mut(pid)
            .unwrap()
            .record_frame(0x1000, pid + 1234)
            .unwrap();
    }
    assert_eq!(
        manager.reap_process(7, true),
        Err(CapabilityError::IllegalOperation)
    );
    for pid in [7, 15, 3] {
        manager.mark_exited(pid, 1, 0).unwrap();
        assert!(manager.find_entry_by_pid(pid).unwrap().exited);
        manager.reap_process(pid, true).unwrap();
        assert!(manager.find_entry_by_pid(pid).is_none());
        assert!(manager.vm_space_mut(pid).is_none());
    }
    for pid in [8, 10] {
        assert_eq!(
            manager.vm_space_mut(pid).unwrap().find_frame(0x1000),
            Some(pid + 1234)
        );
        assert!(manager.find_entry_by_pid(pid).is_some());
    }
    assert_eq!(
        manager.reap_process(0, true),
        Err(CapabilityError::PermissionDenied)
    );
    let (pid, slot) = manager.alloc_process_slot().unwrap();
    install(&mut manager, pid, slot);
    manager.mark_exited(pid, 1, 0).unwrap();
    manager.reap_process(pid, true).unwrap();
    let (next_pid, next_slot) = manager.alloc_process_slot().unwrap();
    assert!(next_pid > pid);
    assert_eq!(slot, next_slot);
}

#[test]
fn exec_reset_and_failed_spawn_discard_do_not_change_other_spaces() {
    let mut manager = manager();
    for pid in [30, 10, 20] {
        install(&mut manager, pid, pid + 200);
        manager.ensure_vm_space_for_pid(pid).unwrap();
        manager
            .vm_space_mut(pid)
            .unwrap()
            .record_frame(4096, pid)
            .unwrap();
        // Repeated ensure must not reset an existing VM.
        manager.ensure_vm_space_for_pid(pid).unwrap();
    }
    manager
        .reset_runtime_memory_for_exec(20, 0, 0x200000, 0x400000)
        .unwrap();
    assert_eq!(manager.vm_space_mut(20).unwrap().find_frame(4096), None);
    manager.discard_process_artifacts(10, 210);
    assert!(manager.vm_space_mut(10).is_none());
    assert_eq!(manager.vm_space_mut(30).unwrap().find_frame(4096), Some(30));
    manager.ensure_vm_space_for_pid(5).unwrap();
    assert_eq!(manager.vm_space_mut(30).unwrap().find_frame(4096), Some(30));
}

#[test]
#[ignore = "host microbenchmark; excludes IPC and insertion/removal"]
fn process_lookup_benchmark() {
    use std::{hint::black_box, time::Instant};
    for count in [16, 64, 256, 1024] {
        let mut manager = manager();
        for pid in 1..=count {
            install(&mut manager, pid, pid + 200);
            manager.ensure_vm_space_for_pid(pid).unwrap();
        }
        let mut samples = Vec::new();
        for _ in 0..5 {
            let start = Instant::now();
            for i in 0..200_000 {
                let pid = black_box((i * 17) % count + 1);
                black_box(manager.find_entry_by_pid(pid).unwrap().address_space);
                black_box(manager.vm_space_mut(pid).unwrap());
            }
            samples.push(start.elapsed().as_nanos() / 200_000);
        }
        samples.sort();
        println!("{count} processes: {} ns per entry+VM lookup", samples[2]);
    }
}

#[test]
fn repeated_install_reap_and_reuse_preserve_lookup_invariants() {
    let mut manager = manager();
    let mut live = std::collections::BTreeMap::new();
    let mut random = 12345u64;
    for operation in 0..1000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let pid = (random >> 32) as usize % 64 + 1;
        if operation % 3 == 0 && live.remove(&pid).is_some() {
            manager.mark_exited(pid, 1, 0).unwrap();
            manager.reap_process(pid, false).unwrap();
            assert!(manager.find_entry_by_pid(pid).is_none());
            assert!(manager.vm_space_mut(pid).is_none());
        } else {
            let root = operation + 200;
            install(&mut manager, pid, root);
            manager.ensure_vm_space_for_pid(pid).unwrap();
            manager
                .vm_space_mut(pid)
                .unwrap()
                .record_frame(4096, root)
                .unwrap();
            live.insert(pid, root);
        }
        assert_eq!(manager.statistics().running, live.len());
        for (&pid, &root) in &live {
            assert_eq!(manager.find_entry_by_pid(pid).unwrap().root_slot, root);
            assert_eq!(
                manager.vm_space_mut(pid).unwrap().find_frame(4096),
                Some(root)
            );
        }
    }
}
