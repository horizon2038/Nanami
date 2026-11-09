//! Real staging reader and ownership bookkeeping, with frame mapping mocked.
use super::*;
use nanami_core::process::ProcessPhysicalAllocation;
use std::{
    cell::Cell,
    collections::BTreeMap,
    ops::{Deref, DerefMut},
    ptr,
    sync::{Mutex, MutexGuard},
};
const PAGE_SIZE: usize = 4096;
const PROCESS_SPAWN_MEMORY_MAX_BYTES: usize = 16 * PAGE_SIZE;
const PROCESS_COPY_TEMP_WINDOW_SIZE: usize = PROCESS_SPAWN_MEMORY_MAX_BYTES;
const PROCESS_COPY_TEMP_BASE: usize = 0x6400_0000;
static BUFFER_LOCK: Mutex<()> = Mutex::new(());
static mut PROCESS_MEMORY_IMAGE_BUFFER: [u8; PROCESS_SPAWN_MEMORY_MAX_BYTES] =
    [0; PROCESS_SPAWN_MEMORY_MAX_BYTES];
fn align_down(value: usize, alignment: usize) -> usize {
    value & !(alignment - 1)
}
fn min_usize(a: usize, b: usize) -> usize {
    a.min(b)
}
fn process_frame_descriptor(root: usize, slot: usize) -> usize {
    root * 100000 + slot
}
struct Processes {
    inner: ProcessManager,
    searches: Cell<usize>,
    snapshots: Cell<usize>,
}
impl Deref for Processes {
    type Target = ProcessManager;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl DerefMut for Processes {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
impl Processes {
    fn find_active_physical_allocation_reference(
        &self,
        pid: usize,
        va: usize,
        pages: usize,
    ) -> Option<ProcessPhysicalAllocation> {
        self.searches.set(self.searches.get() + 1);
        self.inner
            .find_active_physical_allocation_reference(pid, va, pages)
    }
    fn physical_pages_in_range(
        &self,
        pid: usize,
        base: usize,
        end: usize,
    ) -> Vec<ProcessPhysicalAllocation> {
        self.snapshots.set(self.snapshots.get() + 1);
        self.inner.physical_pages_in_range(pid, base, end)
    }
}
#[derive(Default)]
struct Memory {
    pages: BTreeMap<usize, Box<[u8; PAGE_SIZE]>>,
    foreign: Option<usize>,
}
impl Memory {
    fn physical_frame_descriptor_from_index(&self, page: usize) -> Option<usize> {
        if self.foreign == Some(page) {
            return None;
        }
        self.pages.get(&page).map(|_| page + 100000)
    }
}
struct Alpha {
    _guard: MutexGuard<'static, ()>,
    processes: Processes,
    memory: Memory,
    frames: BTreeMap<(usize, usize), usize>,
    frame_pages: BTreeMap<usize, usize>,
    replace_on_lookup: Option<(usize, usize, usize, u8)>,
    temporary_maps: usize,
    pool_maps: usize,
    cached_maps: usize,
    fail_cached_map: bool,
    ready_initialization_frames: std::collections::BTreeSet<usize>,
}
impl Alpha {
    fn new() -> Self {
        // A deliberately failing baseline assertion must not hide the other
        // independent regressions behind a poisoned test-only buffer lock.
        let guard = BUFFER_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut inner = manager();
        install(&mut inner, 1, 201);
        install(&mut inner, 2, 202);
        Self {
            _guard: guard,
            processes: Processes {
                inner,
                searches: Cell::new(0),
                snapshots: Cell::new(0),
            },
            memory: Memory::default(),
            frames: BTreeMap::new(),
            frame_pages: BTreeMap::new(),
            replace_on_lookup: None,
            temporary_maps: 0,
            pool_maps: 0,
            cached_maps: 0,
            fail_cached_map: false,
            ready_initialization_frames: Default::default(),
        }
    }
    fn page(&mut self, va: usize, slot: usize, page: usize, byte: u8) {
        let root = self.processes.memory_entry(1).unwrap().root_node;
        self.memory.pages.insert(page, Box::new([byte; PAGE_SIZE]));
        self.frames
            .insert((1, va), process_frame_descriptor(root, slot));
        self.frame_pages
            .insert(process_frame_descriptor(root, slot), page);
        self.processes
            .register_physical_allocation(1, va, slot, page, 1)
            .unwrap();
    }
    fn process_frame_for_page(&mut self, pid: usize, va: usize) -> Result<usize, CapabilityError> {
        if self
            .replace_on_lookup
            .is_some_and(|(base, _, _, _)| base == va)
        {
            let (base, slot, page, byte) = self.replace_on_lookup.take().unwrap();
            self.page(base, slot, page, byte);
        }
        let owner = self.processes.memory_owner_pid(pid);
        self.frames
            .get(&(owner, va))
            .copied()
            .ok_or(CapabilityError::InvalidArgument)
    }
    fn map_initialization_page(&mut self, page: usize) -> Result<usize, CapabilityError> {
        self.pool_maps += 1;
        self.memory
            .pages
            .get_mut(&page)
            .map(|page| page.as_mut_ptr() as usize)
            .ok_or(CapabilityError::InvalidArgument)
    }
    fn map_process_copy_frame(
        &mut self,
        _: usize,
        frame: usize,
        _: Option<usize>,
    ) -> Result<usize, CapabilityError> {
        self.cached_maps += 1;
        if self.fail_cached_map {
            return Err(CapabilityError::Fatal);
        }
        let page = self
            .frame_pages
            .get(&frame)
            .ok_or(CapabilityError::InvalidArgument)?;
        Ok(self.memory.pages[page].as_ptr() as usize)
    }
    fn cached_initialization_frame(&self, frame: usize) -> Option<usize> {
        if !self.ready_initialization_frames.contains(&frame) { return None; }
        let page=self.frame_pages.get(&frame)?;
        Some(self.memory.pages.get(page)?.as_ptr() as usize)
    }
    fn ready_pool_page(&mut self, va: usize, slot: usize, physical: usize, byte: u8) {
        self.page(va,slot,physical,byte);
        let frame=self.memory.physical_frame_descriptor_from_index(physical).unwrap();
        self.frames.insert((1,va),frame);
        self.frame_pages.insert(frame,physical);
        self.ready_initialization_frames.insert(frame);
    }
    fn map_alpha_temporary_frame(&mut self, _: usize, _: usize) -> Result<(), CapabilityError> {
        self.temporary_maps += 1;
        // An injected mapping error checks that foreign pages still use this
        // path without dereferencing an artificial fixed VA in the host test.
        Err(CapabilityError::Fatal)
    }
    fn unmap_alpha_temporary_frame(&mut self, _: usize, _: usize) -> Result<(), CapabilityError> {
        Ok(())
    }
}
#[allow(unused_variables)]
#[path = "../../nanami/src/nanami_core/alpha/memory_staging.rs"]
mod production;

#[test]
fn bulk_copy_uses_current_frame_aliases_without_any_ownership_walk() {
    let mut alpha = Alpha::new();
    for index in 0..8 {
        alpha.page(
            0x1000 + index * PAGE_SIZE,
            index,
            100 + index,
            b'A' + index as u8,
        );
    }
    for index in 0..4000 {
        alpha
            .processes
            .register_physical_allocation(
                1,
                0x1000000 + index * PAGE_SIZE,
                10000 + index,
                10000 + index,
                1,
            )
            .unwrap();
    }
    let bytes = alpha
        .read_process_memory_into_static_buffer(1, 0x1000, 8 * PAGE_SIZE)
        .unwrap();
    for (index, page) in bytes.chunks_exact(PAGE_SIZE).enumerate() {
        assert!(page.iter().all(|&byte| byte == b'A' + index as u8));
    }
    assert_eq!(alpha.processes.snapshots.get(), 0);
    assert_eq!(alpha.processes.searches.get(), 0);
    assert_eq!(
        (alpha.pool_maps, alpha.temporary_maps, alpha.cached_maps),
        (0, 0, 8)
    );
}

#[test]
fn unaligned_source_and_partial_tail_copy_exactly_the_requested_bytes() {
    let mut alpha = Alpha::new();
    for index in 0..3 {
        alpha.page(
            0x1000 + index * PAGE_SIZE,
            index,
            100 + index,
            b'A' + index as u8,
        );
    }
    let bytes = alpha
        .read_process_memory_into_static_buffer(1, 0x1000 + 17, 2 * PAGE_SIZE + 31)
        .unwrap();
    assert!(bytes[..PAGE_SIZE - 17].iter().all(|&byte| byte == b'A'));
    assert!(bytes[PAGE_SIZE - 17..2 * PAGE_SIZE - 17]
        .iter()
        .all(|&byte| byte == b'B'));
    assert!(bytes[2 * PAGE_SIZE - 17..].iter().all(|&byte| byte == b'C'));
    assert_eq!(alpha.processes.searches.get(), 0);
}

#[test]
fn small_initializers_use_current_frame_aliases_without_ownership_scans() {
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 0, 100, b'A');
    assert_eq!(
        alpha
            .read_process_memory_into_static_buffer(1, 0x1010, 16)
            .unwrap(),
        &[b'A'; 16]
    );
    assert_eq!(
        (
            alpha.processes.snapshots.get(),
            alpha.processes.searches.get()
        ),
        (0, 0)
    );
    assert_eq!(
        (alpha.pool_maps, alpha.temporary_maps, alpha.cached_maps),
        (0, 0, 1)
    );
}

#[test]
fn small_records_do_not_scan_thousands_of_newer_ownership_references() {
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 0, 100, b'A');
    for index in 0..4000 {
        alpha
            .processes
            .register_physical_allocation(
                1,
                0x1000000 + index * PAGE_SIZE,
                10000 + index,
                10000 + index,
                1,
            )
            .unwrap();
    }
    assert_eq!(
        alpha
            .read_process_memory_into_static_buffer(1, 0x1010, 16)
            .unwrap(),
        &[b'A'; 16]
    );
    assert_eq!(alpha.processes.searches.get(), 0);
    assert_eq!(alpha.processes.snapshots.get(), 0);
    assert_eq!(alpha.cached_maps, 1);
}

