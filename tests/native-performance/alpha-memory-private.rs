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
const USER_STACK_BASE: usize = 0x0400_0000;
const USER_STACK_PAGES: usize = 64;
const USER_ANONYMOUS_MAP_BASE: usize = 0x1000_0000;
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
    fail_map_at: Option<usize>,
    fail_map_after_install: bool,
    allocations: usize,
    copies: usize,
    maps: Vec<(usize, usize, usize)>,
    unmaps: Vec<(usize, usize)>,
    child_allocations: usize,
    fail_pool_conversion: bool,
    unready_pool: Option<usize>,
    ready_initializers: BTreeMap<usize, usize>,
    initializer_collision: Option<usize>,
    copy_aliases: Vec<(usize, Option<usize>)>,
    track_copy_aliases: bool,
    initializer_maps: usize,
    fail_copy_alias: bool,
    fail_initializer: bool,
    frame_reads: usize,
    pool_word_reads: usize,
    reject_pool_word: bool,
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
                let fail = kernel.fail_map != 0
                    && kernel.fail_map_at.is_none_or(|at| at == kernel.maps.len());
                kernel.maps.push((space, va, attr.bits()));
                if fail { kernel.fail_map -= 1; }
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
                kernel.unmaps.push((space, va));
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
            kernel.frame_reads += 1;
            if !kernel.physical.contains_key(&physical) || kernel.unready_pool == Some(physical) {
                return None;
            }
            let frame = 1_000_000_000 + physical;
            kernel.caps.insert(frame, physical);
            Some(frame)
        })
    }
    fn read_ready_pool_frames(&self, first: usize, frames: &mut [usize]) -> Result<(), CapabilityError> {
        KERNEL.with_borrow_mut(|kernel| {
            kernel.pool_word_reads += 1;
            if kernel.reject_pool_word {
                // A failed bulk lookup may have filled a prefix. Fallback
                // must replace every element before publishing the word.
                frames.fill(usize::MAX);
                return Err(CapabilityError::InvalidArgument);
            }
            for (offset, frame) in frames.iter_mut().enumerate() {
                let physical = first + offset;
                if !kernel.physical.contains_key(&physical) || kernel.unready_pool == Some(physical) {
                    return Err(CapabilityError::InvalidArgument);
                }
                *frame = 1_000_000_000 + physical;
                kernel.caps.insert(*frame, physical);
            }
            Ok(())
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
            kernel.child_allocations += 1;
            Ok(physical)
        })
    }
    fn allocate_physical_any(&mut self, bytes: usize) -> Result<usize, CapabilityError> {
        assert_eq!(bytes, PAGE_SIZE);
        KERNEL.with(|kernel| {
            let mut kernel = kernel.borrow_mut();
            let physical = kernel.physical.keys().next_back().copied().unwrap_or(99) + 1;
            kernel.physical.insert(physical, Box::new([0xcc; PAGE_SIZE]));
            kernel.allocations += 1;
            Ok(physical)
        })
    }
    fn ensure_alpha_frame_at_physical_index(&mut self, physical: usize) -> Result<(), CapabilityError> {
        if KERNEL.with(|kernel| { let mut kernel = kernel.borrow_mut();
            if kernel.fail_pool_conversion { kernel.unready_pool = Some(physical); true } else { false }
        }) { return Err(CapabilityError::Fatal); }
        self.physical_frame_descriptor_from_index(physical).ok_or(CapabilityError::Fatal).map(|_| ())
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
    frame_chunk_requests: Vec<(usize, usize, usize, usize)>,
    fail_frame_chunks: bool,
}
impl Alpha {
    fn split_private_large_mappings(&mut self, _: usize, _: usize, _: usize, _: bool) -> Result<(), CapabilityError> { Ok(()) }
    fn split_private_large_mappings_for_fork(&mut self, _: usize, _: bool) -> Result<(), CapabilityError> { Ok(()) }
    fn read_process_memory_into_static_buffer(
        &mut self,
        _: usize,
        address: usize,
        bytes: usize,
    ) -> Result<&'static [u8], CapabilityError> {
        if address == 0 || !matches!(bytes, 16 | 24) {
            return Err(CapabilityError::InvalidArgument);
        }
        // Only test-owned native descriptor addresses are passed here. The
        // production caller-memory walker has its own checked-copy regressions.
        Ok(unsafe { core::slice::from_raw_parts(address as *const u8, bytes) })
    }

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
            frame_chunk_requests: Vec::new(),
            fail_frame_chunks: false,
        }
    }
    fn ensure_process_frame_chunks(
        &mut self,
        pid: usize,
        root: usize,
        first: usize,
        pages: usize,
    ) -> Result<(), CapabilityError> {
        self.frame_chunk_requests.push((pid, root, first, pages));
        if self.fail_frame_chunks { Err(CapabilityError::Fatal) } else { Ok(()) }
    }
    fn process_root_slot(&self, root: usize) -> Result<usize, CapabilityError> {
        // Fixture roots are simple numeric tags. Production validates the
        // complete descriptor before this allocation-free snapshot path.
        if root == 0 { Err(CapabilityError::InvalidArgument) } else { Ok(root) }
    }
    fn invalidate_process_copy_mappings(&mut self, pid: usize) -> Result<(), CapabilityError> {
        self.invalidations.push(pid);
        Ok(())
    }
    fn map_initialization_page(&mut self, physical: usize) -> Result<usize, CapabilityError> {
        KERNEL.with(|kernel| {
            let mut kernel=kernel.borrow_mut();
            kernel.initializer_maps+=1;
            if kernel.fail_initializer { return Err(CapabilityError::Fatal); }
            assert!(kernel.initializer_collision.is_none_or(|source| source==physical),
                "must not evict a pinned source initializer alias");
            Ok(kernel.physical.get_mut(&physical).unwrap().as_mut_ptr() as usize)
        })
    }
    fn initialization_alias_address(&self, physical: usize) -> usize {
        KERNEL.with_borrow_mut(|k| {
            let key=k.initializer_collision.unwrap_or(physical);
            k.physical.get_mut(&key).unwrap().as_mut_ptr() as usize
        })
    }
    fn cached_initialization_frame(&self, frame: usize) -> Option<usize> {
        KERNEL.with_borrow(|k| k.ready_initializers.get(&frame).copied())
    }
    fn map_process_transfer_frame(&mut self, pid: usize, frame: usize, pinned: Option<usize>) -> Result<usize,CapabilityError> {
        if let Some(address)=self.cached_initialization_frame(frame) { return Ok(address); }
        self.map_process_copy_frame(pid,frame,pinned)
    }
    fn map_process_copy_frame(
        &mut self,
        _: usize,
        frame: usize,
        pinned: Option<usize>,
    ) -> Result<usize, CapabilityError> {
        KERNEL.with_borrow_mut(|k| {
            if k.track_copy_aliases { k.copy_aliases.push((frame,pinned)); }
            if k.fail_copy_alias { return Err(CapabilityError::Fatal); }
            let physical=k.caps[&frame];
            Ok(k.physical.get_mut(&physical).unwrap().as_mut_ptr() as usize)
        })
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
        self.map_process_initialized_pages_protected(pid, base, count, source, 7)
    }
    fn map_process_initialized_pages_protected(
        &mut self, pid: usize, base: Option<usize>, count: usize,
        source: Option<PageSource>, permissions: usize,
    ) -> Result<usize, CapabilityError> {
        let base = base.unwrap();
        let source = source.unwrap();
        assert_eq!(source.bytes, count * PAGE_SIZE);
        for offset in (0..source.bytes).step_by(PAGE_SIZE) {
            let frame = self.process_frame_for_page(source.pid, source.address + offset)?;
            let source = self.map_process_copy_frame(source.pid, frame, None)?;
            let pfn = self.page(pid, base + offset, 0, permissions);
            let destination = self.map_initialization_page(pfn)?;
            unsafe {
                ptr::copy_nonoverlapping(source as *const u8, destination as *mut u8, PAGE_SIZE);
            }
        }
        Ok(base)
    }
    fn map_process_cloned_unmapped_pages(
        &mut self, pid: usize, base: usize, count: usize, source: PageSource,
    ) -> Result<usize, CapabilityError> {
        self.map_process_initialized_pages_protected(pid, Some(base), count, Some(source), 0)
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
#[path = "../../nanami/src/nanami_core/alpha/memory_snapshot.rs"]
mod snapshots;

#[test]
fn eager_snapshot_marks_the_source_once_and_keeps_all_independent_aliases() {
    for pages in [64, 513, 1024] {
        for rights in [1, 5] {
            let mut a = Alpha::new();
            let source = snapshot(&mut a, pages + 6);
            let input = source + 3 * PAGE_SIZE;
            let base = 0x1001000;
            let before = KERNEL.with_borrow(|k| (k.maps.len(), k.allocations, k.copies));
            a.processes.take_mutable_vm_lookups();
            a.map_process_private_snapshot(4, 1, input, base, pages * PAGE_SIZE, rights).unwrap();
            let lookups = a.processes.take_mutable_vm_lookups();
            println!("SNAPSHOT-ALIAS-BATCH pages={pages} rights={rights} mutable_vm_lookups={lookups} guest_maps={pages}");
            assert!(lookups <= pages + 8, "source marking must not resolve its VM per page: {lookups}");
            KERNEL.with_borrow(|k| assert_eq!((k.maps.len() - before.0, k.allocations, k.copies),
                (pages, before.1, before.2)));
            for page in 0..pages + 6 {
                assert_eq!(a.processes.vm_space(4).unwrap().private_page_shared(source + page * PAGE_SIZE),
                    (3..pages + 3).contains(&page));
            }
            for page in 0..pages {
                let va = base + page * PAGE_SIZE;
                let allocation = a.processes.find_active_physical_allocation_reference(1, va, 1).unwrap();
                assert_eq!(a.processes.physical_reference_count(allocation), 2);
                assert!(!allocation.owns_frame_cap);
                assert_eq!(a.processes.vm_space(1).unwrap().permissions(va), rights);
                assert!(a.processes.vm_space(1).unwrap().private_page_shared(va));
                assert!(!a.processes.vm_space(1).unwrap().mapping_deferred(va));
                assert_eq!(a.byte(1, va), 0x30u8.wrapping_add((page + 3) as u8));
            }
        }
    }
}

#[test]
fn snapshot_map_failure_keeps_the_entire_owned_batch_and_never_invents_tail_ptes() {
    for failed in [0, 1, 63, 64, 65] {
        for after_install in [false, true] {
            let mut a = Alpha::new();
            let pages = 66;
            let source = snapshot(&mut a, pages);
            let base = 0x1000000;
            let before = KERNEL.with_borrow(|k| k.maps.len());
            KERNEL.with_borrow_mut(|k| {
                k.fail_map = 1;
                k.fail_map_at = Some(before + failed);
                k.fail_map_after_install = after_install;
            });
            assert!(a.map_process_private_snapshot(4, 1, source, base, pages * PAGE_SIZE, 5).is_err());
            for page in 0..pages {
                let va = base + page * PAGE_SIZE;
                let allocation = a.processes.find_active_physical_allocation_reference(1, va, 1).unwrap();
                assert_eq!(a.processes.physical_reference_count(allocation), 2);
                let vm = a.processes.vm_space(1).unwrap();
                assert_eq!(vm.find_frame(va).is_some(), page <= failed);
                assert_eq!(vm.mapping_is_uncertain(va), page == failed);
                assert_eq!(vm.frame_may_be_present(va), page <= failed);
                KERNEL.with_borrow(|k| assert_eq!(k.ptes.contains_key(&(501, va)),
                    page < failed || (page == failed && after_install)));
            }
        }
    }
}

fn snapshot(a: &mut Alpha, pages: usize) -> usize {
    source_owner_arena(a, pages);
    let owner = a.processes.find_entry_by_pid(4).unwrap();
    let r = a
        .processes
        .reserve_shared_memory(4, pages, PROCESS_FRAME_TOTAL_PAGES)
        .unwrap();
    let mut first = 0;
    for offset in 0..pages {
        let slot = r.start_slot + offset;
        let physical = a
            .memory
            .allocate_process_frame(
                process_frame_chunk_descriptor(owner.root_node, slot / 512),
                9,
                slot % 512,
            )
            .unwrap();
        if offset == 0 {
            first = physical;
        }
        assert_eq!(physical, first + offset);
        KERNEL.with(|k| {
            k.borrow_mut()
                .physical
                .get_mut(&physical)
                .unwrap()
                .fill(0x30u8.wrapping_add(offset as u8))
        });
        let va = r.base_va + offset * PAGE_SIZE;
        let frame = process_frame_descriptor(owner.root_node, slot);
        let vm = a.processes.vm_space_mut(4).unwrap();
        vm.record_frame(va, frame).unwrap();
        vm.set_permissions(va, 1);
        arch::address_space::map(
            owner.address_space,
            frame,
            va,
            Attribute::from_bits(1).unwrap(),
        )
        .unwrap();
    }
    a.processes
        .register_physical_allocation(4, r.base_va, r.start_slot, first, pages)
        .unwrap();
    r.base_va
}

fn source_owner_arena(a: &mut Alpha, pages: usize) {
    let owner = a.processes.find_entry_by_pid(4).unwrap();
    let source_limit = owner.user_heap_limit_va.max(owner.user_heap_next_va + pages * PAGE_SIZE);
    a.processes
        .install_process(
            4,
            owner.reaper_pid,
            owner.root_slot,
            owner.root_node,
            owner.pcb,
            owner.address_space,
            owner.os_port,
            owner.os_port_identifier,
            owner.affinity,
            owner.next_frame_slot,
            owner.user_heap_next_va,
            source_limit,
        )
        .unwrap();
}

fn immutable_snapshot(a: &mut Alpha, pages: usize) -> usize {
    source_owner_arena(a, pages);
    let r = a.processes.reserve_owned_pool_memory(4, pages, PROCESS_FRAME_TOTAL_PAGES).unwrap();
    let r = a.processes.mark_immutable_memory_source(4, r).unwrap();
    let mut first = 0;
    for offset in 0..pages {
        let physical = a.memory.allocate_physical_any(PAGE_SIZE).unwrap();
        if offset == 0 { first = physical; }
        assert_eq!(physical, first + offset);
        KERNEL.with_borrow_mut(|k| k.physical.get_mut(&physical).unwrap().fill(0x30u8.wrapping_add(offset as u8)));
        let va = r.base_va + offset * PAGE_SIZE;
        let frame = a.memory.physical_frame_descriptor_from_index(physical).unwrap();
        let vm = a.processes.vm_space_mut(4).unwrap();
        vm.record_frame(va, frame).unwrap();
        vm.set_permissions(va, 0);
        vm.mark_private_page_shared(va, true);
        vm.invalidate_page_table_region(va);
    }
    a.processes.register_pool_allocation(4, r.base_va, r.start_slot, first, pages).unwrap();
    r.base_va
}

#[test]
fn snapshot_source_cursor_keeps_per_page_pool_validation_and_observes_replacements() {
    let mut a = Alpha::new();
    let pages = 1024;
    let source = immutable_snapshot(&mut a, pages);
    a.check_private_snapshot_source(4, source, pages * PAGE_SIZE).unwrap();
    vm_space::VmSpace::take_snapshot_point_queries();
    let before = KERNEL.with_borrow(|k| (k.frame_reads, k.allocations, k.maps.len()));
    crate::PROTECT_ALLOCATIONS.with(|n| n.set(Some(0)));
    let plan = a.test_snapshot_backing(4, source, pages * PAGE_SIZE).unwrap();
    let allocations = crate::PROTECT_ALLOCATIONS.with(|n| n.replace(None).unwrap());
    assert_eq!(plan, (true, pages));
    assert_eq!(allocations, 0);
    assert_eq!(vm_space::VmSpace::take_snapshot_point_queries(), (0, 0));
    KERNEL.with_borrow(|k| assert_eq!((k.frame_reads - before.0, k.allocations, k.maps.len()),
                                    (pages, before.1, before.2)));
    let va = source + 512 * PAGE_SIZE;
    let original = a.processes.vm_space(4).unwrap().find_frame(va).unwrap();
    a.processes.vm_space_mut(4).unwrap().record_frame(va, 999).unwrap();
    assert_eq!(a.test_snapshot_backing(4, source, pages * PAGE_SIZE), Err(CapabilityError::PermissionDenied));
    a.processes.vm_space_mut(4).unwrap().record_frame(va, original).unwrap();
    assert_eq!(a.test_snapshot_backing(4, source, pages * PAGE_SIZE), Ok((true, pages)));
}

#[test]
fn contiguous_immutable_sources_skip_the_reference_vector_and_keep_private_bytes() {
    for pages in [1, 63, 64, 65, 513, 1024] {
        for rights in [1, 5] {
            let mut a = Alpha::new();
            let source = immutable_snapshot(&mut a, pages + 6);
            let input = source + 3 * PAGE_SIZE;
            a.check_private_snapshot_source(4, source, (pages + 6) * PAGE_SIZE).unwrap();
            crate::PROTECT_ALLOCATIONS.with(|n| n.set(Some(0)));
            let old = a.prepare_private_snapshot_source(4, input, pages * PAGE_SIZE).unwrap();
            let old_calls = crate::PROTECT_ALLOCATIONS.with(|n| n.replace(None).unwrap());
            crate::PROTECT_ALLOCATIONS.with(|n| n.set(Some(0)));
            let plan = a.test_snapshot_backing(4, input, pages * PAGE_SIZE).unwrap();
            let new_calls = crate::PROTECT_ALLOCATIONS.with(|n| n.replace(None).unwrap());
            assert_eq!(plan, (true, pages));
            assert_eq!((old_calls, new_calls), (1, 0));
            drop(old);
            let base = 0x1001000;
            let eager = (rights == 1).then_some(pages.min(17) * PAGE_SIZE);
            let before = KERNEL.with_borrow(|k| (k.maps.len(), k.allocations, k.copies));
            let reads_before = KERNEL.with_borrow(|k| (k.frame_reads, k.pool_word_reads));
            snapshots::take_backing_plans();
            a.map_private_snapshot_with_prefix(4, 1, input, base, pages * PAGE_SIZE, rights, None, eager).unwrap();
            assert_eq!(snapshots::take_backing_plans(), (1, 0));
            let mapped = eager.map_or(pages, |bytes| bytes / PAGE_SIZE);
            KERNEL.with_borrow(|k| assert_eq!((k.maps.len() - before.0, k.allocations, k.copies), (mapped, before.1, before.2)));
            let first_word = (base + mapped * PAGE_SIZE).div_ceil(vm_space::FRAME_WORD_BYTES)
                * vm_space::FRAME_WORD_BYTES;
            let words = (base + pages * PAGE_SIZE).saturating_sub(first_word) / vm_space::FRAME_WORD_BYTES;
            KERNEL.with_borrow(|k| {
                assert_eq!(k.pool_word_reads - reads_before.1, words);
                assert_eq!(k.frame_reads - reads_before.0, 2 * pages - words * vm_space::FRAME_WORD_PAGES);
            });
            for page in 0..pages {
                let va = base + page * PAGE_SIZE;
                let reference = a.processes.find_active_physical_allocation_reference(1, va, 1).unwrap();
                assert_eq!(a.processes.physical_reference_count(reference), 2);
                let vm = a.processes.vm_space(1).unwrap();
                assert_eq!(vm.permissions(va), rights);
                assert_eq!(vm.mapping_deferred(va), page >= mapped);
                assert_eq!(a.byte(1, va), 0x30u8.wrapping_add((page + 3) as u8));
            }
            if pages >= 64 {
                let va = base + 32 * PAGE_SIZE;
                let shared = a.physical(1, va);
                a.prepare(1, va, PAGE_SIZE, 1).unwrap();
                assert_ne!(a.physical(1, va), shared);
                let private = a.physical(1, va);
                KERNEL.with_borrow_mut(|k| k.physical.get_mut(&private).unwrap()[0] = 0xab);
                assert_eq!(a.byte(4, input + 32 * PAGE_SIZE), 0x53);
            }
            println!("SNAPSHOT-CONTIGUOUS pages={pages} rights={rights} reference-allocations={old_calls}->{new_calls}");
        }
    }
}

#[test]
fn deferred_contiguous_snapshot_word_fallback_replaces_partial_results_and_keeps_ownership() {
    for reject in [false, true] {
        let mut a = Alpha::new();
        let pages = 146;
        let source = immutable_snapshot(&mut a, pages + 6);
        let input = source + 3 * PAGE_SIZE;
        let base = 0x1001000;
        let before = KERNEL.with_borrow(|k| (k.frame_reads, k.pool_word_reads, k.maps.len(), k.allocations));
        KERNEL.with_borrow_mut(|k| k.reject_pool_word = reject);
        a.map_private_snapshot_with_prefix(4, 1, input, base, pages * PAGE_SIZE, 1,
            None, Some(16 * PAGE_SIZE)).unwrap();
        KERNEL.with_borrow(|k| {
            assert_eq!(k.pool_word_reads - before.1, 1);
            let scalar = 2 * pages - if reject { 0 } else { vm_space::FRAME_WORD_PAGES };
            assert_eq!(k.frame_reads - before.0, scalar);
            assert_eq!(k.maps.len() - before.2, 16);
            assert_eq!(k.allocations, before.3);
        });
        for page in 0..pages {
            let va = base + page * PAGE_SIZE;
            let reference = a.processes.find_active_physical_allocation_reference(1, va, 1).unwrap();
            assert_eq!(a.processes.physical_reference_count(reference), 2);
            let vm = a.processes.vm_space(1).unwrap();
            assert_eq!(vm.permissions(va), 1);
            assert_eq!(vm.mapping_deferred(va), page >= 16);
            assert!(vm.private_page_shared(va));
            assert_eq!(a.byte(1, va), 0x30u8.wrapping_add((page + 3) as u8));
        }
    }
}

#[test]
fn contiguous_snapshot_map_failures_keep_every_owner_and_unknown_pte() {
    for failed in [0, 1, 63, 64, 65] {
        for after in [false, true] {
            let mut a = Alpha::new();
            let source = immutable_snapshot(&mut a, 66);
            let base = 0x1000000;
            let before = KERNEL.with_borrow(|k| (k.maps.len(), k.allocations));
            KERNEL.with_borrow_mut(|k| { k.fail_map = 1; k.fail_map_at = Some(before.0 + failed); k.fail_map_after_install = after; });
            snapshots::take_backing_plans();
            assert!(a.map_process_private_snapshot(4, 1, source, base, 66 * PAGE_SIZE, 5).is_err());
            assert_eq!(snapshots::take_backing_plans(), (1, 0));
            assert_eq!(KERNEL.with_borrow(|k| k.allocations), before.1);
            for page in 0..66 {
                let va = base + page * PAGE_SIZE;
                let reference = a.processes.find_active_physical_allocation_reference(1, va, 1).unwrap();
                assert_eq!(a.processes.physical_reference_count(reference), 2);
                let vm = a.processes.vm_space(1).unwrap();
                assert_eq!(vm.find_frame(va).is_some(), page <= failed);
                assert_eq!(vm.mapping_is_uncertain(va), page == failed);
                assert_eq!(vm.frame_may_be_present(va), page <= failed);
                KERNEL.with_borrow(|k| assert_eq!(k.ptes.contains_key(&(501, va)), page < failed || (page == failed && after)));
            }
        }
    }
}

#[test]
fn noncanonical_snapshot_records_keep_the_original_newest_duplicate_fallback() {
    for duplicate in [false, true] {
        let mut a = Alpha::new();
        let source = if duplicate { immutable_snapshot(&mut a, 4) } else { snapshot(&mut a, 4) };
        a.check_private_snapshot_source(4, source, 4 * PAGE_SIZE).unwrap();
        if duplicate {
            let page = a.processes.find_active_physical_allocation_reference(4, source + 2 * PAGE_SIZE, 1).unwrap();
            // An invalid older duplicate must not defeat the latest valid row.
            a.processes.register_pool_allocation(4, page.base_va, page.start_slot + 27, page.base_page, 1).unwrap();
            a.processes.register_pool_allocation(4, page.base_va, page.start_slot, page.base_page, 1).unwrap();
        }
        snapshots::take_backing_plans();
        a.map_process_private_snapshot(4, 1, source, 0x1000000, 4 * PAGE_SIZE, 5).unwrap();
        assert_eq!(snapshots::take_backing_plans(), (0, 1));
        for page in 0..4 {
            assert_eq!(a.byte(1, 0x1000000 + page * PAGE_SIZE), 0x30 + page as u8);
        }
    }
}

#[test]
fn immutable_source_span_rejects_missing_wrong_and_unready_owners_before_publication() {
    for case in 0..6 {
        let mut a = Alpha::new();
        let source = immutable_snapshot(&mut a, 4);
        a.check_private_snapshot_source(4, source, 4 * PAGE_SIZE).unwrap();
        let first = a.processes.find_active_physical_allocation_reference(4, source, 1).unwrap();
        match case {
            0 => a.processes.vm_space_mut(4).unwrap().set_permissions(source + 3 * PAGE_SIZE, 2),
            1 => { a.processes.vm_space_mut(4).unwrap().record_frame(source, 999).unwrap(); },
            2 => KERNEL.with_borrow_mut(|k| k.unready_pool = Some(first.base_page)),
            3 => { a.processes.register_pool_allocation(4, source, first.start_slot + 1, first.base_page, 1).unwrap(); },
            4 => { a.processes.release_physical_allocation_reference(4, source, 1).unwrap(); },
            _ => { a.processes.seal_capabilities(4).unwrap(); },
        }
        let before = KERNEL.with_borrow(|k| (k.maps.len(), k.allocations, k.copies));
        let base = 0x1000000;
        // Case0 changes a page outside this requested subrange: admission must
        // still validate immutability of the complete source reservation.
        assert!(a.map_process_private_snapshot(4, 1, source, base, 2 * PAGE_SIZE, 5).is_err());
        assert_eq!(KERNEL.with_borrow(|k| (k.maps.len(), k.allocations, k.copies)), before);
        assert!(a.processes.physical_pages_in_range(1, base, base + 2 * PAGE_SIZE).is_empty());
        assert!(!a.processes.vm_space(1).unwrap().has_backed_pages(base, base + 2 * PAGE_SIZE));
    }
}

#[test]
fn snapshot_source_admission_precedes_conflicts_without_early_canonicalization() {
    for writable in [false,true] {
        let mut a=Alpha::new();let source=snapshot(&mut a,3);
        a.page(1,0x10000,0x91,3);
        let original=a.processes.shared_memory_reservation(4,source,3).unwrap();
        assert!(!original.has_page_references());
        if writable {a.processes.vm_space_mut(4).unwrap().set_permissions(source+2*PAGE_SIZE,3);}
        let counts=KERNEL.with(|k| (k.borrow().allocations,k.borrow().copies,k.borrow().ptes.clone()));
        assert_eq!(a.map_process_private_snapshot(4,1,source,0x10000,PAGE_SIZE,5),
            Err(if writable {CapabilityError::PermissionDenied} else {CapabilityError::IllegalOperation}));
        assert!(a.processes.shared_memory_reservation(4,source,3)==Some(original));
        assert!(a.processes.find_active_physical_allocation_reference(4,source,3).is_some());
        assert_eq!(counts,KERNEL.with(|k| (k.borrow().allocations,k.borrow().copies,k.borrow().ptes.clone())));
    }
}

#[test]
fn snapshot_pool_aliases_do_not_request_empty_child_frame_nodes() {
    let mut a = Alpha::new();
    let source = snapshot(&mut a, 513);
    a.fail_frame_chunks = true;
    a.map_process_private_snapshot(4, 1, source, 0x10000, 513 * PAGE_SIZE, 1).unwrap();
    assert!(a.frame_chunk_requests.is_empty());
    assert_eq!(a.byte(1, 0x10000), a.byte(4, source));
    assert_eq!(a.byte(1, 0x10000 + 512 * PAGE_SIZE), a.byte(4, source + 512 * PAGE_SIZE));
    assert_eq!(a.processes.private_page_references(1, 0x10000, 0x10000 + 513 * PAGE_SIZE).len(), 513);
}

#[test]
fn pool_alias_cow_keeps_pool_backing_and_needs_no_child_cap_node() {
    let mut a = Alpha::new();
    let source = snapshot(&mut a, 1);
    a.map_process_private_snapshot(4, 1, source, 0x10000, PAGE_SIZE, 1).unwrap();
    assert!(a.frame_chunk_requests.is_empty());
    let old = a.processes.private_page_references(1, 0x10000, 0x11000)[0].allocation;
    a.fail_frame_chunks = true;
    a.prepare(1, 0x10000, PAGE_SIZE, 1).unwrap();
    assert!(a.frame_chunk_requests.is_empty());
    assert!(!a.processes.private_page_references(1, 0x10000, 0x11000)[0].allocation.owns_frame_cap);
    assert_ne!(a.physical(1, 0x10000), a.physical(4, source));
    assert_eq!(a.processes.physical_reference_count(old), 1);
}

#[test]
fn cow_replacement_skips_sealed_child_caps_and_preserves_native_cap_node_failure() {
    for sealed in [false, true] {
        let mut a = Alpha::new();
        if !sealed {
            let e = a.processes.memory_entry(1).unwrap();
            a.processes.install_process(1, e.reaper_pid, e.root_slot, e.root_node,
                e.pcb, e.address_space, e.os_port, e.os_port_identifier, e.affinity,
                e.next_frame_slot, e.user_heap_next_va, e.user_heap_limit_va).unwrap();
            a.processes.ensure_vm_space_for_pid(1).unwrap();
        }
        let mut original = Vec::new();
        for i in 0..64 {
            let va = (i + 1) * PAGE_SIZE;
            let physical = a.page(1, va, i as u8 + 1, 1);
            original.push(physical);
            a.processes.register_pool_alias(2, va, i, physical).unwrap();
            a.processes.vm_space_mut(1).unwrap().mark_private_page_shared(va, true);
        }
        let before = KERNEL.with(|kernel| kernel.borrow().child_allocations);
        a.fail_frame_chunks = true;
        if !sealed {
            assert_eq!(a.prepare(1, PAGE_SIZE, 64 * PAGE_SIZE, 1), Err(CapabilityError::Fatal));
            assert_eq!(KERNEL.with(|kernel| kernel.borrow().child_allocations), before);
            a.fail_frame_chunks = false;
        }
        a.prepare(1, PAGE_SIZE, 64 * PAGE_SIZE, 1).unwrap();
        assert_eq!(KERNEL.with(|kernel| kernel.borrow().child_allocations) - before, if sealed { 0 } else { 64 });
        if sealed { assert!(a.frame_chunk_requests.is_empty()); }
        for (i, physical) in original.into_iter().enumerate() {
            let va = (i + 1) * PAGE_SIZE;
            let replacement = a.processes.find_active_physical_allocation_reference(1, va, 1).unwrap();
            assert_eq!(replacement.owns_frame_cap, !sealed);
            assert_ne!(replacement.base_page, physical);
            KERNEL.with(|kernel| {
                let kernel = kernel.borrow();
                assert_eq!(kernel.physical[&replacement.base_page], kernel.physical[&physical]);
            });
        }
    }
}

#[test]
fn cow_pool_conversion_failure_keeps_original_and_unpublished_replacement_owned() {
    let mut a = Alpha::new();
    a.page(1, PAGE_SIZE, 0x73, 1);
    a.clone_process_vm_shared_readonly(1, 2).unwrap();
    let old = a.processes.find_active_physical_allocation_reference(2, PAGE_SIZE, 1).unwrap();
    KERNEL.with(|kernel| kernel.borrow_mut().fail_pool_conversion = true);
    assert_eq!(a.prepare(2, PAGE_SIZE, PAGE_SIZE, 1), Err(CapabilityError::Fatal));
    assert_eq!(a.processes.physical_reference_count(old), 2);
    let failed: Vec<_> = a.processes.deferred_physical_allocations_for_pid(2).collect();
    assert_eq!(failed.len(), 1);
    assert!(!failed[0].owns_frame_cap);
    assert!(a.memory.physical_frame_descriptor_from_index(failed[0].base_page).is_none());
    assert_eq!(a.byte(1, PAGE_SIZE), a.byte(2, PAGE_SIZE));
    KERNEL.with(|kernel| kernel.borrow_mut().fail_pool_conversion = false);
    a.prepare(2, PAGE_SIZE, PAGE_SIZE, 1).unwrap();
    assert_ne!(a.physical(1, PAGE_SIZE), a.physical(2, PAGE_SIZE));
    assert_eq!(a.byte(1, PAGE_SIZE), a.byte(2, PAGE_SIZE));
    assert_eq!(a.processes.deferred_physical_allocations_for_pid(2).count(), 1);
}

#[test]
fn snapshot_uses_reserved_image_slots_even_at_cursor_limit_and_rejects_reuse() {
    let mut a=Alpha::new();let source=snapshot(&mut a,513);
    a.processes.reset_runtime_memory_for_exec(1,PROCESS_FRAME_TOTAL_PAGES,0x200000,0x400000).unwrap();
    let before=KERNEL.with(|k| (k.borrow().allocations,k.borrow().copies));
    a.map_process_private_snapshot_in_slots(4,1,source,0x10000,513*PAGE_SIZE,5,Some(0)).unwrap();
    assert_eq!(a.processes.find_entry_by_pid(1).unwrap().next_frame_slot,PROCESS_FRAME_TOTAL_PAGES);
    // Neither RAM allocation nor child capability copying is needed.
    assert_eq!(before,KERNEL.with(|k| (k.borrow().allocations,k.borrow().copies)));
    for page in 0..513 {
        assert_eq!(a.physical(1,0x10000+page*PAGE_SIZE),a.physical(4,source+page*PAGE_SIZE));
    }
    let counts=KERNEL.with(|k| (k.borrow().caps.len(),k.borrow().ptes.len()));
    // Different VA must not overwrite a still-owned capability slot.
    assert!(a.map_process_private_snapshot_in_slots(4,1,source,0x400000,PAGE_SIZE,1,Some(0)).is_err());
    assert!(a.map_process_private_snapshot_in_slots(4,1,source,0x400000,PAGE_SIZE,1,Some(PROCESS_FRAME_TOTAL_PAGES)).is_err());
    assert_eq!(counts,KERNEL.with(|k| (k.borrow().caps.len(),k.borrow().ptes.len())));
    a.processes.defer_all_active_physical_allocations_for_pid(1);
    assert!(a.processes.preallocated_process_frame_slots(1,0,1,PROCESS_FRAME_TOTAL_PAGES).is_none());
}

#[test]
fn snapshot_reference_buffer_reserves_once_and_matches_the_original_collector() {
    let mut a = Alpha::new();
    let source = snapshot(&mut a, 1024);
    a.check_private_snapshot_source(4, source, 1024 * PAGE_SIZE).unwrap();
    crate::PROTECT_ALLOCATIONS.with(|n| n.set(Some(0)));
    let mut old = Vec::new();
    for reference in a.processes.private_page_reference_walk(4, source, source + 1024 * PAGE_SIZE) {
        old.push(reference);
    }
    old.sort_by_key(|r| r.allocation.base_va);
    old.dedup_by_key(|r| r.allocation.base_va);
    let old_calls = crate::PROTECT_ALLOCATIONS.with(|n| n.replace(None).unwrap());
    crate::PROTECT_ALLOCATIONS.with(|n| n.set(Some(0)));
    let (_, new) = a.prepare_private_snapshot_source(4, source, 1024 * PAGE_SIZE).unwrap();
    let new_calls = crate::PROTECT_ALLOCATIONS.with(|n| n.replace(None).unwrap());
    assert_eq!(old.len(), new.len());
    for (old, new) in old.iter().zip(&new) { assert!(old.allocation == new.allocation); }
    assert_eq!(new_calls, 1);
    assert!(old_calls > new_calls);
    println!("SNAPSHOT-REFERENCE-BUFFER pages=1024 old-allocations={old_calls} new-allocations={new_calls}");
}

#[test]
fn snapshot_admission_walk_avoids_discarded_page_vectors() {
    let mut a = Alpha::new();
    let source = snapshot(&mut a, 1024);
    // Canonicalization is a real ownership change, and may allocate once.
    a.check_private_snapshot_source(4, source, 1024 * PAGE_SIZE).unwrap();
    crate::PROTECT_ALLOCATIONS.with(|n| n.set(Some(0)));
    let old = a.prepare_private_snapshot_source(4, source, 1024 * PAGE_SIZE);
    let old_allocations = crate::PROTECT_ALLOCATIONS.with(|n| n.replace(None).unwrap());
    assert_eq!(old.unwrap().1.len(), 1024);
    crate::PROTECT_ALLOCATIONS.with(|n| n.set(Some(0)));
    let checked = a.check_private_snapshot_source(4, source, 1024 * PAGE_SIZE);
    let new_allocations = crate::PROTECT_ALLOCATIONS.with(|n| n.replace(None).unwrap());
    assert_eq!(checked, Ok(()));
    assert!(old_allocations > 0);
    assert_eq!(new_allocations, 0);
    println!("SNAPSHOT-PREFLIGHT pages=1024 old-allocations={old_allocations} new-allocations={new_allocations}");
    assert!(a.processes.private_page_references(1, 0, usize::MAX).is_empty());
}

#[test]
fn snapshot_validation_reuses_current_vm_with_all_frame_checks_preserved() {
    let mut a=Alpha::new();
    let source=snapshot(&mut a,1024);
    a.prepare_private_snapshot_source(4,source,1024*PAGE_SIZE).unwrap();
    a.processes.take_immutable_vm_lookups();
    a.check_private_snapshot_source(4,source,1024*PAGE_SIZE).unwrap();
    let checked=a.processes.take_immutable_vm_lookups();
    let (_,references)=a.prepare_private_snapshot_source(4,source,1024*PAGE_SIZE).unwrap();
    let collected=a.processes.take_immutable_vm_lookups();
    assert_eq!(references.len(),1024);
    assert_eq!((checked,collected),(1,1));
    println!("SNAPSHOT-VM-BORROW pages=1024 checked-vm-lookups={checked} collected-vm-lookups={collected}");
    // A current wrong Frame remains rejected, despite unchanged permissions
    // and valid ownership. Replacing it with the real Frame restores admission.
    let va=source+511*PAGE_SIZE;
    let frame=a.processes.vm_space_mut(4).unwrap().find_frame(va).unwrap();
    a.processes.vm_space_mut(4).unwrap().record_frame(va,usize::MAX).unwrap();
    assert_eq!(a.check_private_snapshot_source(4,source,1024*PAGE_SIZE),Err(CapabilityError::PermissionDenied));
    a.processes.vm_space_mut(4).unwrap().record_frame(va,frame).unwrap();
    a.check_private_snapshot_source(4,source,1024*PAGE_SIZE).unwrap();
}

#[test]
fn snapshot_admission_walk_matches_collected_validation_and_duplicate_order() {
    for case in 0..9 {
        let mut a = Alpha::new();
        let source = snapshot(&mut a, 3);
        a.prepare_private_snapshot_source(4, source, 3 * PAGE_SIZE).unwrap();
        let references = a.processes.private_page_references(4, source, source + 3 * PAGE_SIZE);
        match case {
            0 => {},
            1 => { a.processes.vm_space_mut(4).unwrap().set_permissions(source + 2 * PAGE_SIZE, 3); },
            2 => { a.processes.vm_space_mut(4).unwrap().mark_mapping_uncertain(source); },
            3 => { a.processes.vm_space_mut(4).unwrap().forget_frame(source + PAGE_SIZE); },
            4 | 5 | 6 => {
                // Duplicate at different chain positions: old validation
                // selects the newest reference after stable sort/dedup.
                let mut page = references[case - 4].allocation;
                if case == 6 { page.start_slot += 1; }
                a.processes.register_physical_allocation(4, page.base_va, page.start_slot, page.base_page, 1).unwrap();
                a.processes.vm_space_mut(4).unwrap().mark_private_page_shared(page.base_va, true);
            },
            7 => { a.processes.release_physical_allocation_reference(4, source + PAGE_SIZE, 1).unwrap(); },
            _ => { a.processes.vm_space_mut(4).unwrap().record_frame(source, usize::MAX).unwrap(); },
        }
        let old = a.prepare_private_snapshot_source(4, source, 3 * PAGE_SIZE).map(|_| ());
        assert_eq!(a.check_private_snapshot_source(4, source, 3 * PAGE_SIZE), old, "case {case}");
    }
}

#[test]
fn snapshot_source_preflight_creates_no_target_alias_and_rechecks_whole_reservation() {
    let mut a=Alpha::new();let source=snapshot(&mut a,3);
    let counts=KERNEL.with(|k| (k.borrow().allocations,k.borrow().copies));
    let (_,refs)=a.prepare_private_snapshot_source(4,source,3*PAGE_SIZE).unwrap();
    assert_eq!(refs.len(),3);
    for reference in refs {
        assert_eq!(a.processes.physical_reference_count(reference.allocation),1);
        assert!(!a.processes.vm_space_mut(4).unwrap().private_page_shared(reference.allocation.base_va));
    }
    assert!(a.processes.private_page_references(1,0x10000,0x13000).is_empty());
    assert_eq!(counts,KERNEL.with(|k| (k.borrow().allocations,k.borrow().copies)));
    a.processes.vm_space_mut(4).unwrap().set_permissions(source+2*PAGE_SIZE,3);
    assert!(a.prepare_private_snapshot_source(4,source,PAGE_SIZE).is_err());
    assert!(a.map_process_private_snapshot(4,1,source,0x10000,PAGE_SIZE,5).is_err());
    assert!(a.processes.private_page_references(1,0x10000,0x11000).is_empty());
}

#[test]
fn native_snapshot_aliases_without_copy_and_private_detach_preserves_source() {
    for pages in [1, 3, 513] {
        let mut a = Alpha::new();
        let source = snapshot(&mut a, pages);
        let before = KERNEL.with(|k| k.borrow().allocations);
        a.map_process_private_snapshot(4, 1, source, 0x10000, pages * PAGE_SIZE, 5)
            .unwrap();
        // Repeated alias of an already canonical one-page source is valid too.
        a.map_process_private_snapshot(4, 2, source, 0x20000, PAGE_SIZE, 1)
            .unwrap();
        assert_eq!(KERNEL.with(|k| k.borrow().allocations), before);
        for offset in 0..pages {
            let va = 0x10000 + offset * PAGE_SIZE;
            assert_eq!(
                a.physical(1, va),
                a.physical(4, source + offset * PAGE_SIZE)
            );
            assert_eq!(a.byte(1, va), 0x30u8.wrapping_add(offset as u8));
            assert_eq!(a.processes.vm_space_mut(1).unwrap().permissions(va), 5);
        }
        a.prepare(1, 0x10000, PAGE_SIZE, 1).unwrap();
        let private = a.physical(1, 0x10000);
        KERNEL.with(|k| k.borrow_mut().physical.get_mut(&private).unwrap()[0] = 0xaa);
        assert_eq!(a.byte(4, source), 0x30);
        assert_eq!(a.byte(2, 0x20000), 0x30);
        assert_eq!(a.byte(1, 0x10000), 0xaa);
    }
}

#[test]
fn snapshot_alias_checks_owner_immutability_extent_and_destination() {
    for invalid in 0..10 {
        let mut a = Alpha::new();
        let source = snapshot(&mut a, 3);
        let mut caller = 4;
        let mut target = 1;
        let mut start = source;
        let mut bytes = PAGE_SIZE;
        let mut permissions = 1;
        match invalid {
            0 => caller = 3,
            1 => target = 4,
            2 => permissions = 3,
            3 => permissions = 4,
            4 => start += 1,
            5 => bytes = 4 * PAGE_SIZE,
            6 => a
                .processes
                .vm_space_mut(4)
                .unwrap()
                .set_permissions(source + 2 * PAGE_SIZE, 3),
            7 => {
                a.page(1, 0x10000, 0, 0);
            }
            8 => {
                a.processes.seal_capabilities(4).unwrap();
            }
            _ => {
                let allocation = a
                    .processes
                    .find_active_physical_allocation_reference(4, source, 3)
                    .unwrap();
                a.processes
                    .register_physical_allocation(3, 0x80000, 0, allocation.base_page, 3)
                    .unwrap();
            }
        }
        let before = KERNEL.with(|k| {
            (
                k.borrow().allocations,
                k.borrow().copies,
                k.borrow().ptes.clone(),
            )
        });
        assert!(a
            .map_process_private_snapshot(caller, target, start, 0x10000, bytes, permissions)
            .is_err());
        KERNEL.with(|k| {
            assert_eq!(
                (
                    k.borrow().allocations,
                    k.borrow().copies,
                    k.borrow().ptes.clone()
                ),
                before
            )
        });
    }
}

#[test]
fn snapshot_checks_the_entire_lazy_destination_before_canonicalizing_or_copying() {
    use crate::nanami_core::process::ProcessLazyMappingKind::Zero;
    for offset in [0, 256, 512, 513] {
        let mut a = Alpha::new();
        let source = snapshot(&mut a, 513);
        a.processes.register_lazy_mapping(1, 0x10000 + offset * PAGE_SIZE, 1, 2000, Zero).unwrap();
        let before = KERNEL.with(|k| {
            let k = k.borrow(); (k.allocations, k.copies, k.ptes.clone())
        });
        let result = a.map_process_private_snapshot(4, 1, source, 0x10000, 513 * PAGE_SIZE, 5);
        if offset == 513 {
            assert!(result.is_ok()); // Adjacent half-open range is not occupied.
            assert_eq!(a.byte(1, 0x10000 + 512 * PAGE_SIZE), 0x30);
        } else {
            assert!(matches!(result, Err(CapabilityError::IllegalOperation)));
            KERNEL.with(|k| {
                let k = k.borrow(); assert_eq!((k.allocations, k.copies, k.ptes.clone()), before);
            });
            assert!(a.processes.find_active_physical_allocation_reference(4, source, 513).is_some());
            assert!(a.processes.private_page_references(1, 0, usize::MAX).is_empty());
            assert!(!a.processes.vm_space_mut(4).unwrap().private_page_shared(source));
        }
    }
}

#[test]
fn snapshot_alias_failures_keep_possible_pte_and_physical_prefix_owned() {
    for after in [false, true] {
        let mut a = Alpha::new();
        let source = snapshot(&mut a, 3);
        KERNEL.with(|k| {
            let mut k = k.borrow_mut();
            k.fail_map = 1;
            k.fail_map_after_install = after;
        });
        assert!(a
            .map_process_private_snapshot(4, 1, source, 0x10000, 3 * PAGE_SIZE, 1)
            .is_err());
        let allocation = a
            .processes
            .find_active_physical_allocation_reference(1, 0x10000, 1)
            .unwrap();
        assert_eq!(a.processes.physical_reference_count(allocation), 2);
        assert!(a
            .processes
            .vm_space_mut(4)
            .unwrap()
            .private_page_shared(source));
        KERNEL.with(|k| {
            assert_eq!(k.borrow().physical.len(), 3);
            assert_eq!(k.borrow().ptes.contains_key(&(501, 0x10000)), after);
        });
        // Before-MAP failures still retain the registered physical owner;
        // after-MAP errors also retain the possible leaf for root rollback.
        assert_eq!(
            a.processes
                .vm_space_mut(1)
                .unwrap()
                .find_frame(0x10000)
                .is_some(),
            true
        );
        {
            let vm = a.processes.vm_space_mut(1).unwrap();
            assert!(vm.mapping_is_uncertain(0x10000));
            assert_eq!(vm.permissions(0x10000), 0);
            assert!(vm.frame_may_be_present(0x10000));
        }
    }
}

#[test]
fn pool_snapshot_has_no_child_cap_and_detach_does_not_remove_one() {
    let mut a = Alpha::new();
    let source = snapshot(&mut a, 2);
    KERNEL.with(|k| k.borrow_mut().fail_copy = true);
    a.map_process_private_snapshot(4, 1, source, 0x10000, 2 * PAGE_SIZE, 1).unwrap();
    let pages = a.processes.private_page_references(1, 0x10000, 0x12000);
    let root = a.processes.memory_entry(1).unwrap().root_node;
    for reference in pages {
        assert!(!reference.allocation.owns_frame_cap);
        let child = process_frame_descriptor(root, reference.allocation.start_slot);
        KERNEL.with(|k| assert!(!k.borrow().caps.contains_key(&child)));
    }
    // A missing child cap must never be removed, including COW. The source's
    // independent RAM reference and immutable bytes must remain intact.
    KERNEL.with(|k| { let mut k = k.borrow_mut(); k.fail_copy = false; k.fail_remove = true; });
    a.prepare(1, 0x10000, PAGE_SIZE, 1).unwrap();
    assert!(!a.processes.private_page_references(1, 0x10000, 0x11000)[0].allocation.owns_frame_cap);
    assert_ne!(a.physical(1, 0x10000), a.physical(4, source));
    assert_eq!(a.byte(1, 0x10000), a.byte(4, source));
}

#[test]
fn snapshot_child_remains_forkable_and_private_after_native_source_retirement() {
    let mut a = Alpha::new();
    let source = snapshot(&mut a, 2);
    a.map_process_private_snapshot(4, 1, source, 0x10000, 2 * PAGE_SIZE, 1)
        .unwrap();
    a.clone_process_vm_shared_readonly(1, 2).unwrap();
    for offset in 0..2 {
        let va = source + offset * PAGE_SIZE;
        let (allocation, last) = a
            .processes
            .release_physical_allocation_reference(4, va, 1)
            .unwrap();
        assert!(!last);
        let frame = process_frame_descriptor(304, allocation.start_slot);
        arch::address_space::unmap(504, frame, va).unwrap();
        arch::node::remove(
            process_frame_chunk_descriptor(304, allocation.start_slot / 512),
            allocation.start_slot % 512,
        )
        .unwrap();
    }
    assert_eq!(a.byte(1, 0x10000), 0x30);
    assert_eq!(a.byte(2, 0x11000), 0x31);
    a.prepare(2, 0x10000, 2 * PAGE_SIZE, 1).unwrap();
    assert_ne!(a.physical(1, 0x10000), a.physical(2, 0x10000));
    assert_eq!(a.byte(2, 0x11000), 0x31);
}

#[test]
fn fork_and_detach_join_sparse_backing_with_unbacked_ownership_and_writable_gaps() {
    let mut a = Alpha::new();
    for page in 1..=32 {
        a.page(1, page * PAGE_SIZE, page as u8, if page % 3 == 0 { 3 } else { 5 });
    }
    let holes = [2, 8, 20, 31];
    for page in holes {
        a.processes.vm_space_mut(1).unwrap().forget_frame(page * PAGE_SIZE);
    }
    a.clone_process_vm_shared_readonly(1, 2).unwrap();
    for page in 1..=32 {
        let va = page * PAGE_SIZE;
        if holes.contains(&page) {
            assert!(a.processes.vm_space_mut(2).unwrap().find_frame(va).is_none());
            continue;
        }
        assert_eq!(a.byte(2, va), page as u8);
        assert_eq!(a.physical(1, va) == a.physical(2, va), page % 3 != 0);
    }
    a.prepare(2, PAGE_SIZE, 32 * PAGE_SIZE, 1).unwrap();
    for page in 1..=32 {
        if holes.contains(&page) {
            continue;
        }
        let va = page * PAGE_SIZE;
        assert_eq!(a.byte(1, va), page as u8);
        assert_eq!(a.byte(2, va), page as u8);
        assert_ne!(a.physical(1, va), a.physical(2, va));
        assert!(!a.processes.vm_space_mut(2).unwrap().private_page_shared(va));
    }
}

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
            (8, 0)
        )
    });
}

