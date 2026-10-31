#![allow(dead_code, private_interfaces)]
extern crate alloc;
extern crate self as nun;
use std::{collections::BTreeMap, ptr};
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
mod arch {
    pub mod node {
        pub fn copy(_: usize, _: usize, _: usize) -> Result<(), crate::CapabilityError> {
            Ok(())
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
        _: usize,
        frame: usize,
        va: usize,
        vm: &mut vm_space::VmSpace,
    ) -> Result<(), CapabilityError> {
        if self.fail_map == Some(self.published.len()) {
            return Err(CapabilityError::Fatal);
        }
        let page = self.initializing;
        self.frames.insert(frame, page);
        self.published.push((va, self.pages[&page].to_vec()));
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
    fn handle_mapping_release_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        self.rollbacks += 1;
        for va in (request.arg0..request.arg0 + request.arg1).step_by(PAGE_SIZE) {
            self.processes
                .vm_space_mut(request.arg2)
                .unwrap()
                .forget_frame(va);
            if self
                .processes
                .find_active_physical_allocation_reference(request.arg2, va, 1)
                .is_some()
            {
                let (allocation, _) =
                    self.processes
                        .release_physical_allocation_reference(request.arg2, va, 1)?;
                self.processes
                    .defer_physical_allocation_for_pid(request.arg2, allocation);
            }
        }
        Ok(())
    }
}
#[path = "../../nanami/src/nanami_core/alpha/memory_pages.rs"]
mod memory_pages;
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