#[test]
fn small_cross_page_record_preserves_exact_prefix_and_failure_order() {
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 0, 100, b'A');
    alpha.page(0x2000, 1, 101, b'B');
    let bytes = alpha
        .read_process_memory_into_static_buffer(1, 0x1ff1, 32)
        .unwrap();
    assert_eq!(&bytes[..15], &[b'A'; 15]);
    assert_eq!(&bytes[15..], &[b'B'; 17]);
    assert_eq!(alpha.cached_maps, 2);
    assert_eq!(alpha.processes.searches.get(), 0);
    assert_eq!(
        alpha.read_process_memory_into_static_buffer(1, 0x2ff1, 32),
        Err(CapabilityError::InvalidArgument)
    );
    assert_eq!(alpha.cached_maps, 3);
}

#[test]
fn stale_ownership_metadata_cannot_override_a_small_records_current_frame() {
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 0, 100, b'A');
    // A foreign/shared mapping's current cap can differ from an old allocation
    // reference at the same VA. Only the current Frame is authoritative.
    let foreign_frame = 999999;
    alpha.frames.insert((1, 0x1000), foreign_frame);
    alpha.frame_pages.insert(foreign_frame, 999);
    alpha.memory.pages.insert(999, Box::new([b'B'; PAGE_SIZE]));
    assert_eq!(
        alpha
            .read_process_memory_into_static_buffer(1, 0x1010, 16)
            .unwrap(),
        &[b'B'; 16]
    );
    assert_eq!(
        (alpha.pool_maps, alpha.temporary_maps, alpha.cached_maps),
        (0, 0, 1)
    );
}