#[test]
fn writable_fork_pages_publish_final_rights_without_leaf_remapping() {
    let mut a = Alpha::new();
    let mut original = Vec::new();
    for i in 0..64 {
        let permissions = if i < 32 { 3 } else { 7 };
        original.push(a.page(1, (i + 1) * PAGE_SIZE, i as u8 + 1, permissions));
    }
    let child = a.processes.memory_entry(2).unwrap();
    a.clone_process_vm_shared_readonly(1, 2).unwrap();
    for (i, physical) in original.into_iter().enumerate() {
        let va = (i + 1) * PAGE_SIZE;
        assert_ne!(a.physical(2, va), physical);
        assert_eq!(a.byte(1, va), a.byte(2, va));
        assert_eq!(a.processes.vm_space_mut(2).unwrap().permissions(va), if i < 32 { 3 } else { 7 });
    }
    KERNEL.with(|kernel| {
        let kernel = kernel.borrow();
        let maps: Vec<_> = kernel.maps.iter().filter(|(space, _, _)| *space == child.address_space).collect();
        assert_eq!(maps.len(), 64);
        assert!(maps.iter().all(|(_, va, rights)| *rights == if *va <= 32 * PAGE_SIZE { 3 } else { 7 }));
        assert!(!kernel.unmaps.iter().any(|(space, _)| *space == child.address_space));
    });
}

