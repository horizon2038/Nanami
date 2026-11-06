#![allow(dead_code, private_interfaces)]
extern crate alloc;
extern crate self as nun;
use std::{cell::RefCell, collections::BTreeMap, ptr};
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
const PAGE_SIZE: usize = 4096;
const PROCESS_FRAME_CHUNK_PAGES: usize = 512;
const PROCESS_FRAME_NODE_RADIX: usize = 9;
const PROCESS_FRAME_TOTAL_PAGES: usize = 524288;
fn align_down(n: usize, alignment: usize) -> usize {
    n & !(alignment - 1)
}
fn process_frame_descriptor(root: usize, slot: usize) -> usize {
    root * 1000000 + slot
}
fn process_frame_chunk_descriptor(root: usize, chunk: usize) -> usize {
    root * 4096 + chunk
}
#[derive(Clone, Copy)]
enum UnmapFailure {
    Before,
    After,
}
#[derive(Default)]
struct Kernel {
    ptes: BTreeMap<(usize, usize), (usize, usize)>,
    unmaps: usize,
    fail_unmap: Option<UnmapFailure>,
}
thread_local! { static KERNEL: RefCell<Kernel> = RefCell::new(Kernel::default()); }
mod arch {
    pub mod node {
        pub fn copy(_: usize, _: usize, _: usize) -> Result<(), crate::CapabilityError> {
            Ok(())
        }
    }
    pub mod address_space {
        use super::super::*;
        pub fn map(
            space: usize,
            frame: usize,
            va: usize,
            attribute: capability_call::address_space::Attribute,
        ) -> Result<(), CapabilityError> {
            KERNEL.with_borrow_mut(|kernel| {
                if kernel.ptes.contains_key(&(space, va)) {
                    return Err(CapabilityError::IllegalOperation);
                }
                kernel.ptes.insert((space, va), (frame, attribute.bits()));
                Ok(())
            })
        }
        pub fn unmap(space: usize, frame: usize, va: usize) -> Result<(), CapabilityError> {
            KERNEL.with_borrow_mut(|kernel| {
                kernel.unmaps += 1;
                if matches!(kernel.fail_unmap, Some(UnmapFailure::Before)) {
                    return Err(CapabilityError::Fatal);
                }
                if let Some(&(mapped_frame, _)) = kernel.ptes.get(&(space, va)) {
                    assert_eq!(mapped_frame, frame);
                }
                // Current HALs remove an already empty leaf idempotently;
                // a later shootdown error still requires ownership retention.
                kernel.ptes.remove(&(space, va));
                if matches!(kernel.fail_unmap, Some(UnmapFailure::After)) {
                    return Err(CapabilityError::Fatal);
                }
                Ok(())
            })
        }
    }
}
struct OsRequestEvent {
    identifier: usize,
    code: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
}
#[derive(Default)]
struct Memory {
    pages: BTreeMap<usize, Box<[u8; PAGE_SIZE]>>,
    frames: BTreeMap<usize, usize>,
    next: usize,
    initializing: usize,
    published: Vec<(usize, Vec<u8>)>,
    fail_allocate: Option<usize>,
    fail_map: Option<usize>,
    fail_after_map: bool,
    fail_frame_record: bool,
}
impl Memory {
    fn allocate_process_frame(
        &mut self,
        _: usize,
        _: usize,
        _: usize,
    ) -> Result<usize, CapabilityError> {
        if self.fail_allocate == Some(self.next) {
            return Err(CapabilityError::Fatal);
        }
        self.next += 1;
        self.pages.insert(self.next, Box::new([0xcc; PAGE_SIZE]));
        Ok(self.next)
    }
    fn ensure_alpha_frame_at_physical_index(&mut self, page: usize) -> Result<(), CapabilityError> {
        assert!(self.pages.contains_key(&page));
        Ok(())
    }
    fn physical_frame_descriptor_from_index(&self, page: usize) -> Option<usize> {
        self.pages.contains_key(&page).then_some(page + 100000)
    }
    fn map_frame_strict(
        &mut self,
        space: usize,
        frame: usize,
        va: usize,
        vm: &mut vm_space::VmSpace,
    ) -> Result<(), CapabilityError> {
        let fails = self.fail_map == Some(self.published.len());
        if fails && !self.fail_after_map {
            return Err(CapabilityError::Fatal);
        }
        let page = self.initializing;
        self.frames.insert(frame, page);
        arch::address_space::map(
            space,
            frame,
            va,
            capability_call::address_space::Attribute::ALL,
        )?;
        self.published.push((va, self.pages[&page].to_vec()));
        if fails || self.fail_frame_record {
            return Err(CapabilityError::Fatal);
        }
        vm.record_frame(va, frame).unwrap();
        Ok(())
    }
}
struct Alpha {
    memory: Memory,
    processes: ProcessManager,
    rollbacks: usize,
}
impl Alpha {
    fn new() -> Self {
        KERNEL.with(|kernel| *kernel.borrow_mut() = Kernel::default());
        let mut processes = ProcessManager::new_alpha(1, 2, 99, 3, 4096, &[], 4).unwrap();
        for pid in [1, 2] {
            processes
                .install_process(
                    pid,
                    3,
                    200 + pid,
                    100 + pid,
                    11 + pid,
                    21 + pid,
                    0,
                    pid,
                    0,
                    0,
                    0x100000,
                    0x1000000,
                )
                .unwrap();
            processes.ensure_vm_space_for_pid(pid).unwrap();
        }
        Self {
            memory: Memory::default(),
            processes,
            rollbacks: 0,
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
    fn process_frame_for_page(&mut self, pid: usize, va: usize) -> Result<usize, CapabilityError> {
        self.processes
            .vm_space_mut(pid)
            .unwrap()
            .find_frame(va)
            .ok_or(CapabilityError::InvalidArgument)
    }
    fn map_initialization_page(&mut self, page: usize) -> Result<usize, CapabilityError> {
        self.memory.initializing = page;
        Ok(self.memory.pages.get_mut(&page).unwrap().as_mut_ptr() as usize)
    }
    fn map_process_copy_frame(
        &mut self,
        _: usize,
        frame: usize,
        _: Option<usize>,
    ) -> Result<usize, CapabilityError> {
        Ok(self.memory.pages[&self.memory.frames[&frame]].as_ptr() as usize)
    }
    fn source(&mut self, bytes: usize) -> memory_pages::PageSource {
        for i in 0..bytes.div_ceil(PAGE_SIZE) {
            let page = self.memory.allocate_process_frame(0, 0, 0).unwrap();
            for (index, byte) in self
                .memory
                .pages
                .get_mut(&page)
                .unwrap()
                .iter_mut()
                .enumerate()
            {
                *byte = ((i * PAGE_SIZE + index) % 239) as u8;
            }
            let frame = page + 100000;
            self.memory.frames.insert(frame, page);
            self.processes
                .vm_space_mut(1)
                .unwrap()
                .record_frame(0x10000 + i * PAGE_SIZE, frame)
                .unwrap();
        }
        memory_pages::PageSource {
            pid: 1,
            address: 0x10000,
            bytes,
        }
    }
    fn invalidate_process_copy_mappings(&mut self, _: usize) -> Result<(), CapabilityError> {
        self.rollbacks += 1;
        Ok(())
    }
    fn release_shared_memory(
        &mut self,
        _: nanami_core::process::ProcessEntry,
        _: nanami_core::process::SharedMemoryReservation,
    ) -> Result<(), CapabilityError> {
        panic!("private constructor tests do not release shared objects");
    }
}
#[path = "../../nanami/src/nanami_core/alpha/mapping_protection.rs"]
mod mapping_protection;
#[path = "../../nanami/src/nanami_core/alpha/mapping_release.rs"]
mod mapping_release;
#[path = "../../nanami/src/nanami_core/alpha/memory_pages.rs"]
mod memory_pages;

#[test]
fn constructor_map_error_cannot_release_a_live_leaf_hidden_by_old_prot_none() {
    for failure in [UnmapFailure::Before, UnmapFailure::After] {
        let mut a = Alpha::new();
        a.processes
            .vm_space_mut(2)
            .unwrap()
            .set_permissions(0x80000, 0);
        a.memory.fail_map = Some(0);
        a.memory.fail_after_map = true;
        KERNEL.with_borrow_mut(|kernel| kernel.fail_unmap = Some(failure));
        assert!(a.map_process_heap_pages_at(2, 0x80000, 1).is_err());
        assert!(a
            .processes
            .find_active_physical_allocation_reference(2, 0x80000, 1)
            .is_some());
        assert!(a
            .processes
            .vm_space_mut(2)
            .unwrap()
            .mapping_is_uncertain(0x80000));
        assert!(a
            .processes
            .releasable_deferred_physical_allocations_for_pid(2)
            .is_empty());
        KERNEL.with_borrow_mut(|kernel| kernel.fail_unmap = None);
        a.handle_mapping_release_request(OsRequestEvent {
            identifier: 2,
            code: 0,
            arg0: 0x80000,
            arg1: PAGE_SIZE,
            arg2: 2,
            arg3: nanami_uapi::os::OS_MAPPING_RELEASE_ALLOW_HOLES,
        })
        .unwrap();
        assert!(!a
            .processes
            .vm_space_mut(2)
            .unwrap()
            .mapping_is_uncertain(0x80000));
        assert!(a
            .processes
            .find_active_physical_allocation_reference(2, 0x80000, 1)
            .is_none());
        KERNEL.with_borrow(|kernel| assert!(kernel.ptes.is_empty()));
    }
}

#[test]
fn legacy_constructor_records_actual_default_access_before_reservation_protection() {
    let mut a = Alpha::new();
    a.processes
        .vm_space_mut(2)
        .unwrap()
        .set_permissions(0x80000, 0);
    a.map_process_heap_pages_at(2, 0x80000, 1).unwrap();
    assert_eq!(a.processes.vm_space_mut(2).unwrap().permissions(0x80000), 7);
    let space = a.processes.memory_entry(2).unwrap().address_space;
    let frame = a.process_frame_for_page(2, 0x80000).unwrap();
    a.protect_process_page(2, space, 0x80000, frame, 0).unwrap();
    KERNEL.with_borrow(|kernel| {
        assert_eq!(kernel.unmaps, 1);
        assert!(kernel.ptes.is_empty());
    });
}

#[test]
fn pending_destination_without_frame_metadata_cannot_be_reallocated() {
    let mut a = Alpha::new();
    a.processes
        .vm_space_mut(2)
        .unwrap()
        .mark_mapping_uncertain(0x80000);
    assert_eq!(
        a.map_process_heap_pages_at(2, 0x80000, 1),
        Err(CapabilityError::Fatal)
    );
    assert_eq!(a.memory.next, 0);
}

#[test]
fn post_map_frame_record_error_cannot_grant_checked_access_or_retire_backing() {
    let mut a = Alpha::new();
    a.memory.fail_frame_record = true;
    KERNEL.with_borrow_mut(|kernel| kernel.fail_unmap = Some(UnmapFailure::Before));
    assert!(a.map_process_heap_pages_at(2, 0x80000, 1).is_err());
    assert_eq!(a.processes.vm_space_mut(2).unwrap().permissions(0x80000), 0);
    assert!(a
        .processes
        .find_active_physical_allocation_reference(2, 0x80000, 1)
        .is_some());
    assert!(a
        .processes
        .releasable_deferred_physical_allocations_for_pid(2)
        .is_empty());
}
#[test]
fn copies_initialized_prefix_and_only_zeroes_the_suffix_before_publication() {
    for bytes in [0, 1, 4095, 4096, 4097, 8192] {
        let mut a = Alpha::new();
        let source = a.source(bytes);
        a.map_process_initialized_pages(2, Some(0x80000), 2, Some(source))
            .unwrap();
        let actual: Vec<u8> = a
            .memory
            .published
            .iter()
            .flat_map(|(_, page)| page.iter().copied())
            .collect();
        assert_eq!(
            &actual[..bytes],
            &(0..bytes).map(|i| (i % 239) as u8).collect::<Vec<_>>()
        );
        assert!(actual[bytes..].iter().all(|byte| *byte == 0));
        assert_eq!(a.memory.published.len(), 2);
        assert_eq!(a.rollbacks, 0);
    }
}
#[test]
fn source_offsets_can_cross_pages_without_copying_through_a_staging_buffer() {
    let mut a = Alpha::new();
    let mut source = a.source(8192);
    source.address += 4090;
    source.bytes = 4096;
    a.map_process_initialized_pages(2, Some(0x80000), 1, Some(source))
        .unwrap();
    let expected: Vec<u8> = (4090..8186).map(|i| (i % 239) as u8).collect();
    assert_eq!(a.memory.published[0].1, expected);
}
#[test]
fn zero_filled_anonymous_pages_and_reused_frames_do_not_expose_old_contents() {
    let mut a = Alpha::new();
    a.map_process_heap_pages_at(2, 0x80000, 1).unwrap();
    let allocation = a
        .processes
        .find_active_physical_allocation_reference(2, 0x80000, 1)
        .unwrap();
    a.handle_mapping_release_request(OsRequestEvent {
        identifier: 2,
        code: 0,
        arg0: 0x80000,
        arg1: 4096,
        arg2: 2,
        arg3: 0,
    })
    .unwrap();
    a.memory
        .pages
        .get_mut(&allocation.base_page)
        .unwrap()
        .fill(0x99);
    let old_next = a.memory.next;
    a.map_process_heap_pages_at(2, 0x80000, 1).unwrap();
    assert_eq!(a.memory.next, old_next);
    assert!(a
        .memory
        .published
        .last()
        .unwrap()
        .1
        .iter()
        .all(|byte| *byte == 0));
}
#[test]
fn bad_source_overflow_and_overlapping_destination_fail_before_allocation() {
    let mut a = Alpha::new();
    let source = a.source(4096);
    for bad in [
        memory_pages::PageSource {
            bytes: 8192,
            ..source
        },
        memory_pages::PageSource {
            address: usize::MAX,
            ..source
        },
        memory_pages::PageSource {
            address: 0x90000,
            ..source
        },
    ] {
        let old = a.memory.next;
        assert!(a
            .map_process_initialized_pages(2, Some(0x80000), 1, Some(bad))
            .is_err());
        assert_eq!(a.memory.next, old);
    }
    a.map_process_initialized_pages(2, Some(0x80000), 1, Some(source))
        .unwrap();
    let old = a.memory.next;
    assert!(a
        .map_process_initialized_pages(2, Some(0x80000), 1, None)
        .is_err());
    assert_eq!(a.memory.next, old);
}
#[test]
fn allocation_and_mapping_failures_leave_no_active_partial_extent() {
    for allocation_failure in [false, true] {
        let mut a = Alpha::new();
        let source = a.source(12288);
        if allocation_failure {
            a.memory.fail_allocate = Some(a.memory.next + 1);
        } else {
            a.memory.fail_map = Some(1);
        }
        assert!(a
            .map_process_initialized_pages(2, Some(0x80000), 3, Some(source))
            .is_err());
        assert_eq!(a.rollbacks, 1);
        assert!(a
            .processes
            .vm_space_mut(2)
            .unwrap()
            .backed_pages(0, usize::MAX)
            .is_empty());
        assert!(a
            .processes
            .releasable_physical_allocations_for_pid(2)
            .is_empty());
        assert!(!a
            .processes
            .releasable_deferred_physical_allocations_for_pid(2)
            .is_empty());
    }
}
