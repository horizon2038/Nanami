//! Exercise the production shared-memory transaction with real ProcessManager
//! bookkeeping and checked capability/mapping mocks (not hardware TLB tests).
use super::*;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    ptr,
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
    initialization_calls: usize,
    large_initialization: bool,
    large_zero_initializations: usize,
    user_map_calls: usize,
    copy_calls: usize,
    cap_copy_calls: usize,
    chunk_calls: usize,
    descriptor_lookups: usize,
    operation: usize,
    fail_at: Option<usize>,
    fail_unmap: bool,
    fail_remove: bool,
    fail_copy_unmap: bool,
    fail_free: bool,
    unmap_calls: usize,
    remove_calls: usize,
    fail_unmap_at: Option<usize>,
    fail_remove_at: Option<usize>,
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
                m.cap_copy_calls += 1;
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
                m.remove_calls += 1;
                if m.fail_remove || m.fail_remove_at == Some(m.remove_calls) {
                    return Err(CapabilityError::IllegalOperation);
                }
                assert!(
                    !m.maps.values().any(|frame| *frame == node + slot),
                    "removed mapped capability"
                );
                // A9N capability_node::remove_slot also succeeds for an empty
                // slot. Retrying a partially completed release relies on it.
                m.frames.remove(&(node + slot));
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
                if space != 0 { m.user_map_calls += 1; }
                operation(m)?;
                assert!(m.pool_frames.contains_key(&frame));
                assert!(m.maps.insert((space, va), frame).is_none());
                m.permissions.insert((space, va), attributes.bits());
                Ok(())
            })
        }
        pub fn unmap(space: usize, frame: usize, va: usize) -> Result<(), CapabilityError> {
            machine(|m| {
                m.unmap_calls += 1;
                if m.fail_unmap
                    || m.fail_unmap_at == Some(m.unmap_calls)
                    || (space == 0 && m.fail_copy_unmap)
                {
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
    fn map_private_large_alias(&mut self, _: usize, _: Option<usize>) -> Result<Option<(usize, usize)>, CapabilityError> { Ok(None) }
    fn ensure_page_tables(
        &mut self,
        _: usize,
        _: usize,
        _: &mut vm_space::VmSpace,
    ) -> Result<(), CapabilityError> {
        machine(operation)
    }
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
            if m.fail_free { return Err(CapabilityError::Fatal); }
            let page = base / PAGE_SIZE;
            let count = bytes / PAGE_SIZE;
            assert!(!m.maps.iter().any(|(&(space, _), frame)| space != 0
                && m.frames.get(frame).or_else(|| m.pool_frames.get(frame))
                    .is_some_and(|physical| (page..page + count).contains(physical))),
                "freed mapped RAM");
            assert!(
                !m.frames
                    .values()
                    .any(|&p| (page..page + count).contains(&p)),
                "freed referenced RAM"
            );
            // PhysicalAllocator::free accepts an interior subrange and keeps
            // the surrounding allocation records. Model that exact lifetime,
            // not an exact-base-only mock which forbids per-page retirement.
            let (&base, &length) = m
                .live
                .range(..=page)
                .next_back()
                .expect("double free / unknown allocation");
            assert!(
                count != 0 && page + count <= base + length,
                "wrong allocation range"
            );
            m.live.remove(&base);
            if base < page {
                m.live.insert(base, page - base);
            }
            if page + count < base + length {
                m.live.insert(page + count, base + length - page - count);
            }
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
    fn allocation_frame_hint(&self, first: usize, pages: usize) -> Option<(usize, usize)> {
        (pages >= 256).then_some((first, pages))
    }
    fn ensure_alpha_frame_for_allocation(&mut self, page: usize, hint: Option<(usize, usize)>) -> Result<(), CapabilityError> {
        if let Some((first, pages))=hint { assert!(page>=first && page-first<pages); }
        self.ensure_alpha_frame_at_physical_index(page)
    }
    fn ensure_alpha_frames_for_allocation(&mut self, first: usize, pages: usize) -> Result<(), CapabilityError> {
        let hint = self.allocation_frame_hint(first, pages);
        for page in first..first + pages {
            self.ensure_alpha_frame_for_allocation(page, hint)?;
        }
        Ok(())
    }
    fn read_ready_pool_frames(&self, first: usize, frames: &mut [usize]) -> Result<(), CapabilityError> {
        for (offset, frame) in frames.iter_mut().enumerate() {
            *frame = self.physical_frame_descriptor_from_index(first + offset)
                .ok_or(CapabilityError::InvalidArgument)?;
        }
        Ok(())
    }
    fn physical_frame_descriptor_from_index(&self, page: usize) -> Option<usize> {
        machine(|m| {
            m.descriptor_lookups += 1;
            m.pool_frames.contains_key(&pool_frame(page))
        }).then_some(pool_frame(page))
    }
    fn map_frame_strict(
        &mut self,
        space: usize,
        frame: usize,
        va: usize,
        vm: &mut vm_space::VmSpace,
    ) -> Result<(), CapabilityError> {
        self.map_frame_strict_with_attribute(space, frame, va,
            a9n_abi::capability_call::address_space::Attribute::from_bits(7).unwrap(), vm)
    }
    fn map_frame_strict_with_attribute(
        &mut self, space: usize, frame: usize, va: usize,
        attributes: a9n_abi::capability_call::address_space::Attribute,
        vm: &mut vm_space::VmSpace,
    ) -> Result<(), CapabilityError> {
        machine(|m| {
            if space != 0 { m.user_map_calls += 1; }
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
            if attributes.bits() != 7 { m.permissions.insert((space, va), attributes.bits()); }
            // Fail AFTER installing the PTE, as a TLB update error could do.
            operation(m)
        })?;
        vm.record_frame(va, frame).unwrap();
        // Like production Memory, record only the Frame. The constructor must
        // record its logical rights separately from hardware leaf attributes.
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
#[derive(Clone, Copy)]
struct PageSource { pid: usize, address: usize, bytes: usize }
struct Alpha {
    processes: ProcessManager,
    memory: Memory,
    copy_window: memory_copy::CopyWindow,
    descriptor: Vec<u8>,
}
fn response_from_status_result(result: Result<(), CapabilityError>) -> (Word, Word, Word) {
    use nanami_uapi::os::*;
    (match result {
        Ok(()) => OS_RESPONSE_OK,
        Err(CapabilityError::InvalidArgument) => OS_RESPONSE_INVALID_ARGUMENT,
        Err(CapabilityError::PermissionDenied) => OS_RESPONSE_PERMISSION_DENIED,
        Err(CapabilityError::IllegalOperation) => OS_RESPONSE_ILLEGAL_OPERATION,
        Err(CapabilityError::Fatal) => OS_RESPONSE_FATAL,
    }, 0, 0)
}
impl Alpha {
    fn split_private_large_mappings(&mut self, _: usize, _: usize, _: usize, _: bool) -> Result<(), CapabilityError> { Ok(()) }
    fn prepare_large_protection(&mut self, _: usize, _: usize, _: usize, _: usize)
        -> Result<Vec<mapping_protection::LargeProtection>, CapabilityError> { Ok(Vec::new()) }
    fn apply_large_protection(&mut self, _: usize, _: usize, _: &[mapping_protection::LargeProtection],
        _: core::ops::Range<usize>, _: usize) -> Result<(), CapabilityError> { unreachable!() }
    fn validate_owned_memory_initializer(&mut self, source: PageSource, capacity: usize) -> Result<(), CapabilityError> {
        if source.address == 0 || source.bytes == 0 || source.bytes > capacity { return Err(CapabilityError::InvalidArgument); }
        let end = source.address.checked_add(source.bytes).ok_or(CapabilityError::InvalidArgument)?;
        for va in (align_down(source.address, PAGE_SIZE)..end).step_by(PAGE_SIZE) { self.process_frame_for_page(source.pid, va)?; }
        Ok(())
    }
    // Model only initialization order/failure here; alpha-private-pages runs
    // the actual production byte initializer with real host page contents.
    fn initialize_owned_with_large_frames(&mut self, first: usize, pages: usize, source: Option<PageSource>)
        -> Result<bool, CapabilityError> {
        if pages < 512 || !machine(|m| m.large_initialization) { return Ok(false); }
        self.memory.ensure_alpha_frames_for_allocation(first, pages)?;
        if let Some(source) = source {
            self.initialize_owned_physical_pages(first, pages, source)?;
        } else {
            // Model successful zeroing plus demotion as one operation. This
            // fixture checks the transaction; alpha-private-pages checks bytes.
            machine(|m| {
                m.initialization_calls += 1;
                m.large_zero_initializations += 1;
                operation(m)?;
                assert!(m.maps.values().all(|frame| {
                    let page = frame_page(m, *frame);
                    page < first || page >= first + pages
                }));
                for page in first..first + pages { m.zeroed.insert(page); }
                Ok(())
            })?;
        }
        Ok(true)
    }
    fn initialize_owned_physical_pages(&mut self, first: usize, pages: usize, _: PageSource) -> Result<(), CapabilityError> {
        machine(|m| {
            m.initialization_calls += 1;
            operation(m)?;
            for p in first..first+pages { assert!(m.pool_frames.contains_key(&pool_frame(p))); m.zeroed.insert(p); }
            Ok(())
        })
    }
    // This fixture models zeroing without constructing initialization aliases;
    // the copy-window suite combines both production window implementations.
    fn cached_initialization_frame(&self, _: usize) -> Option<usize> { None }
    fn read_process_memory_into_static_buffer(
        &mut self,
        _: usize,
        address: usize,
        bytes: usize,
    ) -> Result<&[u8], CapabilityError> {
        let start = address.checked_sub(1).ok_or(CapabilityError::InvalidArgument)?;
        let end = start.checked_add(bytes).ok_or(CapabilityError::InvalidArgument)?;
        self.descriptor.get(start..end).ok_or(CapabilityError::InvalidArgument)
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
    fn map_process_initialized_pages_with_stack_slot(
        &mut self, _: Word, _: Option<Word>, _: Word, _: Option<()>, _: Word,
        _: Option<crate::nanami_core::process::FreshZeroFrameSlot>,
    ) -> Result<Word, CapabilityError> {
        // This shared-memory harness does not construct private zero backing.
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
            assert!(m.pool_frames.contains_key(&frame) || m.frames.contains_key(&frame));
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
        machine(|m| { m.chunk_calls += 1; operation(m) })
    }
    fn process_root_slot(&self, root: usize) -> Result<usize, CapabilityError> {
        if root == 0 { Err(CapabilityError::InvalidArgument) } else { Ok(root) }
    }
    fn zero_physical_pages(
        &mut self,
        first: usize,
        pages: usize,
    ) -> Result<(), CapabilityError> {
        machine(|m| {
            m.zero_calls += 1;
            for i in 0..pages {
                let page = first + i;
                assert!(m.pool_frames.values().any(|&pool| pool == page));
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
mod implementation {
    include!("../../nanami/src/nanami_core/alpha/shared_memory.rs");

    // Build the historical cap-backed ownership kind with production setup.
    // Current buffer/view/share APIs all use pool references; legacy cleanup
    // must still retain RAM until child-cap removals have acknowledged.
    pub(super) fn legacy_create(alpha: &mut Alpha, pages: usize)
        -> Result<(usize, usize), CapabilityError>
    {
        let mut mappings = [alpha.reserve_shared_mapping(1, pages, true)?,
            alpha.reserve_shared_mapping(2, pages, true)?];
        let first = alpha.memory.allocate_physical_any(pages * PAGE_SIZE)?;
        for mapping in &mappings {
            let r = mapping.reservation;
            alpha.processes.register_physical_allocation(mapping.entry.pid,
                r.base_va, r.start_slot, first, pages)?;
        }
        alpha.install_shared_memory(&mut mappings, first, None)?;
        Ok((mappings[0].reservation.base_va, mappings[1].reservation.base_va))
    }
}
#[path = "../../nanami/src/nanami_core/alpha/mapping_protection.rs"]
mod mapping_protection;
#[path = "../../nanami/src/nanami_core/alpha/memory_snapshot.rs"]
mod memory_snapshot;
#[path = "../../nanami/src/nanami_core/alpha/mapping_release.rs"]
mod mapping_release;

fn release_vector_request(alpha: &mut Alpha, locals: &[Word]) -> OsRequestEvent {
    alpha.descriptor = locals.iter().flat_map(|local| local.to_ne_bytes()).collect();
    OsRequestEvent { identifier: 1, arg0: 1, arg1: locals.len(), arg2: 0, arg3: 0 }
}

#[test]
fn release_vectors_acknowledge_independent_unmaps_and_retry_confirmed_holes() {
    for failure in [None, Some(1), Some(2), Some(3)] {
        let mut alpha = alpha();
        let source = private_source(&mut alpha);
        let allocation = alpha.processes.find_active_physical_allocation_reference(2, source, 1).unwrap();
        let locals = [0x8000_0000, 0x8001_0000, 0x8002_0000];
        for local in locals { alpha.handle_process_page_view_request(view_request(source, local), true).unwrap(); }
        machine(|m| m.fail_unmap_at = failure.map(|index| m.unmap_calls + index));
        let request = release_vector_request(&mut alpha, &locals);
        let expected = failure.map_or(7, |index| 7 ^ (1 << (index - 1)));
        assert_eq!(alpha.handle_release_views_request(request).unwrap(), (expected, 3));
        assert_eq!(alpha.processes.physical_reference_count(allocation), 1 + usize::from(failure.is_some()));
        for (index, local) in locals.into_iter().enumerate() {
            assert_eq!(alpha.processes.shared_memory_reservation(1, local, 1).is_some(), failure == Some(index + 1));
        }
        machine(|m| m.fail_unmap_at = None);
        // This is also the retry after the first response was lost: already
        // released holes and a still-pinned failure each acknowledge safely.
        assert_eq!(alpha.handle_release_views_request(request).unwrap(), (7, 3));
        assert_eq!(alpha.processes.physical_reference_count(allocation), 1);
        assert_eq!(alpha.processes.vm_space(2).unwrap().permissions(source), 7);
    }
}

#[test]
fn release_vector_validates_the_whole_header_before_any_unmap() {
    for invalid in 0..9 {
        let mut alpha = alpha();
        let source = private_source(&mut alpha);
        let local = 0x8000_0000;
        alpha.handle_process_page_view_request(view_request(source, local), false).unwrap();
        let mut request = release_vector_request(&mut alpha, &[local, local]);
        match invalid {
            0 => {},
            1 => request.arg1 = 0,
            2 => request.arg1 = nanami_uapi::os::OS_PAGE_VIEW_VECTOR_MAX + 1,
            3 => request.arg0 = 0,
            4 => { alpha.descriptor.pop(); },
            5 => request.arg2 = 2,
            6 => request.arg3 = 1,
            7 => request.identifier = 0,
            _ => request = release_vector_request(&mut alpha, &[local, local + 1]),
        }
        let before = machine(|m| m.unmap_calls);
        assert!(alpha.handle_release_views_request(request).is_err());
        assert_eq!(machine(|m| m.unmap_calls), before);
        assert!(alpha.processes.shared_memory_reservation(1, local, 1).is_some());
        release(&mut alpha, 1, local, 1).unwrap();
    }
}

#[test]
fn release_vector_refuses_partial_objects_but_still_releases_independent_views() {
    let mut alpha = alpha();
    let source = private_source(&mut alpha);
    let (caller, _) = alpha.handle_shared_memory_request(owned_request(2 * PAGE_SIZE, 0, 0)).unwrap();
    let local = 0x8000_0000;
    alpha.handle_process_page_view_request(view_request(source, local), false).unwrap();
    let request = release_vector_request(&mut alpha, &[caller, local, caller + PAGE_SIZE]);
    assert_eq!(alpha.handle_release_views_request(request).unwrap(), (2, 3));
    assert!(alpha.processes.shared_memory_reservation(1, caller, 2).is_some());
    assert!(alpha.processes.shared_memory_reservation(1, local, 1).is_none());
    for offset in [0, PAGE_SIZE] {
        assert!(machine(|m| m.maps.contains_key(&(alpha.processes.memory_entry(1).unwrap().address_space, caller + offset))));
    }
    release(&mut alpha, 1, caller, 2).unwrap();
}

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
        descriptor: Vec::new(),
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

// Retain historical cap-removal ACK coverage after all shared mapping APIs
// have stopped creating these unused capabilities.
fn legacy_create(alpha: &mut Alpha, pages: usize) -> Result<(usize,usize),CapabilityError> {
    implementation::legacy_create(alpha, pages)
}

fn owned_request(bytes: usize, source: usize, copied: usize) -> OsRequestEvent {
    OsRequestEvent { identifier:1,arg0:0,arg1:bytes,arg2:source,arg3:copied }
}

fn large_arena() -> Alpha {
    let mut a=alpha();let e=a.processes.find_entry_by_pid(1).unwrap();
    a.processes.install_process(1,e.reaper_pid,e.root_slot,e.root_node,e.pcb,
        e.address_space,e.os_port,e.os_port_identifier,e.affinity,e.next_frame_slot,
        e.user_heap_next_va,0x1000000).unwrap();
    a
}

#[test]
fn immutable_source_admission_checks_whole_mixed_reservation_with_permission_words() {
    use crate::vm_space::VmSpace;
    let mut a = large_arena();
    let pages = 1024;
    let (input, _) = a.handle_shared_memory_request(owned_request(pages * PAGE_SIZE, 0, 0)).unwrap();
    let (source, _) = a.handle_memory_source_request(owned_request(pages * PAGE_SIZE, input, pages * PAGE_SIZE)).unwrap();
    for i in [0, 63, 64, 511, 1023] {
        a.handle_mapping_protect_request(OsRequestEvent { identifier: 1, arg0: 1,
            arg1: source + i * PAGE_SIZE, arg2: PAGE_SIZE, arg3: 1 }).unwrap();
    }
    let before = machine(|m| (m.user_map_calls, m.unmap_calls, m.cap_copy_calls));
    VmSpace::take_immutable_permission_counts();
    // Even a one-page request rechecks the complete immutable reservation.
    a.check_private_snapshot_source(1, source, PAGE_SIZE).unwrap();
    let counts = VmSpace::take_immutable_permission_counts();
    assert_eq!(counts, (0, 16));
    assert_eq!(machine(|m| (m.user_map_calls, m.unmap_calls, m.cap_copy_calls)), before);
    for rights in [2, 4, 7, 17, usize::MAX] {
        a.processes.vm_space_mut(1).unwrap().set_permissions(source + (pages - 1) * PAGE_SIZE, rights);
        assert_eq!(a.check_private_snapshot_source(1, source, PAGE_SIZE), Err(CapabilityError::PermissionDenied));
    }
    a.processes.vm_space_mut(1).unwrap().set_permissions(source + (pages - 1) * PAGE_SIZE, 1);
    a.check_private_snapshot_source(1, source, PAGE_SIZE).unwrap();
    println!("IMMUTABLE-SOURCE-ADMISSION reservation_pages=1024 request_pages=1 point_queries=0 permission_words=16 maps_added=0");
    release(&mut a, 1, source, pages).unwrap();
    release(&mut a, 1, input, pages).unwrap();
    assert_empty();
}

#[test]
fn unmapped_immutable_source_removes_every_native_map_and_releases_without_unmaps() {
    for pages in [1,64,378,512,775,858,1024] {
        let mut a=large_arena();
        let (input,_)=a.handle_shared_memory_request(owned_request(pages*PAGE_SIZE,0,0)).unwrap();
        let before=machine(|m|(m.user_map_calls,m.unmap_calls,m.cap_copy_calls,m.initialization_calls));
        let (source,peer)=a.handle_memory_source_request(owned_request(pages*PAGE_SIZE,input,pages*PAGE_SIZE-17)).unwrap();
        assert_eq!(peer,0);
        let r=a.processes.shared_memory_reservation(1,source,pages).unwrap();
        assert!(r.immutable_source());assert!(!r.owns_frame_caps());
        let vm=a.processes.vm_space(1).unwrap();
        for page in 0..pages {
            let va=source+page*PAGE_SIZE;
            assert!(vm.find_frame(va).is_some());assert_eq!(vm.permissions(va),0);
            assert!(vm.private_page_shared(va));assert!(!vm.frame_may_be_present(va));
            assert!(!vm.page_tables_ready(va));
        }
        machine(|m|assert_eq!((m.user_map_calls,m.unmap_calls,m.cap_copy_calls,m.initialization_calls),
            (before.0,before.1,before.2,before.3+1)));
        let descriptors = machine(|m| m.descriptor_lookups);
        release(&mut a,1,source,pages).unwrap();
        assert_eq!(machine(|m|m.unmap_calls),before.1);
        assert_eq!(machine(|m| m.descriptor_lookups), descriptors);
        // The uniquely owned object is retired as one whole extent.
        assert_eq!(machine(|m|m.freed),1);
        release(&mut a,1,input,pages).unwrap();assert_empty();
        println!("UNMAPPED-SOURCE pages={pages} native-map-before={pages} native-map-after=0");
    }
}

#[test]
fn immutable_source_collision_rolls_back_complete_words_without_forgetting_the_collision() {
    const WORD_PAGES: usize = vm_space::FRAME_WORD_PAGES;
    let mut alpha = large_arena();
    let pages = 3 * WORD_PAGES;
    let bytes = pages * PAGE_SIZE;
    let (input, _) = alpha.handle_shared_memory_request(owned_request(bytes, 0, 0)).unwrap();
    let base = input + bytes;
    let collision = base + (2 * WORD_PAGES - 1) * PAGE_SIZE;
    alpha.processes.vm_space_mut(1).unwrap().record_frame(collision, 123).unwrap();

    let unmaps = machine(|m| m.unmap_calls);
    let request = owned_request(bytes, input, bytes);
    assert_eq!(alpha.handle_memory_source_request(request), Err(CapabilityError::IllegalOperation));
    assert!(alpha.processes.shared_memory_reservation(1, base, pages).is_none());
    let vm = alpha.processes.vm_space_mut(1).unwrap();
    assert_eq!(vm.forget_frame(collision), Some(123));
    assert!(!vm.has_backed_aligned_pages(base, base + bytes));
    assert!(!vm.has_deferred_mappings(base, base + bytes));
    machine(|m| {
        assert_eq!(m.unmap_calls, unmaps);
        assert_eq!(m.live.len(), 1);
    });

    let (source, _) = alpha.handle_memory_source_request(request).unwrap();
    assert_eq!(source, base);
    let vm = alpha.processes.vm_space(1).unwrap();
    assert!(vm.fully_backed_aligned_range(base, base + bytes));
    for va in (base..base + bytes).step_by(PAGE_SIZE) {
        assert_eq!(vm.permissions(va), 0);
        assert!(vm.private_page_shared(va));
        assert!(!vm.snapshot_page_managed(va));
        assert!(!vm.frame_may_be_present(va));
    }
    release(&mut alpha, 1, source, pages).unwrap();
    release(&mut alpha, 1, input, pages).unwrap();
    assert_empty();
}

#[test]
fn immutable_source_optional_native_read_mapping_cannot_gain_write_or_writable_peer() {
    let mut a=alpha();let(input,_)=a.handle_shared_memory_request(owned_request(4*PAGE_SIZE,0,0)).unwrap();
    let(source,_)=a.handle_memory_source_request(owned_request(4*PAGE_SIZE,input,4*PAGE_SIZE)).unwrap();
    let before=machine(|m|m.user_map_calls);
    for permissions in [2,3,4,5,7] {
        assert!(a.handle_mapping_protect_request(OsRequestEvent {identifier:1,arg0:1,arg1:source,
            arg2:4*PAGE_SIZE,arg3:permissions}).is_err());
    }
    assert!(share(&mut a,1,2,source,4*PAGE_SIZE).is_err());
    assert_eq!(machine(|m|m.user_map_calls),before);
    a.handle_mapping_protect_request(OsRequestEvent {identifier:1,arg0:1,arg1:source+PAGE_SIZE,
        arg2:PAGE_SIZE,arg3:1}).unwrap();
    assert_eq!(machine(|m|m.user_map_calls),before+1);
    a.handle_mapping_protect_request(OsRequestEvent {identifier:1,arg0:1,arg1:source,
        arg2:4*PAGE_SIZE,arg3:1}).unwrap();
    assert_eq!(machine(|m|m.user_map_calls),before+4);
    assert!(a.processes.vm_space(1).unwrap().readonly_backed_range(source,source+4*PAGE_SIZE));
    assert!(share(&mut a,1,2,source,4*PAGE_SIZE).is_err());
    release(&mut a,1,source,4).unwrap();release(&mut a,1,input,4).unwrap();assert_empty();
}

#[test]
fn immutable_source_native_map_failure_rolls_back_and_release_keeps_unacked_backing() {
    for offset in [1,2,4] {
        let mut a=alpha();let(input,_)=a.handle_shared_memory_request(owned_request(3*PAGE_SIZE,0,0)).unwrap();
        let(source,_)=a.handle_memory_source_request(owned_request(3*PAGE_SIZE,input,3*PAGE_SIZE)).unwrap();
        machine(|m|m.fail_at=Some(m.operation+offset));
        assert!(a.handle_mapping_protect_request(OsRequestEvent {identifier:1,arg0:1,arg1:source,arg2:3*PAGE_SIZE,arg3:1}).is_err());
        machine(|m|m.fail_at=None);
        let vm=a.processes.vm_space(1).unwrap();
        for page in 0..3 {assert_eq!(vm.permissions(source+page*PAGE_SIZE),0);assert!(!vm.frame_may_be_present(source+page*PAGE_SIZE));}
        a.handle_mapping_protect_request(OsRequestEvent {identifier:1,arg0:1,arg1:source,arg2:3*PAGE_SIZE,arg3:1}).unwrap();
        machine(|m|m.fail_unmap=true);
        assert!(release(&mut a,1,source,3).is_err());assert_eq!(machine(|m|m.freed),0);
        assert!(a.processes.shared_memory_reservation(1,source,3).is_some());
        machine(|m|m.fail_unmap=false);
        release(&mut a,1,source,3).unwrap();release(&mut a,1,input,3).unwrap();assert_empty();
    }
}

#[test]
fn immutable_source_creation_failures_and_bad_arguments_never_leave_native_pte() {
    for fail in 1..=5 {
        let mut a=alpha();let(input,_)=a.handle_shared_memory_request(owned_request(3*PAGE_SIZE,0,0)).unwrap();
        let before=machine(|m|m.maps.clone());
        machine(|m|m.fail_at=Some(m.operation+fail));
        assert!(a.handle_memory_source_request(owned_request(3*PAGE_SIZE,input,3*PAGE_SIZE)).is_err());
        machine(|m|{assert_eq!(m.maps,before);assert_eq!(m.live.len(),1);m.fail_at=None;});
        release(&mut a,1,input,3).unwrap();assert_empty();
    }
    for case in 0..6 {
        let mut a=alpha();let(input,_)=a.handle_shared_memory_request(owned_request(PAGE_SIZE,0,0)).unwrap();
        let mut req=owned_request(PAGE_SIZE,input,1);
        match case {0=>req.arg0=2,1=>req.arg2=0,2=>req.arg3=0,3=>req.arg3=PAGE_SIZE+1,
            4=>{a.processes.seal_capabilities(1).unwrap();},_=>req.identifier=99}
        let before=machine(|m|m.operation);
        assert!(a.handle_memory_source_request(req).is_err());assert_eq!(machine(|m|m.operation),before);
        release(&mut a,1,input,1).unwrap();assert_empty();
    }
}

#[test]
fn private_snapshot_from_unmapped_source_survives_source_retirement_in_both_orders() {
    for source_first in [false,true] {
        let mut a=alpha();let(input,_)=a.handle_shared_memory_request(owned_request(3*PAGE_SIZE,0,0)).unwrap();
        let(source,_)=a.handle_memory_source_request(owned_request(3*PAGE_SIZE,input,3*PAGE_SIZE)).unwrap();
        let e=a.processes.find_entry_by_pid(2).unwrap();
        a.processes.install_process(2,1,e.root_slot,e.root_node,e.pcb,e.address_space,e.os_port,
            e.os_port_identifier,e.affinity,e.next_frame_slot,e.user_heap_next_va,e.user_heap_limit_va).unwrap();
        a.processes.seal_capabilities(2).unwrap();
        let before=machine(|m|m.user_map_calls);
        a.map_process_private_snapshot(1,2,source,0x500000,3*PAGE_SIZE,5).unwrap();
        assert_eq!(machine(|m|m.user_map_calls),before+3);
        let reservation=a.processes.shared_memory_reservation(1,source,3).unwrap();
        assert!(reservation.immutable_source());assert!(reservation.has_page_references());
        for page in a.processes.physical_pages_in_range(2,0x500000,0x503000) {assert_eq!(a.processes.physical_reference_count(page),2);}
        if source_first {release(&mut a,1,source,3).unwrap();assert_eq!(machine(|m|m.freed),0);}
        let space=a.processes.memory_context(2).unwrap().address_space;
        for page in a.processes.physical_pages_in_range(2,0x500000,0x503000) {
            let frame=a.processes.vm_space(2).unwrap().find_frame(page.base_va).unwrap();
            arch::address_space::unmap(space,frame,page.base_va).unwrap();
            a.processes.vm_space_mut(2).unwrap().forget_frame(page.base_va);
            let memory=&mut a.memory;
            a.processes.retire_active_physical_allocation(2,page.base_va,1,|allocation,last| {
                if last {memory.free_physical(allocation.base_page*PAGE_SIZE,PAGE_SIZE)} else {Ok(())}
            }).unwrap();
        }
        if !source_first {assert_eq!(machine(|m|m.freed),0);release(&mut a,1,source,3).unwrap();}
        release(&mut a,1,input,3).unwrap();assert_empty();
    }
}

#[test]
fn readonly_owned_snapshot_is_initialized_before_mapping_and_reclaims_normally() {
    let mut a=alpha();let(source,_)=a.handle_shared_memory_request(owned_request(2*PAGE_SIZE,0,0)).unwrap();
    let zero_calls=machine(|m|m.zero_calls);
    let(target,peer)=a.handle_shared_memory_request(owned_request(3*PAGE_SIZE,source+3,PAGE_SIZE+1)).unwrap();
    assert_eq!(peer,0);assert_ne!(source,target);
    let space=a.processes.memory_entry(1).unwrap().address_space;
    let vm=a.processes.vm_space(1).unwrap();
    assert!(vm.readonly_backed_range(target,target+3*PAGE_SIZE));
    machine(|m|{assert_eq!(m.initialization_calls,1);assert_eq!(m.zero_calls,zero_calls);
        for i in 0..3 {assert_eq!(m.permissions[&(space,target+i*PAGE_SIZE)],1);}
    });
    release(&mut a,1,target,3).unwrap();release(&mut a,1,source,2).unwrap();assert_empty();
}

#[test]
fn owned_snapshot_large_initializer_is_not_repeated_before_readonly_publication() {
    let mut a = alpha();
    let entry = a.processes.find_entry_by_pid(1).unwrap();
    a.processes.install_process(1, entry.reaper_pid, entry.root_slot, entry.root_node,
        entry.pcb, entry.address_space, entry.os_port, entry.os_port_identifier,
        entry.affinity, entry.next_frame_slot, entry.user_heap_next_va, 0x800000).unwrap();
    a.processes.ensure_vm_space_for_pid(1).unwrap();
    let (source, _) = a.handle_shared_memory_request(owned_request(PAGE_SIZE, 0, 0)).unwrap();
    machine(|m| m.large_initialization = true);
    let (target, _) = a.handle_shared_memory_request(owned_request(512 * PAGE_SIZE, source + 3, 17)).unwrap();
    assert!(a.processes.vm_space(1).unwrap().readonly_backed_range(target, target + 512 * PAGE_SIZE));
    machine(|m| { assert_eq!(m.initialization_calls, 1); assert_eq!(m.zeroed.len(), 513); });
    release(&mut a, 1, target, 512).unwrap();
    release(&mut a, 1, source, 1).unwrap();
    assert_empty();
}

#[test]
fn shared_zero_large_initializer_finishes_once_before_either_owner_is_mapped() {
    for peer in [false, true] {
        let mut a = large_arena();
        machine(|m| m.large_initialization = true);
        let (local, remote) = if peer { create(&mut a, 512).unwrap() }
            else { a.handle_shared_memory_request(owned_request(512 * PAGE_SIZE, 0, 0)).unwrap() };
        machine(|m| {
            assert_eq!(m.large_zero_initializations, 1);
            assert_eq!(m.initialization_calls, 1);
            assert_eq!(m.zero_calls, 0);
            assert_eq!(m.zeroed.len(), 512);
            assert_eq!(m.cap_copy_calls, 0);
        });
        for (pid, base) in [(1, local), (2, remote)] {
            if base == 0 { continue; }
            let vm = a.processes.vm_space(pid).unwrap();
            for page in 0..512 { assert_eq!(vm.permissions(base + page * PAGE_SIZE), 7); }
        }
        release(&mut a, 1, local, 512).unwrap();
        if peer {
            machine(|m| assert_eq!(m.freed, 0));
            release(&mut a, 2, remote, 512).unwrap();
        }
        assert_empty();
    }
}

#[test]
fn shared_zero_large_initializer_failure_cleans_unpublished_ram_or_retains_unacknowledged_mapping() {
    // ensure512 Frames, initialize, then each side maps its512 leaves.
    for failure in [1, 512, 513, 514, 1025, 1026, 1537] {
        let mut a = large_arena();
        machine(|m| { m.large_initialization = true; m.fail_at = Some(m.operation + failure); });
        assert!(create(&mut a, 512).is_err(), "failure {failure}");
        machine(|m| m.fail_at = None);
        assert_empty();
    }
    let mut a = large_arena();
    machine(|m| {
        m.large_initialization = true;
        m.fail_at = Some(m.operation + 516);
        m.fail_unmap = true;
    });
    assert!(create(&mut a, 512).is_err());
    machine(|m| {
        assert_eq!(m.large_zero_initializations, 1);
        assert!(!m.maps.is_empty());
        assert_eq!(m.live.len(), 1);
        assert_eq!(m.freed, 0);
    });
}

#[test]
fn readonly_owned_snapshot_rejects_peer_and_bad_source_before_reservation() {
    for mode in 0..5 {
        let mut a=alpha();let(source,_)=a.handle_shared_memory_request(owned_request(2*PAGE_SIZE,0,0)).unwrap();
        let mut req=owned_request(3*PAGE_SIZE,source,4097);
        match mode {0=>req.arg0=2,1=>req.arg2=0,2=>req.arg3=0,3=>req.arg3=3*PAGE_SIZE+1,4=>req.arg2=usize::MAX,_=>unreachable!()}
        let before=machine(|m|(m.operation,m.maps.clone(),m.live.clone()));
        assert!(a.handle_shared_memory_request(req).is_err());
        assert_eq!(machine(|m|(m.operation,m.maps.clone(),m.live.clone())),before);
        release(&mut a,1,source,2).unwrap();assert_empty();
    }
}

#[test]
fn readonly_owned_snapshot_failure_matrix_cleans_prefix_or_retains_mapped_ram() {
    for failure in 1..=8 {
        let mut a=alpha();let(source,_)=a.handle_shared_memory_request(owned_request(2*PAGE_SIZE,0,0)).unwrap();
        let before=machine(|m|m.maps.clone());machine(|m|m.fail_at=Some(m.operation+failure));
        assert!(a.handle_shared_memory_request(owned_request(3*PAGE_SIZE,source,4097)).is_err(),"operation {failure}");
        machine(|m|{assert_eq!(m.maps,before);assert_eq!(m.live.len(),1);m.fail_at=None;});
        release(&mut a,1,source,2).unwrap();assert_empty();
    }
    let mut a=alpha();let(source,_)=a.handle_shared_memory_request(owned_request(2*PAGE_SIZE,0,0)).unwrap();
    machine(|m|{m.fail_at=Some(m.operation+6);m.fail_unmap=true;});
    assert!(a.handle_shared_memory_request(owned_request(3*PAGE_SIZE,source,4097)).is_err());
    machine(|m|{assert_eq!(m.live.len(),2);assert_eq!(m.freed,0);assert!(m.permissions.values().any(|&p|p==1));});
}

#[test]
fn readonly_constructor_halves_leaf_maps_and_removes_all_protection_unmaps() {
    fn arena() -> Alpha {
        let mut a=alpha();let e=a.processes.find_entry_by_pid(1).unwrap();
        // Two live 2MiB objects exceed the small fixture's 2MiB heap.
        a.processes.install_process(1,e.reaper_pid,e.root_slot,e.root_node,e.pcb,
            e.address_space,e.os_port,e.os_port_identifier,e.affinity,
            e.next_frame_slot,e.user_heap_next_va,0x1000000).unwrap();
        a
    }
    for pages in [1,4,64,378,512,775,858] {
        let mut old=arena();
        let(source,_)=old.handle_shared_memory_request(owned_request(pages*PAGE_SIZE,0,0)).unwrap();
        let before=machine(|m|(m.user_map_calls,m.unmap_calls,m.zero_calls));
        let(target,_)=old.handle_shared_memory_request(owned_request(pages*PAGE_SIZE,0,0)).unwrap();
        old.handle_mapping_protect_request(OsRequestEvent{identifier:1,arg0:1,arg1:target,arg2:pages*PAGE_SIZE,arg3:1}).unwrap();
        let baseline=machine(|m|(m.user_map_calls-before.0,m.unmap_calls-before.1,m.zero_calls-before.2));
        assert_eq!(baseline,(2*pages,pages,1));
        release(&mut old,1,target,pages).unwrap();release(&mut old,1,source,pages).unwrap();assert_empty();
        let mut new=arena();
        let(source,_)=new.handle_shared_memory_request(owned_request(pages*PAGE_SIZE,0,0)).unwrap();
        let before=machine(|m|(m.user_map_calls,m.unmap_calls,m.zero_calls));
        let(target,_)=new.handle_shared_memory_request(owned_request(pages*PAGE_SIZE,source,pages*PAGE_SIZE-17)).unwrap();
        assert_eq!(machine(|m|(m.user_map_calls-before.0,m.unmap_calls-before.1,m.zero_calls-before.2)),(pages,0,0));
        assert!(new.processes.vm_space(1).unwrap().readonly_backed_range(target,target+pages*PAGE_SIZE));
        release(&mut new,1,target,pages).unwrap();release(&mut new,1,source,pages).unwrap();assert_empty();
    }
}

fn private_source(alpha: &mut Alpha) -> usize {
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
    alpha.processes.seal_capabilities(2).unwrap();
    alpha.processes.ensure_vm_space_for_pid(2).unwrap();
    let source = 0x200000;
    let (_, _, slot) = alpha
        .processes
        .reserve_process_heap_at(2, source, 1, PAGE_SIZE, PROCESS_FRAME_TOTAL_PAGES)
        .unwrap();
    let page = alpha.memory.allocate_physical_any(PAGE_SIZE).unwrap();
    alpha
        .memory
        .ensure_alpha_frame_at_physical_index(page)
        .unwrap();
    let frame = pool_frame(page);
    arch::node::copy(
        process_frame_chunk_descriptor(entry.root_node, slot / PROCESS_FRAME_CHUNK_PAGES),
        slot % PROCESS_FRAME_CHUNK_PAGES,
        frame,
    )
    .unwrap();
    arch::address_space::map(
        entry.address_space,
        frame,
        source,
        a9n_abi::capability_call::address_space::Attribute::ALL,
    )
    .unwrap();
    alpha
        .processes
        .vm_space_mut(2)
        .unwrap()
        .record_frame(source, process_frame_descriptor(entry.root_node, slot))
        .unwrap();
    alpha
        .processes
        .register_physical_allocation(2, source, slot, page, 1)
        .unwrap();
    source
}

#[test]
fn view_vectors_preserve_each_pin_and_read_write_permissions() {
    for writable in [false, true] {
        let mut alpha = alpha();
        let source = private_source(&mut alpha);
        let allocation = alpha.processes.find_active_physical_allocation_reference(2, source, 1).unwrap();
        let locals = [0x8000_0000, 0x8001_0000];
        let request = view_vector_request(&mut alpha, &[(source, locals[0]), (source, locals[1])]);
        assert_eq!(alpha.handle_process_page_views_request(request, writable).unwrap(), (2, 0));
        assert_eq!(alpha.processes.physical_reference_count(allocation), 3);
        for local in locals {
            assert_eq!(alpha.processes.vm_space(1).unwrap().permissions(local), if writable { 3 } else { 1 });
            release(&mut alpha, 1, local, 1).unwrap();
        }
        assert_eq!(alpha.processes.physical_reference_count(allocation), 1);
        assert_eq!(alpha.processes.vm_space(2).unwrap().permissions(source), 7);
    }
}

fn view_vector_request(alpha: &mut Alpha, entries: &[(Word, Word)]) -> OsRequestEvent {
    alpha.descriptor.clear();
    for &(target, local) in entries {
        alpha.descriptor.extend_from_slice(&target.to_ne_bytes());
        alpha.descriptor.extend_from_slice(&local.to_ne_bytes());
    }
    OsRequestEvent { identifier: 1, arg0: 2, arg1: 1, arg2: entries.len(), arg3: 0 }
}

#[test]
fn view_vectors_stop_at_denial_and_leave_the_unattempted_suffix_unowned() {
    use nanami_uapi::os::OS_RESPONSE_PERMISSION_DENIED;
    let mut alpha = alpha();
    let source = private_source(&mut alpha);
    let allocation = alpha.processes.find_active_physical_allocation_reference(2, source, 1).unwrap();
    let request = view_vector_request(&mut alpha, &[(source, 0x8000_0000), (source, 0x8001_0000),
        (source + PAGE_SIZE, 0x8002_0000), (source, 0x8003_0000)]);
    assert_eq!(alpha.handle_process_page_views_request(request, true).unwrap(), (2, OS_RESPONSE_PERMISSION_DENIED));
    assert_eq!(alpha.processes.physical_reference_count(allocation), 3);
    for local in [0x8002_0000, 0x8003_0000] {
        assert!(alpha.processes.shared_memory_reservation(1, local, 1).is_none());
    }
    for local in [0x8000_0000, 0x8001_0000] { release(&mut alpha, 1, local, 1).unwrap(); }
    assert_eq!(alpha.processes.physical_reference_count(allocation), 1);
}

#[test]
fn view_vector_header_errors_install_no_aliases() {
    for invalid in 0..7 {
        let mut alpha = alpha();
        let source = private_source(&mut alpha);
        let allocation = alpha.processes.find_active_physical_allocation_reference(2, source, 1).unwrap();
        let mut request = view_vector_request(&mut alpha, &[(source, 0x8000_0000), (source, 0x8000_0000)]);
        match invalid {
            0 => {}, // Duplicate local VA.
            1 => request.arg2 = 0,
            2 => request.arg2 = nanami_uapi::os::OS_PAGE_VIEW_VECTOR_MAX + 1,
            3 => request.arg1 = 0,
            4 => { alpha.descriptor.pop(); },
            5 => request.arg3 = 1,
            _ => { request = view_vector_request(&mut alpha, &[(source + 1, 0x8000_0000)]); },
        }
        assert!(alpha.handle_process_page_views_request(request, false).is_err());
        assert_eq!(alpha.processes.physical_reference_count(allocation), 1);
        assert!(alpha.processes.shared_memory_reservation(1, 0x8000_0000, 1).is_none());
    }
}

#[test]
fn view_vector_failed_cleanup_keeps_the_failing_pin_and_reports_the_confirmed_prefix() {
    let mut control = alpha();
    let source = private_source(&mut control);
    let start = machine(|m| m.operation);
    control.handle_process_page_view_request(view_request(source, 0x8000_0000), true).unwrap();
    let per_view = machine(|m| m.operation) - start;
    assert!(per_view > 0);
    let mut alpha = alpha();
    let source = private_source(&mut alpha);
    let allocation = alpha.processes.find_active_physical_allocation_reference(2, source, 1).unwrap();
    let request = view_vector_request(&mut alpha, &[(source, 0x8000_0000), (source, 0x8001_0000), (source, 0x8002_0000)]);
    machine(|m| { m.fail_at = Some(m.operation + 2 * per_view); m.fail_unmap = true; });
    assert_eq!(alpha.handle_process_page_views_request(request, true).unwrap(), (1, nanami_uapi::os::OS_RESPONSE_FATAL));
    assert_eq!(alpha.processes.physical_reference_count(allocation), 3);
    assert!(alpha.processes.shared_memory_reservation(1, 0x8002_0000, 1).is_none());
    machine(|m| { m.fail_at = None; m.fail_unmap = false; });
    for local in [0x8000_0000, 0x8001_0000] { release(&mut alpha, 1, local, 1).unwrap(); }
    assert_eq!(alpha.processes.physical_reference_count(allocation), 1);
}

#[test]
fn shared_pool_mapping_work_preserves_leaf_identity_zeroing_and_references() {
    for pages in [1,64,513,1024] {
        let mut alpha=alpha();
        for pid in 1..=2 {
            let e=alpha.processes.find_entry_by_pid(pid).unwrap();
            alpha.processes.install_process(pid,e.reaper_pid,e.root_slot,e.root_node,e.pcb,
                e.address_space,e.os_port,e.os_port_identifier,e.affinity,e.next_frame_slot,
                e.user_heap_next_va,0x1000000).unwrap();
        }
        let (source,peer)=create(&mut alpha,pages).unwrap();
        let allocation=alpha.processes.find_active_physical_allocation_reference(1,source,pages).unwrap();
        assert_eq!(alpha.processes.physical_reference_count(allocation),2);
        for (pid,base) in [(1,source),(2,peer)] {
            let vm=alpha.processes.vm_space_mut(pid).unwrap();
            for i in 0..pages {
                assert_eq!(vm.find_frame(base+i*PAGE_SIZE),Some(pool_frame(allocation.base_page+i)));
                assert_eq!(vm.permissions(base+i*PAGE_SIZE),7);
            }
        }
        machine(|m| {
            println!("SHARED-POOL-WORK pages={pages} cap-copies={} chunks={} maps={}",m.cap_copy_calls,m.chunk_calls,m.user_map_calls);
            assert_eq!((m.cap_copy_calls,m.chunk_calls,m.frames.len()),(0,0,0));
            assert_eq!(m.user_map_calls,2*pages);
            assert_eq!(m.zero_calls,1);
            assert!((allocation.base_page..allocation.base_page+pages).all(|p|m.zeroed.contains(&p)));
        });
        release(&mut alpha,1,source,pages).unwrap();
        assert_eq!(alpha.processes.physical_reference_count(allocation),1);
        release(&mut alpha,2,peer,pages).unwrap();
        assert_empty();
    }
    for writable in [false,true] {
        let mut alpha=alpha();let source=private_source(&mut alpha);
        let before=machine(|m|(m.cap_copy_calls,m.chunk_calls,m.remove_calls));
        alpha.handle_process_page_view_request(view_request(source,0x80000000),writable).unwrap();
        machine(|m|assert_eq!((m.cap_copy_calls-before.0,m.chunk_calls-before.1),(0,0)));
        release(&mut alpha,1,0x80000000,1).unwrap();
        machine(|m|assert_eq!(m.remove_calls-before.2,0));
    }
}

#[test]
fn pool_views_reuse_slots_without_caps_and_retain_loans_until_unmap_ack() {
    let mut alpha=alpha();let source=private_source(&mut alpha);let local=0x80000000;
    let allocation=alpha.processes.find_active_physical_allocation_reference(2,source,1).unwrap();
    let baseline=machine(|m|(m.cap_copy_calls,m.chunk_calls,m.remove_calls,m.frames.len()));
    for i in 0..128 {
        let writable=i%2==0;
        alpha.handle_process_page_view_request(view_request(source,local),writable).unwrap();
        let r=alpha.processes.shared_memory_reservation(1,local,1).unwrap();
        assert!(!r.owns_frame_caps());
        assert_eq!(alpha.processes.physical_reference_count(allocation),2);
        assert_eq!(alpha.processes.vm_space_mut(1).unwrap().permissions(local),if writable {3}else{1});
        if i==64 {
            machine(|m|m.fail_unmap=true);
            assert!(release(&mut alpha,1,local,1).is_err());
            assert_eq!(alpha.processes.physical_reference_count(allocation),2);
            assert!(alpha.handle_process_page_view_request(view_request(source,local),writable).is_err());
            assert_eq!(alpha.processes.physical_reference_count(allocation),2);
            machine(|m|{assert_eq!(m.freed,0);m.fail_unmap=false;});
        }
        release(&mut alpha,1,local,1).unwrap();
        assert_eq!(alpha.processes.physical_reference_count(allocation),1);
    }
    machine(|m|assert_eq!((m.cap_copy_calls,m.chunk_calls,m.remove_calls,m.frames.len()),baseline));
    assert_eq!(alpha.processes.find_entry_by_pid(1).unwrap().next_frame_slot,1);
}

fn view_request(source: usize, local: usize) -> OsRequestEvent {
    OsRequestEvent {
        identifier: 1,
        arg0: 2,
        arg1: source,
        arg2: local,
        arg3: 0,
    }
}

#[test]
fn write_view_pins_only_current_writable_private_ram_without_exec_permission() {
    let mut alpha = alpha();
    let source = private_source(&mut alpha);
    let local = 0x80000000;
    alpha
        .handle_process_page_view_request(view_request(source, local), true)
        .unwrap();
    let caller = alpha.processes.find_entry_by_pid(1).unwrap();
    let allocation = alpha
        .processes
        .find_active_physical_allocation_reference(2, source, 1)
        .unwrap();
    assert_eq!(alpha.processes.physical_reference_count(allocation), 2);
    machine(|m| assert_eq!(m.permissions[&(caller.address_space, local)], 3));
    assert_eq!(
        alpha.processes.vm_space_mut(1).unwrap().permissions(local),
        3
    );
    release(&mut alpha, 1, local, 1).unwrap();
    assert_eq!(alpha.processes.physical_reference_count(allocation), 1);
    machine(|m| assert_eq!(m.freed, 0));
}

#[test]
fn write_view_never_upgrades_readonly_or_private_fork_backing() {
    for failure in 0..9 {
        let mut alpha = alpha();
        let source = private_source(&mut alpha);
        let local = 0x80000000;
        let mut request = view_request(source, local);
        match failure {
            0..=4 => {
                let permissions = [0, 1, 2, 4, 5][failure];
                alpha
                    .processes
                    .vm_space_mut(2)
                    .unwrap()
                    .set_permissions(source, permissions);
            }
            5 => alpha
                .processes
                .vm_space_mut(2)
                .unwrap()
                .mark_private_page_shared(source, true),
            6 => request.identifier = 2,
            7 => {
                alpha.processes.seal_capabilities(1).unwrap();
            }
            8 => alpha.processes.vm_space_mut(2).unwrap().mark_mapping_uncertain(source),
            _ => unreachable!(),
        }
        let before = machine(|m| (m.frames.len(), m.maps.len(), m.live.len()));
        assert!(alpha
            .handle_process_page_view_request(request, true)
            .is_err());
        machine(|m| assert_eq!((m.frames.len(), m.maps.len(), m.live.len()), before));
        assert!(alpha
            .processes
            .shared_memory_reservation(1, local, 1)
            .is_none());
    }
}

#[test]
fn read_view_is_readonly_and_keeps_backing_after_source_root_retirement() {
    let mut alpha = alpha();
    let source = private_source(&mut alpha);
    let local = 0x80000000;
    alpha
        .handle_process_read_view_request(view_request(source, local))
        .unwrap();
    let caller = alpha.processes.find_entry_by_pid(1).unwrap();
    let source_entry = alpha.processes.memory_entry(2).unwrap();
    let allocation = alpha
        .processes
        .find_active_physical_allocation_reference(2, source, 1)
        .unwrap();
    assert_eq!(alpha.processes.physical_reference_count(allocation), 2);
    machine(|m| assert_eq!(m.permissions[&(caller.address_space, local)], 1));
    assert_eq!(
        alpha.processes.vm_space_mut(1).unwrap().permissions(local),
        1
    );
    // Delete the source leaf and child capability. The Alpha pool Frame and
    // view's physical reference remain valid; final release owns the RAM.
    arch::address_space::unmap(
        source_entry.address_space,
        process_frame_descriptor(source_entry.root_node, allocation.start_slot),
        source,
    )
    .unwrap();
    alpha
        .processes
        .vm_space_mut(2)
        .unwrap()
        .forget_frame(source);
    arch::node::remove(
        process_frame_chunk_descriptor(
            source_entry.root_node,
            allocation.start_slot / PROCESS_FRAME_CHUNK_PAGES,
        ),
        allocation.start_slot % PROCESS_FRAME_CHUNK_PAGES,
    )
    .unwrap();
    alpha
        .processes
        .release_physical_allocation_reference(2, source, 1)
        .unwrap();
    machine(|m| assert_eq!(m.freed, 0));
    release(&mut alpha, 1, local, 1).unwrap();
    assert_empty();
    machine(|m| assert_eq!(m.freed, 1));
}

#[test]
fn read_view_checks_source_authority_permissions_and_destination_before_publishing() {
    for failure in 0..7 {
        let mut alpha = alpha();
        let source = private_source(&mut alpha);
        let local = 0x80000000;
        let mut request = view_request(source, local);
        match failure {
            0 => request.identifier = 2,
            1 => request.arg0 = 99,
            2 => request.arg1 += 1,
            3 => request.arg3 = 1,
            4 => alpha
                .processes
                .vm_space_mut(2)
                .unwrap()
                .set_permissions(source, 0),
            5 => alpha
                .processes
                .vm_space_mut(2)
                .unwrap()
                .set_permissions(source, 4),
            6 => request.arg1 += PAGE_SIZE,
            _ => unreachable!(),
        }
        let before = machine(|m| (m.frames.len(), m.maps.len(), m.live.len()));
        assert!(alpha.handle_process_read_view_request(request).is_err());
        machine(|m| assert_eq!((m.frames.len(), m.maps.len(), m.live.len()), before));
        assert!(alpha
            .processes
            .shared_memory_reservation(1, local, 1)
            .is_none());
    }
}

#[test]
fn read_view_installation_failures_retain_source_ownership_and_recycle_clean_reservations() {
    for failed_operation in 1..=3 {
        let mut alpha = alpha();
        let source = private_source(&mut alpha);
        let local = 0x80000000;
        machine(|m| {
            m.operation = 0;
            m.fail_at = Some(failed_operation);
        });
        assert!(alpha
            .handle_process_read_view_request(view_request(source, local))
            .is_err());
        let allocation = alpha
            .processes
            .find_active_physical_allocation_reference(2, source, 1)
            .unwrap();
        assert_eq!(alpha.processes.physical_reference_count(allocation), 1);
        machine(|m| {
            assert_eq!(m.freed, 0);
            m.fail_at = None;
        });
        alpha
            .handle_process_read_view_request(view_request(source, local))
            .unwrap();
        release(&mut alpha, 1, local, 1).unwrap();
    }
}

#[test]
fn sealed_callers_cannot_acquire_supervisor_read_views() {
    let mut alpha = alpha();
    let source = private_source(&mut alpha);
    alpha.processes.seal_capabilities(1).unwrap();
    assert_eq!(
        alpha.handle_process_read_view_request(view_request(source, 0x80000000)),
        Err(CapabilityError::PermissionDenied)
    );
    assert!(alpha
        .processes
        .shared_memory_reservation(1, 0x80000000, 1)
        .is_none());
}

#[test]
fn ambiguous_view_rollback_returns_fatal_and_preserves_ownership_until_retry() {
    let mut alpha = alpha();
    let source = private_source(&mut alpha);
    let local = 0x80000000;
    machine(|m| {
        m.operation = 0;
        m.fail_at = Some(3);
        m.fail_unmap = true;
    });
    assert_eq!(
        alpha.handle_process_read_view_request(view_request(source, local)),
        Err(CapabilityError::Fatal)
    );
    let allocation = alpha
        .processes
        .find_active_physical_allocation_reference(2, source, 1)
        .unwrap();
    assert_eq!(alpha.processes.physical_reference_count(allocation), 2);
    assert!(alpha
        .processes
        .shared_memory_reservation(1, local, 1)
        .is_some());
    machine(|m| {
        assert_eq!(m.freed, 0);
        m.fail_unmap = false;
        m.fail_at = None;
    });
    release(&mut alpha, 1, local, 1).unwrap();
    assert_eq!(alpha.processes.physical_reference_count(allocation), 1);
    alpha
        .handle_process_read_view_request(view_request(source, local))
        .unwrap();
    release(&mut alpha, 1, local, 1).unwrap();
}

#[test]
fn partially_completed_owner_retirement_keeps_peer_ram_until_final_release() {
    for fail_during_unmap in [true, false] {
        let mut alpha = alpha();
        let (owner, peer) = legacy_create(&mut alpha, 3).unwrap();
        let allocation = alpha
            .processes
            .find_active_physical_allocation_reference(1, owner, 3)
            .unwrap();
        assert_eq!(alpha.processes.physical_reference_count(allocation), 2);
        machine(|machine| {
            if fail_during_unmap {
                machine.fail_unmap_at = Some(machine.unmap_calls + 2);
            } else {
                machine.fail_remove_at = Some(machine.remove_calls + 2);
            }
        });
        assert_eq!(
            release(&mut alpha, 1, owner, 3),
            Err(CapabilityError::IllegalOperation)
        );
        assert_eq!(alpha.processes.physical_reference_count(allocation), 2);
        assert!(alpha
            .processes
            .shared_memory_reservation(1, owner, 3)
            .is_some());
        let peer_entry = alpha.processes.find_entry_by_pid(2).unwrap();
        machine(|machine| {
            assert_eq!(machine.freed, 0);
            for page in 0..3 {
                assert!(machine
                    .maps
                    .contains_key(&(peer_entry.address_space, peer + page * PAGE_SIZE)));
            }
            machine.fail_unmap_at = None;
            machine.fail_remove_at = None;
        });
        release(&mut alpha, 1, owner, 3).unwrap();
        assert_eq!(alpha.processes.physical_reference_count(allocation), 1);
        machine(|machine| assert_eq!(machine.freed, 0));
        release(&mut alpha, 2, peer, 3).unwrap();
        machine(|machine| {
            assert_eq!(machine.freed, 1);
            assert!(machine.live.is_empty());
            assert!(machine.frames.is_empty());
            assert!(machine.maps.is_empty());
        });
    }
}

#[test]
fn retired_owner_va_can_be_reused_without_refilling_a_live_peers_backing() {
    let mut alpha = alpha();
    let (old_owner, old_peer) = create(&mut alpha, 3).unwrap();
    let old = alpha
        .processes
        .find_active_physical_allocation_reference(1, old_owner, 3)
        .unwrap();
    release(&mut alpha, 1, old_owner, 3).unwrap();
    assert_eq!(alpha.processes.physical_reference_count(old), 1);
    let (new_owner, new_peer) = create(&mut alpha, 3).unwrap();
    let new = alpha
        .processes
        .find_active_physical_allocation_reference(1, new_owner, 3)
        .unwrap();
    assert_eq!(
        new_owner, old_owner,
        "this exercises actual source VA reuse"
    );
    assert_ne!(new.base_page, old.base_page);
    let peer = alpha.processes.find_entry_by_pid(2).unwrap();
    machine(|machine| {
        assert_eq!(machine.freed, 0);
        for offset in 0..3 {
            let old_frame = machine.maps[&(peer.address_space, old_peer + offset * PAGE_SIZE)];
            let new_frame = machine.maps[&(peer.address_space, new_peer + offset * PAGE_SIZE)];
            assert_eq!(frame_page(machine, old_frame), old.base_page + offset);
            assert_eq!(frame_page(machine, new_frame), new.base_page + offset);
        }
    });
    release(&mut alpha, 1, new_owner, 3).unwrap();
    release(&mut alpha, 2, new_peer, 3).unwrap();
    assert_eq!(alpha.processes.physical_reference_count(old), 1);
    machine(|machine| assert_eq!(machine.freed, 1));
    release(&mut alpha, 2, old_peer, 3).unwrap();
    machine(|machine| assert_eq!(machine.freed, 2));
    assert_empty();
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

#[test]
fn shared_release_reuses_vm_and_preserves_every_failed_unmap_prefix() {
    for failure in [None, Some(0), Some(1), Some(2), Some(3)] {
        let mut alpha = alpha();
        let (a, b) = create(&mut alpha, 4).unwrap();
        if let Some(index) = failure {
            machine(|m| m.fail_unmap_at = Some(m.unmap_calls + index + 1));
        }
        alpha.processes.take_mutable_vm_lookups();
        let result = release(&mut alpha, 1, a, 4);
        let lookups = alpha.processes.take_mutable_vm_lookups();
        eprintln!("shared release failure={failure:?} mutable VM lookups={lookups}");
        assert_eq!(lookups, 1);
        if let Some(index) = failure {
            assert!(result.is_err());
            let vm = alpha.processes.vm_space_mut(1).unwrap();
            for offset in 0..4 {
                let va = a + offset * PAGE_SIZE;
                if offset > 3 - index {
                    assert!(vm.find_frame(va).is_none());
                } else if offset == 3 - index {
                    assert!(vm.mapping_is_uncertain(va));
                    assert_eq!(vm.permissions(va), 0);
                } else {
                    assert!(vm.find_frame(va).is_some());
                    assert!(!vm.mapping_is_uncertain(va));
                }
            }
            assert!(alpha.processes.shared_memory_reservation(1, a, 4).is_some());
            machine(|m| {
                assert_eq!(m.freed, 0);
                assert_eq!(m.frames.len(), 0);
                m.fail_unmap_at = None;
            });
            alpha.processes.take_mutable_vm_lookups();
            release(&mut alpha, 1, a, 4).unwrap();
            let retry_lookups = alpha.processes.take_mutable_vm_lookups();
            eprintln!("shared release retry mutable VM lookups={retry_lookups}");
            assert_eq!(retry_lookups, 1);
        } else {
            result.unwrap();
        }
        release(&mut alpha, 2, b, 4).unwrap();
        assert_empty();
    }
}

#[test]
fn ram_child_copy_alias_must_retire_before_its_cap_and_failure_is_retryable() {
    let mut alpha = alpha();
    let (owner, peer) = legacy_create(&mut alpha, 1).unwrap();
    let reservation = alpha
        .processes
        .shared_memory_reservation(1, owner, 1)
        .unwrap();
    let allocation = alpha
        .processes
        .find_active_physical_allocation_reference(1, owner, 1)
        .unwrap();
    let entry = alpha.processes.memory_entry(1).unwrap();
    let child = process_frame_descriptor(entry.root_node, reservation.start_slot);
    vm_restore(&mut alpha, 1, owner, child);
    let alias = alpha.map_process_copy_frame(1, child, None).unwrap();
    machine(|m| m.fail_copy_unmap = true);
    assert!(release(&mut alpha, 1, owner, 1).is_err());
    assert_eq!(alpha.processes.physical_reference_count(allocation), 2);
    assert!(alpha
        .processes
        .shared_memory_reservation(1, owner, 1)
        .is_some());
    machine(|m| {
        assert!(m.frames.contains_key(&child));
        assert_eq!(m.maps[&(0, alias)], child);
        assert!(m.maps.contains_key(&(entry.address_space, owner)));
        assert_eq!(m.freed, 0);
    });
    machine(|m| m.fail_copy_unmap = false);
    release(&mut alpha, 1, owner, 1).unwrap();
    machine(|m| {
        assert!(!m.frames.contains_key(&child));
        assert!(!m.maps.contains_key(&(0, alias)));
    });
    release(&mut alpha, 2, peer, 1).unwrap();
    assert_empty();
}

fn canonical_owner(alpha: &mut Alpha, pages: usize) -> usize {
    let (source, peer) = alpha
        .handle_shared_memory_request(OsRequestEvent {
            identifier: 1,
            arg0: 0,
            arg1: pages * PAGE_SIZE,
            arg2: 0,
            arg3: 0,
        })
        .unwrap();
    assert_eq!(peer, 0);
    let allocation = alpha
        .processes
        .find_active_physical_allocation_reference(1, source, pages)
        .unwrap();
    let reservation = alpha
        .processes
        .shared_memory_reservation(1, source, pages)
        .unwrap();
    alpha
        .processes
        .canonicalize_shared_memory(1, reservation)
        .unwrap();
    assert_eq!(
        alpha
            .processes
            .shared_memory_reservation(1, source, pages)
            .unwrap()
            .has_page_references(),
        pages > 1
    );
    assert_eq!(
        alpha.processes.physical_reference_count(allocation),
        usize::from(pages == 1)
    );
    source
}

#[test]
fn owned_pool_extent_skips_child_caps_and_chunk_creation_across_chunk_boundary() {
    let mut alpha = alpha();
    let entry = alpha.processes.find_entry_by_pid(1).unwrap();
    alpha.processes.install_process(1, entry.reaper_pid, entry.root_slot,
        entry.root_node, entry.pcb, entry.address_space, entry.os_port,
        entry.os_port_identifier, entry.affinity, entry.next_frame_slot,
        entry.user_heap_next_va, 0x800000).unwrap();
    alpha.processes.ensure_vm_space_for_pid(1).unwrap();
    let (source, peer) = alpha.handle_shared_memory_request(OsRequestEvent {
        identifier: 1, arg0: 0, arg1: 513 * PAGE_SIZE, arg2: 0, arg3: 0,
    }).unwrap();
    assert_eq!(peer, 0);
    let allocation = alpha.processes.find_active_physical_allocation_reference(1, source, 513).unwrap();
    assert!(!allocation.owns_frame_cap);
    assert!(!alpha.processes.shared_memory_reservation(1, source, 513).unwrap().owns_frame_caps());
    let space = alpha.processes.find_entry_by_pid(1).unwrap().address_space;
    machine(|m| {
        assert_eq!((m.cap_copy_calls, m.chunk_calls, m.frames.len()), (0, 0, 0));
        for page in 0..513 {
            assert_eq!(m.maps[&(space, source + page * PAGE_SIZE)], pool_frame(allocation.base_page + page));
            assert!(m.zeroed.contains(&(allocation.base_page + page)));
        }
    });
    release(&mut alpha, 1, source, 513).unwrap();
    assert_empty();
}

#[test]
fn extent_free_failure_preserves_ownership_and_retry_skips_confirmed_unmaps() {
    for owns_caps in [false, true] {
        let mut alpha = alpha();
        let source = if owns_caps {
            let (source, peer) = legacy_create(&mut alpha, 3).unwrap();
            release(&mut alpha, 2, peer, 3).unwrap();
            source
        } else {
            alpha.handle_shared_memory_request(OsRequestEvent {
                identifier: 1, arg0: 0, arg1: 3 * PAGE_SIZE, arg2: 0, arg3: 0,
            }).unwrap().0
        };
        let allocation = alpha.processes.find_active_physical_allocation_reference(1, source, 3).unwrap();
        assert_eq!(allocation.owns_frame_cap, owns_caps);
        machine(|m| m.fail_free = true);
        assert!(release(&mut alpha, 1, source, 3) == Err(CapabilityError::Fatal));
        assert_eq!(alpha.processes.physical_reference_count(allocation), 1);
        assert!(alpha.processes.find_active_physical_allocation_reference(1, source, 3).is_some());
        assert!(alpha.processes.shared_memory_reservation(1, source, 3).is_some());
        for page in 0..3 {
            assert!(alpha.processes.vm_space_mut(1).unwrap().find_frame(source + page * PAGE_SIZE).is_none());
        }
        let unmaps = machine(|m| { m.fail_free = false; m.unmap_calls });
        release(&mut alpha, 1, source, 3).unwrap();
        machine(|m| assert_eq!(m.unmap_calls, unmaps));
        assert_empty();
    }
}

// Ownership-only model of a future readonly alias. This deliberately bypasses
// API permission validation: the private-file alias protocol is not connected.
fn hold_canonical_page(alpha: &mut Alpha, source: usize) -> usize {
    let allocation = alpha
        .processes
        .find_active_physical_allocation_reference(1, source, 1)
        .unwrap();
    let reservation = alpha
        .processes
        .reserve_shared_memory(2, 1, PROCESS_FRAME_TOTAL_PAGES)
        .unwrap();
    let peer = alpha.processes.find_entry_by_pid(2).unwrap();
    let frame = process_frame_descriptor(peer.root_node, reservation.start_slot);
    alpha
        .processes
        .register_physical_allocation(
            2,
            reservation.base_va,
            reservation.start_slot,
            allocation.base_page,
            1,
        )
        .unwrap();
    machine(|m| {
        assert!(m.frames.insert(frame, allocation.base_page).is_none());
        assert!(m
            .maps
            .insert(
                (peer.address_space, reservation.base_va),
                pool_frame(allocation.base_page)
            )
            .is_none());
        m.permissions
            .insert((peer.address_space, reservation.base_va), 1);
    });
    let vm = alpha.processes.vm_space_mut(2).unwrap();
    vm.record_frame(reservation.base_va, pool_frame(allocation.base_page))
        .unwrap();
    vm.set_permissions(reservation.base_va, 1);
    reservation.base_va
}

#[test]
fn canonical_shared_owner_releases_only_pages_without_other_references() {
    for source_first in [false, true] {
        for pages in [2, 512] {
            let mut alpha = alpha();
            let source = canonical_owner(&mut alpha, pages);
            let first = hold_canonical_page(&mut alpha, source);
            let last = hold_canonical_page(&mut alpha, source + (pages - 1) * PAGE_SIZE);
            if source_first {
                release(&mut alpha, 1, source, pages).unwrap();
                machine(|m| assert_eq!(m.live.values().sum::<usize>(), 2));
                release(&mut alpha, 2, last, 1).unwrap();
                release(&mut alpha, 2, first, 1).unwrap();
            } else {
                release(&mut alpha, 2, first, 1).unwrap();
                release(&mut alpha, 2, last, 1).unwrap();
                machine(|m| assert_eq!(m.live.values().sum::<usize>(), pages));
                release(&mut alpha, 1, source, pages).unwrap();
            }
            assert_empty();
        }
    }
}

#[test]
fn canonical_owner_cleanup_failures_retain_every_page_until_caps_are_removed() {
    for fail_unmap in [false, true] {
        let mut alpha = alpha();
        // Mapping-share retains actual child caps; keep exercising their
        // removal failures independently of pool-backed creation.
        let (source, original_peer) = legacy_create(&mut alpha, 3).unwrap();
        release(&mut alpha, 2, original_peer, 3).unwrap();
        let r = alpha.processes.shared_memory_reservation(1, source, 3).unwrap();
        alpha.processes.canonicalize_shared_memory(1, r).unwrap();
        let peer = hold_canonical_page(&mut alpha, source + PAGE_SIZE);
        machine(|m| {
            if fail_unmap {
                m.fail_unmap_at = Some(m.unmap_calls + 2);
            } else {
                m.fail_remove_at = Some(m.remove_calls + 2);
            }
        });
        assert_eq!(
            release(&mut alpha, 1, source, 3),
            Err(CapabilityError::IllegalOperation)
        );
        machine(|m| {
            assert_eq!(m.live.values().sum::<usize>(), 3);
            assert_eq!(m.freed, 0);
            m.fail_unmap_at = None;
            m.fail_remove_at = None;
        });
        assert_eq!(
            alpha
                .processes
                .physical_pages_in_range(1, source, source + 3 * PAGE_SIZE)
                .len(),
            3
        );
        release(&mut alpha, 1, source, 3).unwrap();
        release(&mut alpha, 2, peer, 1).unwrap();
        assert_empty();
    }
}

#[test]
fn incomplete_canonical_ownership_is_rejected_before_unmapping() {
    let mut alpha = alpha();
    let source = canonical_owner(&mut alpha, 3);
    alpha
        .processes
        .defer_active_physical_allocation(1, source + PAGE_SIZE, 1)
        .unwrap();
    let before = machine(|m| (m.unmap_calls, m.remove_calls, m.maps.clone(), m.freed));
    assert_eq!(
        release(&mut alpha, 1, source, 3),
        Err(CapabilityError::IllegalOperation)
    );
    machine(|m| {
        assert_eq!(
            (m.unmap_calls, m.remove_calls, m.maps.clone(), m.freed),
            before
        )
    });
    // Ownership remains quarantined; the missing active page was not freed.
    machine(|m| assert_eq!(m.live.values().sum::<usize>(), 3));
}

#[test]
fn canonical_source_va_reuse_does_not_replace_a_surviving_page_alias() {
    let mut alpha = alpha();
    let source = canonical_owner(&mut alpha, 3);
    let peer = hold_canonical_page(&mut alpha, source + PAGE_SIZE);
    let old = alpha
        .processes
        .find_active_physical_allocation_reference(2, peer, 1)
        .unwrap();
    release(&mut alpha, 1, source, 3).unwrap();
    let (replacement, _) = alpha
        .handle_shared_memory_request(OsRequestEvent {
            identifier: 1,
            arg0: 0,
            arg1: 3 * PAGE_SIZE,
            arg2: 0,
            arg3: 0,
        })
        .unwrap();
    assert_eq!(replacement, source);
    let new = alpha
        .processes
        .find_active_physical_allocation_reference(1, replacement, 3)
        .unwrap();
    assert_ne!(old.base_page, new.base_page + 1);
    machine(|m| {
        let peer = alpha.processes.find_entry_by_pid(2).unwrap();
        assert_eq!(
            frame_page(m, m.maps[&(peer.address_space, old.base_va)]),
            old.base_page
        );
    });
    release(&mut alpha, 1, replacement, 3).unwrap();
    machine(|m| assert_eq!(m.live.values().sum::<usize>(), 1));
    release(&mut alpha, 2, peer, 1).unwrap();
    assert_empty();
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
fn supervised_share_bulk_frame_index_work_and_identity() {
    const DEST: usize = 0x1_0000_0000;
    let mut counts = Vec::new();
    for pages in [31, 32, 64, 65, 96, 128, 1024] {
        let mut alpha = alpha();
        // The default fixture has only a 2MiB virtual heap. Enlarge this
        // test's empty contexts before constructing the 4MiB source extent.
        for pid in 1..=2 {
            let e = alpha.processes.find_entry_by_pid(pid).unwrap();
            alpha.processes.install_process(pid, 0, e.root_slot, e.root_node, e.pcb,
                e.address_space, e.os_port, e.os_port_identifier, e.affinity,
                e.next_frame_slot, e.user_heap_next_va, 0x1000000).unwrap();
        }
        let (source, original_peer) = create(&mut alpha, pages).unwrap();
        let e = alpha.processes.find_entry_by_pid(2).unwrap();
        alpha.processes.install_process(2, 1, e.root_slot, e.root_node, e.pcb,
            e.address_space, e.os_port, e.os_port_identifier, e.affinity,
            e.next_frame_slot, e.user_heap_next_va, e.user_heap_limit_va).unwrap();
        let before = machine(|m| (m.next_page, m.zero_calls, m.cap_copy_calls, m.maps.len()));
        vm_space::VmSpace::take_frame_scalar_insertions();
        PROTECT_ALLOCATIONS.with(|c| c.set(Some(0)));
        let result = alpha.handle_mapping_share_request(OsRequestEvent {
            identifier: 1, arg0: 2, arg1: source, arg2: pages * PAGE_SIZE, arg3: DEST,
        });
        let allocations = PROTECT_ALLOCATIONS.with(|c| c.replace(None).unwrap());
        let scalars = vm_space::VmSpace::take_frame_scalar_insertions();
        assert_eq!(result.unwrap(), (source, DEST));
        let source_frames: Vec<_> = (0..pages).map(|page| alpha.processes.vm_space_mut(1).unwrap().find_frame(source + page * PAGE_SIZE).unwrap()).collect();
        let vm = alpha.processes.vm_space_mut(2).unwrap();
        assert_eq!(vm.backed_pages(DEST, DEST + pages * PAGE_SIZE).len(), pages);
        for (page, &frame) in source_frames.iter().enumerate() {
            let va = DEST + page * PAGE_SIZE;
            assert_eq!(vm.find_frame(va), Some(frame));
            assert_eq!(vm.permissions(va), 7);
        }
        machine(|m| assert_eq!((m.next_page, m.zero_calls, m.cap_copy_calls, m.maps.len()), (before.0, before.1, before.2, before.3 + pages)));
        counts.push((pages, scalars, allocations));
        release(&mut alpha, 2, DEST, pages).unwrap();
        release(&mut alpha, 1, source, pages).unwrap();
        release(&mut alpha, 2, original_peer, pages).unwrap();
        assert_empty();
    }
    println!("SHARE-FRAME-WORDS (pages,scalar-insertions,handler-allocations)={counts:?}");
    assert_eq!(counts.iter().map(|row| row.1).collect::<Vec<_>>(), [31, 0, 0, 1, 0, 0, 0]);
    for (row, before) in counts.iter().zip([18, 20, 27, 28, 43, 54, 423]) {
        if row.0 < 32 { assert!(row.2 <= before); } else { assert!(row.2 < before); }
    }
}

#[test]
fn supervised_share_bulk_failed_word_boundaries_remove_empty_metadata_and_aliases() {
    const PAGES: usize = 128;
    const DEST: usize = 0x1_0000_0000;
    for failed_page in [0, 63, 64, 127] {
        let mut alpha = alpha();
        let (source, original_peer) = create(&mut alpha, PAGES).unwrap();
        let e = alpha.processes.find_entry_by_pid(2).unwrap();
        alpha.processes.install_process(2, 1, e.root_slot, e.root_node, e.pcb,
            e.address_space, e.os_port, e.os_port_identifier, e.affinity,
            e.next_frame_slot, e.user_heap_next_va, e.user_heap_limit_va).unwrap();
        let storage_before = alpha.processes.vm_space_mut(2).unwrap().frame_storage_counts();
        // No chunk/cap-copy operation precedes strict MAP. The failure follows
        // installation of its PTE, including the packed-word boundary pages.
        machine(|m| m.fail_at = Some(m.operation + failed_page + 1));
        assert!(alpha.handle_mapping_share_request(OsRequestEvent {
            identifier: 1, arg0: 2, arg1: source, arg2: PAGES * PAGE_SIZE, arg3: DEST,
        }).is_err());
        machine(|m| { m.fail_at = None; assert_eq!((m.frames.len(), m.maps.len(), m.freed), (0, 2 * PAGES, 0)); });
        let vm = alpha.processes.vm_space_mut(2).unwrap();
        assert_eq!(vm.frame_storage_counts(), storage_before);
        assert!(vm.backed_pages(DEST, DEST + PAGES * PAGE_SIZE).is_empty());
        for page in 0..PAGES { assert!(!vm.frame_may_be_present(DEST + page * PAGE_SIZE)); }
        release(&mut alpha, 1, source, PAGES).unwrap();
        release(&mut alpha, 2, original_peer, PAGES).unwrap();
        assert_empty();
    }
}

#[test]
fn pool_share_outlives_legacy_source_caps_and_keeps_failed_unmap_loan_until_ack() {
    const DEST: usize = 0x1_0000_0000;
    for pages in [1, 64, 513, 1024] {
        for fixed in [false, true] {
            let mut alpha = alpha();
            for pid in 1..=2 {
                let e = alpha.processes.find_entry_by_pid(pid).unwrap();
                alpha.processes.install_process(pid, if pid == 2 { 1 } else { 0 },
                    e.root_slot, e.root_node, e.pcb, e.address_space, e.os_port,
                    e.os_port_identifier, e.affinity, e.next_frame_slot,
                    e.user_heap_next_va, 0x1000000).unwrap();
            }
            let (source, old_peer) = legacy_create(&mut alpha, pages).unwrap();
            let allocation = alpha.processes.find_active_physical_allocation_reference(1, source, pages).unwrap();
            assert_eq!(alpha.processes.physical_reference_count(allocation), 2);
            let before = machine(|m| (m.next_page, m.zero_calls, m.cap_copy_calls, m.chunk_calls));
            let (_, loan) = alpha.handle_mapping_share_request(OsRequestEvent {
                identifier: 1, arg0: 2, arg1: source, arg2: pages * PAGE_SIZE,
                arg3: if fixed { DEST } else { 0 },
            }).unwrap();
            let reservation = alpha.processes.shared_memory_reservation(2, loan, pages).unwrap();
            assert!(!reservation.owns_frame_caps());
            assert_eq!(alpha.processes.physical_reference_count(allocation), 3);
            machine(|m| assert_eq!((m.next_page, m.zero_calls, m.cap_copy_calls, m.chunk_calls), before));
            // Destroy both cap-backed owners. Only the pool loan pins RAM now.
            release(&mut alpha, 1, source, pages).unwrap();
            release(&mut alpha, 2, old_peer, pages).unwrap();
            assert_eq!(alpha.processes.physical_reference_count(allocation), 1);
            machine(|m| {
                assert!(m.frames.is_empty());
                assert_eq!(m.freed, 0);
                let entry = alpha.processes.memory_entry(2).unwrap();
                for page in 0..pages {
                    assert_eq!(m.maps[&(entry.address_space, loan + page * PAGE_SIZE)], pool_frame(allocation.base_page + page));
                }
                m.fail_unmap = true;
            });
            assert!(release(&mut alpha, 2, loan, pages).is_err());
            assert_eq!(alpha.processes.physical_reference_count(allocation), 1);
            assert!(alpha.processes.shared_memory_reservation(2, loan, pages).is_some());
            machine(|m| {
                assert_eq!(m.freed, 0);
                assert_eq!(m.cap_copy_calls, before.2);
                m.fail_unmap = false;
            });
            let removals = machine(|m| m.remove_calls);
            release(&mut alpha, 2, loan, pages).unwrap();
            assert_eq!(alpha.processes.physical_reference_count(allocation), 0);
            machine(|m| { assert_eq!(m.remove_calls, removals); assert_eq!(m.freed, 1); });
            assert_empty();
        }
    }
}

#[test]
fn supervised_share_checks_destination_once_and_keeps_collisions_untouched() {
    const PAGES: usize = 64;
    const DEST: usize = 0x1_0000_0000;
    for collision in 0..6 {
        let mut alpha = alpha();
        let (source, old_peer) = create(&mut alpha, PAGES).unwrap();
        let e = alpha.processes.find_entry_by_pid(2).unwrap();
        alpha.processes.install_process(2, 1, e.root_slot, e.root_node, e.pcb,
            e.address_space, e.os_port, e.os_port_identifier, e.affinity,
            e.next_frame_slot, e.user_heap_next_va, e.user_heap_limit_va).unwrap();
        // An unrelated lazy entry makes repeated negative lookups observable.
        alpha.processes.register_lazy_mapping(2, DEST + PAGES * PAGE_SIZE, 1, 9000,
            nanami_core::process::ProcessLazyMappingKind::Zero).unwrap();
        let blocked = DEST + (PAGES - 1) * PAGE_SIZE;
        if collision == 1 || collision == 2 || collision == 3 || collision == 5 {
            let va = if collision == 5 { blocked + 1 } else { blocked };
            let vm = alpha.processes.vm_space_mut(2).unwrap();
            vm.record_frame(va, 0xfeed).unwrap();
            if collision == 2 { vm.set_permissions(va, 0); }
            if collision == 3 { vm.mark_mapping_uncertain(va); }
        }
        if collision == 4 {
            alpha.processes.register_lazy_mapping(2, blocked, 1, 9001,
                nanami_core::process::ProcessLazyMappingKind::Zero).unwrap();
        }
        let before = machine(|m| (m.operation, m.frames.len(), m.maps.len(), m.freed));
        alpha.processes.take_mutable_vm_lookups();
        ProcessManager::take_lazy_mapping_probes();
        let result = alpha.handle_mapping_share_request(OsRequestEvent {
            identifier: 1, arg0: 2, arg1: source, arg2: PAGES * PAGE_SIZE, arg3: DEST,
        });
        let lookups = alpha.processes.take_mutable_vm_lookups();
        let probes = ProcessManager::take_lazy_mapping_probes();
        eprintln!("destination collision={collision} mutable VM lookups={lookups} point lazy probes={probes}");
        assert_eq!(lookups, if collision == 4 { 2 } else if collision == 0 || collision == 5 { 4 } else { 3 });
        assert_eq!(probes, 0);
        if (1..=4).contains(&collision) {
            assert_eq!(result, Err(CapabilityError::IllegalOperation));
            assert_eq!(machine(|m| (m.operation, m.frames.len(), m.maps.len(), m.freed)), before);
            assert!(alpha.processes.shared_memory_reservation(2, DEST, PAGES).is_none());
        } else {
            assert_eq!(result.unwrap(), (source, DEST));
            release(&mut alpha, 2, DEST, PAGES).unwrap();
        }
        if (1..=3).contains(&collision) || collision == 5 {
            let va = if collision == 5 { blocked + 1 } else { blocked };
            let vm = alpha.processes.vm_space_mut(2).unwrap();
            assert_eq!(vm.find_frame(va), Some(0xfeed));
            if collision == 3 { assert!(vm.mapping_is_uncertain(va)); }
            vm.forget_frame(va);
        }
        release(&mut alpha, 1, source, PAGES).unwrap();
        release(&mut alpha, 2, old_peer, PAGES).unwrap();
        assert_empty();
    }
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
            assert_eq!(m.frames.len(), 0);
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
    for fail in 1..=3 {
        // Three strict maps; no child capability preparation remains.
        let mut alpha = alpha();
        let (source, peer) = create(&mut alpha, 3).unwrap();
        machine(|m| m.fail_at = Some(m.operation + fail));
        assert!(share(&mut alpha, 1, 2, source, 3 * PAGE_SIZE).is_err());
        machine(|m| {
            m.fail_at = None;
            assert_eq!(m.frames.len(), 0);
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
fn both_release_orders_unmap_before_free_and_reuse_reservations() {
    for first in [1, 2] {
        let mut alpha = alpha();
        for _ in 0..1000 {
            let (a, b) = create(&mut alpha, 3).unwrap();
            assert_eq!((a, b), (0x200000, 0x200000));
            let addresses = [a, b];
            release(&mut alpha, first, addresses[first - 1], 3).unwrap();
            machine(|m| {
                assert_eq!(m.live.len(), 1);
                assert_eq!(m.frames.len(), 0);
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
            assert_eq!(m.frames.len(), 0);
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
    assert!(alpha
        .processes
        .vm_space_mut(1)
        .unwrap()
        .mapping_is_uncertain(a));
    assert_eq!(alpha.processes.vm_space_mut(1).unwrap().permissions(a), 0);
    machine(|m| {
        assert_eq!(m.maps.len(), 3);
        assert_eq!(m.frames.len(), 0);
        assert_eq!(m.live.len(), 1);
        m.fail_unmap = false;
    });
    release(&mut alpha, 1, a, 1).unwrap();
    assert!(!alpha
        .processes
        .vm_space_mut(1)
        .unwrap()
        .mapping_is_uncertain(a));
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
        let (a, b) = legacy_create(&mut alpha, 3).unwrap();
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

fn deferred_guest() -> (Alpha,usize,usize,usize) {
    let mut a=large_arena();let(input,_)=a.handle_shared_memory_request(owned_request(64*PAGE_SIZE,0,0)).unwrap();
    let(source,_)=a.handle_memory_source_request(owned_request(64*PAGE_SIZE,input,64*PAGE_SIZE)).unwrap();
    let e=a.processes.find_entry_by_pid(2).unwrap();
    a.processes.install_process(2,1,e.root_slot,e.root_node,e.pcb,e.address_space,e.os_port,
        e.os_port_identifier,e.affinity,e.next_frame_slot,e.user_heap_next_va,e.user_heap_limit_va).unwrap();
    a.processes.seal_capabilities(2).unwrap();let base=0x1000000;
    a.map_private_snapshot_with_prefix(1,2,source,base,64*PAGE_SIZE,1,None,Some(16*PAGE_SIZE)).unwrap();
    (a,input,source,base)
}
#[test]
fn deferred_protection_maps_final_rights_once_and_prot_none_never_unmaps_absent_leaf() {
    for rights in [0,1,5] {
        let(mut a,_,_,base)=deferred_guest();let va=base+32*PAGE_SIZE;
        let before=machine(|m|(m.user_map_calls,m.unmap_calls));
        a.handle_mapping_protect_request(OsRequestEvent{identifier:1,arg0:2,arg1:va,arg2:3*PAGE_SIZE,arg3:rights}).unwrap();
        assert_eq!(machine(|m|m.unmap_calls),before.1);
        assert_eq!(machine(|m|m.user_map_calls),before.0+if rights==0{0}else{3});
        for i in 0..3 {let vm=a.processes.vm_space(2).unwrap();assert_eq!(vm.permissions(va+i*PAGE_SIZE),rights);assert!(!vm.mapping_deferred(va+i*PAGE_SIZE));}
        if rights==0 {
            a.handle_mapping_protect_request(OsRequestEvent{identifier:1,arg0:2,arg1:va,arg2:3*PAGE_SIZE,arg3:1}).unwrap();
            assert_eq!(machine(|m|m.user_map_calls),before.0+3);
        }
    }
}
#[test]
fn deferred_protection_failure_restores_logical_rights_and_original_absence() {
    for failure in 1..=4 {
        let(mut a,_,_,base)=deferred_guest();let va=base+32*PAGE_SIZE;
        machine(|m|m.fail_at=Some(m.operation+failure));
        assert!(a.handle_mapping_protect_request(OsRequestEvent{identifier:1,arg0:2,arg1:va,arg2:3*PAGE_SIZE,arg3:5}).is_err());
        machine(|m|m.fail_at=None);
        let vm=a.processes.vm_space(2).unwrap();
        for i in 0..3 {let at=va+i*PAGE_SIZE;assert_eq!(vm.permissions(at),1);assert!(vm.mapping_deferred(at));assert!(!vm.frame_may_be_present(at));}
        a.handle_mapping_protect_request(OsRequestEvent{identifier:1,arg0:2,arg1:va,arg2:3*PAGE_SIZE,arg3:5}).unwrap();
    }
}
