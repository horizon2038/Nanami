#![allow(dead_code, private_interfaces)]
extern crate alloc;
extern crate self as nun;
use std::{cell::RefCell, collections::BTreeMap, ptr};
pub type Word = usize;
pub type CapabilityDescriptor = usize;
pub use a9n_abi::{TLS_BASE_OFFSET, BYTE_BITS};
#[path = "../../nanami/src/nanami_core/elf_loader.rs"]
pub mod elf_loader;
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
macro_rules! force_info {
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
    pub use crate::elf_loader;
    pub mod memory {
        pub(crate) use crate::LargeMapping as PrivateLargeMapping;
    }
    pub mod process {
        include!(env!("NANAMI_PROCESS_SOURCE"));
    }
}
use nanami_core::process::ProcessManager;
use nanami_core::process::ProcessLazyMappingKind;
use elf_loader::page::fill_image_page;
const PROCESS_SPAWN_MEMORY_MAX_BYTES: usize = 64 * 1024 * 1024;
static mut PROCESS_MEMORY_IMAGE_BUFFER: [u8; PROCESS_SPAWN_MEMORY_MAX_BYTES] = [0; PROCESS_SPAWN_MEMORY_MAX_BYTES];
fn min_usize(a: usize, b: usize) -> usize { a.min(b) }
struct Processes {
    inner: ProcessManager,
    snapshots: std::cell::Cell<usize>,
}
impl core::ops::Deref for Processes {
    type Target=ProcessManager;
    fn deref(&self)->&ProcessManager {&self.inner}
}
impl core::ops::DerefMut for Processes {
    fn deref_mut(&mut self)->&mut ProcessManager {&mut self.inner}
}
impl Processes {
    fn try_physical_pages_in_range(&self,pid:usize,base:usize,end:usize,max:usize)
        ->Option<Vec<nanami_core::process::ProcessPhysicalAllocation>> {
        self.snapshots.set(self.snapshots.get()+1);
        self.inner.try_physical_pages_in_range(pid,base,end,max)
    }
}
use vm_space::VmTracker;
const PAGE_SIZE: usize = 4096;
const PROCESS_FRAME_CHUNK_PAGES: usize = 512;
const PROCESS_FRAME_NODE_RADIX: usize = 9;
const PROCESS_FRAME_TOTAL_PAGES: usize = 524288;
const USER_STACK_BASE: usize = 0x04000000;
const USER_STACK_PAGES: usize = 64;
const USER_ANONYMOUS_MAP_BASE: usize = 0x1000_0000;
const SLICE_KEY: usize = 1 << (usize::BITS - 1);
fn align_up_checked(n: usize, alignment: usize) -> Option<usize> {
    n.checked_add(alignment - 1).map(|n|align_down(n, alignment))
}
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
    map_attempts: usize,
    fail_map_once: Option<(usize, bool)>,
    unmaps: usize,
    fail_unmap: Option<UnmapFailure>,
    cap_copies: usize,
    cap_copy_operations: Vec<(usize, usize, usize)>,
    fail_cap_copy: Option<(usize, bool)>,
    chunk_calls: usize,
    huge: std::collections::BTreeSet<(usize, usize)>,
}
thread_local! { static KERNEL: RefCell<Kernel> = RefCell::new(Kernel::default()); }
mod arch {
    pub mod node {
        pub fn remove(_: usize, _: usize) -> Result<(), crate::CapabilityError> { Ok(()) }
        pub fn copy(node: usize, slot: usize, source: usize) -> Result<(), crate::CapabilityError> {
            super::super::KERNEL.with_borrow_mut(|kernel| {
                let fail = kernel.fail_cap_copy.filter(|(index, _)| *index == kernel.cap_copies);
                kernel.cap_copies += 1;
                if fail == Some((kernel.cap_copies - 1, false)) {
                    return Err(crate::CapabilityError::Fatal);
                }
                kernel.cap_copy_operations.push((node, slot, source));
                if fail.is_some() { Err(crate::CapabilityError::Fatal) } else { Ok(()) }
            })
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
            assert_eq!(frame & SLICE_KEY,0,"logical slice key must never reach the kernel");
            KERNEL.with_borrow_mut(|kernel| {
                kernel.map_attempts += 1;
                let fail = kernel.fail_map_once.filter(|(ordinal, _)| *ordinal == kernel.map_attempts);
                if fail.is_some() { kernel.fail_map_once = None; }
                if fail.is_some_and(|(_, after)| !after) { return Err(CapabilityError::Fatal); }
                if kernel.ptes.contains_key(&(space, va)) {
                    return Err(CapabilityError::IllegalOperation);
                }
                kernel.ptes.insert((space, va), (frame, attribute.bits()));
                if fail.is_some() { return Err(CapabilityError::Fatal); }
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
struct TestPages {
    single: BTreeMap<usize, Box<[u8; PAGE_SIZE]>>,
    large: BTreeMap<usize, TestLargeBytes>,
}
struct TestLargeBytes {
    bytes: Box<[[u8; PAGE_SIZE]]>,
    live: [bool; 512],
    remaining: usize,
}
impl TestPages {
    fn large_offset(&self, page: usize) -> Option<(usize, usize)> {
        let (&first, span) = self.large.range(..=page).next_back()?;
        let offset = page - first;
        span.live.get(offset).copied()?.then_some((first, offset))
    }
    fn contains_key(&self, page: &usize) -> bool {
        self.get(page).is_some()
    }
    fn get(&self, page: &usize) -> Option<&[u8; PAGE_SIZE]> {
        if let Some((first, offset)) = self.large_offset(*page) {
            return Some(&self.large[&first].bytes[offset]);
        }
        self.single.get(page).map(Box::as_ref)
    }
    fn get_mut(&mut self, page: &usize) -> Option<&mut [u8; PAGE_SIZE]> {
        if let Some((first, offset)) = self.large_offset(*page) {
            return Some(&mut self.large.get_mut(&first).unwrap().bytes[offset]);
        }
        self.single.get_mut(page).map(Box::as_mut)
    }
    fn remove(&mut self, page: &usize) -> bool {
        if let Some((first, offset)) = self.large_offset(*page) {
            let span = self.large.get_mut(&first).unwrap();
            span.live[offset] = false;
            span.remaining -= 1;
            if span.remaining == 0 { self.large.remove(&first); }
            return true;
        }
        self.single.remove(page).is_some()
    }
    fn len(&self) -> usize {
        self.single.len() + self.large.values().map(|span| span.remaining).sum::<usize>()
    }
    fn is_empty(&self) -> bool { self.len() == 0 }
}
impl core::ops::Index<&usize> for TestPages {
    type Output = [u8; PAGE_SIZE];
    fn index(&self, page: &usize) -> &Self::Output { self.get(page).unwrap() }
}

#[derive(Default)]
struct Memory {
    pages: TestPages,
    frames: BTreeMap<usize, usize>,
    next: usize,
    initializing: usize,
    published: Vec<(usize, Vec<u8>)>,
    fail_allocate: Option<usize>,
    fail_map: Option<usize>,
    fail_after_map: bool,
    fail_frame_record: bool,
    child_allocations: usize,
    fail_ensure: Option<usize>,
    allocation_requests: Vec<usize>,
    fragmented: bool,
    freed_pages: usize,
    fail_free: bool,
    large_enabled: bool,
    large: BTreeMap<usize, LargeRecord>,
    fail_split: Option<SplitFailure>,
    fail_restore: bool,
    existing_small_table: bool,
    blocked_large_bases: std::collections::BTreeSet<usize>,
    table_preparations: Vec<usize>,
    demotions: usize,
    large_alias_maps: usize,
}
#[derive(Clone,Copy,PartialEq,Eq)]
enum SplitFailure { Demote, Table }
#[derive(Clone,Copy)]
struct LargeRecord {
    huge: bool,
    mapping: Option<(usize,usize,usize)>,
    present: bool,
    quarantined: bool,
}
struct LargeAllocation { first: usize, large: bool }
pub(crate) struct LargeMapping { first:usize, base:usize, permissions:usize, small:bool, quarantined:bool }
impl Memory {
    fn prepare_reserved_large_frame(&mut self, first: usize) -> Result<Option<LargeAllocation>, CapabilityError> {
        if !self.large_enabled || first & 511 != 0 || !self.pages.large.contains_key(&first)
            || self.large.contains_key(&first) { return Ok(None); }
        self.large.insert(first, LargeRecord { huge: true, mapping: None, present: false, quarantined: false });
        self.update_large_keys(first, true);
        Ok(Some(LargeAllocation { first, large: true }))
    }
    fn map_private_large_alias(&mut self, frame: usize, _: Option<usize>)
        -> Result<Option<(usize, usize)>, CapabilityError> {
        if frame & SLICE_KEY == 0 { return Ok(None); }
        let page = self.frames[&frame];
        let (first, offset) = self.pages.large_offset(page).unwrap();
        assert!(self.large[&first].huge);
        self.large_alias_maps += 1;
        Ok(Some((self.pages.large.get_mut(&first).unwrap().bytes[offset..].as_mut_ptr() as usize, 512 - offset)))
    }
    fn ensure_alpha_frames_for_allocation(&mut self, first: usize, pages: usize) -> Result<(), CapabilityError> {
        for page in first..first + pages { self.ensure_alpha_frame_at_physical_index(page)?; }
        Ok(())
    }
    fn read_private_large_frames(&self, first: usize, frames: &mut [usize]) -> Result<(),CapabilityError> {
        for (index, frame) in frames.iter_mut().enumerate() {
            *frame = self.physical_frame_descriptor_from_index(first + index).ok_or(CapabilityError::Fatal)?;
        }
        Ok(())
    }
    fn read_ready_pool_frames(&self, first: usize, frames: &mut [usize]) -> Result<(), CapabilityError> {
        for (index, frame) in frames.iter_mut().enumerate() {
            let current = self.physical_frame_descriptor_from_index(first + index)
                .filter(|frame| frame & SLICE_KEY == 0)
                .ok_or(CapabilityError::InvalidArgument)?;
            *frame = current;
        }
        Ok(())
    }
    fn private_large_virtual_available(&self,_:usize,base:usize)->Result<bool,CapabilityError> {
        Ok(self.large_enabled && !self.existing_small_table && !self.blocked_large_bases.contains(&base))
    }
    // These are kernel/capability primitives. The production Alpha constructor,
    // ownership, split, rollback, protection, release and COW code run below.
    fn allocate_private_large_frame(&mut self) -> Result<Option<LargeAllocation>,CapabilityError> {
        if !self.large_enabled {return Ok(None);}
        self.next=align_up_checked(self.next+1,512).unwrap()-1;
        let first=self.allocate_test_pages(1<<21,true)?;
        self.large.insert(first,LargeRecord {huge:true,mapping:None,present:false,quarantined:false});
        self.update_large_keys(first,true);
        Ok(Some(LargeAllocation {first,large:true}))
    }
    fn update_large_keys(&mut self,first:usize,huge:bool) {
        for page in first..first+512 {
            self.frames.remove(&(page+100000));
            self.frames.remove(&((page+100000)|SLICE_KEY));
            self.frames.insert((page+100000)|if huge {SLICE_KEY} else {0},page);
        }
    }
    fn map_private_large_frame(&mut self,first:usize,space:usize,base:usize,permissions:usize) -> Result<(),CapabilityError> {
        self.large.get_mut(&first).unwrap().mapping=Some((space,base,permissions));
        if permissions == 0 { return Ok(()); }
        let attribute=capability_call::address_space::Attribute::from_bits(permissions).unwrap();
        arch::address_space::map(space,first+100000,base,attribute)?;
        self.large.get_mut(&first).unwrap().present=true;
        KERNEL.with_borrow_mut(|k| {k.huge.insert((space,base));});
        Ok(())
    }
    fn has_private_large_mappings(&self)->bool {self.large.values().any(|r|r.mapping.is_some())}
    fn private_large_mapping(&self,space:usize,start:usize,end:usize)->Option<LargeMapping> {
        self.private_large_mapping_requiring_split(space,start,end,|_,_|false)
    }
    fn private_large_mapping_requiring_split(&self,space:usize,start:usize,end:usize,
        mut preserve:impl FnMut(usize,usize)->bool)->Option<LargeMapping> {
        self.large.iter().find_map(|(&first,r)| {
            let (owner,base,permissions)=r.mapping?;
            if owner!=space || base>=end || start>=base+(1<<21) {return None;}
            if r.huge && !r.quarantined && (r.present || permissions==0)
                && preserve(base,permissions) {return None;}
            Some(LargeMapping {first,base,permissions,small:!r.huge,quarantined:r.quarantined})
        })
    }
    fn private_large_protection(&self, space: usize, base: usize) -> Option<LargeMapping> {
        self.stable_private_large_mappings(space, base, base + 1).find(|mapping| mapping.base == base)
    }
    fn stable_private_large_mappings(&self, space: usize, start: usize, end: usize)
        -> impl Iterator<Item = LargeMapping> + '_ {
        self.large.iter().filter_map(move |(&first, r)| {
            let (owner, base, permissions) = r.mapping?;
            (owner == space && base < end && start < base + (1 << 21) && r.huge && !r.quarantined
                && (r.present || permissions == 0)).then_some(LargeMapping {
                    first, base, permissions, small: false, quarantined: false })
        })
    }
    fn protect_private_large_frame(&mut self, first: usize, permissions: usize) -> Result<(), CapabilityError> {
        let (space, base, old) = self.large[&first].mapping.unwrap();
        let frame = first + 100000;
        let result = (|| {
            self.unmap_private_large_frame(first)?;
            if permissions != 0 {
                arch::address_space::map(space, frame, base,
                    capability_call::address_space::Attribute::from_bits(permissions).unwrap())?;
                KERNEL.with_borrow_mut(|k| { k.huge.insert((space, base)); });
            }
            let r = self.large.get_mut(&first).unwrap();
            r.present = permissions != 0;
            r.mapping = Some((space, base, permissions));
            Ok(())
        })();
        if let Err(error) = result {
            let repair = (|| {
                arch::address_space::unmap(space, frame, base)?;
                if old != 0 {
                    arch::address_space::map(space, frame, base,
                        capability_call::address_space::Attribute::from_bits(old).unwrap())?;
                    KERNEL.with_borrow_mut(|k| { k.huge.insert((space, base)); });
                }
                self.large.get_mut(&first).unwrap().present = old != 0;
                Ok::<(), CapabilityError>(())
            })();
            if repair.is_err() { self.quarantine_private_large_mapping(first); return Err(CapabilityError::Fatal); }
            return Err(error);
        }
        Ok(())
    }
    fn unmap_private_large_frame(&mut self,first:usize)->Result<(),CapabilityError> {
        let r=self.large.get_mut(&first).unwrap();
        let (space,base,_)=r.mapping.unwrap();
        if r.present {
            arch::address_space::unmap(space,first+100000,base)?;
            r.present=false;
            KERNEL.with_borrow_mut(|k| {k.huge.remove(&(space,base));});
        }
        Ok(())
    }
    fn demote_private_large_pool(&mut self,first:usize)->Result<(),CapabilityError> {
        assert!(!self.large[&first].present);
        if self.fail_split==Some(SplitFailure::Demote) {return Err(CapabilityError::Fatal);}
        self.demotions+=1;
        self.large.get_mut(&first).unwrap().huge=false;
        self.update_large_keys(first,false);
        Ok(())
    }
    fn ensure_private_large_split_table(&mut self,first:usize,vm:&mut vm_space::VmSpace)->Result<(),CapabilityError> {
        if self.fail_split==Some(SplitFailure::Table) {return Err(CapabilityError::Fatal);}
        let base = self.large[&first].mapping.unwrap().1;
        self.table_preparations.push(base);
        vm.record_page_table(base,321).unwrap();
        Ok(())
    }
    fn restore_private_large_frame(&mut self,first:usize)->Result<(),CapabilityError> {
        if self.fail_restore {return Err(CapabilityError::Fatal);}
        let (space,base,permissions)=self.large[&first].mapping.unwrap();
        // Models the acknowledged leaf-table unlink; physical-memory-tests
        // separately checks the real capability order and both TLB ACKs.
        KERNEL.with_borrow_mut(|k|k.ptes.retain(|&(s,va),_|s!=space||va<base||va>=base+(1<<21)));
        self.large.get_mut(&first).unwrap().huge=true;
        self.large.get_mut(&first).unwrap().present=false;
        self.update_large_keys(first,true);
        if permissions == 0 { Ok(()) } else { self.map_private_large_frame(first,space,base,permissions) }
    }
    fn finish_private_large_split(&mut self,first:usize)->Result<(),CapabilityError> {
        assert!(!self.large[&first].huge);
        self.large.remove(&first);
        Ok(())
    }
    fn quarantine_private_large_mapping(&mut self,first:usize) {self.large.get_mut(&first).unwrap().quarantined=true;}
    fn ensure_page_tables(&mut self,_:usize,va:usize,vm:&mut vm_space::VmSpace)->Result<(),CapabilityError> {
        self.table_preparations.push(va);
        vm.record_page_table(va, 321).unwrap();
        Ok(())
    }
    fn allocate_process_frame(
        &mut self,
        _: usize,
        _: usize,
        _: usize,
    ) -> Result<usize, CapabilityError> {
        self.child_allocations += 1;
        self.allocate_physical_any(PAGE_SIZE)
    }
    fn allocate_physical_any(&mut self, bytes: usize) -> Result<usize, CapabilityError> {
        self.allocate_test_pages(bytes, false)
    }
    fn allocate_test_pages(&mut self, bytes: usize, large: bool) -> Result<usize, CapabilityError> {
        self.allocation_requests.push(bytes);
        assert!(bytes != 0 && bytes % PAGE_SIZE == 0);
        if self.fragmented && bytes > PAGE_SIZE {
            return Err(CapabilityError::InvalidArgument);
        }
        let count = bytes / PAGE_SIZE;
        if self.fail_allocate.is_some_and(|page| page >= self.next && page < self.next + count) {
            return Err(CapabilityError::Fatal);
        }
        let first = self.next + 1;
        if large {
            assert_eq!(count, 512);
            self.pages.large.insert(first, TestLargeBytes {
                bytes: vec![[0xcc; PAGE_SIZE]; count].into_boxed_slice(),
                live: [true; 512],
                remaining: count,
            });
        }
        for _ in 0..count {
            self.next += 1;
            if !large { self.pages.single.insert(self.next, Box::new([0xcc; PAGE_SIZE])); }
            self.frames.insert(self.next + 100000, self.next);
        }
        Ok(first)
    }
    fn free_physical(&mut self, address: usize, bytes: usize) -> Result<(), CapabilityError> {
        if self.fail_free { return Err(CapabilityError::Fatal); }
        if self.large.iter().any(|(&first,r)|r.mapping.is_some()
            && first*PAGE_SIZE<address+bytes && address<(first+512)*PAGE_SIZE) {
            return Err(CapabilityError::IllegalOperation);
        }
        for page in address / PAGE_SIZE..(address + bytes) / PAGE_SIZE {
            assert!(self.pages.remove(&page));
            self.frames.remove(&(page + 100000));
            self.frames.remove(&((page + 100000)|SLICE_KEY));
            self.freed_pages += 1;
        }
        Ok(())
    }
    fn ensure_alpha_frame_at_physical_index(&mut self, page: usize) -> Result<(), CapabilityError> {
        if self.large.range(..=page).next_back().is_some_and(|(&first,r)|page<first+512 && r.huge) {
            return Err(CapabilityError::IllegalOperation);
        }
        if self.fail_ensure == Some(page) { return Err(CapabilityError::Fatal); }
        assert!(self.pages.contains_key(&page));
        Ok(())
    }
    fn allocation_frame_hint(&self, first: usize, pages: usize) -> Option<(usize, usize)> {
        (pages >= 256).then_some((first, pages))
    }
    fn ensure_alpha_frame_for_allocation(&mut self, page: usize, hint: Option<(usize, usize)>) -> Result<(), CapabilityError> {
        if let Some((first, pages))=hint { assert!(page>=first && page-first<pages); }
        self.ensure_alpha_frame_at_physical_index(page)
    }
    fn physical_frame_descriptor_from_index(&self, page: usize) -> Option<usize> {
        let huge=self.large.range(..=page).next_back().is_some_and(|(&first,r)|page<first+512 && r.huge);
        (self.pages.contains_key(&page) && self.fail_ensure != Some(page))
            .then_some((page + 100000)|if huge {SLICE_KEY} else {0})
    }
    fn is_pool_frame_descriptor(&self, frame: usize) -> bool {
        frame.checked_sub(100000).is_some_and(|page|self.pages.contains_key(&page))
    }
    fn map_frame_strict(
        &mut self,
        space: usize,
        frame: usize,
        va: usize,
        vm: &mut vm_space::VmSpace,
    ) -> Result<(), CapabilityError> {
        self.map_frame_strict_with_attribute(space, frame, va, capability_call::address_space::Attribute::ALL, vm)
    }
    fn map_frame_strict_with_attribute(
        &mut self,
        space: usize,
        frame: usize,
        va: usize,
        attribute: capability_call::address_space::Attribute,
        vm: &mut vm_space::VmSpace,
    ) -> Result<(), CapabilityError> {
        let fails = self.fail_map == Some(self.published.len());
        if fails && !self.fail_after_map {
            return Err(CapabilityError::Fatal);
        }
        let page = frame.checked_sub(100000).filter(|p|self.pages.contains_key(p))
            .unwrap_or(self.initializing);
        self.frames.insert(frame, page);
        arch::address_space::map(
            space,
            frame,
            va,
            attribute,
        )?;
        self.published.push((va, self.pages[&page].to_vec()));
        if fails || self.fail_frame_record {
            return Err(CapabilityError::Fatal);
        }
        vm.record_frame(va, frame).unwrap();
        Ok(())
    }

    fn map_frame_with_attribute(
        &mut self, space: usize, frame: usize, va: usize,
        attribute: capability_call::address_space::Attribute, vm: &mut vm_space::VmSpace,
    ) -> Result<(), CapabilityError> {
        self.initializing = self.frames[&frame];
        self.map_frame_strict_with_attribute(space, frame, va, attribute, vm)
    }
}
struct Alpha {
    memory: Memory,
    processes: Processes,
    rollbacks: usize,
    initialization_sources: std::collections::BTreeSet<usize>,
    cached_copy_frames: std::collections::BTreeSet<usize>,
    copy_maps: usize,
    initialization_maps: usize,
    initialization_evictions: usize,
    pinned_copies: Vec<Option<usize>>,
    lookups: usize,
    replace_on_lookup: Option<(usize, usize, usize, usize)>,
    fail_copy: Option<usize>,
}
impl Alpha {
    fn zero_physical_pages(&mut self,first:usize,pages:usize)->Result<(),CapabilityError> {
        for page in first..first+pages {self.memory.pages.get_mut(&page).unwrap().fill(0);}
        Ok(())
    }
    fn allocate_process_frame(&mut self, _: usize, _: usize, _: usize) -> Result<usize, CapabilityError> {
        panic!("native management caps are outside this prepared-pool fixture")
    }
    fn process_root_slot(&self, root: usize) -> Result<usize, CapabilityError> {
        if root == 0 { Err(CapabilityError::InvalidArgument) } else { Ok(root) }
    }
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
            processes: Processes {inner:processes,snapshots:std::cell::Cell::new(0)},
            rollbacks: 0,
            initialization_sources: Default::default(),
            cached_copy_frames: Default::default(),
            copy_maps: 0,
            initialization_maps: 0,
            initialization_evictions: 0,
            pinned_copies: Vec::new(),
            lookups: 0,
            replace_on_lookup: None,
            fail_copy: None,
        }
    }
    fn ensure_process_frame_chunks(
        &mut self,
        _: usize,
        _: usize,
        _: usize,
        _: usize,
    ) -> Result<(), CapabilityError> {
        KERNEL.with_borrow_mut(|kernel| kernel.chunk_calls += 1);
        Ok(())
    }
    fn process_frame_for_page(&mut self, pid: usize, va: usize) -> Result<usize, CapabilityError> {
        self.lookups += 1;
        if self.replace_on_lookup.is_some_and(|(at,_,_,_)|at==self.lookups) {
            let (_,owner,address,frame)=self.replace_on_lookup.take().unwrap();
            self.processes.vm_space_mut(owner).unwrap().record_frame(address,frame).unwrap();
        }
        self.processes
            .vm_space_mut(pid)
            .unwrap()
            .find_frame(va)
            .ok_or(CapabilityError::InvalidArgument)
    }
    fn process_frame_for_write(&mut self, pid: usize, va: usize) -> Result<usize, CapabilityError> {
        if self.processes.vm_space(pid).is_some_and(|vm| vm.private_page_shared(va)) {
            return Err(CapabilityError::IllegalOperation);
        }
        self.process_frame_for_page(pid, va)
    }
    fn map_process_transfer_frame(&mut self, pid: usize, frame: usize, pinned: Option<usize>) -> Result<usize, CapabilityError> {
        if let Some(address) = self.cached_initialization_frame(frame) {
            return Ok(address);
        }
        self.map_process_copy_frame(pid, frame, pinned)
    }
    fn map_initialization_page(&mut self, page: usize) -> Result<usize, CapabilityError> {
        self.initialization_maps += 1;
        if self.memory.physical_frame_descriptor_from_index(page).is_some_and(|f|f&SLICE_KEY!=0) {
            return Ok(self.memory.pages.get_mut(&page).unwrap().as_mut_ptr() as usize);
        }
        let old = self.initialization_sources.len();
        self.initialization_sources.retain(|old|*old%2048!=page%2048);
        self.initialization_evictions += old - self.initialization_sources.len();
        self.initialization_sources.insert(page);
        self.memory.initializing = page;
        Ok(self.memory.pages.get_mut(&page).unwrap().as_mut_ptr() as usize)
    }
    fn map_initialization_run(&mut self, page: usize, maximum: usize) -> Result<(usize, usize), CapabilityError> {
        if let Some((first, offset)) = self.memory.pages.large_offset(page) {
            let span = self.memory.pages.large.get_mut(&first).unwrap();
            let count = span.live[offset..].iter().take(maximum).take_while(|live| **live).count();
            self.initialization_maps += 1;
            return Ok((span.bytes[offset..offset + count].as_mut_ptr() as usize, count));
        }
        Ok((self.map_initialization_page(page)?, 1))
    }
    fn cached_initialization_page(&self, page: usize) -> Option<usize> {
        self.initialization_sources.contains(&page)
            .then(|| self.memory.pages[&page].as_ptr() as usize)
    }
    fn cached_initialization_frame(&self, frame: usize) -> Option<usize> {
        let page=*self.memory.frames.get(&frame)?;
        if self.memory.physical_frame_descriptor_from_index(page)!=Some(frame) {return None;}
        self.cached_initialization_page(page)
    }
    fn cached_initialization_slot(&self, frame: usize) -> Option<usize> {
        let page = *self.memory.frames.get(&frame)?;
        (self.memory.physical_frame_descriptor_from_index(page) == Some(frame)
            && self.initialization_sources.contains(&page)).then_some(page % 2048)
    }
    fn initialization_alias_address(&self, physical: usize) -> usize {
        self.initialization_sources.iter().find(|page| **page % 2048 == physical % 2048)
            .map_or(0, |page| self.memory.pages[page].as_ptr() as usize)
    }
    fn initialization_page_needs_mapping(&self, page: usize) -> bool {
        self.memory.physical_frame_descriptor_from_index(page).is_some()
            && !self.initialization_sources.contains(&page)
    }
    fn cached_pool_copy_frame(&self, frame: usize) -> Option<usize> {
        if !self.memory.is_pool_frame_descriptor(frame) || !self.cached_copy_frames.contains(&frame) { return None; }
        Some(self.memory.pages[&(frame - 100000)].as_ptr() as usize)
    }
    fn map_process_copy_frame(
        &mut self,
        pid: usize,
        frame: usize,
        pinned: Option<usize>,
    ) -> Result<usize, CapabilityError> {
        self.copy_maps += 1;
        self.pinned_copies.push(pinned);
        if self.fail_copy==Some(self.copy_maps) {return Err(CapabilityError::Fatal);}
        if pid == 0 && pinned.is_none() {
            self.memory.initializing = self.memory.frames[&frame];
        }
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
#[path = "../../nanami/src/nanami_core/alpha/memory_large.rs"]
mod memory_large;
#[path = "../../nanami/src/nanami_core/alpha/memory_private.rs"]
mod memory_private;
#[path = "../../nanami/src/nanami_core/alpha/memory_snapshot.rs"]
mod memory_snapshot;
#[path = "../../nanami/src/nanami_core/alpha/memory_clone.rs"]
mod memory_clone;
#[path = "../../nanami/src/nanami_core/alpha/memory_staging.rs"]
mod memory_staging;
#[path = "../../nanami/src/nanami_core/alpha/memory_transfer.rs"]
mod memory_transfer;
mod memory_init { pub(super) const PAGES: usize = 2048; }
#[path = "../../nanami/src/nanami_core/alpha/memory_image.rs"]
mod memory_image;
#[path = "../../nanami/src/nanami_core/alpha/memory_lazy.rs"]
mod memory_lazy;
#[path = "../../nanami/src/nanami_core/alpha/page_fill.rs"]
mod page_fill;
use page_fill::fill_lazy_page;

use memory_pages::PageSource;

#[test]
fn huge_frame_metadata_keeps_all_slice_keys_permissions_and_one_large_pte() {
    let mut a = huge_alpha();
    let base = 0x800000;
    nanami_core::vm_space::VmSpace::take_frame_point_records();
    a.map_process_initialized_pages_protected(2, Some(base), 512, None, 3).unwrap();
    let point_records = nanami_core::vm_space::VmSpace::take_frame_point_records();
    let vm = a.processes.vm_space(2).unwrap();
    assert_eq!(vm.frame_storage_counts(), (8, 0));
    assert!(!vm.page_tables_ready(base));
    for page in 0..512 {
        let va = base + page * PAGE_SIZE;
        let physical = a.processes.find_active_physical_allocation_reference(2, va, 1).unwrap().base_page;
        assert_eq!(vm.find_frame(va), a.memory.physical_frame_descriptor_from_index(physical));
        assert_eq!(vm.permissions(va), 3);
        assert!(!vm.mapping_is_uncertain(va));
        assert!(!vm.mapping_deferred(va));
        assert!(!vm.private_page_shared(va));
    }
    assert_eq!(transfer_bytes(&a, 2, base, 512 * PAGE_SIZE), vec![0; 512 * PAGE_SIZE]);
    let space = a.processes.memory_context(2).unwrap().address_space;
    KERNEL.with_borrow(|k| {
        assert!(k.huge.contains(&(space, base)));
        assert_eq!(k.ptes.keys().filter(|&&(s,_)| s == space).count(), 1);
    });
    println!("HUGE-FRAME-WORDS pages=512 point-records={point_records} packed-words=8");
    assert_eq!(point_records, 0);
    release_range(&mut a, base, 512 * PAGE_SIZE).unwrap();
    assert_eq!(a.processes.vm_space(2).unwrap().frame_storage_counts(), (0, 0));
}

#[test]
fn direct_deferred_image_has_one_private_owner_and_initialized_bytes_before_any_tail_pte() {
    let mut a=huge_alpha();a.memory.large_enabled=false;
    let source=a.source(3514168);let base=0x800000;let pages=862;
    a.map_process_initialized_pages_deferred(2,base,pages,Some(source),16*PAGE_SIZE).unwrap();
    let space=a.processes.memory_context(2).unwrap().address_space;
    KERNEL.with_borrow(|k|assert_eq!(k.ptes.keys().filter(|&&(s,_)|s==space).count(),16));
    let actual=transfer_bytes(&a,2,base,pages*PAGE_SIZE);
    for (i,byte) in actual.iter().enumerate() {assert_eq!(*byte,if i<source.bytes {(i%239)as u8}else{0});}
    for i in 0..pages {
        let va=base+i*PAGE_SIZE;let vm=a.processes.vm_space(2).unwrap();
        assert_eq!(vm.permissions(va),1);assert_eq!(vm.mapping_deferred(va),i>=16);assert!(!vm.private_page_shared(va));
        let allocation=a.processes.find_active_physical_allocation_reference(2,va,1).unwrap();
        assert!(!allocation.owns_frame_cap);assert_eq!(a.processes.physical_reference_count(allocation),1);
    }
    assert_eq!(a.handle_populate_snapshot(OsRequestEvent{identifier:3,code:0,arg0:2,arg1:base+40*PAGE_SIZE,arg2:1,arg3:0}),Ok(1));
    KERNEL.with_borrow(|k|assert_eq!(k.ptes.keys().filter(|&&(s,_)|s==space).count(),512));
    a.handle_mapping_protect_request(OsRequestEvent{identifier:3,code:0,arg0:2,arg1:base+700*PAGE_SIZE,arg2:PAGE_SIZE,arg3:3}).unwrap();
    assert!(!a.processes.vm_space(2).unwrap().mapping_deferred(base+700*PAGE_SIZE));
    assert_eq!(transfer_bytes(&a,2,base,pages*PAGE_SIZE),actual);
}

#[test]
fn deferred_constructor_keeps_available_huge_leaf_and_defers_only_small_suffix() {
    let mut a=huge_alpha();let base=0x400000;
    let source = a.source(3514168);
    let pages = 862;
    a.map_process_initialized_pages_deferred(2,base,pages,Some(source),269*PAGE_SIZE).unwrap();
    let space=a.processes.memory_context(2).unwrap().address_space;
    KERNEL.with_borrow(|k|{assert!(k.huge.contains(&(space,base)));assert_eq!(k.ptes.keys().filter(|&&(s,_)|s==space).count(),1);});
    let vm=a.processes.vm_space(2).unwrap();
    for i in 0..512 {assert!(!vm.mapping_deferred(base+i*PAGE_SIZE));assert_eq!(vm.permissions(base+i*PAGE_SIZE),1);}
    for i in 512..pages {
        let va = base + i * PAGE_SIZE;
        assert!(vm.mapping_deferred(va));
        assert_eq!(vm.permissions(va), 1);
        assert_eq!(vm.hardware_permissions(va), 0);
    }
    let actual = transfer_bytes(&a,2,base,pages*PAGE_SIZE);
    assert!(actual[..source.bytes].iter().enumerate().all(|(i,&byte)| byte == (i%239) as u8));
    assert!(actual[source.bytes..].iter().all(|&byte| byte == 0));
    release_range(&mut a,base,pages*PAGE_SIZE).unwrap();
}

#[test]
fn deferred_private_fork_and_release_keep_untouched_parent_bytes_and_skip_absent_unmaps() {
    let mut a=huge_alpha();a.memory.large_enabled=false;
    let base=0x800000;
    a.map_process_initialized_pages_deferred(2,base,32,None,PAGE_SIZE).unwrap();
    a.processes.seal_capabilities(1).unwrap();
    a.clone_process_vm_private_cow(2,1).unwrap();
    assert!(a.processes.vm_space(2).unwrap().mapping_deferred(base+2*PAGE_SIZE));
    a.handle_process_private_write(OsRequestEvent{identifier:3,code:0,arg0:1,arg1:base+2*PAGE_SIZE,arg2:PAGE_SIZE,arg3:1}).unwrap();
    let frame=a.processes.vm_space(1).unwrap().find_frame(base+2*PAGE_SIZE).unwrap();
    let page=a.memory.frames[&frame];a.memory.pages.get_mut(&page).unwrap()[0]=0xa5;
    assert_ne!(transfer_bytes(&a,1,base+2*PAGE_SIZE,1),transfer_bytes(&a,2,base+2*PAGE_SIZE,1));
    let before=KERNEL.with_borrow(|k|k.unmaps);release_range(&mut a,base,32*PAGE_SIZE).unwrap();
    assert_eq!(KERNEL.with_borrow(|k|k.unmaps),before+1);
}

#[test]
fn deferred_tail_initialization_error_rolls_back_or_retains_unacknowledged_prefix() {
    for fail_unmap in [false,true] {
        let mut a=huge_alpha();a.memory.large_enabled=false;
        let pages = 192;
        let source=a.source(pages*PAGE_SIZE);let base=0x800000;
        let original=transfer_bytes(&a,1,source.address,source.bytes);
        a.fail_copy=Some(90);
        if fail_unmap {KERNEL.with_borrow_mut(|k|k.fail_unmap=Some(UnmapFailure::Before));}
        assert_eq!(a.map_process_initialized_pages_deferred(2,base,pages,Some(source),16*PAGE_SIZE),Err(CapabilityError::Fatal));
        if fail_unmap {
            assert!(a.processes.vm_space(2).unwrap().mapping_is_uncertain(base));
            assert!(a.processes.find_active_physical_allocation_reference(2,base,1).is_some());
            let partial_word = base + 64 * PAGE_SIZE;
            assert!(a.processes.vm_space(2).unwrap().find_frame(partial_word).is_none());
            assert!(a.processes.find_active_physical_allocation_reference(2,partial_word,1).is_some());
            KERNEL.with_borrow_mut(|k|k.fail_unmap=None);
            a.handle_mapping_release_request(OsRequestEvent{identifier:3,code:0,arg0:base,
                arg1:pages*PAGE_SIZE,arg2:2,arg3:nanami_uapi::os::OS_MAPPING_RELEASE_ALLOW_HOLES}).unwrap();
        }
        assert!(a.processes.physical_pages_in_range(2,base,base+pages*PAGE_SIZE).is_empty());
        let space=a.processes.memory_context(2).unwrap().address_space;
        KERNEL.with_borrow(|k|assert!(!k.ptes.keys().any(|&(s,va)|s==space && va>=base && va<base+pages*PAGE_SIZE)));
        assert_eq!(transfer_bytes(&a,1,source.address,source.bytes),original);
    }
}
fn huge_alpha() -> Alpha {
    let mut a=Alpha::new();
    a.processes.seal_capabilities(2).unwrap();
    a.memory.large_enabled=true;
    a
}

fn readonly_private_pair(pages: usize, rights: usize) -> (Alpha, Vec<u8>) {
    let mut a = huge_alpha();
    a.processes.seal_capabilities(1).unwrap();
    a.map_process_initialized_pages_protected(2, Some(0x400000), pages, None, rights).unwrap();
    for index in 0..pages {
        let page = a.processes.find_active_physical_allocation_reference(2, 0x400000 + index * PAGE_SIZE, 1).unwrap();
        a.memory.pages.get_mut(&page.base_page).unwrap().fill((index % 239) as u8);
    }
    let expected = transfer_bytes(&a, 2, 0x400000, pages * PAGE_SIZE);
    a.clone_process_vm_private_cow(2, 1).unwrap();
    (a, expected)
}

#[test]
fn deferred_detach_and_final_protection_halves_guest_map_and_unmap_work() {
    use nanami_uapi::os::{OS_PRIVATE_WRITE_DETACH, OS_PRIVATE_WRITE_DETACH_DEFERRED};
    for rights in [1, 3, 5, 7] { for final_rights in [3, 7] {
        if rights == final_rights { continue; }
        let mut work = Vec::new();
        for mode in [OS_PRIVATE_WRITE_DETACH, OS_PRIVATE_WRITE_DETACH_DEFERRED] {
            let (mut a, expected) = readonly_private_pair(64, rights);
            let before = KERNEL.with_borrow(|k| (k.map_attempts, k.unmaps));
            a.handle_process_private_write(OsRequestEvent { identifier: 3, code: 0, arg0: 1,
                arg1: 0x400000, arg2: 64 * PAGE_SIZE, arg3: mode }).unwrap();
            for index in 0..64 {
                let va = 0x400000 + index * PAGE_SIZE;
                let vm = a.processes.vm_space(1).unwrap();
                assert_eq!(vm.permissions(va), rights);
                assert!(!vm.private_page_shared(va));
                assert_eq!(vm.mapping_deferred(va), mode == OS_PRIVATE_WRITE_DETACH_DEFERRED);
            }
            a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0, arg0: 1,
                arg1: 0x400000, arg2: 64 * PAGE_SIZE, arg3: final_rights }).unwrap();
            work.push(KERNEL.with_borrow(|k| (k.map_attempts - before.0, k.unmaps - before.1)));
            assert_eq!(transfer_bytes(&a, 1, 0x400000, 64 * PAGE_SIZE), expected);
            assert_eq!(transfer_bytes(&a, 2, 0x400000, 64 * PAGE_SIZE), expected);
            for index in 0..64 {
                let va = 0x400000 + index * PAGE_SIZE;
                let vm = a.processes.vm_space(1).unwrap();
                assert_eq!(vm.permissions(va), final_rights);
                assert!(!vm.mapping_deferred(va));
                assert!(!vm.mapping_is_uncertain(va));
                let parent = a.processes.find_active_physical_allocation_reference(2, va, 1).unwrap();
                let child = a.processes.find_active_physical_allocation_reference(1, va, 1).unwrap();
                assert_ne!(parent.base_page, child.base_page);
                assert_eq!(a.processes.physical_reference_count(parent), 1);
                assert_eq!(a.processes.physical_reference_count(child), 1);
            }
        }
        println!("DEFERRED-DETACH pages=64 old-rights={rights} final-rights={final_rights} maps-unmaps-before={:?} after={:?}", work[0], work[1]);
        assert_eq!(work, [(128, 128), (64, 64)]);
    }}
}

#[test]
fn failed_final_protection_keeps_deferred_old_rights_and_can_populate_without_write_access() {
    let (mut a, expected) = readonly_private_pair(4, 1);
    a.handle_process_private_write(OsRequestEvent { identifier: 3, code: 0, arg0: 1,
        arg1: 0x400000, arg2: 4 * PAGE_SIZE, arg3: nanami_uapi::os::OS_PRIVATE_WRITE_DETACH_DEFERRED }).unwrap();
    for after in [false, true] {
        KERNEL.with_borrow_mut(|k| k.fail_map_once = Some((k.map_attempts + 2, after)));
        assert!(a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0, arg0: 1,
            arg1: 0x400000, arg2: 4 * PAGE_SIZE, arg3: 3 }).is_err());
        for index in 0..4 {
            let va = 0x400000 + index * PAGE_SIZE;
            let vm = a.processes.vm_space(1).unwrap();
            assert_eq!(vm.permissions(va), 1);
            assert!(vm.mapping_deferred(va));
            assert!(!vm.mapping_is_uncertain(va));
        }
        let space = a.processes.memory_context(1).unwrap().address_space;
        KERNEL.with_borrow(|k| assert!(!k.ptes.keys().any(|&(s, _)| s == space)));
    }
    assert_eq!(a.handle_populate_snapshot(OsRequestEvent { identifier: 3, code: 0, arg0: 1,
        arg1: 0x400000, arg2: 2, arg3: 0 }), Ok(0));
    assert_eq!(a.handle_populate_snapshot(OsRequestEvent { identifier: 3, code: 0, arg0: 1,
        arg1: 0x400000, arg2: 1, arg3: 0 }), Ok(1));
    assert_eq!(transfer_bytes(&a, 1, 0x400000, 4 * PAGE_SIZE), expected);
    assert_eq!(transfer_bytes(&a, 2, 0x400000, 4 * PAGE_SIZE), expected);
}