#[test]
fn eager_only_fork_skips_ownership_walk_and_preserves_rights_and_private_bytes() {
    let mut a = Alpha::new();
    let rights = [0, 2, 3, 6, 7];
    for i in 0..125 {
        a.page(1, (i + 1) * PAGE_SIZE, i as u8 + 1, rights[i / 25]);
    }
    // The previous preparation always collected this full ownership snapshot.
    a.processes.take_physical_page_range_visits();
    let old = a.processes.private_page_references(1, 0, usize::MAX);
    assert_eq!(old.len(), 125);
    let old_visits = a.processes.take_physical_page_range_visits();
    assert_eq!(old_visits, 125);
    a.clone_process_vm_shared_readonly(1, 2).unwrap();
    let visits = a.processes.take_physical_page_range_visits();
    assert_eq!(visits, 0);
    for i in 0..125 {
        let va = (i + 1) * PAGE_SIZE;
        assert_eq!(a.byte(1, va), a.byte(2, va));
        assert_ne!(a.physical(1, va), a.physical(2, va));
        for pid in [1, 2] {
            let vm = a.processes.vm_space_mut(pid).unwrap();
            assert_eq!(vm.permissions(va), rights[i / 25]);
            assert!(!vm.private_page_shared(va));
        }
        assert_eq!(a.processes.physical_reference_count(old[i].allocation), 1);
    }
    println!("FORK-SHARE-ADMISSION pages=125 old-ownership-visits={old_visits} new-ownership-visits={visits}");
}