#[test]
fn small_shared_vm_records_use_the_current_owner_frame_after_materialization() {
    let mut alpha = Alpha::new();
    alpha.processes.share_address_space(2, 1).unwrap();
    alpha.replace_on_lookup = Some((0x1000, 9, 109, b'C'));
    assert_eq!(
        alpha
            .read_process_memory_into_static_buffer(2, 0x1010, 16)
            .unwrap(),
        &[b'C'; 16]
    );
    assert_eq!(alpha.processes.searches.get(), 0);
    assert_eq!(alpha.cached_maps, 1);
}

#[test]
fn small_record_mapping_failure_is_returned_without_a_fallback_scan() {
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 0, 100, b'A');
    alpha.fail_cached_map = true;
    assert_eq!(
        alpha.read_process_memory_into_static_buffer(1, 0x1010, 16),
        Err(CapabilityError::Fatal)
    );
    assert_eq!(alpha.processes.searches.get(), 0);
    assert_eq!(
        (alpha.pool_maps, alpha.temporary_maps, alpha.cached_maps),
        (0, 0, 1)
    );
}

#[test]
fn stale_bulk_ownership_reference_cannot_override_the_current_frame_alias() {
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 1, 101, b'A');
    alpha.page(0x2000, 2, 102, b'B');
    alpha.frames.insert((1, 0x1000), 999999);
    alpha.frame_pages.insert(999999, 999);
    alpha.memory.pages.insert(999, Box::new([b'C'; PAGE_SIZE]));
    let bytes = alpha
        .read_process_memory_into_static_buffer(1, 0x1000, 2 * PAGE_SIZE)
        .unwrap();
    assert!(bytes[..PAGE_SIZE].iter().all(|&byte| byte == b'C'));
    assert!(bytes[PAGE_SIZE..].iter().all(|&byte| byte == b'B'));
    assert_eq!(
        (alpha.pool_maps, alpha.temporary_maps, alpha.cached_maps),
        (0, 0, 2)
    );
}