#[test]
fn deferred_detach_requires_sealing_and_retains_both_backings_if_unmap_is_unacknowledged() {
    let mut unsealed = Alpha::new();
    assert_eq!(unsealed.handle_process_private_write(OsRequestEvent { identifier: 3, code: 0, arg0: 1,
        arg1: 0x400000, arg2: PAGE_SIZE, arg3: nanami_uapi::os::OS_PRIVATE_WRITE_DETACH_DEFERRED }), Err(CapabilityError::PermissionDenied));
    for after in [false, true] {
        let (mut a, expected) = readonly_private_pair(2, 1);
        KERNEL.with_borrow_mut(|k| k.fail_unmap = Some(if after { UnmapFailure::After } else { UnmapFailure::Before }));
        assert!(a.handle_process_private_write(OsRequestEvent { identifier: 3, code: 0, arg0: 1,
            arg1: 0x400000, arg2: 2 * PAGE_SIZE, arg3: nanami_uapi::os::OS_PRIVATE_WRITE_DETACH_DEFERRED }).is_err());
        assert!(a.processes.vm_space(1).unwrap().mapping_is_uncertain(0x400000));
        let original = a.processes.find_active_physical_allocation_reference(1, 0x400000, 1).unwrap();
        assert_eq!(a.processes.physical_reference_count(original), 2);
        let held = a.processes.releasable_deferred_physical_allocations_for_pid(1);
        assert_eq!(held.len(), 1);
        assert!(a.memory.pages.contains_key(&held[0].base_page));
        assert_ne!(held[0].base_page, original.base_page);
        assert_eq!(transfer_bytes(&a, 2, 0x400000, 2 * PAGE_SIZE), expected);
    }
}

