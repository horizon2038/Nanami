#![allow(dead_code)]
extern crate alloc;
extern crate self as nun;

pub type Word = usize;
pub type CapabilityDescriptor = usize;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityError {
    IllegalOperation,
    InvalidArgument,
    PermissionDenied,
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

#[path = "shared-memory-lifecycle.rs"]
mod shared_memory_lifecycle;

fn manager() -> ProcessManager {
    ProcessManager::new_alpha(1, 2, 3, 4096, &[], 4).unwrap()
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
