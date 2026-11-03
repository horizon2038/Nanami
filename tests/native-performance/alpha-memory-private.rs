//! Production read-only fork/detach with real ownership and VM indices.
//! Kernel capabilities/PTEs reference the same host page, not copied mock bytes.
use super::*;
use crate::nanami_core::process::ProcessPhysicalAllocation;
use alloc::vec::Vec;
use std::{cell::RefCell, collections::BTreeMap, ptr};

const PAGE_SIZE: usize = 4096;
const PROCESS_FRAME_CHUNK_PAGES: usize = 512;
const PROCESS_FRAME_NODE_RADIX: usize = 9;
const PROCESS_FRAME_TOTAL_PAGES: usize = 1 << 20;
type Attribute = capability_call::address_space::Attribute;

#[derive(Clone, Copy)]
struct OsRequestEvent {
    identifier: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
}
#[derive(Clone, Copy)]
struct PageSource {
    pid: usize,
    address: usize,
    bytes: usize,
}
#[derive(Default)]
struct Kernel {
    physical: BTreeMap<usize, Box<[u8; PAGE_SIZE]>>,
    caps: BTreeMap<usize, usize>,
    ptes: BTreeMap<(usize, usize), (usize, usize)>,
    fail_copy: bool,
    fail_remove: bool,
    fail_map: usize,
    fail_map_after_install: bool,
    allocations: usize,
    copies: usize,
}
thread_local! { static KERNEL: RefCell<Kernel> = RefCell::new(Kernel::default()); }
fn process_frame_chunk_descriptor(root: usize, chunk: usize) -> usize {
    root * 1000 + chunk
}
fn process_frame_descriptor(root: usize, slot: usize) -> usize {
    process_frame_chunk_descriptor(root, slot / PROCESS_FRAME_CHUNK_PAGES)
        * PROCESS_FRAME_CHUNK_PAGES
        + slot % PROCESS_FRAME_CHUNK_PAGES
}
fn align_down(address: usize, alignment: usize) -> usize {
    address & !(alignment - 1)
}
mod arch {
    use super::*;
    pub mod node {
        use super::*;
        pub fn copy(node: usize, slot: usize, source: usize) -> Result<(), CapabilityError> {
            KERNEL.with(|kernel| {
                let mut kernel = kernel.borrow_mut();
                if kernel.fail_copy {
                    return Err(CapabilityError::Fatal);
                }
                let physical = kernel.caps[&source];
                assert!(kernel.caps.insert(node * 512 + slot, physical).is_none());
                kernel.copies += 1;
                Ok(())
            })
        }
        pub fn remove(node: usize, slot: usize) -> Result<(), CapabilityError> {
            KERNEL.with(|kernel| {
                let mut kernel = kernel.borrow_mut();
                if kernel.fail_remove {
                    return Err(CapabilityError::Fatal);
                }
                kernel.caps.remove(&(node * 512 + slot));
                Ok(())
            })
        }
    }
    pub mod address_space {
        use super::*;
        pub fn map(
            space: usize,
            frame: usize,
            va: usize,
            attr: Attribute,
        ) -> Result<(), CapabilityError> {
            KERNEL.with(|kernel| {
                let mut kernel = kernel.borrow_mut();
                let fail = kernel.fail_map != 0;
                kernel.fail_map = kernel.fail_map.saturating_sub(1);
                if fail && !kernel.fail_map_after_install {
                    return Err(CapabilityError::Fatal);
                }
                assert!(kernel.caps.contains_key(&frame));
                if kernel.ptes.contains_key(&(space, va)) {
                    return Err(CapabilityError::IllegalOperation);
                }
                kernel.ptes.insert((space, va), (frame, attr.bits()));
                if fail {
                    return Err(CapabilityError::Fatal);
                }
                Ok(())
            })
        }
        pub fn unmap(space: usize, frame: usize, va: usize) -> Result<(), CapabilityError> {
            KERNEL.with(|kernel| {
                let mut kernel = kernel.borrow_mut();
                if !kernel
                    .ptes
                    .get(&(space, va))
                    .is_some_and(|pte| pte.0 == frame)
                {
                    return Err(CapabilityError::InvalidArgument);
                }
                kernel.ptes.remove(&(space, va));
                Ok(())
            })
        }
    }
}
struct Memory;
impl Memory {
    fn physical_frame_descriptor_from_index(&self, physical: usize) -> Option<usize> {
        KERNEL.with(|kernel| {
            let mut kernel = kernel.borrow_mut();
            if !kernel.physical.contains_key(&physical) {
                return None;
            }
            let frame = 1_000_000_000 + physical;
            kernel.caps.insert(frame, physical);
            Some(frame)
        })
    }
    fn ensure_page_tables(
        &mut self,
        _: usize,
        _: usize,
        _: &mut vm_space::VmSpace,
    ) -> Result<(), CapabilityError> {
        Ok(())
    }
    fn allocate_process_frame(
        &mut self,
        node: usize,
        radix: usize,
        slot: usize,
    ) -> Result<usize, CapabilityError> {
        assert_eq!(radix, PROCESS_FRAME_NODE_RADIX);
        KERNEL.with(|kernel| {
            let mut kernel = kernel.borrow_mut();
            let physical = kernel.physical.keys().next_back().copied().unwrap_or(99) + 1;
            kernel.physical.insert(physical, Box::new([0; PAGE_SIZE]));
            assert!(kernel.caps.insert(node * 512 + slot, physical).is_none());
            kernel.allocations += 1;
            Ok(physical)
        })
    }
    fn free_physical(&mut self, address: usize, bytes: usize) -> Result<(), CapabilityError> {
        assert_eq!(bytes, PAGE_SIZE);
        KERNEL.with(|kernel| {
            let mut kernel = kernel.borrow_mut();
            assert!(!kernel.caps.values().any(|&pfn| pfn * PAGE_SIZE == address));
            kernel.physical.remove(&(address / PAGE_SIZE));
        });
        Ok(())
    }
}
struct Alpha {
    processes: ProcessManager,
    memory: Memory,
    invalidations: Vec<usize>,
}
impl Alpha {
    fn new() -> Self {
        KERNEL.with(|kernel| *kernel.borrow_mut() = Kernel::default());
        let mut processes = manager();
        for pid in 1..=4 {
            processes
                .install_process(
                    pid,
                    4,
                    200 + pid,
                    300 + pid,
                    400 + pid,
                    500 + pid,
                    600 + pid,
                    pid,
                    0,
                    0,
                    0x200000,
                    0x400000,
                )
                .unwrap();
            processes.ensure_vm_space_for_pid(pid).unwrap();
            processes.seal_capabilities(pid).unwrap();
        }
        Self {
            processes,
            memory: Memory,
            invalidations: Vec::new(),
        }
    }
    fn ensure_process_frame_chunks(
        &mut self,
        _: usize,
        _: usize,
        _: usize,
        _: usize,
    ) -> Result<(), CapabilityError> {
        Ok(())
    }
    fn invalidate_process_copy_mappings(&mut self, pid: usize) -> Result<(), CapabilityError> {
        self.invalidations.push(pid);
        Ok(())
    }
    fn map_initialization_page(&mut self, physical: usize) -> Result<usize, CapabilityError> {
        KERNEL.with(|kernel| {
            Ok(kernel
                .borrow_mut()
                .physical
                .get_mut(&physical)
                .unwrap()
                .as_mut_ptr() as usize)
        })
    }
    fn map_process_copy_frame(
        &mut self,
        _: usize,
        frame: usize,
        _: Option<usize>,
    ) -> Result<usize, CapabilityError> {
        let physical = KERNEL.with(|kernel| kernel.borrow().caps[&frame]);
        self.map_initialization_page(physical)
    }
    fn process_frame_for_page(&mut self, pid: usize, va: usize) -> Result<usize, CapabilityError> {
        self.processes
            .vm_space_mut(pid)
            .and_then(|vm| vm.find_frame(va))
            .ok_or(CapabilityError::Fatal)
    }
    fn protect_process_page(
        &mut self,
        pid: usize,
        space: usize,
        va: usize,
        frame: usize,
        permissions: usize,
    ) -> Result<(), CapabilityError> {
        let vm = self.processes.vm_space_mut(pid).unwrap();
        if permissions & 2 != 0 && vm.private_page_shared(va) {
            return Err(CapabilityError::IllegalOperation);
        }
        let previous = vm.permissions(va);
        if previous != 0 {
            arch::address_space::unmap(space, frame, va)?;
        }
        if permissions != 0 {
            arch::address_space::map(space, frame, va, Attribute::from_bits(permissions).unwrap())?;
        }
        vm.set_permissions(va, permissions);
        Ok(())
    }
    fn page(&mut self, pid: usize, va: usize, value: u8, permissions: usize) -> usize {
        let (root, space, slot) = self
            .processes
            .reserve_process_heap_at(pid, va, 1, PAGE_SIZE, PROCESS_FRAME_TOTAL_PAGES)
            .unwrap();
        let physical = self
            .memory
            .allocate_process_frame(
                process_frame_chunk_descriptor(root, slot / 512),
                9,
                slot % 512,
            )
            .unwrap();
        let frame = process_frame_descriptor(root, slot);
        KERNEL.with(|kernel| {
            kernel
                .borrow_mut()
                .physical
                .get_mut(&physical)
                .unwrap()
                .fill(value)
        });
        self.processes
            .register_physical_allocation(pid, va, slot, physical, 1)
            .unwrap();
        let vm = self.processes.vm_space_mut(pid).unwrap();
        vm.record_frame(va, frame).unwrap();
        vm.set_permissions(va, permissions);
        if permissions != 0 {
            arch::address_space::map(space, frame, va, Attribute::from_bits(permissions).unwrap())
                .unwrap();
        }
        physical
    }
    fn map_process_initialized_pages(
        &mut self,
        pid: usize,
        base: Option<usize>,
        count: usize,
        source: Option<PageSource>,
    ) -> Result<usize, CapabilityError> {
        let base = base.unwrap();
        let source = source.unwrap();
        assert_eq!(source.bytes, count * PAGE_SIZE);
        for offset in (0..source.bytes).step_by(PAGE_SIZE) {
            let frame = self.process_frame_for_page(source.pid, source.address + offset)?;
            let source = self.map_process_copy_frame(source.pid, frame, None)?;
            let pfn = self.page(pid, base + offset, 0, 7);
            let destination = self.map_initialization_page(pfn)?;
            unsafe {
                ptr::copy_nonoverlapping(source as *const u8, destination as *mut u8, PAGE_SIZE);
            }
        }
        Ok(base)
    }
    fn clone_process_vm(&mut self, _: usize, _: usize) -> Result<(), CapabilityError> {
        panic!("x86 read-only clone must use the opt-in implementation");
    }
    fn prepare(
        &mut self,
        pid: usize,
        va: usize,
        bytes: usize,
        mode: usize,
    ) -> Result<usize, CapabilityError> {
        self.handle_process_private_write(OsRequestEvent {
            identifier: 4,
            arg0: pid,
            arg1: va,
            arg2: bytes,
            arg3: mode,
        })
    }
    fn physical(&mut self, pid: usize, va: usize) -> usize {
        let frame = self.process_frame_for_page(pid, va).unwrap();
        KERNEL.with(|kernel| kernel.borrow().caps[&frame])
    }
    fn byte(&mut self, pid: usize, va: usize) -> u8 {
        let pfn = self.physical(pid, va);
        KERNEL.with(|kernel| kernel.borrow().physical[&pfn][0])
    }
}
#[path = "../../nanami/src/nanami_core/alpha/memory_private.rs"]
mod production;