#[test]
fn large_copy_preserves_huge_leaf_identity_rights_and_private_ownership() {
    let bytes = 1 << 21;
    for permissions in 0..=7 {
        let mut a = huge_alpha();
        let input = a.source(bytes);
        let source = 0x400000;
        let target = 0xa00000;
        if permissions == 0 {
            a.map_process_cloned_unmapped_pages(2, source, 512, input).unwrap();
            a.map_process_cloned_unmapped_pages(2, target, 512, input).unwrap();
            for page in 0..512 {
                let owned = a.processes.find_active_physical_allocation_reference(2, target + page * PAGE_SIZE, 1).unwrap();
                a.memory.pages.get_mut(&owned.base_page).unwrap().fill(0);
            }
        } else {
            a.map_process_initialized_pages_protected(2, Some(source), 512, Some(input), permissions).unwrap();
            a.map_process_initialized_pages_protected(2, Some(target), 512, None, permissions).unwrap();
        }
        let space = a.processes.memory_context(2).unwrap().address_space;
        let before = KERNEL.with_borrow(|k| (k.map_attempts, k.unmaps, k.cap_copies, k.huge.clone()));
        let frames: Vec<_> = (0..512).map(|page|
            a.processes.vm_space(2).unwrap().find_frame(target + page * PAGE_SIZE).unwrap()).collect();
        a.clone_backed_memory(2, source, 2, target, bytes).unwrap();
        assert_eq!(transfer_bytes(&a, 2, source, bytes), transfer_bytes(&a, 2, target, bytes));
        KERNEL.with_borrow(|k| {
            assert_eq!((k.map_attempts, k.unmaps, k.cap_copies, k.huge.clone()), before);
            assert_eq!(k.huge.contains(&(space, target)), permissions != 0);
        });
        for (page, frame) in frames.into_iter().enumerate() {
            let va = target + page * PAGE_SIZE;
            let vm = a.processes.vm_space(2).unwrap();
            assert_eq!(vm.find_frame(va), Some(frame));
            assert_eq!(vm.permissions(va), permissions);
            let owned = a.processes.find_active_physical_allocation_reference(2, va, 1).unwrap();
            assert_eq!(a.processes.physical_reference_count(owned), 1);
            let original = a.processes.find_active_physical_allocation_reference(2, source + page * PAGE_SIZE, 1).unwrap();
            assert_ne!(original.base_page, owned.base_page);
        }
        release_range(&mut a, target, bytes).unwrap();
        assert_eq!(transfer_bytes(&a, 2, source, bytes), (0..bytes).map(|i| (i % 239) as u8).collect::<Vec<_>>());
    }
}

#[test]
fn overlapping_large_copy_retains_huge_backing_and_memmove_bytes_in_both_directions() {
    for backward in [false, true] {
        let mut a = huge_alpha();
        let base = 0x400000;
        let bytes = (1 << 21) + PAGE_SIZE;
        let input = a.source(bytes);
        a.map_process_initialized_pages_protected(2, Some(base), 513, Some(input), 3).unwrap();
        let mut expected = transfer_bytes(&a, 2, base, bytes);
        let (source, target) = if backward { (0, PAGE_SIZE) } else { (PAGE_SIZE, 0) };
        expected.copy_within(source..source + (1 << 21), target);
        let before = KERNEL.with_borrow(|k| (k.map_attempts, k.unmaps, k.huge.clone()));
        a.clone_backed_memory(2, base + source, 2, base + target, 1 << 21).unwrap();
        assert_eq!(transfer_bytes(&a, 2, base, bytes), expected);
        KERNEL.with_borrow(|k| assert_eq!((k.map_attempts, k.unmaps, k.huge.clone()), before));
    }
}

#[test]
fn failed_large_copy_keeps_huge_frames_owned_and_unacknowledged_release_cannot_free_them() {
    let mut a = huge_alpha();
    let input = a.source(1 << 21);
    a.map_process_initialized_pages_protected(2, Some(0x400000), 512, Some(input), 3).unwrap();
    a.map_process_initialized_pages_protected(2, Some(0xa00000), 512, None, 3).unwrap();
    a.fail_copy = Some(a.copy_maps + 17);
    assert_eq!(a.clone_backed_memory(2, 0x400000, 2, 0xa00000, 1 << 21), Err(CapabilityError::Fatal));
    let space = a.processes.memory_context(2).unwrap().address_space;
    KERNEL.with_borrow_mut(|k| {
        assert!(k.huge.contains(&(space, 0xa00000)));
        k.fail_unmap = Some(UnmapFailure::Before);
    });
    assert_eq!(release_range(&mut a, 0xa00000, 1 << 21), Err(CapabilityError::Fatal));
    for page in 0..512 {
        let owned = a.processes.find_active_physical_allocation_reference(2, 0xa00000 + page * PAGE_SIZE, 1).unwrap();
        assert_eq!(a.processes.physical_reference_count(owned), 1);
        assert!(a.memory.pages.contains_key(&owned.base_page));
    }
    assert_eq!(transfer_bytes(&a, 2, 0x400000, 1 << 21), (0..1 << 21).map(|i| (i % 239) as u8).collect::<Vec<_>>());
}

#[test]
fn automatic_private_reservation_aligns_large_requests_after_a_small_allocation() {
    for pages in [512, 1024] {
        let mut a = huge_alpha();
        let small = a.map_process_initialized_pages_protected(2, None, 1, None, 3).unwrap();
        let base = a.map_process_initialized_pages_protected(2, None, pages, None, 3).unwrap();
        assert_eq!(base & ((1 << 21) - 1), 0);
        assert!(base >= small + PAGE_SIZE);
        let space = a.processes.memory_context(2).unwrap().address_space;
        KERNEL.with_borrow(|k| {
            assert_eq!(k.huge.iter().filter(|&&(s, va)| s == space
                && va >= base && va < base + pages * PAGE_SIZE).count(), pages / 512);
            let leaves: Vec<_> = k.ptes.keys().filter(|&&(s, va)| s == space
                && va >= base && va < base + pages * PAGE_SIZE).collect();
            assert_eq!(leaves.len(), pages / 512);
            assert!(leaves.iter().all(|key| k.huge.contains(key)));
        });
        assert_eq!(transfer_bytes(&a, 2, base, pages * PAGE_SIZE), vec![0; pages * PAGE_SIZE]);
        for offset in 0..pages {
            let va = base + offset * PAGE_SIZE;
            assert_eq!(a.processes.vm_space(2).unwrap().permissions(va), 3);
            let allocation = a.processes.find_active_physical_allocation_reference(2, va, 1).unwrap();
            assert_eq!(a.processes.physical_reference_count(allocation), 1);
        }
        release_range(&mut a, base, pages * PAGE_SIZE).unwrap();
        assert!(a.processes.physical_pages_in_range(2, base, base + pages * PAGE_SIZE).is_empty());
        assert_eq!(transfer_bytes(&a, 2, small, PAGE_SIZE), vec![0; PAGE_SIZE]);
    }
}
fn release_range(a:&mut Alpha,base:usize,bytes:usize)->Result<(),CapabilityError> {
    a.handle_mapping_release_request(OsRequestEvent {
        identifier:3,code:0,arg0:base,arg1:bytes,arg2:2,arg3:0,
    })
}

#[test]
fn mixed_protection_installs_final_partial_huge_permissions_in_one_split() {
    for permissions in [0, 5] {
        let mut a = huge_alpha();
        let base = 0x400000;
        let pages = 513;
        a.map_process_initialized_pages_protected(2, Some(base), pages, None, 1).unwrap();
        fill_private_fork_pattern(&mut a, base, pages);
        let original = transfer_bytes(&a, 2, base, pages * PAGE_SIZE);
        let before = KERNEL.with_borrow(|k| (k.map_attempts, k.unmaps));
        a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0,
            arg0: 2, arg1: base + PAGE_SIZE, arg2: 512 * PAGE_SIZE, arg3: permissions }).unwrap();
        assert_eq!(KERNEL.with_borrow(|k| (k.map_attempts - before.0, k.unmaps - before.1)), (if permissions == 0 { 1 } else { 513 }, 2));
        assert_eq!(transfer_bytes(&a, 2, base, pages * PAGE_SIZE), original);
        let space = a.processes.memory_context(2).unwrap().address_space;
        KERNEL.with_borrow(|k| {
            assert!(k.huge.is_empty());
            assert_eq!(k.ptes[&(space, base)].1, 1);
            for page in 1..pages {
                let pte = k.ptes.get(&(space, base + page * PAGE_SIZE));
                assert_eq!(pte.map(|entry| entry.1), (permissions != 0).then_some(permissions));
            }
        });
    }
}

#[test]
fn failed_mixed_protection_restores_huge_and_ordinary_pages() {
    for ordinal in [1, 6] {
        for after_map in [false, true] {
            let mut a = huge_alpha();
            let base = 0x400000;
            let pages = 513;
            a.map_process_initialized_pages_protected(2, Some(base), pages, None, 1).unwrap();
            fill_private_fork_pattern(&mut a, base, pages);
            let original = transfer_bytes(&a, 2, base, pages * PAGE_SIZE);
            KERNEL.with_borrow_mut(|k| { k.fail_map_once = Some((k.map_attempts + ordinal, after_map)); });
            assert!(a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0,
                arg0: 2, arg1: base + PAGE_SIZE, arg2: 512 * PAGE_SIZE, arg3: 5 }).is_err());
            let space = a.processes.memory_context(2).unwrap().address_space;
            KERNEL.with_borrow(|k| {
                assert!(k.huge.contains(&(space, base)));
                assert_eq!(k.ptes.len(), 2);
                assert_eq!(k.ptes[&(space, base)].1, 1);
                assert_eq!(k.ptes[&(space, base + 512 * PAGE_SIZE)].1, 1);
            });
            assert_eq!(a.memory.demotions, usize::from(ordinal != 1));
            let vm = a.processes.vm_space(2).unwrap();
            for page in 0..pages {
                let va = base + page * PAGE_SIZE;
                assert_eq!(vm.permissions(va), 1);
                assert!(!vm.mapping_is_uncertain(va));
            }
            assert_eq!(transfer_bytes(&a, 2, base, pages * PAGE_SIZE), original);
        }
    }
}

#[test]
fn mixed_protection_keeps_the_whole_huge_leaf_next_to_small_pages() {
    let mut a = huge_alpha();
    let base = 0x400000;
    let bytes = 513 * PAGE_SIZE;
    a.map_process_initialized_pages_protected(2, Some(base), 513, None, 1).unwrap();
    let before = KERNEL.with_borrow(|k| (k.map_attempts, k.unmaps));
    a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0,
        arg0: 2, arg1: base, arg2: bytes, arg3: 5 }).unwrap();
    assert_eq!(a.memory.demotions, 0);
    assert_eq!(KERNEL.with_borrow(|k| (k.map_attempts - before.0, k.unmaps - before.1)), (2, 2));
    let space = a.processes.memory_context(2).unwrap().address_space;
    assert_eq!(a.memory.private_large_protection(space, base).unwrap().permissions, 5);
    assert_eq!(a.processes.vm_space(2).unwrap().permissions(base + 512 * PAGE_SIZE), 5);
}

#[test]
fn whole_huge_permission_changes_keep_large_frames_and_restore_zero_rights_without_small_maps() {
    for pages in [512, 1024] {
        let mut a = huge_alpha();
        let base = 0x400000;
        a.map_process_initialized_pages_protected(2, Some(base), pages, None, 3).unwrap();
        let space = a.processes.memory_context(2).unwrap().address_space;
        let keys: Vec<_> = (0..pages).map(|i| a.processes.vm_space(2).unwrap().find_frame(base + i * PAGE_SIZE)).collect();
        for permissions in [1, 0, 3, 5, 2, 7, 4, 3] {
            a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0,
                arg0: 2, arg1: base, arg2: pages * PAGE_SIZE, arg3: permissions }).unwrap();
            assert_eq!(a.memory.large.len(), pages / 512);
            KERNEL.with_borrow(|k| {
                assert_eq!(k.huge.iter().filter(|&&(s, _)| s == space).count(),
                    if permissions == 0 { 0 } else { pages / 512 });
                assert_eq!(k.ptes.keys().filter(|&&(s, _)| s == space).count(),
                    if permissions == 0 { 0 } else { pages / 512 });
            });
            for (i, &frame) in keys.iter().enumerate() {
                let va = base + i * PAGE_SIZE;
                let vm = a.processes.vm_space(2).unwrap();
                assert_eq!(vm.find_frame(va), frame);
                assert_eq!(vm.permissions(va), permissions);
                assert!(!vm.mapping_is_uncertain(va));
            }
        }
        assert_eq!(transfer_bytes(&a, 2, base, pages * PAGE_SIZE), vec![0; pages * PAGE_SIZE]);
        release_range(&mut a, base, pages * PAGE_SIZE).unwrap();
    }
}

#[test]
fn failed_second_huge_permission_change_restores_the_committed_prefix_and_current_leaf() {
    for after in [false, true] {
        let mut a = huge_alpha();
        let base = 0x400000;
        a.map_process_initialized_pages_protected(2, Some(base), 1024, None, 3).unwrap();
        KERNEL.with_borrow_mut(|k| { k.fail_map_once = Some((k.map_attempts + 2, after)); });
        assert_eq!(a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0,
            arg0: 2, arg1: base, arg2: 1024 * PAGE_SIZE, arg3: 1 }), Err(CapabilityError::Fatal));
        let space = a.processes.memory_context(2).unwrap().address_space;
        KERNEL.with_borrow(|k| {
            assert_eq!(k.huge.len(), 2);
            assert_eq!(k.ptes.len(), 2);
            assert_eq!(k.ptes[&(space, base)].1, 3);
            assert_eq!(k.ptes[&(space, base + (1 << 21))].1, 3);
        });
        for offset in 0..1024 {
            let vm = a.processes.vm_space(2).unwrap();
            assert_eq!(vm.permissions(base + offset * PAGE_SIZE), 3);
            assert!(!vm.mapping_is_uncertain(base + offset * PAGE_SIZE));
        }
        assert!(a.memory.large.values().all(|r| r.huge && !r.quarantined));
        release_range(&mut a, base, 1024 * PAGE_SIZE).unwrap();
    }
}

#[test]
fn unacknowledged_huge_permission_repair_quarantines_every_slice_and_keeps_ram() {
    for after in [false, true] {
        let mut a = huge_alpha();
        let base = 0x400000;
        a.map_process_initialized_pages_protected(2, Some(base), 512, None, 3).unwrap();
        KERNEL.with_borrow_mut(|k| { k.fail_unmap = Some(if after { UnmapFailure::After } else { UnmapFailure::Before }); });
        assert!(a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0,
            arg0: 2, arg1: base, arg2: 512 * PAGE_SIZE, arg3: 1 }).is_err());
        assert!(a.memory.large.values().all(|r| r.quarantined));
        assert_eq!(a.processes.physical_pages_in_range(2, base, base + (1 << 21)).len(), 512);
        for offset in 0..512 { assert!(a.processes.vm_space(2).unwrap().mapping_is_uncertain(base + offset * PAGE_SIZE)); }
        assert!(release_range(&mut a, base, 512 * PAGE_SIZE).is_err());
    }
}

#[test]
fn partial_protection_after_whole_zero_rights_splits_only_when_needed_and_can_restore_absence() {
    for failed_split in [false, true] {
        let mut a = huge_alpha();
        let base = 0x400000;
        a.map_process_initialized_pages_protected(2, Some(base), 512, None, 3).unwrap();
        a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0,
            arg0: 2, arg1: base, arg2: 512 * PAGE_SIZE, arg3: 0 }).unwrap();
        if failed_split { a.memory.fail_split = Some(SplitFailure::Demote); }
        let changed = a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0,
            arg0: 2, arg1: base + PAGE_SIZE, arg2: PAGE_SIZE, arg3: 3 });
        if failed_split {
            assert!(changed.is_err());
            assert_eq!(a.memory.large.len(), 1);
            KERNEL.with_borrow(|k| { assert!(k.ptes.is_empty()); });
            a.memory.fail_split = None;
            a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0,
                arg0: 2, arg1: base, arg2: 512 * PAGE_SIZE, arg3: 3 }).unwrap();
        } else {
            assert!(changed.is_ok());
            assert!(a.memory.large.is_empty());
            KERNEL.with_borrow(|k| { assert_eq!(k.ptes.len(), 1); });
            for offset in 0..512 {
                assert_eq!(a.processes.vm_space(2).unwrap().permissions(base + offset * PAGE_SIZE), if offset == 1 { 3 } else { 0 });
            }
        }
        release_range(&mut a, base, 512 * PAGE_SIZE).unwrap();
    }
}

#[test]
fn a_mixed_huge_permission_range_keeps_whole_leaves_with_distinct_old_permissions() {
    let mut a = huge_alpha();
    let base = 0x400000;
    let pages = 1025;
    a.map_process_initialized_pages_protected(2, Some(base), pages, None, 3).unwrap();
    a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0,
        arg0: 2, arg1: base, arg2: 512 * PAGE_SIZE, arg3: 1 }).unwrap();
    let before = KERNEL.with_borrow(|k| (k.map_attempts, k.unmaps));
    a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0,
        arg0: 2, arg1: base, arg2: pages * PAGE_SIZE, arg3: 5 }).unwrap();
    assert_eq!(a.memory.demotions, 0);
    KERNEL.with_borrow(|k| {
        assert_eq!(k.huge.len(), 2);
        assert_eq!(k.ptes.len(), 3);
        assert_eq!((k.map_attempts - before.0, k.unmaps - before.1), (3, 3));
    });
    for offset in 0..pages { assert_eq!(a.processes.vm_space(2).unwrap().permissions(base + offset * PAGE_SIZE), 5); }
    release_range(&mut a, base, pages * PAGE_SIZE).unwrap();
}

#[test]
fn mixed_protection_repairs_the_partial_leaf_and_rolls_back_whole_leaves_and_tail() {
    for fail_after_map in [false, true] {
        let mut a = huge_alpha();
        let base = 0x400000;
        let pages = 1537;
        let bytes = pages * PAGE_SIZE;
        a.map_process_initialized_pages_protected(2, Some(base), pages, None, 3).unwrap();
        a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0,
            arg0: 2, arg1: base + 512 * PAGE_SIZE, arg2: 512 * PAGE_SIZE, arg3: 1 }).unwrap();
        fill_private_fork_pattern(&mut a, base, pages);
        let original = transfer_bytes(&a, 2, base, bytes);
        // The small tail and two whole leaves change before the first split PTE.
        KERNEL.with_borrow_mut(|k| { k.fail_map_once = Some((k.map_attempts + 4, fail_after_map)); });
        let request = || OsRequestEvent { identifier: 3, code: 0,
            arg0: 2, arg1: base + PAGE_SIZE, arg2: bytes - PAGE_SIZE, arg3: 5 };
        assert_eq!(a.handle_mapping_protect_request(request()), Err(CapabilityError::Fatal));
        let space = a.processes.memory_context(2).unwrap().address_space;
        KERNEL.with_borrow(|k| {
            assert_eq!(k.huge.len(), 3);
            assert_eq!(k.ptes.len(), 4);
            assert_eq!(k.ptes[&(space, base + 512 * PAGE_SIZE)].1, 1);
            assert_eq!(k.ptes[&(space, base + 1024 * PAGE_SIZE)].1, 3);
            assert_eq!(k.ptes[&(space, base + 1536 * PAGE_SIZE)].1, 3);
        });
        let vm = a.processes.vm_space(2).unwrap();
        for page in 0..pages {
            let va = base + page * PAGE_SIZE;
            assert_eq!(vm.permissions(va), if (512..1024).contains(&page) { 1 } else { 3 });
            assert!(!vm.mapping_is_uncertain(va));
        }
        a.handle_mapping_protect_request(request()).unwrap();
        KERNEL.with_borrow(|k| {
            assert_eq!(k.huge.len(), 2);
            assert_eq!(k.ptes.len(), 515);
            assert_eq!(k.ptes[&(space, base)].1, 3);
            assert_eq!(k.ptes[&(space, base + PAGE_SIZE)].1, 5);
            assert_eq!(k.ptes[&(space, base + 1536 * PAGE_SIZE)].1, 5);
        });
        assert_eq!(transfer_bytes(&a, 2, base, bytes), original);
        release_range(&mut a, base, bytes).unwrap();
    }
}

#[test]
fn eager_private_fork_keeps_parent_huge_frames_but_shared_runs_still_split() {
    for cow in [false, true] { for permissions in 0..8 { for pages in [512, 1024] {
        let mut a = huge_alpha();
        a.processes.seal_capabilities(1).unwrap();
        let base = 0x400000;
        a.map_process_initialized_pages_protected(2, Some(base), pages, None, 3).unwrap();
        fill_private_fork_pattern(&mut a, base, pages);
        let expected = transfer_bytes(&a, 2, base, pages * PAGE_SIZE);
        a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0,
            arg0: 2, arg1: base, arg2: pages * PAGE_SIZE, arg3: permissions }).unwrap();
        // Other readonly pages still share; preserving a copied huge range
        // must not depend on the entire address space being copy-only.
        a.map_process_initialized_pages_protected(2, Some(0x900000), 1, None, 1).unwrap();
        let keys: Vec<_> = (0..pages).map(|i| a.processes.vm_space(2).unwrap().find_frame(base + i * PAGE_SIZE)).collect();
        let preserve = matches!(permissions, 0 | 2 | 6) || (!cow && permissions & 2 != 0);
        a.clone_process_vm_private(2, 1, cow).unwrap();
        let space = a.processes.memory_context(2).unwrap().address_space;
        assert_eq!(a.memory.demotions, if preserve { 0 } else { pages / 512 });
        assert_eq!(a.memory.private_large_protection(space, base).is_some(), preserve);
        assert_eq!(transfer_bytes(&a, 1, base, pages * PAGE_SIZE), expected);
        assert_eq!(transfer_bytes(&a, 2, base, pages * PAGE_SIZE), expected);
        assert!(a.processes.vm_space(1).unwrap().private_page_shared(0x900000));
        if preserve {
            for (i, key) in keys.into_iter().enumerate() {
                assert_eq!(a.processes.vm_space(2).unwrap().find_frame(base + i * PAGE_SIZE), key);
            }
            for pid in [1, 2] {
                a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0,
                    arg0: pid, arg1: base, arg2: pages * PAGE_SIZE, arg3: 3 }).unwrap();
                let owner = a.processes.memory_context(pid).unwrap().address_space;
                assert!(a.memory.private_large_protection(owner, base).is_some());
            }
            let frame = a.processes.vm_space(1).unwrap().find_frame(base).unwrap();
            let page = a.memory.frames[&frame];
            a.memory.pages.get_mut(&page).unwrap()[0] = 0xaa;
            assert_eq!(transfer_bytes(&a, 1, base, 1), [0xaa]);
            assert_eq!(transfer_bytes(&a, 2, base, 1), [0]);
        }
    }}}
}

