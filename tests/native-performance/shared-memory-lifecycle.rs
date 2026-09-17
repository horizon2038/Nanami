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

#[derive(Default)]
struct Machine {
    frames: BTreeMap<usize, usize>,
    maps: BTreeMap<(usize, usize), usize>,
    live: BTreeMap<usize, usize>,
    zeroed: BTreeSet<usize>,
    next_page: usize,
    freed: usize,
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

mod arch {
    use super::*;
    pub mod node {
        use super::*;
        pub fn copy(node: usize, slot: Word, source: usize) -> Result<(), CapabilityError> {
            machine(|m| {
                operation(m)?;
                assert!(
                    m.frames.insert(node + slot, source).is_none(),
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
        pub fn unmap(space: usize, frame: usize, va: usize) -> Result<(), CapabilityError> {
            machine(|m| {
                if m.fail_unmap {
                    return Err(CapabilityError::IllegalOperation);
                }
                if let Some(mapped) = m.maps.remove(&(space, va)) {
                    assert_eq!(mapped, frame);
                }
                Ok(())
            })
        }
    }
}

struct Memory;
impl Memory {
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
    fn ensure_alpha_frame_at_physical_index(&mut self, _: usize) -> Result<(), CapabilityError> {
        machine(operation)
    }
    fn physical_frame_descriptor_from_index(&self, page: usize) -> Option<usize> {
        Some(page)
    }
    fn map_frame_strict(
        &mut self,
        space: usize,
        frame: usize,
        va: usize,
        vm: &mut vm_space::VmSpace,
    ) -> Result<(), CapabilityError> {
        machine(|m| {
            let page = *m.frames.get(&frame).expect("map without capability");
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

struct OsRequestEvent {
    identifier: usize,
    arg0: usize,
    arg1: usize,
}
struct Alpha {
    processes: ProcessManager,
    memory: Memory,
}
impl Alpha {
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
            for i in 0..pages {
                let page = m.frames[&process_frame_descriptor(root, slot + i)];
                m.zeroed.insert(page);
            }
        });
        Ok(())
    }
}

#[path = "../../nanami/src/nanami_core/alpha/shared_memory.rs"]
mod implementation;

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
    }
}

fn create(alpha: &mut Alpha, pages: usize) -> Result<(usize, usize), CapabilityError> {
    alpha.handle_shared_memory_request(OsRequestEvent {
        identifier: 1,
        arg0: 2,
        arg1: pages * PAGE_SIZE,
    })
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
            arg1: PAGE_SIZE
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