#[test]
fn readonly_fork_shares_physical_backing_but_not_writable_or_prot_none() {
    let mut a = Alpha::new();
    let physical: Vec<_> = [1, 5, 3, 0, 7]
        .into_iter()
        .enumerate()
        .map(|(i, permission)| a.page(1, (i + 1) * PAGE_SIZE, i as u8 + 40, permission))
        .collect();
    a.clone_process_vm_shared_readonly(1, 2).unwrap();
    for (i, pfn) in physical.into_iter().enumerate() {
        let va = (i + 1) * PAGE_SIZE;
        assert_eq!(a.byte(1, va), a.byte(2, va));
        assert_eq!(pfn == a.physical(2, va), i < 2);
        assert_eq!(
            a.processes.vm_space_mut(2).unwrap().private_page_shared(va),
            i < 2
        );
    }
    KERNEL.with(|kernel| {
        assert_eq!(
            (kernel.borrow().allocations, kernel.borrow().copies),
            (8, 2)
        )
    });
}

#[test]
fn query_is_side_effect_free_and_detach_preserves_bytes_permissions_and_parent() {
    let mut a = Alpha::new();
    let original = a.page(1, 0x1000, 0x51, 5);
    a.clone_process_vm_shared_readonly(1, 2).unwrap();
    assert_eq!(a.prepare(2, 0x1001, 1, 0), Ok(1));
    assert!(a.invalidations.is_empty());
    assert_eq!(a.prepare(2, 0x1001, 1, 1), Ok(0));
    assert_eq!(a.invalidations, [2]);
    assert_eq!(a.prepare(2, 0x1000, PAGE_SIZE, 0), Ok(0));
    assert_eq!(a.byte(2, 0x1000), 0x51);
    assert_ne!(a.physical(2, 0x1000), original);
    assert_eq!(a.processes.vm_space_mut(2).unwrap().permissions(0x1000), 5);
    let replacement = a.physical(2, 0x1000);
    KERNEL.with(|kernel| kernel.borrow_mut().physical.get_mut(&replacement).unwrap()[0] = 0xd8);
    assert_eq!(a.byte(1, 0x1000), 0x51);
    assert_eq!(a.byte(2, 0x1000), 0xd8);
    assert_eq!(
        a.processes
            .physical_reference_count(ProcessPhysicalAllocation {
                base_va: 0x1000,
                start_slot: 0,
                base_page: original,
                page_count: 1
            }),
        1
    );
    let allocated = KERNEL.with(|kernel| kernel.borrow().allocations);
    a.prepare(1, 0x1000, PAGE_SIZE, 1).unwrap();
    assert_eq!(KERNEL.with(|kernel| kernel.borrow().allocations), allocated);
    assert_eq!(a.physical(1, 0x1000), original);
}