fn fill_private_fork_pattern(a: &mut Alpha, base: usize, pages: usize) {
    for offset in 0..pages {
        let frame = a.processes.vm_space(2).unwrap().find_frame(base + offset * PAGE_SIZE).unwrap();
        let page = a.memory.frames[&frame];
        for (index, byte) in a.memory.pages.get_mut(&page).unwrap().iter_mut().enumerate() {
            *byte = ((offset * PAGE_SIZE + index) % 239) as u8;
        }
    }
}

#[test]
fn failed_eager_fork_copy_preserves_parent_huge_bytes_rights_and_capability_identity() {
    for rights in [0, 2, 6] {
        let mut a = huge_alpha();
        a.processes.seal_capabilities(1).unwrap();
        let base = 0x400000;
        a.map_process_initialized_pages_protected(2, Some(base), 512, None, 3).unwrap();
        fill_private_fork_pattern(&mut a, base, 512);
        a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0,
            arg0: 2, arg1: base, arg2: 512 * PAGE_SIZE, arg3: rights }).unwrap();
        let frame = a.processes.vm_space(2).unwrap().find_frame(base).unwrap();
        let original = transfer_bytes(&a, 2, base, 512 * PAGE_SIZE);
        a.fail_copy = Some(5);
        let before = KERNEL.with_borrow(|k| (k.map_attempts, k.unmaps));
        assert_eq!(a.clone_process_vm_private_cow(2, 1), Err(CapabilityError::Fatal));
        assert_eq!(KERNEL.with_borrow(|k| (k.map_attempts - before.0, k.unmaps - before.1)), (0, 0));
        assert_eq!(a.memory.demotions, 0);
        assert_eq!(a.processes.vm_space(2).unwrap().find_frame(base), Some(frame));
        assert_eq!(transfer_bytes(&a, 2, base, 512 * PAGE_SIZE), original);
        let space = a.processes.memory_context(2).unwrap().address_space;
        assert_eq!(a.memory.private_large_protection(space, base).unwrap().permissions, rights);
        assert!(a.processes.physical_pages_in_range(1, base, base + (1 << 21)).is_empty());
    }
}

#[test]
fn eager_fork_does_not_skip_quarantined_or_unproven_large_mapping_cleanup() {
    for invalid in 0..5 {
        let mut a = huge_alpha();
        let base = 0x400000;
        a.map_process_initialized_pages_protected(2, Some(base), 512, None, 3).unwrap();
        a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0,
            arg0: 2, arg1: base, arg2: 512 * PAGE_SIZE, arg3: 0 }).unwrap();
        match invalid {
            0 => a.processes.vm_space_mut(2).unwrap().mark_mapping_uncertain(base),
            1 => a.processes.vm_space_mut(2).unwrap().mark_mapping_deferred(base),
            2 => a.processes.vm_space_mut(2).unwrap().mark_private_page_shared(base, true),
            3 => a.processes.vm_space_mut(2).unwrap().set_permissions(base, 1),
            _ => a.memory.large.values_mut().next().unwrap().quarantined = true,
        }
        let result = a.split_private_large_mappings_for_fork(2, true);
        if invalid == 4 {
            assert_eq!(result, Err(CapabilityError::Fatal));
            assert_eq!(a.memory.demotions, 0);
            assert_eq!(a.processes.physical_pages_in_range(2, base, base + (1 << 21)).len(), 512);
        } else {
            result.unwrap();
            assert_eq!(a.memory.demotions, 1);
        }
    }
}

#[test]
fn a_zero_rights_huge_range_can_fork_and_release_without_exposing_or_losing_backing() {
    for pages in [1, 16, 512, 1024] { for partial in [false, true] {
    let mut a = huge_alpha();
    a.processes.seal_capabilities(1).unwrap();
    let base = 0x400000;
    a.map_process_initialized_pages_protected(2, Some(base), pages, None, 3).unwrap();
    for offset in 0..pages {
        let frame = a.processes.vm_space(2).unwrap().find_frame(base + offset * PAGE_SIZE).unwrap();
        let page = a.memory.frames[&frame];
        for (index, byte) in a.memory.pages.get_mut(&page).unwrap().iter_mut().enumerate() {
            *byte = ((offset * PAGE_SIZE + index) % 239) as u8;
        }
    }
    let expected = transfer_bytes(&a, 2, base, pages * PAGE_SIZE);
    a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0,
        arg0: 2, arg1: base, arg2: pages * PAGE_SIZE, arg3: 0 }).unwrap();
    let before = KERNEL.with_borrow(|k| (k.map_attempts, k.unmaps));
    a.clone_process_vm_private_cow(2, 1).unwrap();
    assert_eq!(KERNEL.with_borrow(|k| (k.map_attempts - before.0, k.unmaps - before.1)), (0, 0));
    for pid in [1, 2] {
        for offset in 0..pages { assert_eq!(a.processes.vm_space(pid).unwrap().permissions(base + offset * PAGE_SIZE), 0); }
        assert_eq!(transfer_bytes(&a, pid, base, pages * PAGE_SIZE), expected);
    }
    a.memory.table_preparations.clear();
    a.handle_mapping_protect_request(OsRequestEvent { identifier: 3, code: 0,
        arg0: 1, arg1: base, arg2: if partial { PAGE_SIZE } else { pages * PAGE_SIZE }, arg3: 3 }).unwrap();
    if partial || pages < 512 {
        assert!(a.memory.table_preparations.contains(&base), "pages={pages} partial={partial}");
        assert!(a.processes.vm_space(1).unwrap().page_tables_ready(base));
    } else {
        let space = a.processes.memory_context(1).unwrap().address_space;
        KERNEL.with_borrow(|k| assert_eq!(k.huge.iter().filter(|&&(s, _)| s == space).count(), pages / 512));
        assert!(a.memory.table_preparations.is_empty());
    }
    let frame = a.processes.vm_space(1).unwrap().find_frame(base).unwrap();
    let page = a.memory.frames[&frame];
    a.memory.pages.get_mut(&page).unwrap()[0] = 0x5a;
    assert_eq!(transfer_bytes(&a, 1, base, 1), [0x5a]);
    assert_eq!(transfer_bytes(&a, 2, base, 1), [0]);
    release_range(&mut a, base, pages * PAGE_SIZE).unwrap();
    a.handle_mapping_release_request(OsRequestEvent { identifier: 3, code: 0,
        arg0: base, arg1: pages * PAGE_SIZE, arg2: 1, arg3: 0 }).unwrap();
    }}
}

#[test]
fn inaccessible_clone_copy_failure_reclaims_owned_prefix_without_guest_maps() {
    for pages in [16, 512] {
        let mut a = huge_alpha();
        let source = a.source(pages * PAGE_SIZE);
        let original = transfer_bytes(&a, 1, source.address, source.bytes);
        a.fail_copy = Some(5);
        let before = KERNEL.with_borrow(|k| (k.map_attempts, k.unmaps));
        assert_eq!(a.map_process_cloned_unmapped_pages(2, 0x800000, pages, source), Err(CapabilityError::Fatal));
        assert_eq!(KERNEL.with_borrow(|k| (k.map_attempts - before.0, k.unmaps - before.1)), (0, 0));
        assert!(a.processes.physical_pages_in_range(2, 0x800000, 0x800000 + pages * PAGE_SIZE).is_empty());
        assert_eq!(transfer_bytes(&a, 1, source.address, source.bytes), original);
    }
}

#[test]
fn inaccessible_clone_rejects_unsealed_or_invalid_targets_and_missing_backing() {
    let mut a = Alpha::new();
    let source = a.source(PAGE_SIZE);
    assert_eq!(a.map_process_cloned_unmapped_pages(2, 0x800000, 1, source), Err(CapabilityError::InvalidArgument));
    a.processes.seal_capabilities(2).unwrap();
    assert_eq!(a.map_process_initialized_pages_protected(2, Some(0x800000), 1, Some(source), 0), Err(CapabilityError::InvalidArgument));
    for pid in [0, 999] {
        assert_eq!(a.map_process_cloned_unmapped_pages(pid, 0x800000, 1, source), Err(CapabilityError::InvalidArgument));
    }
    let missing = PageSource { pid: 1, address: 0x900000, bytes: PAGE_SIZE };
    assert!(a.map_process_cloned_unmapped_pages(2, 0x800000, 1, missing).is_err());
    assert!(a.processes.physical_pages_in_range(2, 0x800000, 0x801000).is_empty());
}

#[test]
fn huge_initializer_reuses_one_pinned_output_and_walks_source_boundaries_once() {
    let bytes = 512 * PAGE_SIZE;
    for (skew, source_bytes) in [(0, bytes), (37, bytes), (37, bytes - PAGE_SIZE - 23)] {
        let mut a = huge_alpha();
        let source = a.source(skew + source_bytes);
        let source = PageSource { address: source.address + skew, bytes: source_bytes, ..source };
        let base = 0x400000;
        a.map_process_initialized_pages_protected(2, Some(base), 512, Some(source), 1).unwrap();
        assert_eq!(a.initialization_maps, 1);
        assert_eq!(a.copy_maps, (skew + source_bytes).div_ceil(PAGE_SIZE));
        let pin = a.pinned_copies[0].expect("large output remains pinned even without role swapping");
        assert!(a.pinned_copies.iter().all(|address| *address == Some(pin)));
        let result = transfer_bytes(&a, 2, base, bytes);
        assert!(result[..source_bytes].iter().enumerate()
            .all(|(index, &byte)| byte == ((index + skew) % 239) as u8));
        assert!(result[source_bytes..].iter().all(|&byte| byte == 0));
        KERNEL.with_borrow(|k| {
            assert_eq!(k.huge.len(), 1);
            assert_eq!(k.map_attempts, 1);
        });
    }
}

#[test]
fn huge_private_construction_initializes_all_bytes_and_maps_only_one_large_leaf() {
    for prefix in [0,1] {for source_bytes in [0,512*PAGE_SIZE+17] {
        let mut a=huge_alpha();
        let source=(source_bytes!=0).then(||a.source(source_bytes));
        let base=0x400000-prefix*PAGE_SIZE;
        let count=512+prefix+1;
        a.map_process_initialized_pages_protected(2,Some(base),count,source,5).unwrap();
        let expected:Vec<_>=(0..count*PAGE_SIZE).map(|i|if i<source_bytes {(i%239) as u8} else {0}).collect();
        assert_eq!(transfer_bytes(&a,2,base,count*PAGE_SIZE),expected);
        let vm=a.processes.vm_space(2).unwrap();
        for i in 0..count {
            let va=base+i*PAGE_SIZE;
            assert_eq!(vm.permissions(va),5);
            assert!(!vm.mapping_is_uncertain(va));
            let owned=a.processes.find_active_physical_allocation_reference(2,va,1).unwrap();
            assert_eq!(a.processes.physical_reference_count(owned),1);
            assert!(!owned.owns_frame_cap);
        }
        let space=a.processes.memory_context(2).unwrap().address_space;
        KERNEL.with_borrow(|k| {
            assert!(k.huge.contains(&(space,0x400000)));
            assert_eq!(k.ptes.keys().filter(|&&(s,_)|s==space).count(),prefix+2);
            assert_eq!(k.cap_copies,0);
        });
    }}
}

#[test]
fn huge_private_unavailable_ram_or_existing_small_table_keeps_four_k_fallback() {
    for enabled in [false,true] {
        let mut a=huge_alpha();a.memory.large_enabled=enabled;
        a.memory.existing_small_table=true;
        a.map_process_initialized_pages_protected(2,Some(0x400000),512,None,3).unwrap();
        assert!(a.memory.large.is_empty());
        assert_eq!(a.memory.published.len(),512);
        assert!(transfer_bytes(&a,2,0x400000,1<<21).iter().all(|&b|b==0));
    }
}

#[test]
fn huge_private_uses_later_intervals_after_blocked_prefix_and_middle_tables() {
    let large = 1 << 21;
    for blocked in [vec![0], vec![large], vec![0, 2 * large]] {
        let mut a = huge_alpha();
        let base = 0x400000 - PAGE_SIZE;
        let first = base + PAGE_SIZE;
        let pages = 4 * 512 + 2;
        let source = a.source(3 * large + 17);
        for offset in &blocked {
            a.memory.blocked_large_bases.insert(first + offset);
        }
        a.map_process_initialized_pages_protected(2, Some(base), pages, Some(source), 5).unwrap();
        let actual = transfer_bytes(&a, 2, base, pages * PAGE_SIZE);
        for (i, byte) in actual.iter().enumerate() {
            assert_eq!(*byte, if i < source.bytes { (i % 239) as u8 } else { 0 });
        }
        let space = a.processes.memory_context(2).unwrap().address_space;
        KERNEL.with_borrow(|k| {
            for offset in (0..4 * large).step_by(large) {
                assert_eq!(k.huge.contains(&(space, first + offset)), !blocked.contains(&offset));
            }
            assert_eq!(k.ptes.keys().filter(|&&(s, _)| s == space).count(),
                2 + blocked.len() * 512 + 4 - blocked.len());
        });
        for page in 0..pages {
            let va = base + page * PAGE_SIZE;
            let vm = a.processes.vm_space(2).unwrap();
            assert_eq!(vm.permissions(va), 5);
            assert!(!vm.mapping_is_uncertain(va));
            let allocation = a.processes.find_active_physical_allocation_reference(2, va, 1).unwrap();
            assert_eq!(a.processes.physical_reference_count(allocation), 1);
        }
        release_range(&mut a, base, pages * PAGE_SIZE).unwrap();
        assert!(a.processes.physical_pages_in_range(2, base, base + pages * PAGE_SIZE).is_empty());
        KERNEL.with_borrow(|k| assert!(!k.ptes.keys().any(|&(s, va)| s == space && va >= base && va < base + pages * PAGE_SIZE)));
    }
}

#[test]
fn deferred_private_constructor_skips_blocked_table_without_losing_pending_prefix_or_tail() {
    let mut a = huge_alpha();
    let base = 0x400000;
    a.memory.blocked_large_bases.insert(base);
    a.map_process_initialized_pages_deferred(2, base, 1025, None, 16 * PAGE_SIZE).unwrap();
    let space = a.processes.memory_context(2).unwrap().address_space;
    KERNEL.with_borrow(|k| {
        assert!(k.huge.contains(&(space, base + (1 << 21))));
        assert_eq!(k.ptes.keys().filter(|&&(s, _)| s == space).count(), 17);
    });
    for page in 0..1025 {
        let va = base + page * PAGE_SIZE;
        let vm = a.processes.vm_space(2).unwrap();
        assert_eq!(vm.mapping_deferred(va), (16..512).contains(&page) || page == 1024);
        assert_eq!(vm.permissions(va), 1);
    }
    assert_eq!(transfer_bytes(&a, 2, base, 1025 * PAGE_SIZE), vec![0; 1025 * PAGE_SIZE]);
    release_range(&mut a, base, 1025 * PAGE_SIZE).unwrap();
}

#[test]
fn huge_private_initialization_failure_after_skipped_table_reclaims_or_retains_owned_prefix() {
    for fail_unmap in [false, true] {
        let mut a = huge_alpha();
        let base = 0x400000;
        a.memory.blocked_large_bases.insert(base + (1 << 21));
        let source = a.source(6 * 1024 * 1024);
        let original = transfer_bytes(&a, 1, source.address, source.bytes);
        // Initialize one huge leaf and the intervening small interval, then
        // fail while constructing the next huge leaf.
        a.fail_copy = Some(1025);
        if fail_unmap {
            KERNEL.with_borrow_mut(|k| k.fail_unmap = Some(UnmapFailure::Before));
        }
        assert_eq!(a.map_process_initialized_pages_protected(2, Some(base), 1536, Some(source), 3),
            Err(CapabilityError::Fatal));
        if fail_unmap {
            // A failed huge UNMAP retains the memory manager's mapping and
            // every physical reference; it has not confirmed absence in VM.
            assert!(a.memory.large.values().any(|record| record.present));
            assert!(a.processes.find_active_physical_allocation_reference(2, base, 1).is_some());
            KERNEL.with_borrow_mut(|k| k.fail_unmap = None);
            a.handle_mapping_release_request(OsRequestEvent { identifier: 3, code: 0,
                arg0: base, arg1: 1536 * PAGE_SIZE, arg2: 2,
                arg3: nanami_uapi::os::OS_MAPPING_RELEASE_ALLOW_HOLES }).unwrap();
        }
        assert!(a.processes.physical_pages_in_range(2, base, base + 1536 * PAGE_SIZE).is_empty());
        assert_eq!(transfer_bytes(&a, 1, source.address, source.bytes), original);
    }
}

#[test]
fn huge_private_partial_protect_keeps_neighbor_bytes_rights_and_original_ram() {
    let mut a=huge_alpha();let source=a.source(1<<21);
    a.map_process_initialized_pages_protected(2,Some(0x400000),512,Some(source),3).unwrap();
    let before=transfer_bytes(&a,2,0x400000,1<<21);
    let owned=a.processes.physical_pages_in_range(2,0x400000,0x600000);
    a.handle_mapping_protect_request(stack_protection_request(0x407000,1)).unwrap();
    assert_eq!(transfer_bytes(&a,2,0x400000,1<<21),before);
    assert!(a.memory.large.is_empty());
    let after=a.processes.physical_pages_in_range(2,0x400000,0x600000);
    assert_eq!(owned.iter().map(|a|a.base_page).collect::<Vec<_>>(),after.iter().map(|a|a.base_page).collect::<Vec<_>>());
    let space=a.processes.memory_context(2).unwrap().address_space;
    KERNEL.with_borrow(|k|for i in 0..512 {
        let va=0x400000+i*PAGE_SIZE;
        assert_eq!(k.ptes[&(space,va)].1,if i==7 {1} else {3});
        assert_eq!(a.processes.vm_space(2).unwrap().permissions(va),if i==7 {1} else {3});
    });
}

#[test]
fn huge_private_partial_release_can_remap_a_zero_page_and_full_release_skips_small_maps() {
    for partial in [false,true] {
        let mut a=huge_alpha();let source=a.source(1<<21);
        a.map_process_initialized_pages_protected(2,Some(0x400000),512,Some(source),3).unwrap();
        if partial {
            release_range(&mut a,0x4ff000,PAGE_SIZE).unwrap();
            assert!(a.processes.vm_space(2).unwrap().find_frame(0x4ff000).is_none());
            a.map_process_initialized_pages_protected(2,Some(0x4ff000),1,None,3).unwrap();
            let mut expected:Vec<_>=(0..1<<21).map(|i|(i%239) as u8).collect();
            expected[0xff000..0x100000].fill(0);
            assert_eq!(transfer_bytes(&a,2,0x400000,1<<21),expected);
        } else {
            release_range(&mut a,0x400000,1<<21).unwrap();
            assert!(a.memory.published.is_empty());
            KERNEL.with_borrow(|k| {assert_eq!(k.unmaps,1);assert!(k.ptes.is_empty());});
            assert!(a.processes.physical_pages_in_range(2,0x400000,0x600000).is_empty());
        }
        assert!(a.memory.large.is_empty());
    }
}

#[test]
fn huge_private_split_failures_restore_original_data_rights_and_authority() {
    for stage in 0..3 {
        let mut a=huge_alpha();let source=a.source(1<<21);
        a.map_process_initialized_pages_protected(2,Some(0x400000),512,Some(source),3).unwrap();
        let before=transfer_bytes(&a,2,0x400000,1<<21);
        match stage {
            0=>a.memory.fail_split=Some(SplitFailure::Demote),
            1=>a.memory.fail_split=Some(SplitFailure::Table),
            _=>{a.memory.fail_map=Some(3);a.memory.fail_after_map=true;},
        }
        assert_eq!(a.handle_mapping_protect_request(stack_protection_request(0x401000,1)),Err(CapabilityError::Fatal));
        assert_eq!(transfer_bytes(&a,2,0x400000,1<<21),before);
        let vm=a.processes.vm_space(2).unwrap();
        for i in 0..512 {
            assert_eq!(vm.permissions(0x400000+i*PAGE_SIZE),3);
            assert!(!vm.mapping_is_uncertain(0x400000+i*PAGE_SIZE));
            assert_ne!(vm.find_frame(0x400000+i*PAGE_SIZE).unwrap()&SLICE_KEY,0);
        }
        assert_eq!(a.memory.freed_pages,0);
        let space=a.processes.memory_context(2).unwrap().address_space;
        KERNEL.with_borrow(|k| {assert_eq!(k.ptes.len(),1);assert_eq!(k.ptes[&(space,0x400000)].1,3);});
    }
}

#[test]
fn huge_private_failed_restore_quarantines_every_slice_and_keeps_all_ownership() {
    let mut a=huge_alpha();
    a.map_process_initialized_pages_protected(2,Some(0x400000),512,None,3).unwrap();
    a.memory.fail_split=Some(SplitFailure::Table);a.memory.fail_restore=true;
    assert_eq!(a.handle_mapping_protect_request(stack_protection_request(0x401000,1)),Err(CapabilityError::Fatal));
    for i in 0..512 {assert!(a.processes.vm_space(2).unwrap().mapping_is_uncertain(0x400000+i*PAGE_SIZE));}
    assert_eq!(a.processes.physical_pages_in_range(2,0x400000,0x600000).len(),512);
    assert_eq!(release_range(&mut a,0x400000,1<<21),Err(CapabilityError::Fatal));
    assert_eq!(a.memory.freed_pages,0);
}