#[test]
fn fork_compacted_references_preserve_mixed_runs_gaps_and_untracked_aliases() {
    let mut a = Alpha::new();
    let mut expected = Vec::new();
    for i in 0..129 {
        let va = (i + 1 + i / 11) * PAGE_SIZE;
        let rights = [1, 3, 5, 0, 7, 1][i % 6];
        let physical = a.page(1, va, i as u8, rights);
        let extra_alias = i % 17 == 0;
        if extra_alias {
            a.processes.register_pool_alias(3, va, i, physical).unwrap();
        }
        expected.push((va, rights, extra_alias, rights != 0 && rights & !5 == 0 && !extra_alias));
    }
    crate::PROTECT_ALLOCATIONS.with(|n| n.set(Some(0)));
    a.clone_process_vm_shared_readonly(1, 2).unwrap();
    let allocations = crate::PROTECT_ALLOCATIONS.with(|n| n.replace(None).unwrap());
    println!("FORK-REFERENCE-COMPACT mixed-pages=129 allocations={allocations}");
    // Inaccessible child pages no longer allocate temporary mock PTE nodes.
    assert!(allocations <= 168);
    for (va, rights, extra_alias, shared) in expected {
        assert_eq!(a.byte(1, va), a.byte(2, va));
        assert_eq!(a.physical(1, va) == a.physical(2, va), shared);
        let original = a.processes.find_active_physical_allocation_reference(1, va, 1).unwrap();
        assert_eq!(a.processes.physical_reference_count(original), if extra_alias || shared { 2 } else { 1 });
        for pid in [1, 2] {
            let vm = a.processes.vm_space_mut(pid).unwrap();
            assert_eq!(vm.permissions(va), rights);
            assert_eq!(vm.private_page_shared(va), shared);
        }
    }
}

