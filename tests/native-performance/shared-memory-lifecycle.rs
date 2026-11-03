//! Exercise the production shared-memory transaction with real ProcessManager
//! bookkeeping and checked capability/mapping mocks (not hardware TLB tests).
use super::*;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};

const PAGE_SIZE: usize = 4096;
const PROCESS_FRAME_CHUNK_PAGES: usize = 512;
const PROCESS_FRAME_TOTAL_PAGES: usize = 1 << 19;

fn align_down(value: usize, alignment: usize) -> usize {
    value & !(alignment - 1)
}

#[derive(Default)]
struct Machine {
    frames: BTreeMap<usize, usize>,
    pool_frames: BTreeMap<usize, usize>,
    maps: BTreeMap<(usize, usize), usize>,
    permissions: BTreeMap<(usize, usize), usize>,
    live: BTreeMap<usize, usize>,
    zeroed: BTreeSet<usize>,
    next_page: usize,
    freed: usize,
    zero_calls: usize,
    copy_calls: usize,
    operation: usize,
    fail_at: Option<usize>,
    fail_unmap: bool,
    fail_remove: bool,
}

thread_local! { static MACHINE: RefCell<Machine> = RefCell::new(Machine::default()); }

fn machine<T>(f: impl FnOnce(&mut Machine) -> T) -> T {
    MACHINE.with(|m| f(&mut m.borrow_mut()))
}

fn operation(m: &mut Machine) -> Result<(), CapabilityError> {
    m.operation += 1;
    if m.fail_at == Some(m.operation) {
        Err(CapabilityError::IllegalOperation)
    } else {
        Ok(())
    }
}

fn process_frame_descriptor(root: usize, slot: usize) -> usize {
    root * (1 << 20) + slot
}
fn process_frame_chunk_descriptor(root: usize, chunk: usize) -> usize {
    process_frame_descriptor(root, chunk * PROCESS_FRAME_CHUNK_PAGES)
}
fn pool_frame(page: usize) -> usize {
    0x4000_0000 + page
}
fn frame_page(machine: &Machine, frame: usize) -> usize {
    *machine
        .frames
        .get(&frame)
        .or_else(|| machine.pool_frames.get(&frame))
        .expect("unknown Frame capability")
}