#[test]
fn huge_private_fork_splits_before_cow_sharing_and_child_detach_keeps_parent_bytes() {
    let mut a=huge_alpha();a.processes.seal_capabilities(1).unwrap();
    a.map_process_initialized_pages_protected(2,Some(0x400000),512,None,3).unwrap();
    a.clone_process_vm_private_cow(2,1).unwrap();
    assert!(a.memory.large.is_empty());
    let original=a.processes.vm_space(2).unwrap().find_frame(0x401000).unwrap();
    assert_eq!(a.processes.vm_space(1).unwrap().find_frame(0x401000),Some(original));
    assert!(a.processes.vm_space(1).unwrap().private_page_cow_write(0x401000));
    assert_eq!(a.handle_process_private_write(OsRequestEvent {identifier:3,code:0,arg0:1,
        arg1:0x401003,arg2:1,arg3:nanami_uapi::os::OS_PRIVATE_WRITE_DETACH}),Ok(1));
    let replacement=a.processes.vm_space(1).unwrap().find_frame(0x401000).unwrap();
    assert_ne!(original,replacement);
    let replacement_page=a.memory.frames[&replacement];
    a.memory.pages.get_mut(&replacement_page).unwrap()[3]=0x7b;
    assert_eq!(transfer_bytes(&a,2,0x401000,PAGE_SIZE),vec![0;PAGE_SIZE]);
    assert_eq!(transfer_bytes(&a,1,0x401000,PAGE_SIZE)[3],0x7b);
}

#[test]
fn huge_private_clone_vm_peer_resolves_same_slices_and_can_split_shared_space() {
    let mut a=huge_alpha();
    a.map_process_initialized_pages_protected(2,Some(0x400000),512,None,3).unwrap();
    a.processes.share_address_space(1,2).unwrap();
    assert_eq!(a.process_frame_for_page(1,0x401000),a.process_frame_for_page(2,0x401000));
    a.handle_mapping_protect_request(OsRequestEvent {identifier:3,code:0,arg0:1,
        arg1:0x401000,arg2:PAGE_SIZE,arg3:1}).unwrap();
    assert!(a.memory.large.is_empty());
    assert_eq!(a.processes.vm_space(2).unwrap().permissions(0x401000),1);
    assert_eq!(a.processes.vm_space(1).unwrap().permissions(0x402000),3);
}

fn pooled_cached_source(a: &mut Alpha, bytes: usize) -> PageSource {
    let source = a.source(bytes);
    for index in 0..bytes.div_ceil(PAGE_SIZE) {
        a.initialization_sources.insert(index + 1);
        a.processes.register_pool_allocation(source.pid, source.address + index * PAGE_SIZE,
            index, index + 1, 1).unwrap();
    }
    source
}

fn original_prepared_fill(a: &mut Alpha, physical: usize, source: memory_pages::PageSource,
    elf: &elf_loader::ElfImage, va: usize) -> Result<(), CapabilityError> {
    let temp = a.map_initialization_page(physical)?;
    let output = unsafe { core::slice::from_raw_parts_mut(temp as *mut u8, PAGE_SIZE) };
    fill_image_page(elf, source.bytes, va, output, |offset, output| {
        let address = source.address.checked_add(offset).ok_or(CapabilityError::InvalidArgument)?;
        a.copy_process_memory_into(source.pid, address, output)
    })
}

fn prepared_fill(a: &mut Alpha, physical: usize, source: PageSource,
    elf: &elf_loader::ElfImage, va: usize, aliases: Option<&memory_image::ImageSourceAliases>)
    -> Result<(), CapabilityError> {
    a.initialize_exec_image_page_content(physical,
        &ProcessLazyMappingKind::Image { image: &[], elf: *elf }, va, Some(source), aliases)
}

fn simple_elf(bytes: usize) -> elf_loader::ElfImage {
    let mut elf = elf_loader::ElfImage::empty();
    elf.segment_count = 1;
    elf.segments[0] = elf_loader::LoadSegment { offset: 0, virtual_address: 0x80000,
        file_size: bytes, memory_size: bytes.div_ceil(PAGE_SIZE) * PAGE_SIZE };
    elf
}

fn transfer_bytes(a: &Alpha, pid: usize, base: usize, bytes: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes);
    for offset in (0..bytes).step_by(PAGE_SIZE) {
        let frame = a.processes.vm_space(pid).unwrap().find_frame(base + offset).unwrap();
        let page = a.memory.frames[&frame];
        output.extend_from_slice(&a.memory.pages[&page][..(bytes - offset).min(PAGE_SIZE)]);
    }
    output
}

#[test]
fn unaligned_transfer_resolves_each_page_without_repeated_frame_searches() {
    let mut a = Alpha::new();
    let source = a.source(65 * PAGE_SIZE);
    a.map_process_initialized_pages_protected(2, Some(0x400000), 65, None, 3).unwrap();
    let bytes = 63 * PAGE_SIZE + 101;
    a.lookups = 0;
    a.copy_process_memory(source.pid, source.address + 13, 2, 0x400000 + 53, bytes).unwrap();
    let lookups = a.lookups;
    let mut expected = vec![0; 65 * PAGE_SIZE];
    expected[53..53 + bytes].copy_from_slice(&transfer_bytes(&a, source.pid, source.address, 65 * PAGE_SIZE)[13..13 + bytes]);
    assert_eq!(transfer_bytes(&a, 2, 0x400000, 65 * PAGE_SIZE), expected);
    println!("UNALIGNED-TRANSFER bytes={bytes} frame_lookups={lookups}");
    assert_eq!(lookups, 128);
}

#[test]
fn transfer_cursors_preserve_memmove_for_both_directions_and_page_offsets() {
    for source_offset in [0, 1, 13, PAGE_SIZE - 1, PAGE_SIZE + 17] {
        for destination_offset in [0, 1, 53, PAGE_SIZE - 1, PAGE_SIZE + 37] {
            for bytes in [0, 1, 37, PAGE_SIZE - 3, PAGE_SIZE + 53, 3 * PAGE_SIZE + 101] {
                let mut a = Alpha::new();
                let source = a.source(8 * PAGE_SIZE);
                let mut expected = transfer_bytes(&a, 1, source.address, 8 * PAGE_SIZE);
                expected.copy_within(source_offset..source_offset + bytes, destination_offset);
                a.copy_process_memory(1, source.address + source_offset, 1, source.address + destination_offset, bytes).unwrap();
                assert_eq!(transfer_bytes(&a, 1, source.address, 8 * PAGE_SIZE), expected,
                    "{source_offset}/{destination_offset}/{bytes}");
            }
        }
    }
}

#[test]
fn transfer_alias_failures_keep_the_original_completed_chunk_prefix() {
    let bytes = 3 * PAGE_SIZE + 101;
    let mut ends = vec![bytes];
    for offset in [13, 53] {
        ends.extend((PAGE_SIZE - offset..bytes).step_by(PAGE_SIZE));
    }
    ends.sort_unstable();
    ends.dedup();
    for failure in 1..=2 * ends.len() {
        let mut a = Alpha::new();
        let source = a.source(4 * PAGE_SIZE);
        let input = transfer_bytes(&a, 1, source.address, 4 * PAGE_SIZE);
        a.map_process_initialized_pages_protected(2, Some(0x400000), 4, None, 3).unwrap();
        a.copy_maps = 0;
        a.fail_copy = Some(failure);
        assert_eq!(a.copy_process_memory(1, source.address + 13, 2, 0x400000 + 53, bytes), Err(CapabilityError::Fatal));
        assert_eq!(a.copy_maps, failure);
        let completed = (failure - 1) / 2;
        let copied = if completed == 0 { 0 } else { ends[completed - 1] };
        let mut expected = vec![0; 4 * PAGE_SIZE];
        expected[53..53 + copied].copy_from_slice(&input[13..13 + copied]);
        assert_eq!(transfer_bytes(&a, 2, 0x400000, 4 * PAGE_SIZE), expected, "failure={failure}");
    }
}

#[test]
fn transfer_cursors_recheck_each_new_page_and_reset_between_requests() {
    for private_destination in [false, true] {
        let mut a = Alpha::new();
        let source = a.source(4 * PAGE_SIZE);
        let input = transfer_bytes(&a, 1, source.address, 4 * PAGE_SIZE);
        a.map_process_initialized_pages_protected(2, Some(0x400000), 4, None, 3).unwrap();
        let expected_error = if private_destination {
            a.processes.vm_space_mut(2).unwrap().mark_private_page_shared(0x400000 + PAGE_SIZE, true);
            CapabilityError::IllegalOperation
        } else {
            a.processes.vm_space_mut(1).unwrap().forget_frame(source.address + PAGE_SIZE);
            CapabilityError::InvalidArgument
        };
        assert_eq!(a.copy_process_memory(1, source.address + 13, 2, 0x400000 + 53, 3 * PAGE_SIZE), Err(expected_error));
        let copied = PAGE_SIZE - if private_destination { 53 } else { 13 };
        let mut expected = vec![0; 4 * PAGE_SIZE];
        expected[53..53 + copied].copy_from_slice(&input[13..13 + copied]);
        assert_eq!(transfer_bytes(&a, 2, 0x400000, 4 * PAGE_SIZE), expected);
    }
    let mut a = Alpha::new();
    let source = a.source(PAGE_SIZE);
    a.map_process_initialized_pages_protected(2, Some(0x400000), 1, None, 3).unwrap();
    a.copy_process_memory(1, source.address, 2, 0x400000, 1000).unwrap();
    let fresh = a.memory.allocate_physical_any(PAGE_SIZE).unwrap();
    a.memory.pages.get_mut(&fresh).unwrap().fill(0x7d);
    a.processes.vm_space_mut(1).unwrap().record_frame(source.address, fresh + 100000).unwrap();
    a.copy_process_memory(1, source.address, 2, 0x400000, 1000).unwrap();
    assert!(transfer_bytes(&a, 2, 0x400000, 1000).iter().all(|&byte| byte == 0x7d));
    a.processes.vm_space_mut(2).unwrap().mark_private_page_shared(0x400000, true);
    assert_eq!(a.copy_process_memory(1, source.address, 2, 0x400000, 1000), Err(CapabilityError::IllegalOperation));
}

fn staged_lazy_image(a: &mut Alpha, image: &'static [u8]) -> (usize, usize, usize) {
    a.processes.ensure_vm_space_for_pid(77).unwrap();
    a.processes.register_lazy_mapping(77, 0x80000, 64, 0,
        ProcessLazyMappingKind::Image { image, elf: simple_elf(image.len()) }).unwrap();
    a.processes.preinstall_lazy_mapping_plan(77, 0x80000, 0x80000 + 64 * PAGE_SIZE).unwrap()
}

#[test]
fn staged_image_batch_keeps_bytes_permissions_and_page_ownership_with_fewer_allocations() {
    let image: &'static [u8] = Box::leak((0..64 * PAGE_SIZE - 37).map(|i| (i % 251) as u8).collect::<Vec<_>>().into_boxed_slice());
    for (batched, fragmented) in [(false, false), (true, false), (true, true)] {
        let mut a = Alpha::new();
        let plan = staged_lazy_image(&mut a, image);
        a.memory.fragmented = fragmented;
        let mut batch = if batched { a.prepare_preinstall_pool_batch(77, 177, 0x80000, 64).unwrap() } else { None };
        for i in 0..64 {
            let va = 0x80000 + i * PAGE_SIZE;
            let frame = a.materialize_lazy_page_from_plan(77, 177, 277, va, 5, true, plan, batch.as_mut()).unwrap();
            let allocation = a.processes.find_active_physical_allocation_reference(77, va, 1).unwrap();
            assert!(!allocation.owns_frame_cap);
            assert_eq!(a.processes.physical_reference_count(allocation), 1);
            assert_eq!(frame, allocation.base_page + 100000);
            assert_eq!(a.processes.vm_space(77).unwrap().permissions(va), 5);
            for (offset, &byte) in a.memory.pages[&allocation.base_page].iter().enumerate() {
                assert_eq!(byte, image.get(i * PAGE_SIZE + offset).copied().unwrap_or(0));
            }
        }
        if let Some(batch) = batch { a.release_preinstall_pool_batch_tail(batch).unwrap(); }
        let expected = if !batched { 64 } else if fragmented { 65 } else { 1 };
        assert_eq!(a.memory.allocation_requests.len(), expected);
        assert_eq!(a.memory.freed_pages, 0);
        println!("STAGED-IMAGE-BATCH batched={batched} fragmented={fragmented} allocator={expected}");
    }
}

#[test]
fn staged_image_map_failures_preserve_prefix_and_release_unregistered_tail() {
    let image: &'static [u8] = Box::leak(vec![0x37; 64 * PAGE_SIZE].into_boxed_slice());
    for fail_at in [0, 17, 63] {
        for after_map in [false, true] {
            let mut a = Alpha::new();
            let plan = staged_lazy_image(&mut a, image);
            let mut batch = a.prepare_preinstall_pool_batch(77, 177, 0x80000, 64).unwrap().unwrap();
            a.memory.fail_map = Some(fail_at);
            a.memory.fail_after_map = after_map;
            for i in 0..=fail_at {
                let result = a.materialize_lazy_page_from_plan(77, 177, 277, 0x80000 + i * PAGE_SIZE, 5, true, plan, Some(&mut batch));
                assert_eq!(result.is_err(), i == fail_at);
            }
            a.release_preinstall_pool_batch_tail(batch).unwrap();
            assert_eq!(a.memory.pages.len(), fail_at + 1);
            assert_eq!(a.memory.freed_pages, 63 - fail_at);
            KERNEL.with_borrow(|kernel| assert_eq!(kernel.ptes.len(), fail_at + usize::from(after_map)));
            for i in 0..64 {
                assert_eq!(a.processes.find_active_physical_allocation_reference(77, 0x80000 + i * PAGE_SIZE, 1).is_some(), i <= fail_at);
            }
        }
    }
}

#[test]
fn staged_image_batch_rejects_zero_plans_without_claiming_any_page() {
    for prepared_pool in [false, true] {
        let mut a = Alpha::new();
        a.processes.ensure_vm_space_for_pid(77).unwrap();
        a.processes.register_lazy_mapping(77, 0x80000, 64, 0, ProcessLazyMappingKind::Zero).unwrap();
        let plan = a.processes.preinstall_lazy_mapping_plan(77, 0x80000, 0xc0000).unwrap();
        let mut batch = a.prepare_preinstall_pool_batch(77, 177, 0x80000, 64).unwrap().unwrap();
        assert_eq!(a.materialize_lazy_page_from_plan(77, 177, 277, 0x80000, 7, prepared_pool, plan, Some(&mut batch)), Err(CapabilityError::InvalidArgument));
        a.release_preinstall_pool_batch_tail(batch).unwrap();
        assert_eq!(a.memory.freed_pages, 64);
        assert!(a.memory.published.is_empty());
        assert!(a.processes.find_active_physical_allocation_reference(77, 0x80000, 1).is_none());
    }
}

#[test]
fn prepared_pool_aliases_protect_later_source_pages_without_extra_initialization_maps() {
    for optimized in [false, true] {
        let mut a = Alpha::new();
        let source = pooled_cached_source(&mut a, 64 * PAGE_SIZE - 37);
        let elf = simple_elf(source.bytes);
        let aliases = if optimized { a.prepare_image_initialization_source(Some(source)) } else { None };
        // Destination slot 2 evicts FUTURE source page 2 before reading page 1
        // on the legacy path. Protect the whole source, not only this page.
        a.memory.next = 2049;
        for index in 0..64 {
            let physical = a.memory.allocate_physical_any(PAGE_SIZE).unwrap();
            if optimized {
                prepared_fill(&mut a, physical, source, &elf, 0x80000 + index * PAGE_SIZE, aliases.as_ref()).unwrap();
            } else {
                original_prepared_fill(&mut a, physical, source, &elf, 0x80000 + index * PAGE_SIZE).unwrap();
            }
            for (within, &byte) in a.memory.pages[&physical].iter().enumerate() {
                let offset = index * PAGE_SIZE + within;
                assert_eq!(byte, if offset < source.bytes { (offset % 239) as u8 } else { 0 });
            }
        }
        let maps = a.copy_maps + a.initialization_maps;
        eprintln!("prepared pool optimized={optimized} init={} copy={} total={} evictions={}",
            a.initialization_maps, a.copy_maps, maps, a.initialization_evictions);
        if optimized && swap_enabled() {
            assert_eq!(maps, 64);
            assert_eq!(a.initialization_maps, 1);
            assert_eq!(a.initialization_evictions, 0);
            assert!((1..=64).all(|page| a.initialization_sources.contains(&page)));
        } else {
            assert_eq!(maps, 127);
            assert_eq!(a.initialization_evictions, 63);
        }
    }
}

fn prepared_case(optimized: bool, first: usize, offset: usize, bytes: usize,
    overlay: bool, collision: bool, cold: bool, failure: Option<usize>)
    -> (Result<(), CapabilityError>, Vec<u8>) {
    let mut a = Alpha::new();
    let source = pooled_cached_source(&mut a, 3 * PAGE_SIZE);
    let mut elf = simple_elf(3 * PAGE_SIZE);
    elf.segment_count = if overlay { 2 } else { 1 };
    elf.segments[0] = elf_loader::LoadSegment { offset, virtual_address: 0x80000 + first,
        file_size: bytes, memory_size: bytes + PAGE_SIZE };
    if overlay {
        elf.segments[1] = elf_loader::LoadSegment { offset: PAGE_SIZE + 37,
            virtual_address: 0x80400, file_size: 512, memory_size: 1024 };
    }
    if (first + bytes) % 2 == 0 { elf.ipc_buffer_start = Some(0x80000); }
    if cold { a.initialization_sources.clear(); }
    if collision { a.memory.next = 2048; }
    let physical = a.memory.allocate_physical_any(PAGE_SIZE).unwrap();
    a.fail_copy = failure;
    let result = if optimized {
        let aliases = a.prepare_image_initialization_source(Some(source));
        prepared_fill(&mut a, physical, source, &elf, 0x80000, aliases.as_ref())
    } else {
        original_prepared_fill(&mut a, physical, source, &elf, 0x80000)
    };
    (result, a.memory.pages[&physical].to_vec())
}

#[test]
fn prepared_pool_fill_matches_legacy_overlay_tls_and_failure_prefixes() {
    let mut cases = 0;
    for first in [0, 1, 2048, 4095] {
        for offset in [0, 1, 4095, 8191] {
            for bytes in [0, 1, 2048, 4096] {
                for overlay in [false, true] {
                    for collision in [false, true] {
                        assert_eq!(prepared_case(true, first, offset, bytes, overlay, collision, false, None),
                            prepared_case(false, first, offset, bytes, overlay, collision, false, None));
                        cases += 1;
                    }
                }
            }
        }
    }
    for failure in [None, Some(1), Some(2), Some(3)] {
        assert_eq!(prepared_case(true, 0, 4095, 4096, true, false, true, failure),
            prepared_case(false, 0, 4095, 4096, true, false, true, failure));
    }
    for offset in [3 * PAGE_SIZE - 1, usize::MAX - 1] {
        assert_eq!(prepared_case(true, 0, offset, 4096, false, true, false, None),
            prepared_case(false, 0, offset, 4096, false, true, false, None));
    }
    eprintln!("prepared pool legacy cases={cases} plus 6 cold/error cases");
}

#[test]
fn prepared_pool_hints_never_supply_a_stale_frame_or_change_native_cap_fallback() {
    for pool in [false, true] {
        let mut a = Alpha::new();
        let source = if pool { pooled_cached_source(&mut a, PAGE_SIZE) }
            else { owned_cached_source(&mut a, PAGE_SIZE) };
        let aliases = a.prepare_image_initialization_source(Some(source));
        let extra = a.memory.allocate_physical_any(PAGE_SIZE).unwrap();
        a.memory.pages.get_mut(&extra).unwrap().fill(0x7d);
        let root = a.processes.memory_root_node(1).unwrap();
        let frame = process_frame_descriptor(root, 500);
        a.memory.frames.insert(frame, extra);
        a.replace_on_lookup = Some((1, 1, source.address, frame));
        a.memory.next = 2048;
        let physical = a.memory.allocate_physical_any(PAGE_SIZE).unwrap();
        prepared_fill(&mut a, physical, source, &simple_elf(PAGE_SIZE), 0x80000, aliases.as_ref()).unwrap();
        assert!(a.memory.pages[&physical].iter().all(|byte| *byte == 0x7d));
        assert!(a.pinned_copies.last().unwrap().is_some());
        if !pool { assert!(aliases.is_none()); }
    }
}

#[test]
fn prepared_pool_collision_pins_destination_when_an_earlier_source_piece_is_cold() {
    let mut a = Alpha::new();
    let mut source = pooled_cached_source(&mut a, 2 * PAGE_SIZE);
    source.address += PAGE_SIZE - 1;
    source.bytes = 2;
    a.initialization_sources.remove(&1);
    let aliases = a.prepare_image_initialization_source(Some(source));
    a.memory.next = 2049;
    let physical = a.memory.allocate_physical_any(PAGE_SIZE).unwrap();
    prepared_fill(&mut a, physical, source, &simple_elf(2), 0x80000, aliases.as_ref()).unwrap();
    let bytes = &a.memory.pages[&physical];
    assert_eq!(&bytes[..2], &[((PAGE_SIZE - 1) % 239) as u8, (PAGE_SIZE % 239) as u8]);
    assert!(bytes[2..].iter().all(|byte| *byte == 0));
    if swap_enabled() {
        assert!(a.initialization_sources.contains(&2));
        assert_eq!(a.pinned_copies.len(), 2);
        assert_eq!(a.pinned_copies[0], None);
        assert!(a.pinned_copies[1].is_some());
    }
}

#[test]
fn prepared_pool_mapping_failure_preserves_source_and_owned_destination() {
    let mut a = Alpha::new();
    let source = pooled_cached_source(&mut a, PAGE_SIZE);
    let aliases = a.prepare_image_initialization_source(Some(source));
    a.memory.next = 2048;
    let physical = a.memory.allocate_physical_any(PAGE_SIZE).unwrap();
    a.fail_copy = Some(1);
    assert_eq!(prepared_fill(&mut a, physical, source, &simple_elf(PAGE_SIZE), 0x80000, aliases.as_ref()),
        Err(CapabilityError::Fatal));
    assert!(a.memory.pages[&physical].iter().all(|byte| *byte == 0xcc));
    if swap_enabled() { assert!(a.initialization_sources.contains(&1)); }
}