#[test]
fn mixed_fork_bounds_ownership_collection_to_the_readonly_span() {
    let mut a = Alpha::new();
    // Writable records bracket the candidate span in both VA and chain order.
    // The two-interval ownership bounds can skip that old, disjoint tail.
    for i in 0..256 {
        a.page(1, (i + 1) * PAGE_SIZE, i as u8, 3);
        a.page(1, (i + 273) * PAGE_SIZE, (i as u8).wrapping_add(1), 7);
    }
    for i in 0..16 {
        a.page(1, (i + 257) * PAGE_SIZE, i as u8 + 2, if i % 2 == 0 { 1 } else { 5 });
    }
    a.processes.take_physical_page_range_visits();
    let original = a.processes.private_page_references(1, 0, usize::MAX);
    let old_visits = a.processes.take_physical_page_range_visits();
    assert_eq!((original.len(), old_visits), (528, 528));
    a.clone_process_vm_shared_readonly(1, 2).unwrap();
    let visits = a.processes.take_physical_page_range_visits();
    println!("FORK-REFERENCE-SPAN backed=528 candidates=16 old-range-visits={old_visits} new-range-visits={visits}");
    assert_eq!(visits, 17);
    for i in 0..528 {
        let va = (i + 1) * PAGE_SIZE;
        let shared = (256..272).contains(&i);
        assert_eq!(a.byte(1, va), a.byte(2, va));
        assert_eq!(a.physical(1, va) == a.physical(2, va), shared);
        for pid in [1, 2] {
            let vm = a.processes.vm_space_mut(pid).unwrap();
            assert_eq!(vm.private_page_shared(va), shared);
            let expected = if i < 256 { 3 } else if i >= 272 { 7 }
                else if i % 2 == 0 { 1 } else { 5 };
            assert_eq!(vm.permissions(va), expected);
        }
        assert_eq!(a.processes.physical_reference_count(original[i].allocation), if shared { 2 } else { 1 });
    }
}

#[test]
fn readonly_fork_pool_aliases_skip_caps_and_nodes_and_detach_without_old_cap_removal() {
    let mut a = Alpha::new();
    for i in 0..513 {
        a.page(1, (i + 1) * PAGE_SIZE, i as u8, if i % 2 == 0 { 1 } else { 5 });
    }
    let before = KERNEL.with(|kernel| (kernel.borrow().allocations, kernel.borrow().copies));
    a.fail_frame_chunks = true;
    KERNEL.with(|kernel| kernel.borrow_mut().fail_copy = true);
    a.processes.take_mutable_vm_lookups();
    a.processes.take_immutable_vm_lookups();
    a.clone_process_vm_shared_readonly(1, 2).unwrap();
    let mutable_vm=a.processes.take_mutable_vm_lookups();
    let immutable_vm=a.processes.take_immutable_vm_lookups();
    assert_eq!((mutable_vm,immutable_vm),(1028,1));
    println!("FORK-VM-BORROW pages=513 mutable-vm-lookups={mutable_vm} immutable-vm-lookups={immutable_vm}");
    assert!(a.frame_chunk_requests.is_empty());
    assert_eq!(KERNEL.with(|kernel| (kernel.borrow().allocations, kernel.borrow().copies)), before);
    let child = a.processes.memory_entry(2).unwrap();
    for i in 0..513 {
        let va = (i + 1) * PAGE_SIZE;
        let allocation = a.processes.find_active_physical_allocation_reference(2, va, 1).unwrap();
        assert!(!allocation.owns_frame_cap);
        assert_eq!(a.processes.physical_reference_count(allocation), 2);
        assert_eq!(a.physical(1, va), a.physical(2, va));
        assert_eq!(a.byte(1, va), a.byte(2, va));
        KERNEL.with(|kernel| assert!(!kernel.borrow().caps.contains_key(&process_frame_descriptor(child.root_node, i))));
    }
    a.fail_frame_chunks = false;
    KERNEL.with(|kernel| { let mut kernel = kernel.borrow_mut(); kernel.fail_copy = false; kernel.fail_remove = true; });
    a.prepare(2, PAGE_SIZE, PAGE_SIZE, 1).unwrap();
    assert_ne!(a.physical(1, PAGE_SIZE), a.physical(2, PAGE_SIZE));
    assert_eq!(a.byte(1, PAGE_SIZE), a.byte(2, PAGE_SIZE));
}

#[test]
fn readonly_fork_and_private_detach_reject_uncertain_source_backing() {
    let mut a = Alpha::new();
    a.page(1, 0x1000, 0x51, 5);
    a.processes
        .vm_space_mut(1)
        .unwrap()
        .mark_mapping_uncertain(0x1000);
    let before = KERNEL.with(|kernel| (kernel.borrow().allocations, kernel.borrow().copies));
    assert_eq!(
        a.clone_process_vm_shared_readonly(1, 2),
        Err(CapabilityError::Fatal)
    );
    assert_eq!(
        KERNEL.with(|kernel| (kernel.borrow().allocations, kernel.borrow().copies)),
        before
    );

    let mut a = Alpha::new();
    a.page(1, 0x1000, 0x51, 5);
    a.clone_process_vm_shared_readonly(1, 2).unwrap();
    a.processes
        .vm_space_mut(2)
        .unwrap()
        .mark_mapping_uncertain(0x1000);
    let before = KERNEL.with(|kernel| (kernel.borrow().allocations, kernel.borrow().copies));
    assert!(a.prepare(2, 0x1000, PAGE_SIZE, 1).is_err());
    assert_eq!(
        KERNEL.with(|kernel| (kernel.borrow().allocations, kernel.borrow().copies)),
        before
    );
}