#[test]
fn materialization_uses_the_fresh_current_frame_without_ownership_search() {
    let mut alpha = Alpha::new();
    alpha.page(0x2000, 2, 102, b'B');
    alpha.replace_on_lookup = Some((0x1000, 1, 101, b'A'));
    let bytes = alpha
        .read_process_memory_into_static_buffer(1, 0x1000, 2 * PAGE_SIZE)
        .unwrap();
    assert!(bytes[..PAGE_SIZE].iter().all(|&byte| byte == b'A'));
    assert!(bytes[PAGE_SIZE..].iter().all(|&byte| byte == b'B'));
    assert_eq!(alpha.processes.searches.get(), 0);
    assert_eq!(alpha.temporary_maps, 0);
}

#[test]
fn a_changed_frame_must_not_reuse_a_previous_physical_allocation() {
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 1, 101, b'X');
    alpha.page(0x2000, 2, 102, b'B');
    alpha.replace_on_lookup = Some((0x1000, 9, 109, b'A'));
    let bytes = alpha
        .read_process_memory_into_static_buffer(1, 0x1000, 2 * PAGE_SIZE)
        .unwrap();
    assert!(bytes[..PAGE_SIZE].iter().all(|&byte| byte == b'A'));
    assert!(bytes[PAGE_SIZE..].iter().all(|&byte| byte == b'B'));
    assert_eq!(alpha.processes.searches.get(), 0);
}

#[test]
fn shared_vm_sources_resolve_the_memory_owner_not_the_calling_thread_root() {
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 1, 101, b'A');
    alpha.page(0x2000, 2, 102, b'B');
    alpha.processes.share_address_space(2, 1).unwrap();
    let bytes = alpha
        .read_process_memory_into_static_buffer(2, 0x1000, 2 * PAGE_SIZE)
        .unwrap();
    assert!(bytes[..PAGE_SIZE].iter().all(|&byte| byte == b'A'));
    assert!(bytes[PAGE_SIZE..].iter().all(|&byte| byte == b'B'));
    assert_eq!(alpha.processes.searches.get(), 0);
}