fn stack_protection_request(va: usize, permissions: usize) -> OsRequestEvent {
    OsRequestEvent {identifier:3,code:0,arg0:2,arg1:va,arg2:PAGE_SIZE,arg3:permissions}
}

#[test]
fn protection_demand_stack_uses_original_slot_even_at_cursor_limit_and_publishes_zero() {
    let mut a=Alpha::new();
    a.processes.seal_capabilities(2).unwrap();
    a.processes.reset_runtime_memory_for_exec(2,PROCESS_FRAME_TOTAL_PAGES,0x100000,0x1000000).unwrap();
    let root=a.processes.memory_entry(2).unwrap().root_node;
    a.processes.register_fresh_stack_mapping(2,root,0x4000000,64,100).unwrap();
    for page in (32..64).rev().chain([1,0]) {
        let va=0x4000000+page*PAGE_SIZE;
        a.handle_mapping_protect_request(stack_protection_request(va,3)).unwrap();
        assert_eq!(a.processes.find_entry_by_pid(2).unwrap().next_frame_slot,PROCESS_FRAME_TOTAL_PAGES);
        assert_eq!(a.processes.find_active_physical_allocation_reference(2,va,1).unwrap().start_slot,100+page);
        assert_eq!(a.processes.vm_space_mut(2).unwrap().permissions(va),3);
        assert!(a.processes.take_fresh_stack_frame_slot(2,va,PROCESS_FRAME_TOTAL_PAGES).is_none());
    }
    assert_eq!(a.memory.published.len(),34);
    assert!(a.memory.published.iter().all(|(_,bytes)|bytes.iter().all(|byte|*byte==0)));
    let count=a.memory.published.len();
    a.handle_mapping_protect_request(stack_protection_request(0x4000000,3)).unwrap();
    assert_eq!(a.memory.published.len(),count);
}

#[test]
fn sealed_private_pages_skip_child_caps_and_nodes_but_native_pages_keep_them() {
    for sealed in [false, true] {
        let mut a = Alpha::new();
        if sealed { a.processes.seal_capabilities(2).unwrap(); }
        let base = a.map_process_initialized_pages_protected(2, None, 513, None, 3).unwrap();
        assert_eq!(a.memory.child_allocations, 0);
        assert_eq!(a.memory.allocation_requests, [513 * PAGE_SIZE]);
        KERNEL.with_borrow(|kernel| {
            assert_eq!(kernel.chunk_calls, if sealed { 0 } else { 1 });
            assert_eq!(kernel.cap_copies, if sealed { 0 } else { 513 });
        });
        for offset in 0..513 {
            let va = base + offset * PAGE_SIZE;
            let allocation = a.processes.find_active_physical_allocation_reference(2, va, 1).unwrap();
            assert_eq!(allocation.owns_frame_cap, !sealed);
            assert_eq!(a.processes.vm_space_mut(2).unwrap().permissions(va), 3);
            if sealed {
                assert_eq!(a.processes.vm_space_mut(2).unwrap().find_frame(va), Some(allocation.base_page + 100000));
            }
        }
        assert!(a.memory.published.iter().all(|(_, bytes)| bytes.iter().all(|byte| *byte == 0)));
    }
}

#[test]
fn native_initialized_batch_keeps_payload_caps_and_fragmented_fallback() {
    for fragmented in [false, true] {
        for source_bytes in [0, 64 * PAGE_SIZE - 37] {
            let mut a = Alpha::new();
            let source = (source_bytes != 0).then(|| a.source(source_bytes));
            a.memory.fragmented = fragmented;
            a.memory.allocation_requests.clear();
            a.memory.child_allocations = 0;
            let base = a.map_process_initialized_pages_protected(2, None, 64, source, 3).unwrap();
            assert_eq!(a.memory.allocation_requests.len(), if fragmented { 65 } else { 1 });
            assert_eq!(a.memory.child_allocations, if fragmented { 64 } else { 0 });
            assert_eq!(a.memory.allocation_requests[0], 64 * PAGE_SIZE);
            let root = a.processes.memory_root_node(2).unwrap();
            for (index, (va, payload)) in a.memory.published.iter().enumerate() {
                assert_eq!(*va, base + index * PAGE_SIZE);
                for (within, &byte) in payload.iter().enumerate() {
                    let offset = index * PAGE_SIZE + within;
                    assert_eq!(byte, if offset < source_bytes { (offset % 239) as u8 } else { 0 });
                }
                let allocation = a.processes.find_active_physical_allocation_reference(2, *va, 1).unwrap();
                assert!(allocation.owns_frame_cap);
                assert_eq!(a.processes.physical_reference_count(allocation), 1);
                let vm = a.processes.vm_space_mut(2).unwrap();
                assert_eq!(vm.permissions(*va), 3);
                assert_eq!(vm.find_frame(*va), Some(process_frame_descriptor(root, allocation.start_slot)));
                if !fragmented {
                    KERNEL.with_borrow(|kernel| assert_eq!(kernel.cap_copy_operations[index], (
                        process_frame_chunk_descriptor(root, allocation.start_slot / PROCESS_FRAME_CHUNK_PAGES),
                        allocation.start_slot % PROCESS_FRAME_CHUNK_PAGES,
                        allocation.base_page + 100000,
                    )));
                }
            }
            println!("NATIVE-INITIALIZED-BATCH fragmented={fragmented} source={source_bytes} allocator=64->{}",
                a.memory.allocation_requests.len());
        }
    }
}

#[test]
fn native_batch_cap_copy_failure_before_or_after_install_keeps_all_ram_owned() {
    for at in [0, 31, 63] {
        for after in [false, true] {
            for free_fails in [false, true] {
                let mut a = Alpha::new();
                KERNEL.with_borrow_mut(|kernel| kernel.fail_cap_copy = Some((at, after)));
                a.memory.fail_free = free_fails;
                assert_eq!(a.map_process_initialized_pages_protected(2, Some(0x80000), 64, None, 3),
                    Err(CapabilityError::Fatal));
                let retained = if free_fails { 64 } else { at + 1 };
                assert_eq!(a.memory.pages.len(), retained);
                assert_eq!(a.memory.freed_pages, 64 - retained);
                assert!(a.processes.releasable_physical_allocations_for_pid(2).is_empty());
                let deferred = a.processes.releasable_deferred_physical_allocations_for_pid(2);
                assert_eq!(deferred.len(), retained);
                for allocation in deferred {
                    assert!(allocation.owns_frame_cap);
                    assert_eq!(a.processes.physical_reference_count(allocation), 1);
                    assert!(a.memory.pages.contains_key(&allocation.base_page));
                }
                KERNEL.with_borrow(|kernel| assert!(kernel.ptes.is_empty()));
                assert!(!a.processes.vm_space_mut(2).unwrap().has_backed_pages(0x80000, 0xc0000));
            }
        }
    }
}

#[test]
fn sealed_initialized_extent_preserves_bytes_and_fragmented_fallback() {
    for fragmented in [false, true] {
        let mut a = Alpha::new();
        let source = a.source(64 * PAGE_SIZE - 37);
        a.processes.seal_capabilities(2).unwrap();
        a.memory.fragmented = fragmented;
        a.memory.allocation_requests.clear();
        a.memory.child_allocations = 0;
        let base = a.map_process_initialized_pages_protected(2, None, 64, Some(source), 3).unwrap();
        eprintln!("fragmented={fragmented} allocation requests={:?}", a.memory.allocation_requests);
        if fragmented {
            assert_eq!(a.memory.allocation_requests.len(), 65);
            assert_eq!(a.memory.allocation_requests[0], 64 * PAGE_SIZE);
            assert!(a.memory.allocation_requests[1..].iter().all(|&bytes| bytes == PAGE_SIZE));
        } else {
            assert_eq!(a.memory.allocation_requests, [64 * PAGE_SIZE]);
        }
        assert_eq!(a.memory.child_allocations, 0);
        assert_eq!(a.memory.published.len(), 64);
        for (index, (va, bytes)) in a.memory.published.iter().enumerate() {
            assert_eq!(*va, base + index * PAGE_SIZE);
            for (within, &byte) in bytes.iter().enumerate() {
                let offset = index * PAGE_SIZE + within;
                assert_eq!(byte, if offset < source.bytes { (offset % 239) as u8 } else { 0 });
            }
            let allocation = a.processes.find_active_physical_allocation_reference(2, *va, 1).unwrap();
            assert!(!allocation.owns_frame_cap);
            assert_eq!(a.processes.physical_reference_count(allocation), 1);
            assert_eq!(a.processes.vm_space_mut(2).unwrap().permissions(*va), 3);
        }
    }
}

#[test]
fn sealed_batch_failures_return_unused_tail_and_keep_claimed_pages_owned() {
    initialized_batch_failure_cases(true);
}

#[test]
fn native_batch_failures_return_unused_tail_and_keep_claimed_pages_owned() {
    initialized_batch_failure_cases(false);
}

fn initialized_batch_failure_cases(sealed: bool) {
    for failure in 0..10 {
        let mut a = Alpha::new();
        if sealed { a.processes.seal_capabilities(2).unwrap(); }
        if failure == 0 {
            a.memory.fail_allocate = Some(0);
        } else if failure <= 3 {
            a.memory.fail_ensure = Some([1, 32, 64][failure - 1]);
        } else {
            a.memory.fail_map = Some([0, 31, 63][(failure - 4) / 2]);
            a.memory.fail_after_map = failure % 2 != 0;
        }
        assert_eq!(a.map_process_initialized_pages_protected(2, Some(0x80000), 64, None, 3), Err(CapabilityError::Fatal));
        assert!(a.processes.releasable_physical_allocations_for_pid(2).is_empty());
        let held = a.processes.releasable_deferred_physical_allocations_for_pid(2);
        let claimed = if failure == 0 { 0 } else if failure <= 3 {
            [1, 32, 64][failure - 1]
        } else {
            [1, 32, 64][(failure - 4) / 2]
        };
        assert_eq!(held.len(), claimed);
        assert_eq!(a.memory.pages.len(), claimed);
        assert_eq!(a.memory.freed_pages, if failure == 0 { 0 } else { 64 - claimed });
        for (index, allocation) in held.iter().enumerate() {
            assert_eq!(allocation.page_count, 1);
            assert_eq!(allocation.owns_frame_cap, !sealed);
            assert_eq!(a.processes.physical_reference_count(*allocation), 1);
            assert!(held[..index].iter().all(|old| old.base_page != allocation.base_page));
        }
        let vm = a.processes.vm_space_mut(2).unwrap();
        assert!(!vm.has_backed_pages(0x80000, 0xc0000));
        for page in 0..64 { assert!(!vm.mapping_is_uncertain(0x80000 + page * PAGE_SIZE)); }
        KERNEL.with_borrow(|kernel| assert!(kernel.ptes.is_empty()));
        a.memory.fail_allocate = None;
        a.memory.fail_ensure = None;
        a.memory.fail_map = None;
        a.memory.published.clear();
        a.memory.allocation_requests.clear();
        a.map_process_initialized_pages_protected(2, Some(0x80000), 64, None, 3).unwrap();
        assert_eq!(a.memory.allocation_requests, [64 * PAGE_SIZE]);
        assert_eq!(a.processes.releasable_deferred_physical_allocations_for_pid(2).len(), claimed);
        assert_eq!(a.memory.published.len(), 64);
        assert!(a.memory.published.iter().all(|(_, bytes)| bytes.iter().all(|byte| *byte == 0)));
    }
}

#[test]
fn sealed_batch_free_or_unmap_failure_retains_all_unreleased_backing() {
    initialized_batch_retained_failure_cases(true);
}

#[test]
fn native_batch_free_or_unmap_failure_retains_all_unreleased_backing() {
    initialized_batch_retained_failure_cases(false);
}

fn initialized_batch_retained_failure_cases(sealed: bool) {
    for free_fails in [false, true] {
        for unmap in [None, Some(UnmapFailure::Before), Some(UnmapFailure::After)] {
            let mut a = Alpha::new();
            if sealed { a.processes.seal_capabilities(2).unwrap(); }
            a.memory.fail_map = Some(31);
            a.memory.fail_after_map = true;
            a.memory.fail_free = free_fails;
            KERNEL.with_borrow_mut(|kernel| kernel.fail_unmap = unmap);
            assert!(a.map_process_initialized_pages_protected(2, Some(0x80000), 64, None, 3).is_err());
            let retained = if free_fails { 64 } else { 32 };
            let active = a.processes.releasable_physical_allocations_for_pid(2);
            let deferred = a.processes.releasable_deferred_physical_allocations_for_pid(2);
            assert_eq!(a.memory.pages.len(), retained);
            assert_eq!(a.memory.freed_pages, 64 - retained);
            assert_eq!(active.len() + deferred.len(), retained);
            if unmap.is_some() {
                assert_eq!(active.len(), retained);
                assert!(a.processes.vm_space_mut(2).unwrap().mapping_is_uncertain(0x80000));
            } else {
                assert!(active.is_empty());
                KERNEL.with_borrow(|kernel| assert!(kernel.ptes.is_empty()));
            }
            for allocation in active.iter().chain(&deferred) {
                assert_eq!(a.processes.physical_reference_count(*allocation), 1);
                assert!(a.memory.pages.contains_key(&allocation.base_page));
            }
            KERNEL.with_borrow_mut(|kernel| kernel.fail_unmap = None);
            a.memory.fail_free = false;
            a.handle_mapping_release_request(OsRequestEvent {
                identifier: 2, code: 0, arg0: 0x80000, arg1: 64 * PAGE_SIZE,
                arg2: 2, arg3: nanami_uapi::os::OS_MAPPING_RELEASE_ALLOW_HOLES,
            }).unwrap();
            assert!(a.processes.releasable_physical_allocations_for_pid(2).is_empty());
            assert_eq!(a.processes.releasable_deferred_physical_allocations_for_pid(2).len(), retained);
            KERNEL.with_borrow(|kernel| assert!(kernel.ptes.is_empty()));
        }
    }
}

#[test]
fn sealed_pool_conversion_failure_retires_unpublished_ram_without_a_descriptor() {
    let mut a = Alpha::new();
    a.processes.seal_capabilities(2).unwrap();
    a.memory.fail_ensure = Some(1);
    assert_eq!(a.map_process_initialized_pages(2, Some(0x80000), 1, None), Err(CapabilityError::Fatal));
    assert!(a.processes.find_active_physical_allocation_reference(2, 0x80000, 1).is_none());
    let retained: Vec<_> = a.processes.releasable_deferred_physical_allocations_for_pid(2);
    assert_eq!(retained.len(), 1);
    assert!(!retained[0].owns_frame_cap);
    assert!(a.processes.vm_space_mut(2).unwrap().find_frame(0x80000).is_none());
    a.memory.fail_ensure = None;
    a.map_process_initialized_pages(2, Some(0x80000), 1, None).unwrap();
    assert_eq!(a.memory.next, 1);
    assert_eq!(a.memory.child_allocations, 0);
}

#[test]
fn exec_image_frame_pool_requires_both_sealed_owner_and_raw_image() {
    for sealed in [false, true] {
        for raw in [false, true] {
            let mut a = Alpha::new();
            if sealed { a.processes.seal_capabilities(2).unwrap(); }
            let root = a.processes.memory_entry(2).unwrap().root_node;
            for i in 0..513 {
                let va = 0x80000 + i * PAGE_SIZE;
                let (physical, frame) = a.allocate_image_page_frame(2, root, va, i, raw).unwrap();
                let allocation = a.processes.find_active_physical_allocation_reference(2, va, 1).unwrap();
                assert_eq!(allocation.owns_frame_cap, !(sealed && raw));
                assert_eq!(allocation.base_page, physical);
                assert_eq!(frame, if sealed && raw { physical + 100000 } else { process_frame_descriptor(root, i) });
            }
            assert_eq!(a.memory.child_allocations, if sealed && raw { 0 } else { 513 });
            KERNEL.with_borrow(|kernel| assert_eq!(kernel.chunk_calls, if sealed && raw { 0 } else { 513 }));
        }
    }
    let mut a = Alpha::new();
    a.allocate_image_page_frame(77, 177, 0x80000, 0, true).unwrap();
    assert_eq!(a.memory.child_allocations, 1);
    assert!(a.processes.find_active_physical_allocation_reference(77, 0x80000, 1).unwrap().owns_frame_cap);
}

#[test]
fn exec_pool_frame_validates_root_and_accounts_for_failed_conversion_before_returning() {
    let mut a = Alpha::new();
    a.processes.seal_capabilities(2).unwrap();
    let root = a.processes.memory_entry(2).unwrap().root_node;
    assert_eq!(a.allocate_image_page_frame(2, root + 1, 0x80000, 0, true), Err(CapabilityError::InvalidArgument));
    assert_eq!(a.memory.next, 0);
    a.memory.fail_ensure = Some(1);
    assert_eq!(a.allocate_image_page_frame(2, root, 0x80000, 0, true), Err(CapabilityError::Fatal));
    let allocation = a.processes.find_active_physical_allocation_reference(2, 0x80000, 1).unwrap();
    assert!(!allocation.owns_frame_cap);
    assert_eq!(a.processes.physical_reference_count(allocation), 1);
    assert!(a.memory.physical_frame_descriptor_from_index(allocation.base_page).is_none());
    a.handle_mapping_release_request(OsRequestEvent {
        identifier: 2, code: 0, arg0: 0x80000, arg1: PAGE_SIZE, arg2: 2,
        arg3: nanami_uapi::os::OS_MAPPING_RELEASE_ALLOW_HOLES,
    }).unwrap();
    assert_eq!(a.processes.deferred_physical_allocations_for_pid(2).count(), 1);
}

#[test]
fn prepared_image_batch_reduces_allocator_calls_and_keeps_single_page_ownership() {
    for fragmented in [false, true] {
        let mut old = Alpha::new();
        let mut new = Alpha::new();
        new.memory.fragmented = fragmented;
        let base = 0x100000;
        let mut batch = new.prepare_preinstall_pool_batch(77, 177, base, 64).unwrap();
        for i in 0..64 {
            let expected = old.allocate_preinstall_pool_frame(77, 177, base + i * PAGE_SIZE, i).unwrap();
            let actual = if let Some(batch) = batch.as_mut() {
                new.claim_preinstall_pool_frame(77, 177, base + i * PAGE_SIZE, i, batch).unwrap()
            } else {
                new.allocate_preinstall_pool_frame(77, 177, base + i * PAGE_SIZE, i).unwrap()
            };
            assert_eq!(actual, expected);
            let allocation = new.processes.find_active_physical_allocation_reference(77, base + i * PAGE_SIZE, 1).unwrap();
            assert!(!allocation.owns_frame_cap);
            assert_eq!(new.processes.physical_reference_count(allocation), 1);
        }
        if let Some(batch) = batch {
            new.release_preinstall_pool_batch_tail(batch).unwrap();
        }
        assert_eq!(old.memory.allocation_requests.len(), 64);
        assert_eq!(new.memory.allocation_requests.len(), if fragmented { 65 } else { 1 });
        assert_eq!(new.memory.freed_pages, 0);
        println!("PREINSTALL-IMAGE-BATCH fragmented={fragmented} allocator=64->{}", new.memory.allocation_requests.len());
    }
}

#[test]
fn prepared_image_batch_failure_releases_only_unregistered_tail_or_retains_ownership() {
    for fail_at in [0, 1, 31, 63] {
        for fail_free in [false, true] {
            let mut a = Alpha::new();
            let base = 0x100000;
            let mut batch = a.prepare_preinstall_pool_batch(77, 177, base, 64).unwrap().unwrap();
            a.memory.fail_ensure = Some(fail_at + 1);
            for i in 0..=fail_at {
                let result = a.claim_preinstall_pool_frame(77, 177, base + i * PAGE_SIZE, i, &mut batch);
                assert_eq!(result.is_err(), i == fail_at);
            }
            a.memory.fail_free = fail_free;
            a.release_preinstall_pool_batch_tail(batch).unwrap();
            let retained = if fail_free { 64 } else { fail_at + 1 };
            assert_eq!(a.memory.pages.len(), retained);
            assert_eq!(a.memory.freed_pages, 64 - retained);
            for i in 0..64 {
                assert_eq!(a.processes.find_active_physical_allocation_reference(77, base + i * PAGE_SIZE, 1).is_some(), i < retained);
            }
        }
    }
}

#[test]
fn prepared_image_batch_declines_small_and_fragmented_requests_but_not_other_errors() {
    let mut a = Alpha::new();
    assert!(a.prepare_preinstall_pool_batch(77, 177, 0x100000, 1).unwrap().is_none());
    assert!(a.memory.allocation_requests.is_empty());
    for (pid, base, count) in [(0, 0x100000, 64), (2, 0x100000, 64), (77, 0x100001, 64), (77, 0x100000, PROCESS_FRAME_TOTAL_PAGES + 1), (77, usize::MAX & !(PAGE_SIZE - 1), 64)] {
        assert!(a.prepare_preinstall_pool_batch(pid, 177, base, count).is_err());
    }
    assert!(a.memory.allocation_requests.is_empty());
    a.memory.fail_allocate = Some(0);
    assert!(matches!(a.prepare_preinstall_pool_batch(77, 177, 0x100000, 64), Err(CapabilityError::Fatal)));
    assert_eq!(a.memory.allocation_requests.len(), 1);
    assert!(a.memory.pages.is_empty());
    a.memory.fail_allocate = None;
    let mut batch = a.prepare_preinstall_pool_batch(77, 177, 0x100000, 64).unwrap().unwrap();
    assert!(a.claim_preinstall_pool_frame(77, 177, 0x100000 + PAGE_SIZE, 1, &mut batch).is_err());
    assert!(a.claim_preinstall_pool_frame(78, 177, 0x100000, 0, &mut batch).is_err());
    a.release_preinstall_pool_batch_tail(batch).unwrap();
    assert!(a.memory.pages.is_empty());
}

#[test]
fn prepared_pool_policy_requires_raw_suspended_final_permission_construction() {
    for raw in [false, true] {
        for auto_resume in [false, true] {
            for final_permissions in [false, true] {
                assert_eq!(memory_pages::prepared_pool_allowed(raw, auto_resume, final_permissions),
                    cfg!(target_arch = "x86_64") && raw && !auto_resume && final_permissions);
            }
        }
    }
}