#[test]
fn detach_reuses_ready_source_and_pins_it_across_initializer_slot_collision() {
    for (warm,collision) in [(false,false),(true,false),(true,true)] {
        let mut a=Alpha::new();
        let original=a.page(1,PAGE_SIZE,0x59,5);
        a.clone_process_vm_shared_readonly(1,2).unwrap();
        let source_frame=a.memory.physical_frame_descriptor_from_index(original).unwrap();
        let source=KERNEL.with_borrow_mut(|k| k.physical.get_mut(&original).unwrap().as_mut_ptr() as usize);
        KERNEL.with_borrow_mut(|k| {
            k.copy_aliases.clear();k.track_copy_aliases=true;k.initializer_maps=0;
            if warm { k.ready_initializers.insert(source_frame,source); }
            if collision { k.initializer_collision=Some(original); }
        });
        assert_eq!(a.prepare(2,PAGE_SIZE,PAGE_SIZE,1),Ok(1));
        let replacement=a.physical(2,PAGE_SIZE);
        assert_ne!(replacement,original);
        assert_eq!(a.byte(1,PAGE_SIZE),0x59);assert_eq!(a.byte(2,PAGE_SIZE),0x59);
        let destination_frame=a.memory.physical_frame_descriptor_from_index(replacement).unwrap();
        KERNEL.with_borrow(|k| {
            assert_eq!(k.copy_aliases,if collision {vec![(destination_frame,Some(source))]}
                else if warm {vec![]} else {vec![(source_frame,None)]});
            assert_eq!(k.initializer_maps,usize::from(!collision));
        });
        KERNEL.with_borrow_mut(|k| k.physical.get_mut(&replacement).unwrap()[0]=0x9a);
        assert_eq!(a.byte(1,PAGE_SIZE),0x59);assert_eq!(a.byte(2,PAGE_SIZE),0x9a);
        assert_eq!(a.processes.vm_space_mut(2).unwrap().permissions(PAGE_SIZE),5);
    }
}

#[test]
fn detach_alias_failures_keep_replacement_owned_and_warm_hits_never_bypass_live_vm_checks() {
    for case in 0..4 {
        let mut a=Alpha::new();
        let original=a.page(1,PAGE_SIZE,0x59,5);
        a.clone_process_vm_shared_readonly(1,2).unwrap();
        let source_frame=a.memory.physical_frame_descriptor_from_index(original).unwrap();
        KERNEL.with_borrow_mut(|k| {
            let source=k.physical.get_mut(&original).unwrap().as_mut_ptr() as usize;
            if case!=0 { k.ready_initializers.insert(source_frame,source); }
            if case==2 { k.initializer_collision=Some(original); }
            k.fail_copy_alias=case==0||case==2;
            k.fail_initializer=case==1;
            k.copy_aliases.clear();k.track_copy_aliases=true;k.initializer_maps=0;
        });
        if case==3 { a.processes.vm_space_mut(2).unwrap().mark_mapping_uncertain(PAGE_SIZE); }
        assert_eq!(a.prepare(2,PAGE_SIZE,PAGE_SIZE,1),Err(CapabilityError::Fatal));
        assert_eq!(a.physical(2,PAGE_SIZE),original);
        assert_eq!(a.byte(1,PAGE_SIZE),0x59);assert_eq!(a.byte(2,PAGE_SIZE),0x59);
        assert!(a.processes.vm_space_mut(2).unwrap().private_page_shared(PAGE_SIZE));
        assert_eq!(a.processes.deferred_physical_allocations_for_pid(2).count(),usize::from(case!=3));
    }
}

#[test]
fn ready_detach_sources_remove_sixty_four_copy_window_requests_without_changing_data() {
    for warm in [false,true] {
        let mut a=Alpha::new();
        let mut sources=Vec::new();
        for page in 1..=64 {
            let physical=a.page(1,page*PAGE_SIZE,page as u8,1);
            let frame=a.memory.physical_frame_descriptor_from_index(physical).unwrap();
            sources.push((physical,frame));
        }
        a.clone_process_vm_shared_readonly(1,2).unwrap();
        KERNEL.with_borrow_mut(|k| {
            k.copy_aliases.clear();k.track_copy_aliases=true;k.initializer_maps=0;
            if warm {
                for &(physical,frame) in &sources {
                    let address=k.physical.get_mut(&physical).unwrap().as_mut_ptr() as usize;
                    k.ready_initializers.insert(frame,address);
                }
            }
        });
        assert_eq!(a.prepare(2,PAGE_SIZE,64*PAGE_SIZE,1),Ok(1));
        KERNEL.with_borrow(|k| {assert_eq!(k.copy_aliases.len(),if warm {0}else{64});assert_eq!(k.initializer_maps,64);});
        for page in 1..=64 {
            assert_eq!(a.byte(1,page*PAGE_SIZE),page as u8);
            assert_eq!(a.byte(2,page*PAGE_SIZE),page as u8);
        }
    }
}