#[test]
fn foreign_current_frames_need_no_alpha_pool_descriptor_and_propagate_cache_failure() {
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 1, 101, b'A');
    alpha.page(0x2000, 2, 102, b'B');
    alpha.memory.foreign = Some(101);
    assert_eq!(alpha.memory.physical_frame_descriptor_from_index(101), None);
    let bytes = alpha
        .read_process_memory_into_static_buffer(1, 0x1000, 2 * PAGE_SIZE)
        .unwrap();
    assert!(bytes[..PAGE_SIZE].iter().all(|&byte| byte == b'A'));
    assert_eq!(
        (alpha.pool_maps, alpha.temporary_maps, alpha.cached_maps),
        (0, 0, 2)
    );
    alpha.fail_cached_map = true;
    assert_eq!(
        alpha.read_process_memory_into_static_buffer(1, 0x1000, 2 * PAGE_SIZE),
        Err(CapabilityError::Fatal)
    );
}

#[test]
fn empty_invalid_missing_and_overflowing_sources_preserve_failure_boundaries() {
    let mut alpha = Alpha::new();
    assert!(alpha
        .read_process_memory_into_static_buffer(999, usize::MAX, 0)
        .unwrap()
        .is_empty());
    assert_eq!(
        alpha.read_process_memory_into_static_buffer(1, 0x1000, PROCESS_SPAWN_MEMORY_MAX_BYTES + 1),
        Err(CapabilityError::InvalidArgument)
    );
    assert_eq!(
        alpha.read_process_memory_into_static_buffer(1, usize::MAX - 100, PAGE_SIZE + 1),
        Err(CapabilityError::InvalidArgument)
    );
    alpha.page(0x1000, 1, 101, b'A');
    assert_eq!(
        alpha.read_process_memory_into_static_buffer(1, 0x1000, 2 * PAGE_SIZE),
        Err(CapabilityError::InvalidArgument)
    );
    assert_eq!(alpha.pool_maps, 0);
    assert_eq!(alpha.cached_maps, 1);
}

#[test]
fn multi_page_owned_extents_do_not_require_per_page_allocation_records() {
    let mut alpha = Alpha::new();
    for index in 0..4 {
        let va = 0x1000 + index * PAGE_SIZE;
        alpha.page(va, index, 100 + index, b'A' + index as u8);
        alpha
            .processes
            .release_physical_allocation_reference(1, va, 1)
            .unwrap();
    }
    // request_memory_create registers one shared/owned extent, not a separate
    // page_count=1 record for every Frame. The Frame tracker still has all pages.
    alpha
        .processes
        .register_physical_allocation(1, 0x1000, 0, 100, 4)
        .unwrap();
    assert!(alpha
        .processes
        .inner
        .find_active_physical_allocation_reference(1, 0x1000, 4)
        .is_some());
    assert!(alpha
        .processes
        .inner
        .find_active_physical_allocation_reference(1, 0x1000, 1)
        .is_none());
    let bytes = alpha
        .read_process_memory_into_static_buffer(1, 0x1000, 4 * PAGE_SIZE)
        .unwrap();
    for (index, page) in bytes.chunks_exact(PAGE_SIZE).enumerate() {
        assert!(page.iter().all(|&byte| byte == b'A' + index as u8));
    }
    assert_eq!(
        (
            alpha.processes.searches.get(),
            alpha.processes.snapshots.get()
        ),
        (0, 0)
    );
    assert_eq!(
        (alpha.pool_maps, alpha.temporary_maps, alpha.cached_maps),
        (0, 0, 4)
    );
}

#[test]
fn repeated_same_frame_reads_observe_changed_bytes_not_cached_contents() {
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 0, 100, b'A');
    assert_eq!(
        alpha
            .read_process_memory_into_static_buffer(1, 0x1010, 16)
            .unwrap(),
        &[b'A'; 16]
    );
    alpha.memory.pages.get_mut(&100).unwrap().fill(b'B');
    assert_eq!(
        alpha
            .read_process_memory_into_static_buffer(1, 0x1010, 16)
            .unwrap(),
        &[b'B'; 16]
    );
    assert_eq!(alpha.cached_maps, 2);
}