mod arch {
    use super::*;
    pub mod node {
        use super::*;
        pub fn copy(node: usize, slot: Word, source: usize) -> Result<(), CapabilityError> {
            machine(|m| {
                operation(m)?;
                let physical = frame_page(m, source);
                assert!(
                    m.frames.insert(node + slot, physical).is_none(),
                    "overwrote live capability"
                );
                Ok(())
            })
        }
        pub fn remove(node: usize, slot: Word) -> Result<(), CapabilityError> {
            machine(|m| {
                if m.fail_remove {
                    return Err(CapabilityError::IllegalOperation);
                }
                assert!(
                    !m.maps.values().any(|frame| *frame == node + slot),
                    "removed mapped capability"
                );
                assert!(
                    m.frames.remove(&(node + slot)).is_some(),
                    "removed empty slot"
                );
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
            attributes: a9n_abi::capability_call::address_space::Attribute,
        ) -> Result<(), CapabilityError> {
            machine(|m| {
                operation(m)?;
                assert!(m.pool_frames.contains_key(&frame));
                assert!(m.maps.insert((space, va), frame).is_none());
                m.permissions.insert((space, va), attributes.bits());
                Ok(())
            })
        }
        pub fn unmap(space: usize, frame: usize, va: usize) -> Result<(), CapabilityError> {
            machine(|m| {
                if m.fail_unmap {
                    return Err(CapabilityError::IllegalOperation);
                }
                if let Some(mapped) = m.maps.remove(&(space, va)) {
                    // Kernel MAP/UNMAP identify the Frame by physical object,
                    // not by which alias capability supplied its address.
                    assert_eq!(frame_page(m, mapped), frame_page(m, frame));
                }
                m.permissions.remove(&(space, va));
                Ok(())
            })
        }
    }
}

struct Memory;
impl Memory {
    fn is_pool_frame_descriptor(&self, frame: usize) -> bool {
        machine(|m| m.pool_frames.contains_key(&frame))
    }
    fn allocate_physical_any(&mut self, bytes: usize) -> Result<usize, CapabilityError> {
        machine(|m| {
            operation(m)?;
            let page = m.next_page;
            m.next_page += bytes / PAGE_SIZE;
            m.live.insert(page, bytes / PAGE_SIZE);
            Ok(page)
        })
    }
    fn free_physical(&mut self, base: usize, bytes: usize) -> Result<(), CapabilityError> {
        machine(|m| {
            let page = base / PAGE_SIZE;
            let count = bytes / PAGE_SIZE;
            assert!(
                !m.frames
                    .values()
                    .any(|&p| (page..page + count).contains(&p)),
                "freed referenced RAM"
            );
            assert_eq!(
                m.live.remove(&page),
                Some(count),
                "double free / wrong allocation"
            );
            m.freed += 1;
            Ok(())
        })
    }
    fn ensure_alpha_frame_at_physical_index(&mut self, page: usize) -> Result<(), CapabilityError> {
        machine(|m| {
            operation(m)?;
            m.pool_frames.insert(pool_frame(page), page);
            Ok(())
        })
    }
    fn physical_frame_descriptor_from_index(&self, page: usize) -> Option<usize> {
        machine(|m| m.pool_frames.contains_key(&pool_frame(page))).then_some(pool_frame(page))
    }
    fn map_frame_strict(
        &mut self,
        space: usize,
        frame: usize,
        va: usize,
        vm: &mut vm_space::VmSpace,
    ) -> Result<(), CapabilityError> {
        machine(|m| {
            assert!(
                m.pool_frames.contains_key(&frame),
                "tracked a revocable child alias"
            );
            let page = frame_page(m, frame);
            assert!(m.zeroed.contains(&page), "exposed nonzero recycled page");
            assert!(
                m.maps.insert((space, va), frame).is_none(),
                "overwrote live mapping"
            );
            // Fail AFTER installing the PTE, as a TLB update error could do.
            operation(m)
        })?;
        vm.record_frame(va, frame).unwrap();
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct OsRequestEvent {
    identifier: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
}
struct Alpha {
    processes: ProcessManager,
    memory: Memory,
    copy_window: memory_copy::CopyWindow,
}
impl Alpha {
    fn read_process_memory_into_static_buffer(
        &mut self,
        _: usize,
        _: usize,
        _: usize,
    ) -> Result<&[u8], CapabilityError> {
        panic!("this lifecycle fixture does not issue vector requests");
    }
    fn process_frame_for_page(
        &mut self,
        pid: Word,
        va: Word,
    ) -> Result<CapabilityDescriptor, CapabilityError> {
        self.processes
            .vm_space_mut(pid)
            .and_then(|vm| vm.find_frame(va))
            .ok_or(CapabilityError::InvalidArgument)
    }
    fn map_process_heap_pages_at(
        &mut self,
        _: Word,
        _: Word,
        _: Word,
    ) -> Result<(Word, Word), CapabilityError> {
        Err(CapabilityError::InvalidArgument)
    }
    // Model the byte-transfer boundary with real current-Frame lookup and
    // CopyWindow selection. Authorization must reject before reaching it.
    fn copy_process_memory(
        &mut self,
        source_pid: usize,
        source: usize,
        destination_pid: usize,
        destination: usize,
        bytes: usize,
    ) -> Result<(), CapabilityError> {
        assert!(bytes != 0 && source % PAGE_SIZE + bytes <= PAGE_SIZE);
        assert!(destination % PAGE_SIZE + bytes <= PAGE_SIZE);
        let source_frame = self.process_frame_for_page(source_pid, source)?;
        let source_alias = self.map_process_copy_frame(source_pid, source_frame, None)?;
        let destination_frame = self.process_frame_for_page(destination_pid, destination)?;
        self.map_process_copy_frame(destination_pid, destination_frame, Some(source_alias))?;
        machine(|m| m.copy_calls += 1);
        Ok(())
    }
    fn map_alpha_temporary_frame(
        &mut self,
        frame: usize,
        va: usize,
    ) -> Result<(), CapabilityError> {
        machine(|m| {
            assert!(m.pool_frames.contains_key(&frame));
            assert!(m.maps.insert((0, va), frame).is_none());
        });
        Ok(())
    }
    fn unmap_alpha_frame(&mut self, frame: usize, va: usize) -> Result<(), CapabilityError> {
        arch::address_space::unmap(0, frame, va)
    }
    fn ensure_process_frame_chunks(
        &mut self,
        _: usize,
        _: usize,
        _: usize,
        _: usize,
    ) -> Result<(), CapabilityError> {
        machine(operation)
    }
    fn zero_process_frames(
        &mut self,
        root: usize,
        slot: usize,
        pages: usize,
    ) -> Result<(), CapabilityError> {
        machine(|m| {
            m.zero_calls += 1;
            for i in 0..pages {
                let page = m.frames[&process_frame_descriptor(root, slot + i)];
                m.zeroed.insert(page);
            }
        });
        Ok(())
    }
}

#[path = "../../nanami/src/nanami_core/alpha/memory_copy.rs"]
mod memory_copy;

#[path = "../../nanami/src/nanami_core/alpha/checked_memory.rs"]
mod checked_memory;
#[path = "../../nanami/src/nanami_core/alpha/shared_memory.rs"]
mod implementation;
#[path = "../../nanami/src/nanami_core/alpha/mapping_protection.rs"]
mod mapping_protection;

fn alpha() -> Alpha {
    machine(|m| {
        *m = Machine {
            next_page: 0x100,
            ..Machine::default()
        }
    });
    let mut processes = manager();
    for pid in 1..=2 {
        install(&mut processes, pid, 200 + pid);
        processes.ensure_vm_space_for_pid(pid).unwrap();
    }
    Alpha {
        processes,
        memory: Memory,
        copy_window: memory_copy::CopyWindow::new(),
    }
}

fn create(alpha: &mut Alpha, pages: usize) -> Result<(usize, usize), CapabilityError> {
    alpha.handle_shared_memory_request(OsRequestEvent {
        identifier: 1,
        arg0: 2,
        arg1: pages * PAGE_SIZE,
        arg2: 0,
        arg3: 0,
    })
}

#[test]
fn owned_memory_can_be_shared_at_supervised_addresses_and_reclaimed_in_either_order() {
    let mut alpha = alpha();
    let (owned, peer) = alpha
        .handle_shared_memory_request(OsRequestEvent {
            identifier: 1,
            arg0: 0,
            arg1: PAGE_SIZE,
            arg2: 0,
            arg3: 0,
        })
        .unwrap();
    assert_eq!(peer, 0);
    let entry = alpha.processes.find_entry_by_pid(2).unwrap();
    alpha
        .processes
        .install_process(
            2,
            1,
            entry.root_slot,
            entry.root_node,
            entry.pcb,
            entry.address_space,
            entry.os_port,
            entry.os_port_identifier,
            entry.affinity,
            entry.next_frame_slot,
            entry.user_heap_next_va,
            entry.user_heap_limit_va,
        )
        .unwrap();
    alpha.processes.ensure_vm_space_for_pid(2).unwrap();
    let (_, alias) = alpha
        .handle_mapping_share_request(OsRequestEvent {
            identifier: 1,
            arg0: 2,
            arg1: owned,
            arg2: PAGE_SIZE,
            arg3: 0x1_0000_0000,
        })
        .unwrap();
    assert_eq!(alias, 0x1_0000_0000);
    release(&mut alpha, 1, owned, 1).unwrap();
    assert_eq!(machine(|m| m.freed), 0);
    release(&mut alpha, 2, alias, 1).unwrap();
    assert_empty();
}

#[test]
fn private_child_frame_metadata_remains_shareable_without_promoting_foreign_frames() {
    let mut alpha = alpha();
    let (owned, _) = alpha
        .handle_shared_memory_request(OsRequestEvent {
            identifier: 1,
            arg0: 0,
            arg1: PAGE_SIZE,
            arg2: 0,
            arg3: 0,
        })
        .unwrap();
    let allocation = alpha
        .processes
        .find_active_physical_allocation_reference(1, owned, 1)
        .unwrap();
    let entry = alpha.processes.memory_entry(1).unwrap();
    let child = process_frame_descriptor(entry.root_node, allocation.start_slot);
    vm_restore(&mut alpha, 1, owned, pool_frame(allocation.base_page + 1));
    assert!(share(&mut alpha, 1, 2, owned, PAGE_SIZE).is_err());
    vm_restore(&mut alpha, 1, owned, child);
    let (_, alias) = share(&mut alpha, 1, 2, owned, PAGE_SIZE).unwrap();
    assert_eq!(
        alpha.processes.vm_space_mut(1).unwrap().find_frame(owned),
        Some(child)
    );
    assert_eq!(
        alpha.processes.vm_space_mut(2).unwrap().find_frame(alias),
        Some(pool_frame(allocation.base_page))
    );
    release(&mut alpha, 1, owned, 1).unwrap();
    release(&mut alpha, 2, alias, 1).unwrap();
    assert_empty();
}

#[test]
fn hardware_protection_retains_frame_and_allocation_across_prot_none() {
    let mut alpha = alpha();
    let (caller, peer) = create(&mut alpha, 1).unwrap();
    let entry = alpha.processes.find_entry_by_pid(1).unwrap();
    let original = alpha.processes.vm_space_mut(1).unwrap().find_frame(caller);
    for permissions in [0, 1, 3, 7, 0] {
        alpha
            .handle_mapping_protect_request(OsRequestEvent {
                identifier: 1,
                arg0: 1,
                arg1: caller,
                arg2: PAGE_SIZE,
                arg3: permissions,
            })
            .unwrap();
        let vm = alpha.processes.vm_space_mut(1).unwrap();
        assert_eq!(vm.find_frame(caller), original);
        assert_eq!(vm.permissions(caller), permissions);
        assert_eq!(
            machine(|m| m.maps.contains_key(&(entry.address_space, caller))),
            permissions != 0
        );
        if permissions != 0 {
            assert_eq!(
                machine(|m| m.permissions[&(entry.address_space, caller)]),
                permissions
            );
        }
        assert_eq!(machine(|m| m.freed), 0);
    }
    release(&mut alpha, 1, caller, 1).unwrap();
    release(&mut alpha, 2, peer, 1).unwrap();
    assert_empty();
}

#[test]
fn failed_permission_map_rolls_back_and_foreign_supervisors_are_denied() {
    let mut alpha = alpha();
    let (caller, peer) = create(&mut alpha, 2).unwrap();
    let mut request = OsRequestEvent {
        identifier: 2,
        arg0: 1,
        arg1: caller,
        arg2: 2 * PAGE_SIZE,
        arg3: 1,
    };
    assert_eq!(
        alpha.handle_mapping_protect_request(request),
        Err(CapabilityError::PermissionDenied)
    );
    request.identifier = 1;
    machine(|m| m.fail_at = Some(m.operation + 2));
    assert_eq!(
        alpha.handle_mapping_protect_request(request),
        Err(CapabilityError::IllegalOperation)
    );
    for page in 0..2 {
        assert_eq!(
            alpha
                .processes
                .vm_space_mut(1)
                .unwrap()
                .permissions(caller + page * PAGE_SIZE),
            7
        );
    }
    release(&mut alpha, 1, caller, 2).unwrap();
    release(&mut alpha, 2, peer, 2).unwrap();
    assert_empty();
}

fn release(alpha: &mut Alpha, pid: usize, va: usize, pages: usize) -> Result<(), CapabilityError> {
    let entry = alpha.processes.find_entry_by_pid(pid).unwrap();
    let reservation = alpha
        .processes
        .shared_memory_reservation(pid, va, pages)
        .unwrap();
    alpha.release_shared_memory(entry, reservation)
}

fn assert_empty() {
    machine(|m| {
        assert!(m.frames.is_empty());
        assert!(m.maps.is_empty());
        assert!(m.live.is_empty());
    });
}

fn share(
    alpha: &mut Alpha,
    pid: usize,
    peer: usize,
    va: usize,
    bytes: usize,
) -> Result<(usize, usize), CapabilityError> {
    alpha.handle_mapping_share_request(OsRequestEvent {
        identifier: pid,
        arg0: peer,
        arg1: va,
        arg2: bytes,
        arg3: 0,
    })
}

#[test]
fn sharing_owned_ram_adds_a_reference_without_allocation_or_zeroing() {
    for source_first in [false, true] {
        let mut alpha = alpha();
        let (source, original_peer) = create(&mut alpha, 3).unwrap();
        let (next_page, zero_calls) = machine(|m| (m.next_page, m.zero_calls));
        let (unchanged, peer) = share(&mut alpha, 1, 2, source, 3 * PAGE_SIZE).unwrap();
        assert_eq!(unchanged, source);
        assert_ne!(peer, original_peer);
        machine(|m| {
            assert_eq!(m.next_page, next_page);
            assert_eq!(m.zero_calls, zero_calls);
            assert_eq!(m.live.len(), 1);
            assert_eq!(m.frames.len(), 9);
        });
        if source_first {
            release(&mut alpha, 1, source, 3).unwrap();
        }
        release(&mut alpha, 2, original_peer, 3).unwrap();
        assert_eq!(machine(|m| m.live.len()), 1);
        release(&mut alpha, 2, peer, 3).unwrap();
        if !source_first {
            assert_eq!(machine(|m| m.live.len()), 1);
            release(&mut alpha, 1, source, 3).unwrap();
        }
        assert_empty();
    }
}

#[test]
fn share_rejects_foreign_partial_unaligned_and_missing_mappings() {
    let mut alpha = alpha();
    let (source, peer) = create(&mut alpha, 3).unwrap();
    for (pid, to, va, bytes) in [
        (0, 2, source, 3 * PAGE_SIZE),
        (1, 1, source, 3 * PAGE_SIZE),
        (1, 3, source, 3 * PAGE_SIZE),
        (1, 2, source + 1, 3 * PAGE_SIZE),
        (1, 2, source + PAGE_SIZE, 2 * PAGE_SIZE),
        (1, 2, source, PAGE_SIZE),
        (1, 2, source, usize::MAX),
        (1, 2, source + 10 * PAGE_SIZE, 3 * PAGE_SIZE),
    ] {
        assert!(share(&mut alpha, pid, to, va, bytes).is_err());
    }
    let vm = alpha.processes.vm_space_mut(1).unwrap();
    let frame = vm.find_frame(source).unwrap();
    vm.forget_frame(source);
    assert!(share(&mut alpha, 1, 2, source, 3 * PAGE_SIZE).is_err());
    vm_restore(&mut alpha, 1, source, frame);
    release(&mut alpha, 1, source, 3).unwrap();
    release(&mut alpha, 2, peer, 3).unwrap();
    assert_empty();
}

fn vm_restore(alpha: &mut Alpha, pid: usize, va: usize, frame: usize) {
    alpha
        .processes
        .vm_space_mut(pid)
        .unwrap()
        .record_frame(va, frame)
        .unwrap();
}

#[test]
fn every_share_install_failure_preserves_the_source_and_original_peer() {
    for fail in 1..=7 {
        // ensure chunks, three copies, three strict maps.
        let mut alpha = alpha();
        let (source, peer) = create(&mut alpha, 3).unwrap();
        machine(|m| m.fail_at = Some(m.operation + fail));
        assert!(share(&mut alpha, 1, 2, source, 3 * PAGE_SIZE).is_err());
        machine(|m| {
            m.fail_at = None;
            assert_eq!(m.frames.len(), 6);
            assert_eq!(m.maps.len(), 6);
            assert_eq!(m.live.len(), 1);
            assert_eq!(m.freed, 0);
        });
        release(&mut alpha, 1, source, 3).unwrap();
        release(&mut alpha, 2, peer, 3).unwrap();
        assert_empty();
    }
}

#[test]
fn both_release_orders_remove_caps_before_free_and_reuse_reservations() {
    for first in [1, 2] {
        let mut alpha = alpha();
        for _ in 0..1000 {
            let (a, b) = create(&mut alpha, 3).unwrap();
            assert_eq!((a, b), (0x200000, 0x200000));
            let addresses = [a, b];
            release(&mut alpha, first, addresses[first - 1], 3).unwrap();
            machine(|m| {
                assert_eq!(m.live.len(), 1);
                assert_eq!(m.frames.len(), 3);
            });
            let last = 3 - first;
            release(&mut alpha, last, addresses[last - 1], 3).unwrap();
            assert_empty();
        }
        for pid in 1..=2 {
            let entry = alpha.processes.find_entry_by_pid(pid).unwrap();
            assert_eq!(entry.next_frame_slot, 3);
            assert_eq!(entry.user_heap_next_va, 0x203000);
        }
    }
}

#[test]
fn resident_pool_alias_never_bypasses_current_permissions_or_supervisor_checks() {
    let mut alpha = alpha();
    let (caller, peer) = create(&mut alpha, 2).unwrap();
    let target = peer + PAGE_SIZE;
    let request = || OsRequestEvent {
        identifier: 2,
        arg0: 2,
        arg1: target,
        arg2: peer,
        arg3: 32 | nanami_uapi::os::OS_MEMORY_COPY_USER_ACCESS,
    };
    alpha
        .handle_process_memory_copy_request(request(), true)
        .unwrap();
    let frame = alpha
        .processes
        .vm_space_mut(2)
        .unwrap()
        .find_frame(target)
        .unwrap();
    let alias = alpha.map_process_copy_frame(2, frame, None).unwrap();
    for permissions in [0, 1, 3, 0, 7] {
        alpha
            .handle_mapping_protect_request(OsRequestEvent {
                identifier: 2,
                arg0: 2,
                arg1: target,
                arg2: PAGE_SIZE,
                arg3: permissions,
            })
            .unwrap();
        machine(|m| assert_eq!(m.maps.get(&(0, alias)), Some(&frame)));
        for write in [false, true] {
            let before = machine(|m| m.copy_calls);
            let right = if write { 2 } else { 1 };
            if permissions & right == 0 {
                assert_eq!(
                    alpha.handle_process_memory_copy_request(request(), write),
                    Err(CapabilityError::PermissionDenied)
                );
                assert_eq!(machine(|m| m.copy_calls), before);
            } else {
                alpha
                    .handle_process_memory_copy_request(request(), write)
                    .unwrap();
                assert_eq!(machine(|m| m.copy_calls), before + 1);
            }
        }
    }
    let before = machine(|m| m.copy_calls);
    let mut foreign = request();
    foreign.identifier = 3;
    assert_eq!(
        alpha.handle_process_memory_copy_request(foreign, false),
        Err(CapabilityError::PermissionDenied)
    );
    assert_eq!(machine(|m| m.copy_calls), before);
    release(&mut alpha, 1, caller, 2).unwrap();
    release(&mut alpha, 2, peer, 2).unwrap();
    machine(|m| {
        assert!(m.frames.is_empty());
        assert!(m.live.is_empty());
        assert_eq!(m.maps.len(), 2);
        assert_eq!(m.maps.get(&(0, alias)), Some(&frame));
    });
}

#[test]
fn pool_copy_alias_survives_both_guest_aliases_and_recycled_physical_backing() {
    let mut alpha = alpha();
    let mut original_alias = None;
    for _ in 0..20 {
        let (a, b) = create(&mut alpha, 1).unwrap();
        let fa = alpha
            .processes
            .vm_space_mut(1)
            .unwrap()
            .find_frame(a)
            .unwrap();
        let fb = alpha
            .processes
            .vm_space_mut(2)
            .unwrap()
            .find_frame(b)
            .unwrap();
        assert_eq!(fa, fb);
        assert_eq!(fa, pool_frame(0x100));
        let alias = alpha.map_process_copy_frame(1, fa, None).unwrap();
        assert_eq!(alpha.map_process_copy_frame(2, fb, None).unwrap(), alias);
        if let Some(previous) = original_alias {
            assert_eq!(alias, previous);
        }
        original_alias = Some(alias);
        machine(|m| assert_eq!(m.maps.len(), 3));
        release(&mut alpha, 1, a, 1).unwrap();
        machine(|m| {
            assert_eq!(m.maps.len(), 2);
            assert_eq!(m.frames.len(), 1);
            assert_eq!(m.maps[&(0, alias)], fa);
        });
        release(&mut alpha, 2, b, 1).unwrap();
        machine(|m| {
            assert!(m.frames.is_empty());
            assert!(m.live.is_empty());
            assert_eq!(m.maps.len(), 1);
            assert_eq!(m.maps[&(0, alias)], fa);
            // The pool capability/alias is permanent; all guest ownership is gone.
            // Reuse this free physical page for a fresh zeroed allocation next round.
            m.next_page = 0x100;
        });
    }
    assert_eq!(machine(|m| m.zero_calls), 20);
}

#[test]
fn failed_guest_leaf_unmap_keeps_shared_cap_and_backing_live() {
    let mut alpha = alpha();
    let (a, b) = create(&mut alpha, 1).unwrap();
    let frame = alpha
        .processes
        .vm_space_mut(1)
        .unwrap()
        .find_frame(a)
        .unwrap();
    alpha.map_process_copy_frame(1, frame, None).unwrap();
    machine(|m| m.fail_unmap = true);
    assert!(release(&mut alpha, 1, a, 1).is_err());
    machine(|m| {
        assert_eq!(m.maps.len(), 3);
        assert_eq!(m.frames.len(), 2);
        assert_eq!(m.live.len(), 1);
        m.fail_unmap = false;
    });
    release(&mut alpha, 1, a, 1).unwrap();
    release(&mut alpha, 2, b, 1).unwrap();
    machine(|m| {
        assert!(m.frames.is_empty());
        assert!(m.live.is_empty());
        assert_eq!(m.maps.len(), 1); // Only the permanent pool alias remains.
        assert!(m.maps.keys().all(|&(space, _)| space == 0));
    });
}

#[test]
fn every_allocation_copy_and_map_failure_rolls_back_both_sides() {
    let mut baseline = alpha();
    let (a, b) = create(&mut baseline, 3).unwrap();
    let operations = machine(|m| m.operation);
    release(&mut baseline, 1, a, 3).unwrap();
    release(&mut baseline, 2, b, 3).unwrap();
    for fail_at in 1..=operations {
        let mut alpha = alpha();
        machine(|m| m.fail_at = Some(fail_at));
        assert!(create(&mut alpha, 3).is_err(), "failure point {fail_at}");
        assert_empty();
        machine(|m| m.fail_at = None);
        let (a, b) = create(&mut alpha, 3).unwrap();
        assert_eq!((a, b), (0x200000, 0x200000));
        release(&mut alpha, 1, a, 3).unwrap();
        release(&mut alpha, 2, b, 3).unwrap();
        assert_empty();
    }
}

#[test]
fn one_side_can_reuse_slots_while_the_peer_keeps_the_old_mapping() {
    let mut alpha = alpha();
    let (a, b) = create(&mut alpha, 3).unwrap();
    release(&mut alpha, 1, a, 3).unwrap();
    let (next_a, next_b) = create(&mut alpha, 3).unwrap();
    assert_eq!(next_a, a);
    assert_ne!(next_b, b);
    machine(|m| assert_eq!(m.live.len(), 2));
    release(&mut alpha, 2, b, 3).unwrap();
    machine(|m| {
        assert_eq!(m.live.len(), 1);
        assert_eq!(m.maps.len(), 6);
    });
    release(&mut alpha, 2, next_b, 3).unwrap();
    release(&mut alpha, 1, next_a, 3).unwrap();
    assert_empty();
}

#[test]
fn missing_peer_recycles_caller_reservation_without_allocating_ram() {
    let mut alpha = alpha();
    assert!(alpha
        .handle_shared_memory_request(OsRequestEvent {
            identifier: 1,
            arg0: 99,
            arg1: PAGE_SIZE,
            arg2: 0,
            arg3: 0,
        })
        .is_err());
    assert_empty();
    assert_eq!(create(&mut alpha, 1).unwrap(), (0x200000, 0x200000));
}

#[test]
fn overlap_failure_does_not_unmap_an_existing_fixed_mapping() {
    let mut alpha = alpha();
    let peer = alpha.processes.find_entry_by_pid(2).unwrap();
    let existing_frame = 0xfeed;
    machine(|m| {
        m.maps
            .insert((peer.address_space, 0x200000), existing_frame);
    });
    alpha
        .processes
        .vm_space_mut(2)
        .unwrap()
        .record_frame(0x200000, existing_frame)
        .unwrap();
    assert!(create(&mut alpha, 3).is_err());
    machine(|m| {
        assert!(m.frames.is_empty());
        assert!(m.live.is_empty());
        assert_eq!(m.maps.len(), 1);
        assert_eq!(m.maps[&(peer.address_space, 0x200000)], existing_frame);
    });
    assert_eq!(
        alpha
            .processes
            .vm_space_mut(2)
            .unwrap()
            .find_frame(0x200000),
        Some(existing_frame)
    );
}

#[test]
fn cleanup_failure_does_not_free_ram_or_recycle_still_owned_slots() {
    for fail_unmap in [true, false] {
        let mut alpha = alpha();
        let (a, b) = create(&mut alpha, 3).unwrap();
        machine(|m| {
            m.fail_unmap = fail_unmap;
            m.fail_remove = !fail_unmap;
        });
        assert!(release(&mut alpha, 1, a, 3).is_err());
        machine(|m| {
            assert_eq!(m.freed, 0);
            m.fail_unmap = false;
            m.fail_remove = false;
        });
        release(&mut alpha, 2, b, 3).unwrap();
        machine(|m| {
            assert_eq!(m.freed, 0);
            assert_eq!(m.live.len(), 1);
        });
        let (next, _) = create(&mut alpha, 3).unwrap();
        assert_ne!(next, a);
        release(&mut alpha, 1, a, 3).unwrap();
    }
}