#[test]
fn descendants_share_existing_alias_and_large_detach_commits_stable_indices() {
    let mut a = Alpha::new();
    for page in 1..=64 {
        a.page(1, page * PAGE_SIZE, page as u8, 1);
    }
    a.clone_process_vm_shared_readonly(1, 2).unwrap();
    a.clone_process_vm_shared_readonly(2, 3).unwrap();
    a.prepare(2, PAGE_SIZE, 64 * PAGE_SIZE, 1).unwrap();
    for page in 1..=64 {
        let va = page * PAGE_SIZE;
        assert_eq!(a.byte(2, va), page as u8);
        assert_ne!(a.physical(1, va), a.physical(2, va));
        assert_eq!(a.physical(1, va), a.physical(3, va));
    }
    assert_eq!(
        a.processes.deferred_physical_allocations_for_pid(2).count(),
        0
    );
}

#[test]
fn prot_none_retains_shared_bytes_until_explicit_detach() {
    let mut a = Alpha::new();
    a.page(1, 0x1000, 0x71, 1);
    a.clone_process_vm_shared_readonly(1, 2).unwrap();
    let memory = a.processes.memory_entry(2).unwrap();
    let frame = a.process_frame_for_page(2, 0x1000).unwrap();
    assert_eq!(
        a.protect_process_page(2, memory.address_space, 0x1000, frame, 3),
        Err(CapabilityError::IllegalOperation)
    );
    a.protect_process_page(2, memory.address_space, 0x1000, frame, 0)
        .unwrap();
    a.prepare(2, 0x1000, PAGE_SIZE, 1).unwrap();
    let frame = a.process_frame_for_page(2, 0x1000).unwrap();
    a.protect_process_page(2, memory.address_space, 0x1000, frame, 3)
        .unwrap();
    assert_eq!(a.byte(2, 0x1000), 0x71);
    assert_eq!(a.byte(1, 0x1000), 0x71);
    // Retired cap slots are reusable without releasing the still-live VA.
    let next = a.processes.memory_entry(2).unwrap().next_frame_slot;
    let (_, _, recycled) = a
        .processes
        .reserve_private_replacement_slot(2, PROCESS_FRAME_TOTAL_PAGES)
        .unwrap();
    assert_eq!(recycled, 0);
    assert_eq!(a.processes.memory_entry(2).unwrap().next_frame_slot, next);
    assert_eq!(a.byte(2, 0x1000), 0x71);
}