#[test]
fn ready_pool_sources_copy_fresh_bytes_and_unaligned_edges_without_a_second_window() {
    let mut alpha=Alpha::new();
    for i in 0..4 {alpha.ready_pool_page(0x1000+i*PAGE_SIZE,i,100+i,b'A'+i as u8);}
    alpha.fail_cached_map=true;
    let bytes=alpha.read_process_memory_into_static_buffer(1,0x1011,3*PAGE_SIZE+31).unwrap();
    assert!(bytes[..PAGE_SIZE-17].iter().all(|b|*b==b'A'));
    assert!(bytes[PAGE_SIZE-17..2*PAGE_SIZE-17].iter().all(|b|*b==b'B'));
    assert!(bytes[2*PAGE_SIZE-17..3*PAGE_SIZE-17].iter().all(|b|*b==b'C'));
    assert!(bytes[3*PAGE_SIZE-17..].iter().all(|b|*b==b'D'));
    alpha.memory.pages.get_mut(&100).unwrap().fill(b'Z');
    assert_eq!(alpha.read_process_memory_into_static_buffer(1,0x1010,16).unwrap(),&[b'Z';16]);
    assert_eq!((alpha.pool_maps,alpha.temporary_maps,alpha.cached_maps),(0,0,0));
    assert_eq!((alpha.processes.searches.get(),alpha.processes.snapshots.get()),(0,0));
}

#[test]
fn stale_ready_source_cannot_override_the_frame_resolved_during_this_read() {
    let mut alpha=Alpha::new();alpha.ready_pool_page(0x1000,0,100,b'A');
    alpha.replace_on_lookup=Some((0x1000,9,109,b'C'));
    assert_eq!(alpha.read_process_memory_into_static_buffer(1,0x1010,16).unwrap(),&[b'C';16]);
    assert_eq!(alpha.cached_maps,1);
    alpha.fail_cached_map=true;
    assert_eq!(alpha.read_process_memory_into_static_buffer(1,0x1010,16),Err(CapabilityError::Fatal));
    assert_eq!(alpha.cached_maps,2);
}

#[test]
fn ready_prefix_then_copy_window_failure_keeps_the_exact_committed_prefix() {
    let mut alpha=Alpha::new();alpha.ready_pool_page(0x1000,0,100,b'A');alpha.page(0x2000,1,101,b'B');
    unsafe {ptr::write_bytes(core::ptr::addr_of_mut!(PROCESS_MEMORY_IMAGE_BUFFER) as *mut u8,0xee,PROCESS_SPAWN_MEMORY_MAX_BYTES);}
    alpha.fail_cached_map=true;
    assert_eq!(alpha.read_process_memory_into_static_buffer(1,0x1000,2*PAGE_SIZE),Err(CapabilityError::Fatal));
    let output=unsafe {core::slice::from_raw_parts(core::ptr::addr_of!(PROCESS_MEMORY_IMAGE_BUFFER) as *const u8,2*PAGE_SIZE)};
    assert!(output[..PAGE_SIZE].iter().all(|b|*b==b'A'));
    assert!(output[PAGE_SIZE..].iter().all(|b|*b==0xee));
    assert_eq!(alpha.cached_maps,1);
    assert_eq!((alpha.processes.searches.get(),alpha.processes.snapshots.get()),(0,0));
}

#[test]
fn small_overflow_fails_before_resolving_or_mapping_a_page() {
    let mut alpha = Alpha::new();
    assert_eq!(
        alpha.read_process_memory_into_static_buffer(1, usize::MAX - 7, 16),
        Err(CapabilityError::InvalidArgument)
    );
    assert_eq!(
        (alpha.pool_maps, alpha.temporary_maps, alpha.cached_maps),
        (0, 0, 0)
    );
}