#[test]
fn prepared_spawn_image_and_stack_keep_pool_ownership_after_process_install() {
    let mut a = Alpha::new();
    for i in 0..545 {
        let va = 0x100000 + i * PAGE_SIZE;
        let (physical, frame) = a.allocate_preinstall_pool_frame(77, 177, va, i).unwrap();
        assert_eq!(frame, physical + 100000);
        assert!(!a.processes.find_active_physical_allocation_reference(77, va, 1).unwrap().owns_frame_cap);
    }
    assert_eq!(a.memory.child_allocations, 0);
    KERNEL.with_borrow(|kernel| assert_eq!((kernel.cap_copies, kernel.chunk_calls), (0, 0)));
    a.processes.install_process(77, 3, 277, 177, 87, 97, 0, 77, 0, 545, 0x400000, 0x1000000).unwrap();
    a.processes.seal_capabilities(77).unwrap();
    for i in 0..545 {
        let allocation = a.processes.find_active_physical_allocation_reference(77, 0x100000 + i * PAGE_SIZE, 1).unwrap();
        assert!(!allocation.owns_frame_cap);
        assert_eq!(a.processes.physical_reference_count(allocation), 1);
    }
    assert_eq!(a.allocate_preinstall_pool_frame(77, 177, 0x400000, 545), Err(CapabilityError::InvalidArgument));
    assert_eq!(a.memory.next, 545);
}

#[test]
fn prepared_pool_rejects_invalid_slots_and_retains_conversion_failure_before_install() {
    let mut a = Alpha::new();
    for (pid, va, slot) in [(0, 0x80000, 0), (2, 0x80000, 0), (77, 0x80001, 0), (77, 0x80000, PROCESS_FRAME_TOTAL_PAGES)] {
        assert_eq!(a.allocate_preinstall_pool_frame(pid, 177, va, slot), Err(CapabilityError::InvalidArgument));
    }
    assert_eq!(a.memory.next, 0);
    a.memory.fail_ensure = Some(1);
    assert_eq!(a.allocate_preinstall_pool_frame(77, 177, 0x80000, 0), Err(CapabilityError::Fatal));
    let retained = a.processes.find_active_physical_allocation_reference(77, 0x80000, 1).unwrap();
    assert!(!retained.owns_frame_cap);
    assert_eq!(a.processes.physical_reference_count(retained), 1);
    assert!(a.memory.physical_frame_descriptor_from_index(retained.base_page).is_none());
}

#[test]
fn bulk_frame_preparation_keeps_failed_first_map_owned_and_rolls_back_its_word() {
    for after_map in [false, true] {
        let mut a = Alpha::new();
        a.processes.seal_capabilities(2).unwrap();
        a.memory.fail_map = Some(0);
        a.memory.fail_after_map = after_map;
        assert_eq!(a.map_process_initialized_pages_protected(2, Some(0x80000), 64, None, 3), Err(CapabilityError::Fatal));
        assert_eq!(a.memory.next, 64);
        assert_eq!(a.memory.child_allocations, 0);
        assert!(a.processes.releasable_physical_allocations_for_pid(2).is_empty());
        let held = a.processes.releasable_deferred_physical_allocations_for_pid(2);
        assert_eq!(held.len(), 1);
        for allocation in held {
            assert!(!allocation.owns_frame_cap);
            assert_eq!(a.processes.physical_reference_count(allocation), 1);
        }
        let vm = a.processes.vm_space_mut(2).unwrap();
        assert!(!vm.has_backed_pages(0x80000, 0xc0000));
        assert!(!vm.mapping_is_uncertain(0x80000));
        KERNEL.with_borrow(|kernel| assert!(kernel.ptes.is_empty()));
    }
}

#[test]
fn protection_demand_stack_failure_burns_original_slot_and_retry_uses_new_slot() {
    let mut a=Alpha::new();
    a.processes.seal_capabilities(2).unwrap();
    a.processes.reset_runtime_memory_for_exec(2,1000,0x100000,0x1000000).unwrap();
    let root=a.processes.memory_entry(2).unwrap().root_node;
    a.processes.register_fresh_stack_mapping(2,root,0x4000000,64,100).unwrap();
    a.memory.fail_allocate=Some(a.memory.next);
    assert!(a.handle_mapping_protect_request(stack_protection_request(0x4002000,3)).is_err());
    assert_eq!(a.processes.find_entry_by_pid(2).unwrap().next_frame_slot,1000);
    assert!(a.processes.take_fresh_stack_frame_slot(2,0x4002000,PROCESS_FRAME_TOTAL_PAGES).is_none());
    a.memory.fail_allocate=None;
    a.handle_mapping_protect_request(stack_protection_request(0x4002000,3)).unwrap();
    assert_eq!(a.processes.find_active_physical_allocation_reference(2,0x4002000,1).unwrap().start_slot,1000);
    assert_eq!(a.processes.find_entry_by_pid(2).unwrap().next_frame_slot,1001);
}

#[test]
fn protection_demand_stack_post_map_error_retains_uncertain_original_ownership() {
    let mut a=Alpha::new();
    a.processes.seal_capabilities(2).unwrap();
    a.processes.reset_runtime_memory_for_exec(2,1000,0x100000,0x1000000).unwrap();
    let root=a.processes.memory_entry(2).unwrap().root_node;
    a.processes.register_fresh_stack_mapping(2,root,0x4000000,64,100).unwrap();
    a.memory.fail_map=Some(0);a.memory.fail_after_map=true;
    KERNEL.with_borrow_mut(|k|k.fail_unmap=Some(UnmapFailure::Before));
    assert!(a.handle_mapping_protect_request(stack_protection_request(0x4002000,3)).is_err());
    assert!(a.processes.vm_space_mut(2).unwrap().mapping_is_uncertain(0x4002000));
    assert_eq!(a.processes.find_active_physical_allocation_reference(2,0x4002000,1).unwrap().start_slot,102);
    assert_eq!(a.processes.find_entry_by_pid(2).unwrap().next_frame_slot,1000);
    assert!(a.processes.take_fresh_stack_frame_slot(2,0x4002000,PROCESS_FRAME_TOTAL_PAGES).is_none());
}

#[test]
fn protection_demand_legacy_zero_plans_keep_fresh_slot_reservation() {
    let mut a=Alpha::new();
    a.processes.reset_runtime_memory_for_exec(2,1000,0x100000,0x1000000).unwrap();
    a.processes.register_lazy_mapping(2,0x4000000,64,100,
        nanami_core::process::ProcessLazyMappingKind::Zero).unwrap();
    a.handle_mapping_protect_request(stack_protection_request(0x4002000,3)).unwrap();
    assert_eq!(a.processes.find_active_physical_allocation_reference(2,0x4002000,1).unwrap().start_slot,1000);
    assert_eq!(a.processes.find_entry_by_pid(2).unwrap().next_frame_slot,1001);
}

#[test]
fn virgin_stack_slot_initializes_before_publication_without_cursor_growth() {
    let mut a=Alpha::new();
    a.processes.seal_capabilities(2).unwrap();
    a.processes.reset_runtime_memory_for_exec(2,PROCESS_FRAME_TOTAL_PAGES,0x100000,0x1000000).unwrap();
    let root=a.processes.memory_entry(2).unwrap().root_node;
    a.processes.register_fresh_stack_mapping(2,root,0x4000000,64,100).unwrap();
    let token=a.processes.take_fresh_stack_frame_slot(2,0x4001000,PROCESS_FRAME_TOTAL_PAGES).unwrap();
    a.map_process_initialized_pages_with_stack_slot(2,Some(0x4001000),1,None,3,Some(token)).unwrap();
    assert_eq!(a.processes.find_entry_by_pid(2).unwrap().next_frame_slot,PROCESS_FRAME_TOTAL_PAGES);
    let allocation=a.processes.find_active_physical_allocation_reference(2,0x4001000,1).unwrap();
    assert_eq!(allocation.start_slot,101);
    assert!(a.memory.published[0].1.iter().all(|byte|*byte==0));
    assert_eq!(a.processes.vm_space_mut(2).unwrap().permissions(0x4001000),3);
    assert!(a.processes.take_fresh_stack_frame_slot(2,0x4001000,PROCESS_FRAME_TOTAL_PAGES).is_none());
}

#[test]
fn failed_stack_construction_consumes_claim_and_cannot_reuse_uncertain_cap_slot() {
    let mut a=Alpha::new();
    a.processes.seal_capabilities(2).unwrap();
    a.processes.reset_runtime_memory_for_exec(2,1000,0x100000,0x1000000).unwrap();
    let root=a.processes.memory_entry(2).unwrap().root_node;
    a.processes.register_fresh_stack_mapping(2,root,0x4000000,64,100).unwrap();
    let token=a.processes.take_fresh_stack_frame_slot(2,0x4002000,PROCESS_FRAME_TOTAL_PAGES).unwrap();
    a.memory.fail_allocate=Some(a.memory.next);
    assert!(a.map_process_initialized_pages_with_stack_slot(2,Some(0x4002000),1,None,3,Some(token)).is_err());
    assert!(a.processes.take_fresh_stack_frame_slot(2,0x4002000,PROCESS_FRAME_TOTAL_PAGES).is_none());
    a.memory.fail_allocate=None;
    a.map_process_initialized_pages_protected(2,Some(0x4002000),1,None,3).unwrap();
    assert_eq!(a.processes.find_active_physical_allocation_reference(2,0x4002000,1).unwrap().start_slot,1000);
}

#[test]
fn reserved_stack_map_failure_retains_uncertain_ownership_and_burns_the_claim() {
    let mut a=Alpha::new();
    a.processes.seal_capabilities(2).unwrap();
    a.processes.reset_runtime_memory_for_exec(2,1000,0x100000,0x1000000).unwrap();
    let root=a.processes.memory_entry(2).unwrap().root_node;
    a.processes.register_fresh_stack_mapping(2,root,0x4000000,64,100).unwrap();
    let token=a.processes.take_fresh_stack_frame_slot(2,0x4002000,PROCESS_FRAME_TOTAL_PAGES).unwrap();
    a.memory.fail_map=Some(0);a.memory.fail_after_map=true;
    KERNEL.with_borrow_mut(|kernel|kernel.fail_unmap=Some(UnmapFailure::Before));
    assert!(a.map_process_initialized_pages_with_stack_slot(2,Some(0x4002000),1,None,3,Some(token)).is_err());
    assert!(a.processes.vm_space_mut(2).unwrap().mapping_is_uncertain(0x4002000));
    assert_eq!(a.processes.find_active_physical_allocation_reference(2,0x4002000,1).unwrap().start_slot,102);
    assert!(a.processes.take_fresh_stack_frame_slot(2,0x4002000,PROCESS_FRAME_TOTAL_PAGES).is_none());
    assert_eq!(a.processes.find_entry_by_pid(2).unwrap().next_frame_slot,1000);
}

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
fn initialized_pages_publish_final_rights_without_a_protection_remap() {
    for permissions in [1, 3, 5, 7] {
        let mut a = Alpha::new();
        let source = a.source(4097);
        a.map_process_initialized_pages_protected(2, Some(0x80000), 2, Some(source), permissions).unwrap();
        assert_eq!(a.memory.published.len(), 2);
        KERNEL.with_borrow(|kernel| {
            assert_eq!(kernel.unmaps, 0);
            assert_eq!(kernel.ptes.len(), 2);
            assert!(kernel.ptes.values().all(|(_, rights)| *rights == permissions));
        });
        assert_eq!(a.processes.vm_space_mut(2).unwrap().permissions(0x80000), permissions);
        assert_eq!(a.processes.vm_space_mut(2).unwrap().permissions(0x81000), permissions);
        assert!(a.memory.published[1].1[1..].iter().all(|byte| *byte == 0));
    }
    for permissions in [0, 8, usize::MAX] {
        let mut a = Alpha::new();
        assert!(a.map_process_initialized_pages_protected(2, Some(0x80000), 2, None, permissions).is_err());
        assert_eq!(a.memory.published.len(), 0);
        assert!(a.memory.pages.is_empty());
    }
}

#[test]
fn zero_source_tail_publishes_only_zeroes_with_final_permissions() {
    for permissions in 1..=7 {
        let mut a = Alpha::new();
        a.map_process_initialized_pages_protected(2, Some(0x80000), 2,
            Some(memory_pages::PageSource { pid: 1, address: 0, bytes: 0 }), permissions).unwrap();
        assert_eq!(a.memory.published.len(), 2);
        assert!(a.memory.published.iter().all(|(_, page)| page.iter().all(|byte| *byte == 0)));
        KERNEL.with_borrow(|kernel| {
            assert_eq!(kernel.unmaps, 0);
            assert_eq!(kernel.ptes.len(), 2);
            assert!(kernel.ptes.values().all(|(_, rights)| *rights == permissions));
        });
    }
}

#[test]
fn automatic_final_zero_pages_keep_legacy_placement_and_one_map_per_page() {
    for permissions in 1..=7 { for pages in [1, 2, 64] {
        let mut legacy = Alpha::new();
        let expected = legacy.map_process_heap_pages(2, pages).unwrap();
        let expected_cursor = legacy.processes.find_entry_by_pid(2).unwrap().user_heap_next_va;
        let mut a = Alpha::new();
        let base = a.map_process_initialized_pages_protected(2, None, pages,
            Some(memory_pages::PageSource { pid: 1, address: 0, bytes: 0 }), permissions).unwrap();
        assert_eq!(base, expected);
        assert_eq!(a.processes.find_entry_by_pid(2).unwrap().user_heap_next_va, expected_cursor);
        assert_eq!(a.memory.published.len(), pages);
        assert!(a.memory.published.iter().all(|(_, page)| page.iter().all(|byte| *byte == 0)));
        KERNEL.with_borrow(|kernel| {
            assert_eq!(kernel.unmaps, 0);
            assert_eq!(kernel.ptes.len(), pages);
            assert!(kernel.ptes.values().all(|(_, rights)| *rights == permissions));
        });
    }}
}

#[test]
fn final_rights_map_failure_keeps_uncertain_backing_until_unmap_is_confirmed() {
    for failure in [UnmapFailure::Before, UnmapFailure::After] {
        let mut a = Alpha::new();
        a.memory.fail_map = Some(0);
        a.memory.fail_after_map = true;
        KERNEL.with_borrow_mut(|kernel| kernel.fail_unmap = Some(failure));
        assert!(a.map_process_initialized_pages_protected(2, Some(0x80000), 1, None, 5).is_err());
        assert!(a.processes.find_active_physical_allocation_reference(2, 0x80000, 1).is_some());
        assert!(a.processes.vm_space_mut(2).unwrap().mapping_is_uncertain(0x80000));
        assert!(a.processes.releasable_deferred_physical_allocations_for_pid(2).is_empty());
        KERNEL.with_borrow_mut(|kernel| kernel.fail_unmap = None);
        a.handle_mapping_release_request(OsRequestEvent {
            identifier: 2, code: 0, arg0: 0x80000, arg1: PAGE_SIZE,
            arg2: 2, arg3: nanami_uapi::os::OS_MAPPING_RELEASE_ALLOW_HOLES,
        }).unwrap();
        assert!(!a.processes.vm_space_mut(2).unwrap().mapping_is_uncertain(0x80000));
    }
}

#[test]
fn initialization_source_reuse_requires_current_private_ram_and_reads_current_bytes() {
    let mut a = Alpha::new();
    let source = a.source(PAGE_SIZE);
    let physical = 1;
    let slot = 9;
    let root = a.processes.memory_entry(source.pid).unwrap().root_node;
    let mut pages = memory_pages::InitializationSource::new(source.pid, root, Vec::new());
    let frame = process_frame_descriptor(root, slot);
    a.processes.vm_space_mut(source.pid).unwrap().record_frame(source.address, frame).unwrap();
    a.memory.frames.insert(frame, physical);
    a.initialization_sources.insert(physical);
    assert_eq!(a.cached_source_initialization_page(source.pid, source.address, frame, usize::MAX, &pages), None);
    a.processes.register_physical_allocation(source.pid, source.address, slot, physical, 1).unwrap();
    pages.pages = a.processes.try_physical_pages_in_range(source.pid, source.address, source.address + PAGE_SIZE, 2048).unwrap();
    a.initialization_sources.remove(&physical);
    assert_eq!(a.cached_source_initialization_page(source.pid, source.address, frame, usize::MAX, &pages), None);
    a.initialization_sources.insert(physical);
    pages.pid += 1;
    assert_eq!(a.cached_source_initialization_page(source.pid, source.address, frame, usize::MAX, &pages), None);
    pages.pid = source.pid;
    pages.root += 1;
    assert_eq!(a.cached_source_initialization_page(source.pid, source.address, frame, usize::MAX, &pages), None);
    pages.root = root;
    let address = a.cached_source_initialization_page(source.pid, source.address, frame, usize::MAX, &pages).unwrap();
    assert_eq!(a.cached_source_initialization_page(source.pid, source.address, frame, address, &pages), None);
    assert_eq!(a.cached_source_initialization_page(source.pid, source.address, frame + 1, usize::MAX, &pages), None);
    a.processes.vm_space_mut(source.pid).unwrap().record_frame(source.address, frame + 1).unwrap();
    assert_eq!(a.cached_source_initialization_page(source.pid, source.address, frame + 1, usize::MAX, &pages), None);
    a.processes.vm_space_mut(source.pid).unwrap().record_frame(source.address, frame).unwrap();
    a.processes.vm_space_mut(source.pid).unwrap().mark_private_page_shared(source.address, true);
    assert_eq!(a.cached_source_initialization_page(source.pid, source.address, frame, usize::MAX, &pages), None);
    a.processes.vm_space_mut(source.pid).unwrap().mark_private_page_shared(source.address, false);
    a.processes.vm_space_mut(source.pid).unwrap().set_permissions(source.address, 0);
    assert_eq!(a.cached_source_initialization_page(source.pid, source.address, frame, usize::MAX, &pages), None);
    a.processes.vm_space_mut(source.pid).unwrap().set_permissions(source.address, 1);
    a.memory.pages.get_mut(&physical).unwrap()[0] = 0x7d;
    a.map_process_initialized_pages(2, Some(0x80000), 1, Some(source)).unwrap();
    assert_eq!(a.memory.published[0].1[0], 0x7d);
    let expected_maps = if option_env!("NANAMI_INIT_SOURCE_REUSE").is_none_or(|value| value == "1") { 0 } else { 1 };
    assert_eq!(a.copy_maps, expected_maps);
    a.processes.vm_space_mut(source.pid).unwrap().mark_mapping_uncertain(source.address);
    assert_eq!(a.cached_source_initialization_page(source.pid, source.address, frame, usize::MAX, &pages), None);
}

#[test]
fn bounded_source_ownership_walk_sorts_and_declines_ambiguous_or_excess_rows() {
    let mut a = Alpha::new();
    for (slot, va) in [0x3000, 0x1000, 0x2000].into_iter().enumerate() {
        a.processes.register_physical_allocation(1, va, slot, slot + 1, 1).unwrap();
    }
    let pages = a.processes.try_physical_pages_in_range(1, 0x1000, 0x4000, 3).unwrap();
    assert_eq!(pages.iter().map(|page| page.base_va).collect::<Vec<_>>(), vec![0x1000, 0x2000, 0x3000]);
    assert!(a.processes.try_physical_pages_in_range(1, 0x1000, 0x4000, 2).is_none());
    assert!(a.processes.try_physical_pages_in_range(1, 0x1000, 0x4000, 0).is_none());
    a.processes.register_physical_allocation(1, 0x1000, 9, 9, 1).unwrap();
    assert!(a.processes.try_physical_pages_in_range(1, 0x1000, 0x4000, 8).is_none());
    assert_eq!(a.processes.try_physical_pages_in_range(2, 0x1000, 0x4000, 8).unwrap().len(), 0);
}

#[test]
fn initialization_reference_search_matches_scalar_for_repeats_holes_and_rewinds() {
    use nanami_core::process::ProcessPhysicalAllocation;
    let rows: Vec<_> = (0..2048).map(|index| ProcessPhysicalAllocation {
        base_va: (index + 1) * 2 * PAGE_SIZE, start_slot: index,
        base_page: index + 100, page_count: 1, owns_frame_cap: index % 3 != 0,
    }).collect();
    let source = memory_pages::InitializationSource::new(1, 201, rows);
    memory_pages::take_source_reference_probes();
    for row in &source.pages {
        for _ in 0..2 {
            assert!(source.find_page(row.base_va).copied() == Some(*row));
        }
    }
    let probes = memory_pages::take_source_reference_probes();
    println!("INIT-SOURCE-CURSOR sequential-pages=2048 repeats=2 va-probes={probes}");
    assert_eq!(probes, 6143);
    let mut seed = 0x7d91_a3f5u32;
    for _ in 0..10_000 {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let page = seed as usize % 5000 * PAGE_SIZE;
        let expected = source.pages.iter().copied().find(|row| row.base_va == page);
        assert!(source.find_page(page).copied() == expected, "page={page:#x}");
    }
    for page in [0, PAGE_SIZE, 2 * PAGE_SIZE, usize::MAX] {
        let empty = memory_pages::InitializationSource::new(1, 201, Vec::new());
        assert!(empty.find_page(page).is_none());
    }
}

#[test]
fn scalar_memory_root_matches_owned_snapshots_for_owners_peers_and_unknown_pids() {
    let mut a = Alpha::new();
    for pid in [0, 1, 2, 3, usize::MAX] {
        assert_eq!(a.processes.memory_root_node(pid), a.processes.memory_entry(pid).map(|entry| entry.root_node));
    }
    a.processes.share_address_space(2, 1).unwrap();
    assert_eq!(a.processes.memory_root_node(2), a.processes.memory_root_node(1));
    assert_eq!(a.processes.memory_root_node(2), a.processes.memory_entry(2).map(|entry| entry.root_node));
}