#[test]
fn failed_retarget_or_cap_removal_never_exposes_writable_peer_backing() {
    for (maps, after_install, remove) in [
        (1, false, false),
        (1, true, false),
        (2, false, false),
        (0, false, true),
    ] {
        let mut a = Alpha::new();
        let original = a.page(1, 0x1000, 0x3a, 5);
        a.clone_process_vm_shared_readonly(1, 2).unwrap();
        KERNEL.with(|kernel| {
            let mut kernel = kernel.borrow_mut();
            kernel.fail_map = maps;
            kernel.fail_map_after_install = after_install;
            kernel.fail_remove = remove;
        });
        assert_eq!(
            a.prepare(2, 0x1000, PAGE_SIZE, 1),
            Err(CapabilityError::Fatal)
        );
        assert_eq!(a.byte(1, 0x1000), 0x3a);
        assert!(a
            .processes
            .vm_space_mut(2)
            .unwrap()
            .private_page_shared(0x1000));
        assert_eq!(
            a.processes.deferred_physical_allocations_for_pid(2).count(),
            1
        );
        let child = a.processes.memory_entry(2).unwrap();
        KERNEL.with(|kernel| {
            let kernel = kernel.borrow();
            assert!(kernel.caps.values().any(|&pfn| pfn == original));
            if let Some(&(_, permissions)) = kernel.ptes.get(&(child.address_space, 0x1000)) {
                assert_eq!(permissions & 2, 0);
            }
            assert_eq!(kernel.physical.len(), 2);
        });
    }
}