#[test]
fn query_is_side_effect_free_and_detach_preserves_bytes_permissions_and_parent() {
    let mut a = Alpha::new();
    let original = a.page(1, 0x1000, 0x51, 5);
    a.clone_process_vm_shared_readonly(1, 2).unwrap();
    assert_eq!(a.prepare(2, 0x1001, 1, 0), Ok(1));
    assert!(a.invalidations.is_empty());
    assert_eq!(a.prepare(2, 0x1001, 1, 1), Ok(1));
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
                page_count: 1,
                owns_frame_cap: true,
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
    production::take_detach_context_resolutions();
    a.prepare(2, PAGE_SIZE, 64 * PAGE_SIZE, 1).unwrap();
    let contexts = production::take_detach_context_resolutions();
    println!("DETACH-CONTEXT pages=64 pid=2 resolutions={contexts}");
    assert_eq!(contexts, 1);
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
    for pid in [1, 3] {
        let allocated = KERNEL.with(|kernel| kernel.borrow().allocations);
        production::take_detach_context_resolutions();
        a.prepare(pid, PAGE_SIZE, 64 * PAGE_SIZE, 1).unwrap();
        let contexts = production::take_detach_context_resolutions();
        println!("DETACH-CONTEXT pages=64 pid={pid} resolutions={contexts}");
        assert_eq!(contexts, 1);
        if pid == 3 {
            assert_eq!(KERNEL.with(|kernel| kernel.borrow().allocations), allocated);
        }
        for page in 1..=64 {
            assert_eq!(a.byte(pid, page * PAGE_SIZE), page as u8);
            assert!(!a.processes.vm_space_mut(pid).unwrap().private_page_shared(page * PAGE_SIZE));
        }
    }
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
        let target = if remove { 1 } else { 2 };
        assert_eq!(
            a.prepare(target, 0x1000, PAGE_SIZE, 1),
            Err(CapabilityError::Fatal)
        );
        assert_eq!(a.byte(1, 0x1000), 0x3a);
        assert!(a
            .processes
            .vm_space_mut(target)
            .unwrap()
            .private_page_shared(0x1000));
        assert_eq!(
            a.processes.deferred_physical_allocations_for_pid(target).count(),
            1
        );
        let child = a.processes.memory_entry(target).unwrap();
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
fn failed_fork_map_holds_reference_and_source_marker_for_normal_rollback() {
    let mut a = Alpha::new();
    let original = a.page(1, 0x1000, 0x42, 1);
    KERNEL.with(|kernel| kernel.borrow_mut().fail_map = 1);
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
fn deferred_detach_keeps_child_cap_replacement_and_failed_remove_on_the_strict_present_path() {
    for fail_remove in [false, true] {
        let mut a = Alpha::new();
        let old = a.page(2, 0x1000, 0x47, 1);
        a.processes.register_physical_allocation(3, 0x9000, 0, old, 1).unwrap();
        a.processes.vm_space_mut(2).unwrap().mark_private_page_shared(0x1000, true);
        KERNEL.with_borrow_mut(|k| k.fail_remove = fail_remove);
        let result = a.prepare(2, 0x1000, PAGE_SIZE, nanami_uapi::os::OS_PRIVATE_WRITE_DETACH_DEFERRED);
        assert_eq!(result, if fail_remove { Err(CapabilityError::Fatal) } else { Ok(1) });
        let vm = a.processes.vm_space(2).unwrap();
        assert_eq!(vm.permissions(0x1000), 1);
        assert!(!vm.mapping_deferred(0x1000));
        assert!(!vm.mapping_is_uncertain(0x1000));
        assert_eq!(vm.private_page_shared(0x1000), fail_remove);
        KERNEL.with_borrow(|k| assert!(k.ptes.contains_key(&(502, 0x1000))));
        assert_eq!(a.byte(2, 0x1000), 0x47);
        assert_eq!(a.physical(2, 0x1000) == old, fail_remove);
    }
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
        a.prepare(2, 0x1000, 1, 3),
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

#[test]
fn writable_cow_fork_defers_all_heap_copies_and_detaches_only_written_pages() {
    let mut a = Alpha::new();
    let pages = 128;
    for i in 0..pages { a.page(1, (i + 1) * PAGE_SIZE, i as u8, if i < 64 { 3 } else { 7 }); }
    let before = KERNEL.with(|k| k.borrow().allocations);
    a.clone_process_vm_private_cow(1, 2).unwrap();
    assert_eq!(KERNEL.with(|k| k.borrow().allocations), before);
    for pid in [1, 2] {
        let space = a.processes.memory_entry(pid).unwrap().address_space;
        for i in 0..pages {
            let va = (i + 1) * PAGE_SIZE;
            let vm = a.processes.vm_space_mut(pid).unwrap();
            assert_eq!(vm.permissions(va), if i < 64 { 3 } else { 7 });
            assert!(vm.private_page_cow_write(va));
            KERNEL.with(|k| assert_eq!(k.borrow().ptes[&(space, va)].1, if i < 64 { 1 } else { 5 }));
        }
    }
    for va in [PAGE_SIZE, 65 * PAGE_SIZE] {
        assert_eq!(a.physical(1, va), a.physical(2, va));
        let old = a.byte(1, va);
        a.prepare(2, va, PAGE_SIZE, nanami_uapi::os::OS_PRIVATE_WRITE_DETACH).unwrap();
        assert_ne!(a.physical(1, va), a.physical(2, va));
        let pfn = a.physical(2, va);
        KERNEL.with(|k| k.borrow_mut().physical.get_mut(&pfn).unwrap()[0] = 0xef);
        assert_eq!(a.byte(1, va), old);
        // The original is now the sole owner: restore its writable leaf with
        // no allocation or copy, before clearing the COW marker.
        let allocations = KERNEL.with(|k| k.borrow().allocations);
        a.prepare(1, va, PAGE_SIZE, nanami_uapi::os::OS_PRIVATE_WRITE_DETACH).unwrap();
        assert_eq!(KERNEL.with(|k| k.borrow().allocations), allocations);
        let vm = a.processes.vm_space_mut(1).unwrap();
        assert!(!vm.private_page_shared(va));
        let space = a.processes.memory_entry(1).unwrap().address_space;
        KERNEL.with(|k| assert!(k.borrow().ptes[&(space, va)].1 & 2 != 0));
    }
    assert_eq!(KERNEL.with(|k| k.borrow().allocations) - before, 2);
    println!("WRITABLE-COW pages=128 eager-copy-bytes=524288 fork-copy-bytes=0 written-pages=2 detach-copy-bytes=8192");
}

#[test]
fn fork_copies_the_anonymous_allocator_prefix_but_keeps_large_tail_on_cow() {
    for permissions in [3, 7] {
        let mut a = Alpha::new();
        let base = USER_ANONYMOUS_MAP_BASE;
        let pages = 160;
        for page in 0..pages { a.page(1, base + page * PAGE_SIZE, page as u8, permissions); }
        let before = KERNEL.with(|k| k.borrow().allocations);
        a.clone_process_vm_private_cow(1, 2).unwrap();
        assert_eq!(KERNEL.with(|k| k.borrow().allocations) - before, 32);
        for page in 0..pages {
            let va = base + page * PAGE_SIZE;
            assert_eq!(a.byte(1, va), page as u8);
            assert_eq!(a.byte(2, va), page as u8);
            let shared = page >= 32;
            assert_eq!(a.physical(1, va) == a.physical(2, va), shared);
            for pid in [1, 2] {
                let vm = a.processes.vm_space_mut(pid).unwrap();
                assert_eq!(vm.permissions(va), permissions);
                assert_eq!(vm.private_page_cow_write(va), shared);
                let space = a.processes.memory_entry(pid).unwrap().address_space;
                KERNEL.with(|k| assert_eq!(k.borrow().ptes[&(space,va)].1,
                    if shared { permissions & !2 } else { permissions }));
            }
        }
        // Hot-prefix writes need no detach. Later allocator writes use the
        // ordinary COW commit and leave the parent's snapshot intact.
        for offset in [0x12000, 0x25000, 0x42000] {
            let va = base + offset;
            if offset >= 128 * 1024 {
                let before = KERNEL.with_borrow(|k| k.allocations);
                a.prepare(2, va, PAGE_SIZE, nanami_uapi::os::OS_PRIVATE_WRITE_DETACH).unwrap();
                assert_eq!(KERNEL.with_borrow(|k| k.allocations), before + 1);
            }
            let physical = a.physical(2, va);
            KERNEL.with(|k| k.borrow_mut().physical.get_mut(&physical).unwrap()[0] = 0xef);
            assert_ne!(a.byte(1, va), 0xef);
            assert_eq!(a.byte(2, va), 0xef);
        }
        let va = base + 32 * PAGE_SIZE;
        a.prepare(2, va, PAGE_SIZE, nanami_uapi::os::OS_PRIVATE_WRITE_DETACH).unwrap();
        assert_ne!(a.physical(1, va), a.physical(2, va));
    }
}

#[test]
fn allocator_prefix_policy_keeps_readonly_aliases_and_does_not_cover_other_arenas() {
    for (base, permissions) in [(USER_ANONYMOUS_MAP_BASE, 1),
        (USER_ANONYMOUS_MAP_BASE, 5), (USER_ANONYMOUS_MAP_BASE - 32 * PAGE_SIZE, 3),
        (USER_ANONYMOUS_MAP_BASE + 128 * 1024, 3), (USER_ANONYMOUS_MAP_BASE + 512 * 1024, 3)]
    {
        let mut a = Alpha::new();
        for page in 0..32 { a.page(1, base + page * PAGE_SIZE, 0x59, permissions); }
        let before = KERNEL.with(|k| k.borrow().allocations);
        a.clone_process_vm_private_cow(1, 2).unwrap();
        assert_eq!(KERNEL.with(|k| k.borrow().allocations), before);
        for page in 0..32 {
            let va = base + page * PAGE_SIZE;
            assert_eq!(a.physical(1, va), a.physical(2, va));
            assert_eq!(a.byte(2, va), 0x59);
        }
    }
}

#[test]
fn allocator_prefix_boundary_retains_the_minimum_cow_run_size() {
    for tail in [31, 32, 33] {
        let mut a = Alpha::new();
        let base = USER_ANONYMOUS_MAP_BASE + 128 * 1024 - 16 * PAGE_SIZE;
        for page in 0..16 + tail { a.page(1,base + page * PAGE_SIZE,0x59,3); }
        let before=KERNEL.with(|k|k.borrow().allocations);
        a.clone_process_vm_private_cow(1,2).unwrap();
        assert_eq!(KERNEL.with(|k|k.borrow().allocations)-before,if tail < 32 {16 + tail}else{16});
        for page in 0..16 + tail {
            let va=base + page * PAGE_SIZE;
            let shared=page >= 16 && tail >= 32;
            assert_eq!(a.physical(1,va)==a.physical(2,va),shared);
            assert_eq!(a.byte(1,va),0x59);
            assert_eq!(a.byte(2,va),0x59);
            assert_eq!(a.processes.vm_space(1).unwrap().private_page_cow_write(va),shared);
        }
    }
}

#[test]
fn writable_cow_fork_map_failures_restore_source_or_keep_shared_leaves_readonly() {
    for after in [false, true] {
        for position in 0..8 {
            let mut a = Alpha::new();
            for i in 0..32 { a.page(1, (i + 1) * PAGE_SIZE, i as u8 + 31, 3); }
            KERNEL.with(|k| { let mut k = k.borrow_mut();
                k.fail_map = 1; k.fail_map_at = Some(k.maps.len() + position);
                k.fail_map_after_install = after;
            });
            assert!(a.clone_process_vm_private_cow(1, 2).is_err());
            for i in 0..4 {
                let va = (i + 1) * PAGE_SIZE;
                assert_eq!(a.byte(1, va), i as u8 + 31);
                let vm = a.processes.vm_space_mut(1).unwrap();
                assert!(!vm.mapping_is_uncertain(va));
                assert_eq!(vm.permissions(va), 3);
                let shared = vm.private_page_shared(va);
                let space = a.processes.memory_entry(1).unwrap().address_space;
                KERNEL.with(|k| assert_eq!(k.borrow().ptes[&(space, va)].1, if shared { 1 } else { 3 }));
                if let Some(reference) = a.processes.find_active_physical_allocation_reference(2, va, 1) {
                    assert!(shared);
                    assert_eq!(a.processes.physical_reference_count(reference), 2);
                }
            }
        }
    }
}

#[test]
fn detach_commit_boolean_covers_copied_sole_owner_and_empty_repairs() {
    for permissions in [3, 7] {
        let mut a = Alpha::new();
        for i in 0..32 { a.page(1, (i + 1) * PAGE_SIZE, 0x51, permissions); }
        a.clone_process_vm_private_cow(1, 2).unwrap();
        let before = KERNEL.with(|k| k.borrow().allocations);
        let va = 4 * PAGE_SIZE;
        assert_eq!(a.prepare(2, va, PAGE_SIZE, 1), Ok(1));
        assert_eq!(KERNEL.with(|k| k.borrow().allocations), before + 1);
        assert_eq!(a.prepare(1, va, PAGE_SIZE, 1), Ok(1));
        assert_eq!(KERNEL.with(|k| k.borrow().allocations), before + 1);
        for pid in [1, 2] {
            assert_eq!(a.byte(pid, va), 0x51);
            let space = a.processes.memory_entry(pid).unwrap().address_space;
            KERNEL.with(|k| assert_eq!(k.borrow().ptes[&(space, va)].1, permissions));
            let operations = KERNEL.with(|k| (k.borrow().maps.len(), k.borrow().unmaps.len()));
            assert_eq!(a.prepare(pid, va, PAGE_SIZE, 1), Ok(0));
            assert_eq!(a.prepare(pid, 1000 * PAGE_SIZE, PAGE_SIZE, 1), Ok(0));
            assert_eq!(KERNEL.with(|k| (k.borrow().maps.len(), k.borrow().unmaps.len())), operations);
        }
    }
}

#[test]
fn detach_with_a_committed_prefix_and_later_failure_never_reports_success() {
    let mut a = Alpha::new();
    for i in 0..32 { a.page(1, (i + 1) * PAGE_SIZE, i as u8, 3); }
    a.clone_process_vm_private_cow(1, 2).unwrap();
    KERNEL.with(|k| { let mut k = k.borrow_mut();
        k.fail_map = 1; k.fail_map_at = Some(k.maps.len() + 1);
    });
    assert!(a.prepare(2, PAGE_SIZE, 2 * PAGE_SIZE, 1).is_err());
    assert_eq!(a.prepare(2, PAGE_SIZE, PAGE_SIZE, 0), Ok(0));
    assert_eq!(a.prepare(2, 2 * PAGE_SIZE, PAGE_SIZE, 0), Ok(1));
    for pid in [1,2] { assert_eq!(a.byte(pid, PAGE_SIZE), 0); assert_eq!(a.byte(pid, 2 * PAGE_SIZE), 1); }
}

#[test]
fn writable_cow_descendants_detach_failures_never_restore_a_writable_shared_frame() {
    for fail_maps in [0, 1, 2] {
        for after in [false, true] {
            let mut a = Alpha::new();
            for i in 0..32 { a.page(1, (i + 1) * PAGE_SIZE, 0x51, 7); }
            a.clone_process_vm_private_cow(1, 2).unwrap();
            a.clone_process_vm_private_cow(2, 3).unwrap();
            let old = a.physical(1, PAGE_SIZE);
            assert_eq!(a.physical(3, PAGE_SIZE), old);
            KERNEL.with(|k| { let mut k = k.borrow_mut(); k.fail_map = fail_maps; k.fail_map_after_install = after; });
            let result = a.prepare(2, PAGE_SIZE, PAGE_SIZE, nanami_uapi::os::OS_PRIVATE_WRITE_DETACH);
            assert_eq!(result.is_ok(), fail_maps == 0);
            for pid in [1, 3] { assert_eq!(a.physical(pid, PAGE_SIZE), old); assert_eq!(a.byte(pid, PAGE_SIZE), 0x51); }
            let space = a.processes.memory_entry(2).unwrap().address_space;
            KERNEL.with(|k| {
                let k = k.borrow();
                if let Some(&(frame, permissions)) = k.ptes.get(&(space, PAGE_SIZE)) {
                    assert!(permissions & 2 == 0 || k.caps[&frame] != old);
                }
            });
            let vm = a.processes.vm_space_mut(2).unwrap();
            assert_eq!(vm.mapping_is_uncertain(PAGE_SIZE), fail_maps == 2);
            assert_eq!(vm.private_page_shared(PAGE_SIZE), fail_maps != 0);
        }
    }
}

#[test]
fn cow_admission_keeps_small_writable_runs_and_the_fixed_stack_eager() {
    let mut a = Alpha::new();
    for pid in [1, 2] {
        let p = a.processes.find_entry_by_pid(pid).unwrap();
        a.processes.install_process(pid, p.reaper_pid, p.root_slot, p.root_node, p.pcb,
            p.address_space, p.os_port, p.os_port_identifier, p.affinity, p.next_frame_slot,
            p.user_heap_next_va, 0x0800_0000).unwrap();
        a.processes.seal_capabilities(pid).unwrap();
    }
    for (base, pages) in [(PAGE_SIZE, 31), (0x30000, 32), (USER_STACK_BASE, 32)] {
        for i in 0..pages { a.page(1, base + i * PAGE_SIZE, i as u8, 3); }
    }
    let before = KERNEL.with(|k| k.borrow().allocations);
    a.clone_process_vm_private_cow(1, 2).unwrap();
    assert_eq!(KERNEL.with(|k| k.borrow().allocations) - before, 63);
    for (base, pages, shared) in [(PAGE_SIZE, 31, false), (0x30000, 32, true), (USER_STACK_BASE, 32, false)] {
        for i in 0..pages {
            let va = base + i * PAGE_SIZE;
            assert_eq!(a.physical(1, va) == a.physical(2, va), shared);
            assert_eq!(a.byte(1, va), a.byte(2, va));
            for pid in [1, 2] {
                let vm = a.processes.vm_space_mut(pid).unwrap();
                assert_eq!(vm.private_page_cow_write(va), shared);
                assert_eq!(vm.permissions(va), 3);
                let space = a.processes.memory_entry(pid).unwrap().address_space;
                KERNEL.with(|k| assert_eq!(k.borrow().ptes[&(space, va)].1, if shared { 1 } else { 3 }));
            }
        }
    }
}

#[test]
fn exited_cow_task_can_prepare_robust_and_clear_tid_writes_for_live_vm_peers() {
    let mut a = Alpha::new();
    for i in 0..32 { a.page(1, (i + 1) * PAGE_SIZE, 0x61, 3); }
    a.clone_process_vm_private_cow(1, 2).unwrap();
    a.processes.share_address_space(3, 2).unwrap();
    a.processes.mark_exited(2, 1, 9).unwrap();
    assert_eq!(a.prepare(2, PAGE_SIZE, 4, nanami_uapi::os::OS_PRIVATE_WRITE_QUERY), Ok(1));
    a.prepare(2, PAGE_SIZE, 4, nanami_uapi::os::OS_PRIVATE_WRITE_DETACH).unwrap();
    let physical = a.physical(3, PAGE_SIZE);
    assert_ne!(physical, a.physical(1, PAGE_SIZE));
    assert_eq!(physical, a.physical(2, PAGE_SIZE));
    KERNEL.with(|k| k.borrow_mut().physical.get_mut(&physical).unwrap()[0] = 0);
    assert_eq!(a.byte(3, PAGE_SIZE), 0);
    assert_eq!(a.byte(1, PAGE_SIZE), 0x61);
    assert_eq!(a.prepare(2, PAGE_SIZE, 4, nanami_uapi::os::OS_PRIVATE_WRITE_QUERY), Ok(0));
}

fn populate(a: &mut Alpha, page: usize, rights: usize) -> Result<usize,CapabilityError> {
    a.handle_populate_snapshot(OsRequestEvent{identifier:4,arg0:1,arg1:page,arg2:rights,arg3:0})
}

#[test]
fn readonly_cow_fork_retains_all_backing_but_maps_only_large_run_prefixes() {
    for rights in [1,4,5] {
        for pages in [127,128,512,1024] {
            let mut a=Alpha::new();let base=0x1000000;
            for page in 0..pages {a.page(1,base+page*PAGE_SIZE,page as u8,rights);}
            let before=KERNEL.with_borrow(|k|(k.maps.len(),k.allocations,k.copies));
            a.clone_process_vm_private_cow(1,2).unwrap();
            let eager=if pages>=128 {16}else{pages};
            assert_eq!(KERNEL.with_borrow(|k|(k.maps.len()-before.0,k.allocations,k.copies)),(eager,before.1,before.2));
            for page in 0..pages {
                let va=base+page*PAGE_SIZE;let vm=a.processes.vm_space(2).unwrap();
                assert_eq!(vm.permissions(va),rights);assert_eq!(vm.mapping_deferred(va),page>=eager);
                assert_eq!(vm.hardware_permissions(va),if page<eager {rights}else{0});
                assert_eq!(a.physical(1,va),a.physical(2,va));assert_eq!(a.byte(2,va),page as u8);
                let reference=a.processes.find_active_physical_allocation_reference(2,va,1).unwrap();
                assert!(!reference.owns_frame_cap);assert_eq!(a.processes.physical_reference_count(reference),2);
            }
            let page=pages-1;let va=base+page*PAGE_SIZE;let required=if rights&4!=0 {4}else{1};
            assert_eq!(a.handle_populate_snapshot(OsRequestEvent{identifier:4,arg0:2,arg1:va,arg2:required,arg3:0}),Ok(usize::from(pages>=128)));
            assert_eq!(a.processes.vm_space(2).unwrap().hardware_permissions(va),rights);
            assert_eq!(a.handle_populate_snapshot(OsRequestEvent{identifier:4,arg0:2,arg1:va,arg2:2,arg3:0}),Ok(0));
            assert_eq!(a.byte(2,va),page as u8);
            let mapped=KERNEL.with_borrow(|k|k.maps.iter().filter(|&&(space,_,_)|space==502).count());
            assert!(mapped<=eager+512 && mapped<=pages);
            println!("FORK-READONLY pages={pages} rights={rights} initial-maps={eager} maps-after-tail-fault={mapped}");
        }
    }
}

#[test]
fn deferred_fork_child_keeps_old_bytes_when_parent_detaches_before_first_access() {
    let mut a=Alpha::new();let base=0x1000000;
    for page in 0..128 {a.page(1,base+page*PAGE_SIZE,0x58,5);}
    a.clone_process_vm_private_cow(1,2).unwrap();
    let va=base+40*PAGE_SIZE;assert!(a.processes.vm_space(2).unwrap().mapping_deferred(va));
    a.prepare(1,va,PAGE_SIZE,nanami_uapi::os::OS_PRIVATE_WRITE_DETACH).unwrap();
    let physical=a.physical(1,va);KERNEL.with_borrow_mut(|k|k.physical.get_mut(&physical).unwrap()[0]=0xaa);
    assert_eq!(a.byte(1,va),0xaa);assert_eq!(a.byte(2,va),0x58);
    assert_eq!(a.handle_populate_snapshot(OsRequestEvent{identifier:4,arg0:2,arg1:va,arg2:4,arg3:0}),Ok(1));
    assert_eq!(a.byte(2,va),0x58);assert_eq!(a.processes.vm_space(2).unwrap().hardware_permissions(va),5);
}

#[test]
fn detached_child_page_does_not_break_population_of_neighboring_fork_backing() {
    let mut a=Alpha::new();let base=0x1000000;
    for page in 0..128 {a.page(1,base+page*PAGE_SIZE,0x59,1);}
    a.clone_process_vm_private_cow(1,2).unwrap();
    let va=base+64*PAGE_SIZE;
    a.prepare(2,va,PAGE_SIZE,nanami_uapi::os::OS_PRIVATE_WRITE_DETACH).unwrap();
    let physical=a.physical(2,va);KERNEL.with_borrow_mut(|k|k.physical.get_mut(&physical).unwrap()[0]=0xab);
    let neighbor=base+80*PAGE_SIZE;
    assert_eq!(a.handle_populate_snapshot(OsRequestEvent{identifier:4,arg0:2,arg1:neighbor,arg2:1,arg3:0}),Ok(1));
    assert_eq!(a.byte(2,va),0xab);assert_eq!(a.byte(1,va),0x59);assert_eq!(a.byte(2,neighbor),0x59);
}

#[test]
fn failed_deferred_fork_population_keeps_uncertain_child_and_parent_references() {
    for after in [false,true] {
        let mut a=Alpha::new();let base=0x1000000;
        for page in 0..128 {a.page(1,base+page*PAGE_SIZE,0x63,1);}
        a.clone_process_vm_private_cow(1,2).unwrap();
        KERNEL.with_borrow_mut(|k|{k.fail_map=1;k.fail_map_after_install=after;});
        let va=base+16*PAGE_SIZE;
        assert_eq!(a.handle_populate_snapshot(OsRequestEvent{identifier:4,arg0:2,arg1:va,arg2:1,arg3:0}),Err(CapabilityError::Fatal));
        let vm=a.processes.vm_space(2).unwrap();assert!(vm.mapping_is_uncertain(va));assert!(vm.frame_may_be_present(va));
        let reference=a.processes.find_active_physical_allocation_reference(2,va,1).unwrap();
        assert_eq!(a.processes.physical_reference_count(reference),2);assert_eq!(a.byte(1,va),0x63);
        assert_eq!(a.handle_populate_snapshot(OsRequestEvent{identifier:4,arg0:2,arg1:va,arg2:1,arg3:0}),Ok(0));
    }
}

#[test]
fn deferred_snapshot_maps_only_header_then_populates_a_bounded_cluster_with_original_bytes() {
    for pages in [64,775,858,1024] {
        let mut a=Alpha::new();let source=snapshot(&mut a,pages);let base=0x1000000;
        let before=KERNEL.with_borrow(|k|k.maps.len());
        a.map_private_snapshot_with_prefix(4,1,source,base,pages*PAGE_SIZE,1,None,Some(16*PAGE_SIZE)).unwrap();
        assert_eq!(KERNEL.with_borrow(|k|k.maps.len()),before+16);
        for i in 0..pages {let va=base+i*PAGE_SIZE;assert_eq!(a.byte(1,va),0x30u8.wrapping_add(i as u8));
            let vm=a.processes.vm_space(1).unwrap();assert_eq!(vm.permissions(va),1);assert_eq!(vm.mapping_deferred(va),i>=16);}
        a.processes.vm_space_mut(1).unwrap().set_permissions(base+33*PAGE_SIZE,0);
        assert_eq!(populate(&mut a,base+20*PAGE_SIZE,1),Ok(1));
        assert_eq!(KERNEL.with_borrow(|k|k.maps.len()),before+pages.min(512)-1);
        assert_eq!(populate(&mut a,base+20*PAGE_SIZE,1),Ok(1));
        assert_eq!(KERNEL.with_borrow(|k|k.maps.len()),before+pages.min(512)-1);
        assert_eq!(populate(&mut a,base+32*PAGE_SIZE,2),Ok(0));
        assert_eq!(populate(&mut a,base+32*PAGE_SIZE,1),Ok(1));
        assert!(!KERNEL.with_borrow(|k|k.ptes.contains_key(&(501,base+33*PAGE_SIZE))));
        assert_eq!(populate(&mut a,base+33*PAGE_SIZE,1),Ok(0));
        println!("DEFERRED-SNAPSHOT pages={pages} initial-maps=16 fault-cluster-max=512");
    }
}

#[test]
fn deferred_snapshot_words_share_only_the_requested_source_and_keep_page_references() {
    let mut a = Alpha::new();
    let pages = 146;
    let source = snapshot(&mut a, pages + 6);
    let base = 0x1001000;
    let source_offset = 3;
    let eager = 16;
    let before = KERNEL.with_borrow(|k| k.maps.len());
    a.map_private_snapshot_with_prefix(4, 1, source + source_offset * PAGE_SIZE,
        base, pages * PAGE_SIZE, 1, None, Some(eager * PAGE_SIZE)).unwrap();
    assert_eq!(KERNEL.with_borrow(|k| k.maps.len()) - before, eager);
    let source_vm = a.processes.vm_space(4).unwrap();
    for page in 0..pages + 6 {
        let address = source + page * PAGE_SIZE;
        assert_eq!(source_vm.private_page_shared(address), (source_offset..source_offset + pages).contains(&page));
    }
    for page in 0..pages {
        let address = base + page * PAGE_SIZE;
        let vm = a.processes.vm_space(1).unwrap();
        assert!(vm.private_page_shared(address));
        assert_eq!(vm.mapping_deferred(address), page >= eager);
        assert_eq!(vm.hardware_permissions(address), usize::from(page < eager));
        let allocation = a.processes.find_active_physical_allocation_reference(1, address, 1).unwrap();
        assert_eq!(a.processes.physical_reference_count(allocation), 2);
        assert_eq!(a.byte(1, address), 0x30u8.wrapping_add((page + source_offset) as u8));
    }
}

#[test]
fn deferred_snapshot_fork_and_detach_preserve_absent_parent_and_private_bytes() {
    let mut a=Alpha::new();let source=snapshot(&mut a,64);let base=0x1000000;
    a.map_private_snapshot_with_prefix(4,1,source,base,64*PAGE_SIZE,1,None,Some(16*PAGE_SIZE)).unwrap();
    a.clone_process_vm_shared_readonly(1,2).unwrap();
    let va=base+40*PAGE_SIZE;let before=a.physical(1,va);
    assert!(a.processes.vm_space(1).unwrap().mapping_deferred(va));
    assert_eq!(a.byte(2,va),0x58);
    a.prepare(1,va,PAGE_SIZE,1).unwrap();
    assert_ne!(a.physical(1,va),before);assert_eq!(a.byte(1,va),0x58);
    assert!(a.processes.vm_space(1).unwrap().mapping_deferred(va));
    assert!(!KERNEL.with_borrow(|k|k.ptes.contains_key(&(501,va))));
    assert_eq!(populate(&mut a,va,1),Ok(1));
    let physical=a.physical(1,va);KERNEL.with_borrow_mut(|k|k.physical.get_mut(&physical).unwrap()[0]=0xaa);
    assert_eq!(a.byte(2,va),0x58);assert_eq!(a.byte(4,source+40*PAGE_SIZE),0x58);
}
#[test]
fn deferred_snapshot_population_rejects_stale_frames_and_keeps_failed_map_owned() {
    for after in [false,true] {
        let mut a=Alpha::new();let source=snapshot(&mut a,64);let base=0x1000000;
        a.map_private_snapshot_with_prefix(4,1,source,base,64*PAGE_SIZE,1,None,Some(16*PAGE_SIZE)).unwrap();
        KERNEL.with_borrow_mut(|k|{k.fail_map=1;k.fail_map_after_install=after;});
        assert_eq!(populate(&mut a,base+16*PAGE_SIZE,1),Err(CapabilityError::Fatal));
        let vm=a.processes.vm_space(1).unwrap();assert!(vm.mapping_is_uncertain(base+16*PAGE_SIZE));
        assert!(vm.frame_may_be_present(base+16*PAGE_SIZE));assert!(vm.find_frame(base+16*PAGE_SIZE).is_some());
        assert_eq!(populate(&mut a,base+16*PAGE_SIZE,1),Ok(0));
        assert!(a.processes.find_active_physical_allocation_reference(1,base+16*PAGE_SIZE,1).is_some());
    }
    for populated in [false,true] {
        let mut a=Alpha::new();let source=snapshot(&mut a,64);let base=0x1000000;
        a.map_private_snapshot_with_prefix(4,1,source,base,64*PAGE_SIZE,1,None,Some(16*PAGE_SIZE)).unwrap();
        let va=base+16*PAGE_SIZE;if populated {populate(&mut a,va,1).unwrap();}
        a.processes.vm_space_mut(1).unwrap().record_frame(va,999).unwrap();
        assert_eq!(populate(&mut a,va,1),Err(CapabilityError::PermissionDenied));
    }
}

#[test]
fn deferred_unique_page_after_native_source_retirement_detaches_without_mapping_or_copying() {
    let mut a=Alpha::new();let source=snapshot(&mut a,2);let base=0x1000000;
    a.map_private_snapshot_with_prefix(4,1,source,base,2*PAGE_SIZE,1,None,Some(PAGE_SIZE)).unwrap();
    for offset in 0..2 {
        let va=source+offset*PAGE_SIZE;
        let(allocation,last)=a.processes.release_physical_allocation_reference(4,va,1).unwrap();assert!(!last);
        let frame=process_frame_descriptor(304,allocation.start_slot);
        arch::address_space::unmap(504,frame,va).unwrap();
        arch::node::remove(process_frame_chunk_descriptor(304,allocation.start_slot/512),allocation.start_slot%512).unwrap();
        a.processes.vm_space_mut(4).unwrap().forget_frame(va);
    }
    let va=base+PAGE_SIZE;let physical=a.physical(1,va);
    let before=KERNEL.with_borrow(|k|(k.maps.len(),k.unmaps.len(),k.allocations,k.copies));
    a.prepare(1,va,PAGE_SIZE,1).unwrap();
    assert_eq!(a.physical(1,va),physical);assert_eq!(a.byte(1,va),0x31);
    assert_eq!(KERNEL.with_borrow(|k|(k.maps.len(),k.unmaps.len(),k.allocations,k.copies)),before);
    let vm=a.processes.vm_space(1).unwrap();assert!(vm.mapping_deferred(va));assert!(!vm.private_page_shared(va));
    assert_eq!(populate(&mut a,va,1),Ok(1));
    assert_eq!(a.byte(1,va),0x31);
}