#[test]
fn compact_memory_context_matches_owned_snapshot_across_owner_lifecycle() {
    fn check(a: &Alpha) {
        for pid in [0, 1, 2, 3, usize::MAX] {
            let compact = a.processes.memory_context(pid).map(|m| (m.root_node, m.address_space));
            let owned = a.processes.memory_entry(pid).map(|m| (m.root_node, m.address_space));
            assert_eq!(compact, owned, "pid {pid}");
        }
    }
    let mut a = Alpha::new();
    check(&a);
    a.processes.share_address_space(2, 1).unwrap();
    check(&a);
    // Retargeting an owner must preserve the existing single-hop lookup,
    // rather than recursively following the owner's new memory owner.
    a.processes.share_address_space(1, 0).unwrap();
    check(&a);
    assert_eq!(a.processes.memory_context(2).unwrap().root_node,
        a.processes.find_entry_by_pid(1).unwrap().root_node);
    a.processes.mark_exited(1, 1, 0).unwrap();
    check(&a);
    a.processes.reap_process(1, false).unwrap();
    check(&a); // Peer now has a dangling memory owner.
    assert!(a.processes.memory_context(2).is_none());
    a.processes.share_address_space(2, 0).unwrap();
    a.processes.quarantine_resources(2).unwrap();
    check(&a);
    assert!(core::mem::size_of::<nanami_core::process::ProcessMemoryContext>()
        < core::mem::size_of::<nanami_core::process::ProcessEntry>());
}

fn owned_cached_source(a: &mut Alpha, bytes: usize) -> memory_pages::PageSource {
    let source = a.source(bytes);
    let root = a.processes.memory_entry(source.pid).unwrap().root_node;
    for index in 0..bytes.div_ceil(PAGE_SIZE) {
        let frame = process_frame_descriptor(root, 9 + index);
        let address = source.address + index * PAGE_SIZE;
        a.processes.vm_space_mut(source.pid).unwrap().record_frame(address, frame).unwrap();
        a.memory.frames.insert(frame, index + 1);
        a.processes.register_physical_allocation(source.pid, address, 9 + index, index + 1, 1).unwrap();
        a.initialization_sources.insert(index + 1);
    }
    source
}

#[test]
fn ready_copy_destinations_keep_bytes_zero_tail_and_pin_each_source_piece() {
    for unaligned in [false, true] {
        let mut a = Alpha::new();
        a.processes.seal_capabilities(2).unwrap();
        let mut source = a.source(PAGE_SIZE + 17);
        if unaligned { source.address += PAGE_SIZE - 1; source.bytes = 2; }
        let first = a.memory.next + 1;
        let pages = source.bytes.div_ceil(PAGE_SIZE);
        for page in first..first + pages { a.cached_copy_frames.insert(page + 100000); }
        a.map_process_initialized_pages_protected(2, Some(0x80000), pages, Some(source), 5).unwrap();
        let actual: Vec<u8> = a.memory.published.iter().flat_map(|(_, bytes)| bytes.iter().copied()).collect();
        for (index, &byte) in actual.iter().enumerate() {
            let expected = if index >= source.bytes { 0 }
                else { ((if unaligned { PAGE_SIZE - 1 } else { 0 } + index) % 239) as u8 };
            assert_eq!(byte, expected, "unaligned={unaligned} index={index}");
        }
        if swap_enabled() {
            assert_eq!(a.initialization_maps, 0);
            assert_eq!(a.pinned_copies.len(), 2);
            assert!(a.pinned_copies.iter().all(Option::is_some));
        }
        for page in 0..pages {
            assert_eq!(a.processes.vm_space(2).unwrap().permissions(0x80000 + page * PAGE_SIZE), 5);
        }
    }
}

#[test]
fn ready_copy_destination_does_not_publish_after_source_mapping_failure() {
    let mut a = Alpha::new();
    a.processes.seal_capabilities(2).unwrap();
    let source = a.source(17);
    let original = a.memory.pages[&1].to_vec();
    a.cached_copy_frames.insert(a.memory.next + 1 + 100000);
    a.fail_copy = Some(1);
    assert!(a.map_process_initialized_pages(2, Some(0x80000), 1, Some(source)).is_err());
    assert!(a.memory.published.is_empty());
    assert_eq!(a.memory.pages[&1].to_vec(), original);
    if swap_enabled() { assert_eq!(a.initialization_maps, 0); }
    assert!(!a.processes.vm_space_mut(2).unwrap().has_backed_pages(0x80000, 0x81000));
}

#[test]
fn colliding_destination_can_use_copy_window_without_discarding_source() {
    let mut a = Alpha::new();
    let source = owned_cached_source(&mut a, PAGE_SIZE);
    a.memory.next = 2048;
    a.map_process_initialized_pages(2, Some(0x80000), 1, Some(source)).unwrap();
    assert_eq!(a.memory.published[0].1, a.memory.pages[&1].to_vec());
    if option_env!("NANAMI_INIT_SOURCE_REUSE").is_none_or(|value| value == "1")
        && option_env!("NANAMI_INIT_SOURCE_ROLE_SWAP").is_none_or(|value| value == "1") {
        assert!(a.initialization_sources.contains(&1));
        assert_eq!(a.pinned_copies, vec![None]); // Destination mapped, source reused.
    }
}

#[test]
fn partial_source_second_page_collision_pins_destination_during_fallback() {
    let mut a = Alpha::new();
    let mut source = owned_cached_source(&mut a, 2 * PAGE_SIZE);
    source.address += PAGE_SIZE - 1;
    source.bytes = 2;
    a.initialization_sources.remove(&1);
    a.memory.next = 2049;
    a.map_process_initialized_pages(2, Some(0x80000), 1, Some(source)).unwrap();
    let data = &a.memory.published[0].1;
    assert_eq!(&data[..2], &[((PAGE_SIZE - 1) % 239) as u8, (PAGE_SIZE % 239) as u8]);
    assert!(data[2..].iter().all(|byte| *byte == 0));
    if option_env!("NANAMI_INIT_SOURCE_REUSE").is_none_or(|value| value == "1")
        && option_env!("NANAMI_INIT_SOURCE_ROLE_SWAP").is_none_or(|value| value == "1") {
        assert_eq!(a.pinned_copies.len(), 2);
        assert_eq!(a.pinned_copies[0], None);
        assert!(a.pinned_copies[1].is_some()); // Source miss cannot evict destination.
    }
}

fn reuse_enabled() -> bool {
    option_env!("NANAMI_INIT_SOURCE_REUSE").is_none_or(|v|v=="1")
}
fn swap_enabled() -> bool {
    reuse_enabled() && option_env!("NANAMI_INIT_SOURCE_ROLE_SWAP").is_none_or(|v|v=="1")
}

#[test]
fn ready_pool_source_avoids_ownership_snapshot_and_observes_fresh_bytes_with_zero_tail() {
    let mut a=Alpha::new();let source=a.source(2*PAGE_SIZE+17);
    for page in 1..=3 {a.initialization_sources.insert(page);}
    for i in 0..4000 {a.processes.register_physical_allocation(1,0x1000000+i*PAGE_SIZE,10000+i,10000+i,1).unwrap();}
    a.memory.pages.get_mut(&1).unwrap()[0]=0x7d;
    a.map_process_initialized_pages_protected(2,Some(0x80000),3,Some(source),5).unwrap();
    assert_eq!(a.processes.snapshots.get(),0);
    assert_eq!(a.memory.published[0].1[0],0x7d);
    for i in 0..2 {assert_eq!(a.memory.published[i].1,a.memory.pages[&(i+1)].to_vec());}
    assert_eq!(&a.memory.published[2].1[..17],&a.memory.pages[&3][..17]);
    assert!(a.memory.published[2].1[17..].iter().all(|b|*b==0));
    assert_eq!(a.copy_maps,if reuse_enabled(){0}else{3});
    KERNEL.with_borrow(|k|assert!(k.ptes.values().all(|(_,rights)|*rights==5)));
}

#[test]
fn pool_source_collision_swaps_destination_without_needing_private_ownership_rows() {
    let mut a=Alpha::new();let source=a.source(PAGE_SIZE);a.initialization_sources.insert(1);a.memory.next=2048;
    a.map_process_initialized_pages(2,Some(0x80000),1,Some(source)).unwrap();
    assert_eq!(a.processes.snapshots.get(),0);
    assert_eq!(a.memory.published[0].1,a.memory.pages[&1].to_vec());
    if swap_enabled() {assert_eq!(a.pinned_copies,vec![None]);assert!(a.initialization_sources.contains(&1));}
}

#[test]
fn mixed_pool_source_miss_and_second_page_collision_keep_destination_pinned() {
    let mut a=Alpha::new();let mut source=a.source(2*PAGE_SIZE);source.address+=PAGE_SIZE-1;source.bytes=2;
    a.initialization_sources.insert(2);a.memory.next=2049;
    a.map_process_initialized_pages(2,Some(0x80000),1,Some(source)).unwrap();
    let data=&a.memory.published[0].1;
    assert_eq!(&data[..2],&[((PAGE_SIZE-1)%239) as u8,(PAGE_SIZE%239) as u8]);
    assert!(data[2..].iter().all(|b|*b==0));assert_eq!(a.processes.snapshots.get(),0);
    if swap_enabled() {assert_eq!(a.pinned_copies.len(),2);assert_eq!(a.pinned_copies[0],None);assert!(a.pinned_copies[1].is_some());}
}

#[test]
fn source_frame_change_after_pool_preflight_uses_the_current_private_frame() {
    let mut a=Alpha::new();let mut source=a.source(2*PAGE_SIZE);source.bytes=PAGE_SIZE;a.initialization_sources.insert(1);
    let frame=process_frame_descriptor(a.processes.memory_entry(1).unwrap().root_node,99);
    a.memory.frames.insert(frame,2);a.memory.pages.get_mut(&2).unwrap().fill(0x7b);
    // With reuse disabled there is no role-swap probe; lookup2 is the copy.
    a.replace_on_lookup=Some((2,1,source.address,frame));
    a.map_process_initialized_pages(2,Some(0x80000),1,Some(source)).unwrap();
    assert!(a.memory.published[0].1.iter().all(|b|*b==0x7b));
    assert_eq!(a.copy_maps,1);assert_eq!(a.processes.snapshots.get(),0);
}

#[test]
fn single_source_frame_pages_resolve_once_after_preflight_with_hits_misses_and_collision() {
    for ready in [false,true] {for collision in [false,true] {
        let mut a=Alpha::new();let source=a.source(2*PAGE_SIZE+17);
        if ready {for page in 1..=3 {a.initialization_sources.insert(page);}}
        if collision {a.memory.next=2048;}
        a.map_process_initialized_pages(2,Some(0x80000),3,Some(source)).unwrap();
        // Three preflight Frames, then one fresh resolution for each copied
        // page. Destination alias selection cannot mutate the process source.
        assert_eq!(a.lookups,6);
        for i in 0..2 {assert_eq!(a.memory.published[i].1,a.memory.pages[&(i+1)].to_vec());}
        assert_eq!(&a.memory.published[2].1[..17],&a.memory.pages[&3][..17]);
        assert!(a.memory.published[2].1[17..].iter().all(|byte|*byte==0));
    }}
    let mut a=Alpha::new();let mut source=a.source(PAGE_SIZE);source.address+=17;source.bytes=300;
    a.map_process_initialized_pages(2,Some(0x80000),1,Some(source)).unwrap();
    assert_eq!(a.lookups,2);
    assert_eq!(&a.memory.published[0].1[..300],&a.memory.pages[&1][17..317]);
    assert!(a.memory.published[0].1[300..].iter().all(|byte|*byte==0));
}

#[test]
fn cross_frame_source_keeps_chunk_resolution_and_pinned_destination() {
    let mut a=Alpha::new();let mut source=a.source(3*PAGE_SIZE);
    source.address+=PAGE_SIZE-1;source.bytes=PAGE_SIZE+2;
    a.map_process_initialized_pages(2,Some(0x80000),2,Some(source)).unwrap();
    assert_eq!(a.lookups,if swap_enabled(){11}else{7});
    let expected:Vec<_>=(PAGE_SIZE-1..2*PAGE_SIZE+1).map(|index|(index%239) as u8).collect();
    let mut actual=a.memory.published[0].1.clone();actual.extend_from_slice(&a.memory.published[1].1);
    assert_eq!(&actual[..expected.len()],expected.as_slice());
    assert!(actual[expected.len()..].iter().all(|byte|*byte==0));
}

#[test]
fn failed_pool_collision_destination_mapping_rolls_back_without_touching_source() {
    if !swap_enabled() {return;}
    let mut a=Alpha::new();let source=a.source(PAGE_SIZE);a.initialization_sources.insert(1);a.memory.next=2048;a.fail_copy=Some(1);
    let before=a.memory.pages[&1].to_vec();
    assert_eq!(a.map_process_initialized_pages(2,Some(0x80000),1,Some(source)),Err(CapabilityError::Fatal));
    assert_eq!(a.memory.pages[&1].to_vec(),before);assert!(a.initialization_sources.contains(&1));
    assert!(a.memory.published.is_empty());assert!(a.processes.find_active_physical_allocation_reference(2,0x80000,1).is_none());
    assert_eq!(a.processes.snapshots.get(),0);
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
fn owned_snapshot_initializer_copies_exact_prefix_and_zeros_without_publishing_leaves() {
    for offset in [0,3,4095] { for bytes in [1,4095,4096,4097,65537] {
        let mut a=Alpha::new();let mut source=a.source(offset+bytes);source.address+=offset;source.bytes=bytes;
        let pages=bytes.div_ceil(PAGE_SIZE)+1;
        a.validate_owned_memory_initializer(source,pages*PAGE_SIZE).unwrap();
        let first=a.memory.allocate_physical_any(pages*PAGE_SIZE).unwrap();
        a.initialize_owned_physical_pages(first,pages,source).unwrap();
        let actual:Vec<_>=(first..first+pages).flat_map(|p|a.memory.pages[&p].iter().copied()).collect();
        assert_eq!(&actual[..bytes],&(offset..offset+bytes).map(|i|(i%239) as u8).collect::<Vec<_>>());
        assert!(actual[bytes..].iter().all(|&b|b==0));
        assert!(a.memory.published.is_empty());
        KERNEL.with_borrow(|k|assert!(k.ptes.is_empty()));
    }}
}

#[test]
fn large_initializer_preflight_avoids_per_page_owner_lookups_but_copy_resolves_live_frames() {
    let mut a = Alpha::new();
    let pages = 256;
    let source = a.source(pages * PAGE_SIZE);
    a.validate_owned_memory_initializer(source, source.bytes).unwrap();
    assert_eq!(a.lookups, 0);
    let base = 0x800000;
    a.map_process_initialized_pages_protected(2, Some(base), pages, Some(source), 1).unwrap();
    assert_eq!(a.lookups, pages);
    assert_eq!(transfer_bytes(&a, 2, base, source.bytes), transfer_bytes(&a, 1, source.address, source.bytes));
    assert_eq!(a.processes.snapshots.get(), 0);
    println!("INITIALIZER-RUN source-pages={pages} per-page-resolutions={} eager-preflight-resolutions=0", a.lookups);
}

#[test]
fn owned_snapshot_initializer_uses_one_contiguous_destination_and_keeps_it_pinned() {
    let mut a = Alpha::new();
    let mut source = a.source(2 * 1024 * 1024 - 17);
    source.address += 3;
    source.bytes -= 3;
    let pages = 512;
    let first = a.memory.allocate_test_pages(pages * PAGE_SIZE, true).unwrap();
    a.initialization_sources.extend(first..first + pages);
    a.initialize_owned_physical_pages(first, pages, source).unwrap();
    assert_eq!(a.initialization_maps, 1);
    assert_eq!(a.copy_maps, pages);
    assert!(a.pinned_copies.iter().all(|&address| address == a.pinned_copies[0] && address.is_some()));
    let actual: Vec<_> = (first..first+pages).flat_map(|p| a.memory.pages[&p].iter().copied()).collect();
    assert!(actual[..source.bytes].iter().enumerate().all(|(i, &byte)| byte == ((i + 3) % 239) as u8));
    assert!(actual[source.bytes..].iter().all(|&byte| byte == 0));
    assert!(a.memory.published.is_empty());
}

#[test]
fn owned_huge_initialization_preserves_unaligned_bytes_and_zeros_small_and_large_suffixes() {
    for bytes in [17, 514 * PAGE_SIZE - 3] {
        let mut a = Alpha::new();
        let mut source = a.source(bytes + 4095);
        source.address += 4095;
        source.bytes = bytes;
        a.memory.large_enabled = true;
        a.memory.next = 1021;
        let first = a.memory.allocate_physical_any(2 * PAGE_SIZE).unwrap();
        a.memory.allocate_test_pages(512 * PAGE_SIZE, true).unwrap();
        a.memory.allocate_physical_any(3 * PAGE_SIZE).unwrap();
        assert!(a.initialize_owned_with_large_frames(first, 517, Some(source)).unwrap());
        assert_eq!((a.memory.large_alias_maps, a.memory.demotions), (1, 1));
        let actual: Vec<_> = (first..first + 517).flat_map(|page| a.memory.pages[&page].iter().copied()).collect();
        for (index, byte) in actual.iter().enumerate() {
            assert_eq!(*byte, if index < bytes { ((index + 4095) % 239) as u8 } else { 0 });
        }
        for page in first..first + 517 {
            assert_eq!(a.memory.physical_frame_descriptor_from_index(page).unwrap() & SLICE_KEY, 0);
        }
        assert!(a.memory.published.is_empty());
        assert_eq!(a.memory.freed_pages, 0);
    }
}

#[test]
fn owned_huge_initialization_leaves_warm_fallback_unmodified_and_retains_failed_work() {
    let mut a = Alpha::new();
    let source = a.source(17);
    a.memory.next = 1023;
    let first = a.memory.allocate_physical_any(512 * PAGE_SIZE).unwrap();
    a.memory.large_enabled = true;
    assert!(!a.initialize_owned_with_large_frames(first, 512, Some(source)).unwrap());
    assert_eq!((a.copy_maps, a.initialization_maps, a.memory.large_alias_maps), (0, 0, 0));
    assert!(a.memory.pages[&first].iter().all(|&byte| byte == 0xcc));
    for copy_failure in [true, false] {
        let mut a = Alpha::new();
        let source = a.source(17);
        a.memory.next = 1023;
        let first = a.memory.allocate_test_pages(512 * PAGE_SIZE, true).unwrap();
        a.memory.large_enabled = true;
        if copy_failure { a.fail_copy = Some(1); }
        else { a.memory.fail_split = Some(SplitFailure::Demote); }
        assert_eq!(a.initialize_owned_with_large_frames(first, 512, Some(source)), Err(CapabilityError::Fatal));
        assert!(a.memory.large[&first].huge);
        assert!(a.memory.published.is_empty());
        assert_eq!(a.memory.freed_pages, 0);
    }
}

#[test]
fn huge_zero_initialization_clears_exact_owned_extent_without_source_or_extra_ram() {
    for (prefix, suffix, chunks) in [(0, 0, 1), (2, 3, 1), (511, 511, 1), (2, 3, 2)] {
        let mut a = Alpha::new();
        a.memory.large_enabled = true;
        a.memory.next = 1024 - prefix - 2;
        let before = a.memory.allocate_physical_any(PAGE_SIZE).unwrap();
        let first = before + 1;
        if prefix != 0 { a.memory.allocate_physical_any(prefix * PAGE_SIZE).unwrap(); }
        for _ in 0..chunks { a.memory.allocate_test_pages(512 * PAGE_SIZE, true).unwrap(); }
        if suffix != 0 { a.memory.allocate_physical_any(suffix * PAGE_SIZE).unwrap(); }
        let after = a.memory.allocate_physical_any(PAGE_SIZE).unwrap();
        let pages = prefix + chunks * 512 + suffix;
        let allocations = a.memory.allocation_requests.clone();
        assert!(a.initialize_owned_with_large_frames(first, pages, None).unwrap());
        assert_eq!(a.memory.allocation_requests, allocations);
        assert_eq!((a.memory.large_alias_maps, a.memory.demotions), (chunks, chunks));
        assert_eq!(a.copy_maps, 0);
        assert!(a.memory.pages[&before].iter().all(|&byte| byte == 0xcc));
        assert!(a.memory.pages[&after].iter().all(|&byte| byte == 0xcc));
        for page in first..first + pages {
            assert!(a.memory.pages[&page].iter().all(|&byte| byte == 0));
            assert_eq!(a.memory.physical_frame_descriptor_from_index(page).unwrap() & SLICE_KEY, 0);
        }
        assert!(a.memory.published.is_empty());
        assert_eq!(a.memory.freed_pages, 0);
    }
}

#[test]
fn huge_zero_fallback_is_untouched_and_failed_demotion_retains_unpublished_backing() {
    for pages in [511, 512, 1024] {
        let mut a = Alpha::new();
        let first = a.memory.allocate_physical_any(pages * PAGE_SIZE).unwrap();
        assert!(!a.initialize_owned_with_large_frames(first, pages, None).unwrap());
        assert!(a.memory.pages[&first].iter().all(|&byte| byte == 0xcc));
        assert_eq!((a.copy_maps, a.memory.large_alias_maps, a.memory.demotions), (0, 0, 0));
    }
    let mut a = Alpha::new();
    a.memory.next = 1023;
    let first = a.memory.allocate_test_pages(512 * PAGE_SIZE, true).unwrap();
    a.memory.large_enabled = true;
    a.memory.fail_split = Some(SplitFailure::Demote);
    assert_eq!(a.initialize_owned_with_large_frames(first, 512, None), Err(CapabilityError::Fatal));
    assert!(a.memory.large[&first].huge);
    assert!(a.memory.published.is_empty());
    assert_eq!(a.memory.freed_pages, 0);
}

#[test]
fn owned_snapshot_source_preflight_rejects_missing_pages_overflow_and_excess_before_allocation() {
    for mode in 0..5 {
        let mut a=Alpha::new();let mut source=a.source(PAGE_SIZE);
        match mode {0=>source.address=0,1=>source.bytes=0,2=>source.bytes=PAGE_SIZE+1,3=>source.address=usize::MAX,4=>source.address+=PAGE_SIZE,_=>unreachable!()}
        let allocated=a.memory.pages.len();
        assert!(a.validate_owned_memory_initializer(source,PAGE_SIZE).is_err());
        assert_eq!(a.memory.pages.len(),allocated);assert!(a.memory.published.is_empty());
    }
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
        assert_eq!(a
            .processes
            .releasable_deferred_physical_allocations_for_pid(2)
            .is_empty(), allocation_failure);
    }
}