#[test]
fn failed_fork_copy_holds_reference_and_source_marker_for_normal_rollback() {
    let mut a = Alpha::new();
    let original = a.page(1, 0x1000, 0x42, 1);
    KERNEL.with(|kernel| kernel.borrow_mut().fail_copy = true);
    assert_eq!(
        a.clone_process_vm_shared_readonly(1, 2),
        Err(CapabilityError::Fatal)
    );
    assert!(a
        .processes
        .vm_space_mut(1)
        .unwrap()
        .private_page_shared(0x1000));
    assert_eq!(
        a.processes.private_page_references(2, 0x1000, 0x2000)[0]
            .allocation
            .base_page,
        original
    );
    assert_eq!(a.byte(1, 0x1000), 0x42);
}

#[test]
fn only_reaper_can_prepare_and_invalid_ranges_have_no_side_effects() {
    let mut a = Alpha::new();
    a.page(1, 0x1000, 0x42, 1);
    a.clone_process_vm_shared_readonly(1, 2).unwrap();
    for caller in [0, 1, 2, 3] {
        assert_eq!(
            a.handle_process_private_write(OsRequestEvent {
                identifier: caller,
                arg0: 2,
                arg1: 0x1000,
                arg2: PAGE_SIZE,
                arg3: 1
            }),
            Err(CapabilityError::PermissionDenied)
        );
    }
    assert_eq!(
        a.prepare(2, usize::MAX, 2, 1),
        Err(CapabilityError::InvalidArgument)
    );
    assert_eq!(
        a.prepare(2, 0x1000, 0, 1),
        Err(CapabilityError::InvalidArgument)
    );
    assert_eq!(
        a.prepare(2, 0x1000, 1, 2),
        Err(CapabilityError::InvalidArgument)
    );
    assert!(a.invalidations.is_empty());
}

#[test]
fn untracked_external_aliases_are_copied_instead_of_promoted_to_private_sharing() {
    let mut a = Alpha::new();
    let original = a.page(1, 0x1000, 0x53, 1);
    a.processes
        .register_physical_allocation(3, 0x8000, 0, original, 1)
        .unwrap();
    a.clone_process_vm_shared_readonly(1, 2).unwrap();
    assert_ne!(a.physical(2, 0x1000), original);
    assert_eq!(a.byte(2, 0x1000), 0x53);
    assert!(!a
        .processes
        .vm_space_mut(1)
        .unwrap()
        .private_page_shared(0x1000));
    assert!(!a
        .processes
        .vm_space_mut(2)
        .unwrap()
        .private_page_shared(0x1000));
}

#[test]
fn readonly_clone_requires_an_empty_private_sealed_destination() {
    let mut a = Alpha::new();
    a.page(1, 0x1000, 0x53, 1);
    a.page(2, 0x2000, 0x87, 3);
    assert_eq!(
        a.clone_process_vm_shared_readonly(1, 2),
        Err(CapabilityError::IllegalOperation)
    );
    assert_eq!(a.byte(2, 0x2000), 0x87);
    a.processes.share_address_space(3, 1).unwrap();
    assert_eq!(
        a.clone_process_vm_shared_readonly(1, 3),
        Err(CapabilityError::PermissionDenied)
    );
    a.processes
        .install_process(3, 4, 203, 303, 403, 503, 603, 3, 0, 0, 0x200000, 0x400000)
        .unwrap();
    assert_eq!(
        a.clone_process_vm_shared_readonly(1, 3),
        Err(CapabilityError::PermissionDenied)
    );
}
