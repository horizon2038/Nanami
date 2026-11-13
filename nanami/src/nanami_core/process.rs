use alloc::{boxed::Box, collections::BTreeMap, vec::Vec};

#[cfg(test)]
static VM_IMMUTABLE_LOOKUPS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
static VM_MUTABLE_LOOKUPS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[path = "process/shared_memory.rs"]
mod shared_memory;
pub use shared_memory::SharedMemoryReservation;
use shared_memory::SharedMemorySpace;
#[path = "process/root_slots.rs"]
mod root_slots;
pub use root_slots::{install_notification_alias, RootSlotTypes};
#[path = "process/physical_allocations.rs"]
mod physical_allocations;
use physical_allocations::{AllocationState, PhysicalAllocations};

use crate::nanami_core::elf_loader::ElfImage;
use crate::nanami_core::vm_space::{BootstrapVmSpace, VmSpace};
use nun::{CapabilityDescriptor, CapabilityError, Word};

pub const PROCESS_ROOT_SLOT_BASE: usize = 200;
pub const MAX_IO_RANGES_PER_PROCESS: usize = 16;
pub const MAX_IRQS_PER_PROCESS: usize = 8;
pub const A9N_CPU_COUNT_MAX: Word = 64;

/// Diagnostic A/B switch only: keep Alpha and every spawned process on the BSP
/// without changing the kernel's SMP build, online CPU count or IRQ routing.
/// An unset value (including "0") retains the normal round-robin policy.
pub const SAME_CORE_DIAGNOSTIC: bool = match option_env!("NANAMI_SAME_CORE_DIAGNOSTIC") {
    Some(value) => value.len() == 1 && value.as_bytes()[0] == b'1',
    None => false,
};

const INVALID_IRQ: Word = usize::MAX;
const USER_PAGE_SIZE: usize = 4096;

static mut ALPHA_VM_SPACE: BootstrapVmSpace = BootstrapVmSpace::new();

#[derive(Clone, Copy)]
pub struct IoPortRange {
    pub min: Word,
    pub max: Word,
}

impl IoPortRange {
    pub const EMPTY: Self = Self { min: 0, max: 0 };
}

#[derive(Clone, Copy)]
pub struct ProcessEntry {
    pub root_types: RootSlotTypes,
    pub used: bool,
    pub pid: usize,
    pub reaper_pid: usize,
    // A thread owns its PCB/root, but shares this process's VM and frame arena.
    pub memory_owner: usize,
    pub deferred_vm_reap: bool,
    pub root_slot: usize,
    pub root_node: CapabilityDescriptor,
    pub pcb: CapabilityDescriptor,
    pub address_space: CapabilityDescriptor,
    pub os_port: CapabilityDescriptor,
    pub os_port_identifier: Word,
    pub affinity: Word,
    pub platform_information_access: bool,
    pub service_admin: bool,
    pub resource_quarantined: bool,
    /// The executing task sees an empty capability root, not `root_node`.
    /// Alpha's private management root and its objects remain alive unchanged.
    pub capabilities_sealed: bool,
    pub irq_count: usize,
    pub irq_numbers: [Word; MAX_IRQS_PER_PROCESS],
    pub io_range_count: usize,
    pub io_ranges: [IoPortRange; MAX_IO_RANGES_PER_PROCESS],
    pub next_frame_slot: usize,
    pub user_heap_next_va: usize,
    pub user_heap_limit_va: usize,
    pub exited: bool,
    pub exit_is_ok: Word,
    pub exit_code: Word,
}

#[derive(Clone, Copy)]
pub struct ProcessMemoryContext {
    pub root_node: CapabilityDescriptor,
    pub address_space: CapabilityDescriptor,
}

#[derive(Clone, Copy)]
pub struct ProcessStatistics {
    pub running: usize,
    pub exited: usize,
}

impl ProcessEntry {
    pub fn has_irq(&self) -> bool {
        self.irq_count > 0
    }

    pub fn has_irq_number(&self, irq_number: Word) -> bool {
        let mut i = 0usize;
        while i < self.irq_count {
            if self.irq_numbers[i] == irq_number {
                return true;
            }
            i += 1;
        }
        false
    }
}

const PROCESS_ENTRY_HINTS: usize = 64;
const PROCESS_VM_HINTS: usize = 64;
const LAZY_MAPPING_BUCKETS: usize = 64;
const PROCESS_FRAME_HINTS: usize = 64;

fn frame_chunk_bucket(pid: usize, chunk: usize) -> usize {
    pid.wrapping_add(chunk.wrapping_mul(17)) % PROCESS_FRAME_HINTS
}

#[cfg(test)]
static LAZY_MAPPING_PROBES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static FRAME_CHUNK_PROBES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

pub struct ProcessManager {
    alpha_entry: ProcessEntry,
    alpha_vm_space: *mut BootstrapVmSpace,
    next_pid: usize,
    online_core_count: Word,
    next_affinity: Word,
    root_slot_limit: usize,
    reserved_root_slots: &'static [usize],
    // Sorted storage remains authoritative. Hints contain index+1, never a
    // pointer or ownership/authorization state; every hit checks exact PID/used.
    entries: Vec<ProcessEntry>,
    entry_hints: [usize; PROCESS_ENTRY_HINTS],
    used_root_slots: Vec<usize>,
    vm_spaces: Vec<ProcessVmSpace>,
    vm_hints: [usize; PROCESS_VM_HINTS],
    frame_chunks: Vec<ProcessFrameChunk>,
    frame_chunk_hints: [core::cell::Cell<usize>; PROCESS_FRAME_HINTS],
    physical_allocations: PhysicalAllocations,
    shared_memory: BTreeMap<usize, SharedMemorySpace>,
    lazy_mappings: Vec<ProcessLazyMappingEntry>,
    // Ordered index+1 chains; exact owner PID and range remain authoritative.
    lazy_heads: [usize; LAZY_MAPPING_BUCKETS],
    lazy_tails: [usize; LAZY_MAPPING_BUCKETS],
}

struct ProcessVmSpace {
    pid: usize,
    space: Box<VmSpace>,
}

struct ProcessFrameChunk {
    pid: usize,
    chunk_index: usize,
}

#[derive(Clone, Copy)]
pub enum ProcessLazyMappingKind {
    Image { image: &'static [u8], elf: ElfImage },
    Zero,
}

#[derive(Clone, Copy)]
pub struct ProcessLazyMapping {
    pub base_va: usize,
    pub page_count: usize,
    pub start_slot: usize,
    pub kind: ProcessLazyMappingKind,
}

struct ProcessLazyMappingEntry {
    pid: usize,
    next_in_bucket: usize,
    mapping: ProcessLazyMapping,
    // Only fresh-root stack registration may mint unused-slot claims. A bit
    // is consumed before any capability operation, never inferred from the
    // absence of an active/deferred physical record.
    fresh_zero_slots: Option<(CapabilityDescriptor, u64)>,
}

/// Single-use request-local claim on an original, never-used stack Frame slot.
/// No Clone/Copy: consume before constructing the page, within the same event.
pub struct FreshZeroFrameSlot {
    owner: usize,
    base: usize,
    root: CapabilityDescriptor,
    space: CapabilityDescriptor,
    slot: usize,
}

impl FreshZeroFrameSlot {
    pub fn into_parts(self, owner: usize, base: usize)
        -> Option<(CapabilityDescriptor, CapabilityDescriptor, usize)>
    {
        (self.owner == owner && self.base == base).then_some((self.root, self.space, self.slot))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ProcessPhysicalAllocation {
    pub base_va: usize,
    pub start_slot: usize,
    pub base_page: usize,
    pub page_count: usize,
    // Ownership of RAM is independent of a copied capability in the task root.
    // Sealed snapshot aliases may map the permanent Alpha pool Frame directly.
    pub owns_frame_cap: bool,
}

/// Request-local ownership handle. Never retain across event-loop iterations.
#[derive(Clone, Copy)]
pub struct PhysicalPageReference {
    pub allocation: ProcessPhysicalAllocation,
    owner: usize,
    index: usize,
}

impl ProcessManager {
    pub fn new_alpha(
        alpha_root_node: CapabilityDescriptor,
        alpha_address_space: CapabilityDescriptor,
        alpha_pcb: CapabilityDescriptor,
        alpha_os_port: CapabilityDescriptor,
        root_slot_limit: usize,
        reserved_root_slots: &'static [usize],
        online_core_count: Word,
    ) -> Result<Self, CapabilityError> {
        if !(1..=A9N_CPU_COUNT_MAX).contains(&online_core_count) {
            crate::error!(
                "[smp.err] invalid Alpha environment core_count={}",
                online_core_count
            );
            return Err(CapabilityError::InvalidArgument);
        }
        crate::info!("process: ProcessManager::new_alpha");
        let alpha_entry = ProcessEntry {
            root_types: RootSlotTypes::empty(),
            used: true,
            pid: 0,
            reaper_pid: 0,
            memory_owner: 0,
            deferred_vm_reap: false,
            root_slot: 0,
            root_node: alpha_root_node,
            pcb: alpha_pcb,
            address_space: alpha_address_space,
            os_port: alpha_os_port,
            os_port_identifier: 0,
            affinity: 0,
            platform_information_access: false,
            service_admin: false,
            resource_quarantined: false,
            capabilities_sealed: false,
            irq_count: 0,
            irq_numbers: [INVALID_IRQ; MAX_IRQS_PER_PROCESS],
            io_range_count: 0,
            io_ranges: [IoPortRange::EMPTY; MAX_IO_RANGES_PER_PROCESS],
            next_frame_slot: 0,
            user_heap_next_va: 0,
            user_heap_limit_va: 0,
            exited: false,
            exit_is_ok: 0,
            exit_code: 0,
        };
        let alpha_vm_space = core::ptr::addr_of_mut!(ALPHA_VM_SPACE);
        Ok(Self {
            alpha_entry,
            alpha_vm_space,
            next_pid: 1,
            online_core_count,
            next_affinity: if online_core_count > 1 { 1 } else { 0 },
            root_slot_limit,
            reserved_root_slots,
            entries: Vec::new(),
            entry_hints: [0; PROCESS_ENTRY_HINTS],
            used_root_slots: Vec::new(),
            vm_spaces: Vec::new(),
            vm_hints: [0; PROCESS_VM_HINTS],
            frame_chunks: Vec::new(),
            frame_chunk_hints: core::array::from_fn(|_| core::cell::Cell::new(0)),
            physical_allocations: PhysicalAllocations::default(),
            shared_memory: BTreeMap::new(),
            lazy_mappings: Vec::new(),
            lazy_heads: [0; LAZY_MAPPING_BUCKETS],
            lazy_tails: [0; LAZY_MAPPING_BUCKETS],
        })
    }

    pub fn alpha_vm_space_mut(&mut self) -> &mut BootstrapVmSpace {
        unsafe { &mut *self.alpha_vm_space }
    }

    pub fn alpha_entry(&self) -> ProcessEntry {
        self.alpha_entry
    }

    pub fn online_core_count(&self) -> Word {
        self.online_core_count
    }

    pub fn allocate_affinity(&mut self) -> Word {
        if SAME_CORE_DIAGNOSTIC || self.online_core_count <= 1 {
            return 0;
        }

        let affinity = if self.next_affinity == 0 || self.next_affinity >= self.online_core_count {
            1
        } else {
            self.next_affinity
        };
        self.next_affinity = affinity + 1;
        if self.next_affinity >= self.online_core_count {
            self.next_affinity = 1;
        }
        affinity
    }

    pub fn active_core_mask(&self) -> Word {
        let mut mask = if self.alpha_entry.used && !self.alpha_entry.exited {
            1usize << self.alpha_entry.affinity
        } else {
            0
        };
        for entry in self.entries.iter() {
            if entry.used && !entry.exited && entry.affinity < Word::BITS as usize {
                mask |= 1usize << entry.affinity;
            }
        }
        mask
    }

    pub fn vm_space_mut(&mut self, pid: usize) -> Option<&mut VmSpace> {
        #[cfg(test)]
        VM_MUTABLE_LOOKUPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let pid = self.memory_owner_pid(pid);
        let index = self.vm_index(pid)?;
        Some(self.vm_spaces[index].space.as_mut())
    }

    pub fn vm_space(&self, pid: usize) -> Option<&VmSpace> {
        #[cfg(test)]
        VM_IMMUTABLE_LOOKUPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let pid = self.memory_owner_pid(pid);
        let index = self.vm_index(pid)?;
        Some(self.vm_spaces[index].space.as_ref())
    }

    fn hinted_vm_index(&self, pid: usize) -> Option<usize> {
        let index = self.vm_hints[pid % PROCESS_VM_HINTS].checked_sub(1)?;
        self.vm_spaces.get(index).filter(|vm| vm.pid == pid).map(|_| index)
    }

    // Exact row PID, without owner resolution. Installation and failed-spawn
    // cleanup also manipulate rows before a ProcessEntry exists.
    fn vm_index(&self, pid: usize) -> Option<usize> {
        self.hinted_vm_index(pid).or_else(||
            self.vm_spaces.binary_search_by_key(&pid, |vm| vm.pid).ok())
    }

    fn remove_vm_space(&mut self, pid: usize) {
        if let Some(index) = self.vm_index(pid) {
            self.vm_spaces.remove(index);
            for hint in &mut self.vm_hints {
                if *hint == index + 1 { *hint = 0; }
                else if *hint > index + 1 { *hint -= 1; }
            }
        }
    }

    #[cfg(test)]
    pub fn vm_hint_hit(&self, pid: usize) -> bool { self.hinted_vm_index(pid).is_some() }

    #[cfg(test)]
    pub fn take_immutable_vm_lookups(&self) -> usize {
        VM_IMMUTABLE_LOOKUPS.swap(0, core::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(test)]
    pub fn take_mutable_vm_lookups(&self) -> usize {
        VM_MUTABLE_LOOKUPS.swap(0, core::sync::atomic::Ordering::Relaxed)
    }

    pub fn ensure_vm_space_for_pid(&mut self, pid: usize) -> Result<(), CapabilityError> {
        if pid == 0 {
            return Ok(());
        }
        let location = match self.hinted_vm_index(pid) {
            Some(index) => Ok(index),
            None => self.vm_spaces.binary_search_by_key(&pid, |vm| vm.pid),
        };
        let index = match location {
            Ok(index) => index,
            Err(index) => {
                self.vm_spaces.insert(
                    index,
                    ProcessVmSpace {
                        pid,
                        space: Box::new(VmSpace::new()),
                    },
                );
                for hint in &mut self.vm_hints { if *hint > index { *hint += 1; } }
                index
            }
        };
        self.vm_hints[pid % PROCESS_VM_HINTS] = index + 1;
        Ok(())
    }

    fn entry_mut_by_pid(&mut self, pid: usize) -> Option<&mut ProcessEntry> {
        if pid == 0 {
            return Some(&mut self.alpha_entry);
        }
        let index = self.entry_index(pid)?;
        self.entries.get_mut(index).filter(|entry| entry.used)
    }

    pub fn has_frame_chunk(&self, pid: usize, chunk_index: usize) -> bool {
        let pid = self.memory_owner_pid(pid);
        let hint = &self.frame_chunk_hints[frame_chunk_bucket(pid, chunk_index)];
        let slot = hint.get();
        if slot != 0 {
            #[cfg(test)]
            FRAME_CHUNK_PROBES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            // The current record, not a cached key, establishes residency.
            // Retain/reuse may move rows; stale positions simply miss here.
            if self.frame_chunks.get(slot - 1).is_some_and(|chunk|
                chunk.pid == pid && chunk.chunk_index == chunk_index)
            {
                return true;
            }
        }
        let found = self.frame_chunks
            .iter()
            .position(|chunk| {
                #[cfg(test)]
                FRAME_CHUNK_PROBES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                chunk.pid == pid && chunk.chunk_index == chunk_index
            });
        if let Some(index) = found {
            hint.set(index + 1);
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    pub fn take_frame_chunk_probes() -> usize {
        FRAME_CHUNK_PROBES.swap(0, core::sync::atomic::Ordering::Relaxed)
    }

    pub fn register_frame_chunk(
        &mut self,
        pid: usize,
        chunk_index: usize,
    ) -> Result<(), CapabilityError> {
        let pid = self.memory_owner_pid(pid);
        if self.has_frame_chunk(pid, chunk_index) {
            return Ok(());
        }
        self.frame_chunks
            .push(ProcessFrameChunk { pid, chunk_index });
        self.frame_chunk_hints[frame_chunk_bucket(pid, chunk_index)].set(self.frame_chunks.len());
        Ok(())
    }

    pub fn frame_chunk_indices(&self, pid: usize) -> impl Iterator<Item = usize> + '_ {
        self.frame_chunks
            .iter()
            .filter(move |chunk| chunk.pid == pid)
            .map(|chunk| chunk.chunk_index)
    }

    pub fn register_physical_allocation(
        &mut self,
        pid: usize,
        base_va: usize,
        start_slot: usize,
        base_page: usize,
        page_count: usize,
    ) -> Result<(), CapabilityError> {
        self.register_physical_allocation_with_cap(pid, base_va, start_slot, base_page, page_count, true)
    }

    pub fn register_pool_alias(
        &mut self, pid: usize, base_va: usize, slot: usize, physical: usize,
    ) -> Result<(), CapabilityError> {
        self.register_pool_allocation(pid, base_va, slot, physical, 1)
    }

    pub fn register_pool_allocation(
        &mut self, pid: usize, base_va: usize, slot: usize, physical: usize, pages: usize,
    ) -> Result<(), CapabilityError> {
        self.register_physical_allocation_with_cap(pid, base_va, slot, physical, pages, false)
    }

    /// Register a contiguous pool run as individual page references so later
    /// partial release, demotion and COW retain their existing granularity.
    pub fn register_pool_page_run(
        &mut self, pid: usize, base_va: usize, slot: usize, physical: usize, pages: usize,
    ) -> Result<(), CapabilityError> {
        let end = physical.checked_add(pages).ok_or(CapabilityError::InvalidArgument)?;
        self.register_pool_aliases(pid, base_va, slot, physical..end)
    }

    /// Register independently releasable 4KiB aliases in ascending VA order.
    pub fn register_pool_aliases(
        &mut self, pid: usize, base_va: usize, slot: usize,
        physical_pages: impl ExactSizeIterator<Item = usize>,
    ) -> Result<(), CapabilityError> {
        let pages = physical_pages.len();
        let pid = self.memory_owner_pid(pid);
        if pid == 0 || pages == 0 || base_va & (USER_PAGE_SIZE - 1) != 0
            || pages.checked_mul(USER_PAGE_SIZE).and_then(|bytes| base_va.checked_add(bytes)).is_none()
            || slot.checked_add(pages).is_none()
        {
            return Err(CapabilityError::InvalidArgument);
        }
        self.physical_allocations.insert_many(AllocationState::Active, pid, physical_pages.enumerate().map(|(page, physical)| {
            ProcessPhysicalAllocation {
                base_va: base_va + page * USER_PAGE_SIZE,
                start_slot: slot + page,
                base_page: physical,
                page_count: 1,
                owns_frame_cap: false,
            }
        }));
        Ok(())
    }

    fn register_physical_allocation_with_cap(
        &mut self, pid: usize, base_va: usize, start_slot: usize, base_page: usize,
        page_count: usize, owns_frame_cap: bool,
    ) -> Result<(), CapabilityError> {
        let pid = self.memory_owner_pid(pid);
        if pid == 0 || page_count == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        self.physical_allocations.insert(
            AllocationState::Active,
            pid,
            ProcessPhysicalAllocation {
                base_va,
                start_slot,
                base_page,
                page_count,
                owns_frame_cap,
            },
        );
        Ok(())
    }

    pub fn register_lazy_mapping(
        &mut self,
        pid: usize,
        base_va: usize,
        page_count: usize,
        start_slot: usize,
        kind: ProcessLazyMappingKind,
    ) -> Result<(), CapabilityError> {
        let pid = self.memory_owner_pid(pid);
        if pid == 0 || page_count == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let index = self.lazy_mappings.len();
        self.lazy_mappings.push(ProcessLazyMappingEntry {
            pid,
            next_in_bucket: 0,
            fresh_zero_slots: None,
            mapping: ProcessLazyMapping {
                base_va,
                page_count,
                start_slot,
                kind,
            },
        });
        self.link_lazy_mapping(index);
        Ok(())
    }

    fn link_lazy_mapping(&mut self, index: usize) {
        let bucket = self.lazy_mappings[index].pid % LAZY_MAPPING_BUCKETS;
        self.lazy_mappings[index].next_in_bucket = 0;
        let previous = self.lazy_tails[bucket];
        if previous == 0 {
            self.lazy_heads[bucket] = index + 1;
        } else {
            self.lazy_mappings[previous - 1].next_in_bucket = index + 1;
        }
        self.lazy_tails[bucket] = index + 1;
    }

    fn rebuild_lazy_mapping_buckets(&mut self) {
        self.lazy_heads.fill(0);
        self.lazy_tails.fill(0);
        for index in 0..self.lazy_mappings.len() { self.link_lazy_mapping(index); }
    }

    /// Trusted fresh-root construction only: these original slots must never
    /// have held Frame caps. Exec must revoke the old Frame directory first.
    /// The fixed initial stack has at most 64 pages; other zero plans retain
    /// the ordinary fresh-slot constructor and cannot mint this claim.
    pub fn register_fresh_stack_mapping(
        &mut self, pid: usize, root: CapabilityDescriptor, base: usize, pages: usize, first: usize,
    ) -> Result<(), CapabilityError> {
        if root == 0 || pages == 0 || pages > 64 || base & (USER_PAGE_SIZE - 1) != 0
            || pages.checked_mul(USER_PAGE_SIZE).and_then(|size| base.checked_add(size)).is_none()
            || first.checked_add(pages).is_none()
        { return Err(CapabilityError::InvalidArgument); }
        // A child that could run with its native root may have populated caps
        // outside Alpha's records before being sealed. Only a root already
        // sealed when this fresh directory was registered can retain claims.
        let track = self.find_entry_by_pid(pid).is_some_and(|entry|
            entry.capabilities_sealed && !entry.exited && !entry.resource_quarantined
                && entry.memory_owner == pid && entry.root_node == root);
        self.register_lazy_mapping(pid, base, pages, first, ProcessLazyMappingKind::Zero)?;
        if track {
            self.lazy_mappings.last_mut().unwrap().fresh_zero_slots =
                Some((root, if pages == 64 { u64::MAX } else { (1u64 << pages) - 1 }));
        }
        Ok(())
    }

    pub fn take_fresh_stack_frame_slot(
        &mut self, pid: usize, va: usize, max_slots: usize,
    ) -> Option<FreshZeroFrameSlot> {
        let owner = self.memory_owner_pid(pid);
        let entry = self.find_entry_by_pid(owner)?;
        if !entry.capabilities_sealed || entry.exited || entry.resource_quarantined
            || entry.root_node == 0 || entry.address_space == 0 {
            return None;
        }
        let (index, base, first) = self.find_lazy_mapping_plan(pid, va)?;
        let page = (va - base) / USER_PAGE_SIZE;
        let slot = first.checked_add(page)?;
        if slot >= entry.next_frame_slot || slot >= max_slots { return None; }
        if va & (USER_PAGE_SIZE - 1) != 0 || page >= 64 { return None; }
        // Nothing has mutated the mapping list or dispatched between plan
        // resolution and this reborrow. Consume this exact validated record.
        let mapping = &mut self.lazy_mappings[index];
        if !matches!(mapping.mapping.kind, ProcessLazyMappingKind::Zero) { return None; }
        let (recorded_root, available) = mapping.fresh_zero_slots.as_mut()?;
        if *recorded_root != entry.root_node { return None; }
        let bit = 1u64 << page;
        if *available & bit == 0 { return None; }
        // Burn before any fallible capability operation can use the token.
        *available &= !bit;
        Some(FreshZeroFrameSlot { owner, base: va, root: entry.root_node, space: entry.address_space, slot })
    }

    pub fn find_lazy_mapping(
        &self,
        pid: usize,
        virtual_address: usize,
    ) -> Option<ProcessLazyMapping> {
        self.find_lazy_mapping_entry(pid, virtual_address).map(|(_, mapping)| *mapping)
    }

    /// Request-local location plus scalar construction fields. Callers must
    /// not mutate lazy_mappings before consuming the borrowed kind at index.
    pub fn find_lazy_mapping_plan(&self, pid: usize, virtual_address: usize) -> Option<(usize, usize, usize)> {
        self.find_lazy_mapping_entry(pid, virtual_address)
            .map(|(index, mapping)| (index, mapping.base_va, mapping.start_slot))
    }

    pub fn lazy_mapping_kind(&self, index: usize) -> Option<&ProcessLazyMappingKind> {
        self.lazy_mappings.get(index).map(|entry| &entry.mapping.kind)
    }

    /// One plan only when every sampled page has the same first-match entry.
    /// Earlier partial overlaps and malformed owner entries decline the hint;
    /// callers can retain their ordinary per-page lookup in those cases.
    pub fn uniform_lazy_mapping_plan(&self, pid: usize, start: usize, end: usize)
        -> Option<(usize, usize, usize)> {
        if start >= end { return None; }
        let last = start + ((end - start - 1) / USER_PAGE_SIZE) * USER_PAGE_SIZE;
        let pid = self.memory_owner_pid(pid);
        let mut next = self.lazy_heads[pid % LAZY_MAPPING_BUCKETS];
        while next != 0 {
            let index = next - 1;
            let entry = &self.lazy_mappings[index];
            next = entry.next_in_bucket;
            #[cfg(test)]
            LAZY_MAPPING_PROBES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if entry.pid != pid { continue; }
            let base = entry.mapping.base_va;
            let limit = entry.mapping.page_count.checked_mul(USER_PAGE_SIZE)
                .and_then(|bytes| base.checked_add(bytes))?;
            let first = if base <= start {
                start
            } else {
                let pages = (base - start - 1) / USER_PAGE_SIZE + 1;
                let Some(first) = pages.checked_mul(USER_PAGE_SIZE)
                    .and_then(|offset| start.checked_add(offset)) else { continue; };
                first
            };
            if first < end && first < limit {
                return (base <= start && last < limit)
                    .then_some((index, base, entry.mapping.start_slot));
            }
        }
        None
    }

    /// Pre-install image construction has no runnable child or concurrent
    /// event dispatch. Prove the whole aligned range has no existing Frame
    /// before omitting each page's presence/installed-zero/plan preflight.
    /// The returned indices remain valid only while the lazy list is stable.
    pub fn preinstall_lazy_mapping_plan(&self, pid: usize, start: usize, end: usize)
        -> Option<(usize, usize, usize)> {
        if start >= end || (start | end) & (USER_PAGE_SIZE - 1) != 0
            || self.memory_entry(pid).is_some()
            || self.vm_space(pid)?.has_backed_aligned_pages(start, end)
        {
            return None;
        }
        self.uniform_lazy_mapping_plan(pid, start, end)
    }

    fn find_lazy_mapping_entry(&self, pid: usize, virtual_address: usize) -> Option<(usize, &ProcessLazyMapping)> {
        let pid = self.memory_owner_pid(pid);
        let mut next = self.lazy_heads[pid % LAZY_MAPPING_BUCKETS];
        while next != 0 {
            let index = next - 1;
            let entry = &self.lazy_mappings[index];
            next = entry.next_in_bucket;
            #[cfg(test)]
            LAZY_MAPPING_PROBES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if entry.pid != pid {
                continue;
            }
            let start = entry.mapping.base_va;
            let bytes = entry.mapping.page_count.checked_mul(USER_PAGE_SIZE)?;
            let end = start.checked_add(bytes)?;
            if virtual_address >= start && virtual_address < end {
                return Some((index, &entry.mapping));
            }
        }
        None
    }

    #[cfg(test)]
    pub fn take_lazy_mapping_probes() -> usize {
        LAZY_MAPPING_PROBES.swap(0, core::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(test)]
    pub fn linear_lazy_mapping_plan_for_test(&self, pid: usize, va: usize) -> Option<(usize, usize, usize)> {
        let pid = self.memory_owner_pid(pid);
        for (index, entry) in self.lazy_mappings.iter().enumerate() {
            if entry.pid != pid { continue; }
            let end = entry.mapping.page_count.checked_mul(USER_PAGE_SIZE)
                .and_then(|size| entry.mapping.base_va.checked_add(size))?;
            if va >= entry.mapping.base_va && va < end {
                return Some((index, entry.mapping.base_va, entry.mapping.start_slot));
            }
        }
        None
    }

    /// Equivalent to testing find_lazy_mapping at start, start + PAGE_SIZE, ...
    /// below end, but resolve the memory owner and scan the mapping list once.
    /// Preserve point lookup's early stop on a malformed owner entry. No lazy
    /// page is materialized, and no result is retained across requests.
    pub fn has_lazy_mapping_at_pages(&self, pid: usize, start: usize, end: usize) -> bool {
        if start >= end { return false; }
        let pid = self.memory_owner_pid(pid);
        let mut next = self.lazy_heads[pid % LAZY_MAPPING_BUCKETS];
        while next != 0 {
            let entry = &self.lazy_mappings[next - 1];
            next = entry.next_in_bucket;
            if entry.pid != pid { continue; }
            let Some(mapping_end) = entry.mapping.page_count.checked_mul(USER_PAGE_SIZE)
                .and_then(|bytes| entry.mapping.base_va.checked_add(bytes)) else {
                return false;
            };
            let first = if entry.mapping.base_va <= start {
                start
            } else {
                let delta = entry.mapping.base_va - start;
                let pages = (delta - 1) / USER_PAGE_SIZE + 1;
                let Some(first) = pages.checked_mul(USER_PAGE_SIZE)
                    .and_then(|offset| start.checked_add(offset)) else { continue; };
                first
            };
            if first < end && first < mapping_end { return true; }
        }
        false
    }

    pub fn drop_image_lazy_mappings_for_pid(&mut self, pid: usize) {
        self.lazy_mappings.retain(|entry| {
            entry.pid != pid || !matches!(entry.mapping.kind, ProcessLazyMappingKind::Image { .. })
        });
        self.rebuild_lazy_mapping_buckets();
    }

    pub fn releasable_physical_allocations_for_pid(
        &self,
        pid: usize,
    ) -> Vec<ProcessPhysicalAllocation> {
        self.physical_allocations
            .releasable(AllocationState::Active, pid)
    }

    /// Retire only this PID's active references, not a shared-VM owner's records.
    /// Deferred references must stay counted until their own cleanup completes.
    pub fn drop_active_physical_allocations_for_pid(&mut self, pid: usize) {
        self.physical_allocations
            .remove_pid(AllocationState::Active, pid, |_, _| {});
    }

    pub fn drop_physical_allocations_for_pid(&mut self, pid: usize) {
        self.shared_memory.remove(&pid);
        self.drop_active_physical_allocations_for_pid(pid);
        self.physical_allocations
            .remove_pid(AllocationState::Deferred, pid, |_, _| {});
    }

    pub fn defer_physical_allocation_for_pid(
        &mut self,
        pid: usize,
        allocation: ProcessPhysicalAllocation,
    ) {
        self.defer_physical_allocations_for_pid(pid, core::iter::once(allocation));
    }

    /// Prepend in iterator order, retaining an independent physical reference
    /// for every page, including the unpublished suffix of a failed batch.
    pub fn defer_physical_allocations_for_pid(
        &mut self,
        pid: usize,
        allocations: impl Iterator<Item = ProcessPhysicalAllocation>,
    ) {
        let pid = self.memory_owner_pid(pid);
        self.physical_allocations
            .insert_many(AllocationState::Deferred, pid, allocations);
    }

    /// After all leaves/capabilities are gone, retain RAM ownership until each
    /// fallible release commits. This is deliberately PID-local, not VM-wide.
    pub fn defer_all_active_physical_allocations_for_pid(&mut self, pid: usize) {
        self.physical_allocations.defer_all(pid);
    }

    /// A deferred allocation still owns its RAM. Move the existing record so
    /// peers cannot mistake this state transition for their last reference.
    pub fn defer_active_physical_allocation(
        &mut self,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) -> Result<ProcessPhysicalAllocation, CapabilityError> {
        let pid = self.memory_owner_pid(pid);
        self.physical_allocations
            .defer(pid, base_va, page_count)
            .ok_or(CapabilityError::InvalidArgument)
    }

    pub fn try_defer_physical_pages(
        &mut self, pid: usize, start: usize, end: usize,
        pages: &[ProcessPhysicalAllocation],
    ) -> bool {
        if pages.len() <= 1 { return false; }
        let pid = self.memory_owner_pid(pid);
        self.physical_allocations.try_defer_pages(pid, start, end, pages)
    }

    pub fn try_retire_physical_pages<E>(
        &mut self, pid: usize, start: usize, end: usize,
        pages: &[ProcessPhysicalAllocation],
        release: impl FnMut(ProcessPhysicalAllocation, bool) -> Result<(), E>,
    ) -> Result<bool, E> {
        if pages.len() <= 1 { return Ok(false); }
        let pid = self.memory_owner_pid(pid);
        self.physical_allocations.try_retire_pages(pid, start, end, pages, release)
    }

    pub fn take_deferred_physical_allocation(
        &mut self,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) -> Option<ProcessPhysicalAllocation> {
        let pid = self.memory_owner_pid(pid);
        self.physical_allocations
            .take_unique_deferred(pid, base_va, page_count)
    }

    pub fn releasable_deferred_physical_allocations_for_pid(
        &self,
        pid: usize,
    ) -> Vec<ProcessPhysicalAllocation> {
        self.physical_allocations
            .releasable(AllocationState::Deferred, pid)
    }

    pub fn deferred_physical_allocations_for_pid(
        &self,
        pid: usize,
    ) -> impl Iterator<Item = ProcessPhysicalAllocation> + '_ {
        self.physical_allocations
            .for_pid(AllocationState::Deferred, pid)
    }

    pub fn drop_deferred_physical_allocations_for_pid(&mut self, pid: usize) {
        self.physical_allocations
            .remove_pid(AllocationState::Deferred, pid, |_, _| {});
    }

    pub fn retire_deferred_physical_allocations(
        &mut self,
        pid: usize,
        release: impl FnMut(ProcessPhysicalAllocation, bool) -> Result<(), CapabilityError>,
    ) -> Result<(), CapabilityError> {
        self.physical_allocations
            .try_remove_pid(AllocationState::Deferred, pid, release)
    }

    pub fn reset_runtime_memory_for_exec(
        &mut self,
        pid: usize,
        next_frame_slot: usize,
        user_heap_next_va: usize,
        user_heap_limit_va: usize,
    ) -> Result<Vec<(ProcessPhysicalAllocation, bool)>, CapabilityError> {
        let Some(entry) = self.entry_mut_by_pid(pid) else {
            return Err(CapabilityError::InvalidArgument);
        };
        entry.next_frame_slot = next_frame_slot;
        entry.user_heap_next_va = user_heap_next_va;
        entry.user_heap_limit_va = user_heap_limit_va;
        entry.exited = false;
        entry.exit_is_ok = 0;
        entry.exit_code = 0;
        self.lazy_mappings.retain(|mapping| mapping.pid != pid);
        self.rebuild_lazy_mapping_buckets();
        self.shared_memory.remove(&pid);
        if let Some(vm) = self.vm_space_mut(pid) {
            *vm = VmSpace::new();
        }
        let mut allocations = Vec::new();
        self.physical_allocations
            .remove_pid(AllocationState::Active, pid, |allocation, last| {
                allocations.push((allocation, last));
            });
        Ok(allocations)
    }

    pub fn find_active_physical_allocation_reference(
        &self,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) -> Option<ProcessPhysicalAllocation> {
        let pid = self.memory_owner_pid(pid);
        self.physical_allocations
            .find(AllocationState::Active, pid, base_va, page_count)
    }

    pub fn release_physical_allocation_reference(
        &mut self,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) -> Result<(ProcessPhysicalAllocation, bool), CapabilityError> {
        let pid = self.memory_owner_pid(pid);
        self.physical_allocations
            .take(AllocationState::Active, pid, base_va, page_count)
            .ok_or(CapabilityError::InvalidArgument)
    }

    pub fn retire_active_physical_allocation(
        &mut self, pid: usize, base: usize, pages: usize,
        release: impl FnOnce(ProcessPhysicalAllocation, bool) -> Result<(), CapabilityError>,
    ) -> Result<(), CapabilityError> {
        self.physical_allocations.try_take(
            AllocationState::Active, self.memory_owner_pid(pid), base, pages, release,
        )?.ok_or(CapabilityError::InvalidArgument).map(|_| ())
    }

    /// Snapshot one-page ownership references for a request-local range walk.
    /// Mapping release and ELF staging avoid a full search for every page.
    pub fn physical_pages_in_range(
        &self,
        pid: usize,
        base_va: usize,
        end_va: usize,
    ) -> Vec<ProcessPhysicalAllocation> {
        let pid = self.memory_owner_pid(pid);
        let mut pages = Vec::new();
        let mut descending = true;
        let mut previous = None;
        for allocation in self
            .physical_allocations
            .pages_in_range(pid, base_va, end_va)
            .filter(|allocation| {
                allocation.page_count == 1
                    && allocation.base_va >= base_va
                    && allocation.base_va < end_va
                    && (allocation.base_va - base_va) % USER_PAGE_SIZE == 0
            })
        {
            if previous.is_some_and(|va| va <= allocation.base_va) { descending = false; }
            previous = Some(allocation.base_va);
            pages.push(allocation);
        }
        // Stable sorting retains the newest reference at a duplicated VA,
        // matching find_active_physical_allocation_reference's existing rule.
        // Strict descending order also proves uniqueness. Normal sequential
        // page construction prepends these nodes; reverse without sort scratch.
        // Duplicates/other order retain stable sorting and newest-VA selection.
        if descending { pages.reverse(); }
        else {
            pages.sort_by_key(|allocation| allocation.base_va);
            pages.dedup_by_key(|allocation| allocation.base_va);
        }
        pages
    }

    /// Bounded, request-local optimization metadata. Allocation failure, excess
    /// rows or duplicate VAs decline the shortcut rather than limit mappings.
    pub fn try_physical_pages_in_range(
        &self, pid: usize, start: usize, end: usize, limit: usize,
    ) -> Option<Vec<ProcessPhysicalAllocation>> {
        let count = end.checked_sub(start)?.checked_add(USER_PAGE_SIZE - 1)? / USER_PAGE_SIZE;
        let capacity = count.min(limit);
        let mut pages = Vec::new();
        pages.try_reserve_exact(capacity).ok()?;
        for allocation in self.physical_allocations.pages_in_range(self.memory_owner_pid(pid), start, end) {
            if allocation.page_count == 1 && allocation.base_va >= start && allocation.base_va < end
                && (allocation.base_va - start) % USER_PAGE_SIZE == 0
            {
                if pages.len() == capacity {
                    return None;
                }
                pages.push(allocation);
            }
        }
        pages.sort_unstable_by_key(|allocation| allocation.base_va);
        if pages.windows(2).any(|pair| pair[0].base_va == pair[1].base_va) {
            return None;
        }
        Some(pages)
    }

    pub fn private_page_references(
        &self,
        pid: usize,
        start: usize,
        end: usize,
    ) -> Vec<PhysicalPageReference> {
        self.physical_allocations
            .page_references(self.memory_owner_pid(pid), start, end)
    }

    #[cfg(test)]
    pub fn take_physical_page_range_visits(&self) -> usize {
        self.physical_allocations.take_range_visits()
    }

    #[cfg(test)]
    pub fn take_physical_page_point_visits(&self) -> usize {
        self.physical_allocations.take_point_visits()
    }

    pub fn private_page_reference_walk(
        &self, pid: usize, start: usize, end: usize,
    ) -> impl Iterator<Item = PhysicalPageReference> + '_ {
        self.physical_allocations.page_reference_walk(self.memory_owner_pid(pid), start, end)
    }

    /// Request-local capacity hint for a source already checked as fully
    /// backed. Collection and all caller-side ownership validation still run.
    pub fn private_page_references_reserved(
        &self, pid: usize, start: usize, end: usize,
    ) -> Vec<PhysicalPageReference> {
        let capacity = end.checked_sub(start).map_or(0, |bytes| bytes / USER_PAGE_SIZE);
        self.physical_allocations.page_references_reserved(
            self.memory_owner_pid(pid), start, end, capacity,
        )
    }

    pub fn physical_reference_count(&self, allocation: ProcessPhysicalAllocation) -> usize {
        self.physical_allocations.count(allocation)
    }

    /// Commit a copied private page without another search or refcount gap.
    pub fn replace_private_backing(
        &mut self,
        reference: PhysicalPageReference,
        replacement: ProcessPhysicalAllocation,
    ) -> Result<bool, CapabilityError> {
        self.physical_allocations
            .replace_backing(reference, replacement)
            .ok_or(CapabilityError::Fatal)
    }

    pub fn install_process(
        &mut self,
        pid: usize,
        reaper_pid: usize,
        root_slot: usize,
        root_node: CapabilityDescriptor,
        pcb: CapabilityDescriptor,
        address_space: CapabilityDescriptor,
        os_port: CapabilityDescriptor,
        os_port_identifier: Word,
        affinity: Word,
        next_frame_slot: usize,
        user_heap_next_va: usize,
        user_heap_limit_va: usize,
    ) -> Result<(), CapabilityError> {
        let entry = ProcessEntry {
            root_types: RootSlotTypes::native(),
            used: true,
            pid,
            reaper_pid,
            memory_owner: pid,
            deferred_vm_reap: false,
            root_slot,
            root_node,
            pcb,
            address_space,
            os_port,
            os_port_identifier,
            affinity,
            platform_information_access: false,
            service_admin: false,
            resource_quarantined: false,
            capabilities_sealed: false,
            irq_count: 0,
            irq_numbers: [INVALID_IRQ; MAX_IRQS_PER_PROCESS],
            io_range_count: 0,
            io_ranges: [IoPortRange::EMPTY; MAX_IO_RANGES_PER_PROCESS],
            next_frame_slot,
            user_heap_next_va,
            user_heap_limit_va,
            exited: false,
            exit_is_ok: 0,
            exit_code: 0,
        };

        if pid == 0 {
            self.alpha_entry = entry;
            return Ok(());
        }

        let index = match self.entries.binary_search_by_key(&pid, |entry| entry.pid) {
            Ok(index) => { self.entries[index] = entry; index }
            Err(index) => {
                self.entries.insert(index, entry);
                for hint in &mut self.entry_hints {
                    if *hint > index { *hint += 1; }
                }
                index
            }
        };
        self.entry_hints[pid % PROCESS_ENTRY_HINTS] = index + 1;
        Ok(())
    }

    pub fn alloc_process_slot(&mut self) -> Result<(usize, usize), CapabilityError> {
        let mut slot = PROCESS_ROOT_SLOT_BASE;
        while slot < self.root_slot_limit {
            if !self.is_root_slot_reserved(slot) && !self.used_root_slots.contains(&slot) {
                self.used_root_slots.push(slot);
                let pid = self.next_pid;
                self.next_pid = self
                    .next_pid
                    .checked_add(1)
                    .ok_or(CapabilityError::InvalidArgument)?;
                return Ok((pid, slot));
            }
            slot += 1;
        }
        Err(CapabilityError::InvalidArgument)
    }

    pub fn set_platform_information_access(
        &mut self,
        pid: usize,
        allowed: bool,
    ) -> Result<(), CapabilityError> {
        let entry = self
            .entry_mut_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        entry.platform_information_access = allowed;
        Ok(())
    }

    pub fn release_process_slot_reservation(&mut self, root_slot: usize) {
        if let Some(slot_index) = self
            .used_root_slots
            .iter()
            .position(|slot| *slot == root_slot)
        {
            self.used_root_slots.swap_remove(slot_index);
        }
    }

    pub fn set_service_admin(&mut self, pid: usize, allowed: bool) -> Result<(), CapabilityError> {
        let entry = self
            .entry_mut_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        entry.service_admin = allowed;
        Ok(())
    }

    pub fn seal_capabilities(&mut self, pid: usize) -> Result<(), CapabilityError> {
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        self.entry_mut_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?
            .capabilities_sealed = true;
        Ok(())
    }

    pub fn quarantine_resources(&mut self, pid: usize) -> Result<(), CapabilityError> {
        self.entry_mut_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?
            .resource_quarantined = true;
        Ok(())
    }

    pub fn discard_process_artifacts(&mut self, pid: usize, root_slot: usize) {
        self.release_process_slot_reservation(root_slot);
        self.remove_vm_space(pid);
        self.frame_chunks.retain(|chunk| chunk.pid != pid);
        self.lazy_mappings.retain(|mapping| mapping.pid != pid);
        self.rebuild_lazy_mapping_buckets();
        self.drop_physical_allocations_for_pid(pid);
    }

    fn is_root_slot_reserved(&self, slot: usize) -> bool {
        self.reserved_root_slots
            .iter()
            .any(|reserved| *reserved == slot)
    }

    pub fn find_entry_by_pid(&self, pid: usize) -> Option<ProcessEntry> {
        self.entry_by_pid(pid).copied()
    }

    fn entry_by_pid(&self, pid: usize) -> Option<&ProcessEntry> {
        if pid == 0 && self.alpha_entry.used {
            return Some(&self.alpha_entry);
        }
        let index = self.entry_index(pid)?;
        self.entries.get(index).filter(|entry| entry.used)
    }

    fn hinted_entry_index(&self, pid: usize) -> Option<usize> {
        let index = self.entry_hints[pid % PROCESS_ENTRY_HINTS].checked_sub(1)?;
        self.entries.get(index)
            .filter(|entry| entry.used && entry.pid == pid).map(|_| index)
    }

    fn entry_index(&self, pid: usize) -> Option<usize> {
        self.hinted_entry_index(pid).or_else(||
            self.entries.binary_search_by_key(&pid, |entry| entry.pid).ok())
    }

    #[cfg(test)]
    pub fn entry_hint_hit(&self, pid: usize) -> bool {
        pid != 0 && self.hinted_entry_index(pid).is_some()
    }

    pub fn root_types_mut(&mut self, pid: usize) -> Result<&mut RootSlotTypes, CapabilityError> {
        self.entry_mut_by_pid(pid)
            .filter(|entry| pid != 0 && !entry.exited)
            .map(|entry| &mut entry.root_types)
            .ok_or(CapabilityError::PermissionDenied)
    }

    pub fn memory_owner_pid(&self, pid: usize) -> usize {
        self.entry_by_pid(pid)
            .map_or(pid, |entry| entry.memory_owner)
    }

    pub fn memory_entry(&self, pid: usize) -> Option<ProcessEntry> {
        self.memory_entry_ref(pid).copied()
    }

    fn memory_entry_ref(&self, pid: usize) -> Option<&ProcessEntry> {
        let entry = self.entry_by_pid(pid)?;
        if entry.memory_owner == pid { Some(entry) }
        else { self.entry_by_pid(entry.memory_owner) }
    }

    /// Current one-hop owner's mapping descriptors, without the task's IRQ/I/O
    /// arrays. This metadata view does not replace per-page authority checks.
    pub fn memory_context(&self, pid: usize) -> Option<ProcessMemoryContext> {
        let owner = self.memory_entry_ref(pid)?;
        Some(ProcessMemoryContext {
            root_node: owner.root_node,
            address_space: owner.address_space,
        })
    }

    /// Scalar view of the current memory owner's root. Keep the full owned
    /// snapshot API for callers that need it, without copying IRQ/I/O arrays.
    pub fn memory_root_node(&self, pid: usize) -> Option<CapabilityDescriptor> {
        self.memory_entry_ref(pid).map(|entry| entry.root_node)
    }

    pub fn share_address_space(
        &mut self,
        pid: usize,
        source: usize,
    ) -> Result<(), CapabilityError> {
        let owner = self.memory_owner_pid(source);
        self.entry_mut_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?
            .memory_owner = owner;
        Ok(())
    }

    /// Commit only after a stopped shared task's PCB uses its new private AS.
    /// Allocate the empty tracker before that irreversible kernel operation.
    pub fn prepare_private_exec(&mut self, pid: usize) -> Result<(), CapabilityError> {
        let entry = self.entry_by_pid(pid).ok_or(CapabilityError::InvalidArgument)?;
        if entry.memory_owner == pid || !entry.capabilities_sealed || entry.next_frame_slot != 0
            || self.lazy_mappings.iter().any(|mapping| mapping.pid == pid)
            || self.shared_memory.contains_key(&pid)
            || self.physical_allocations.for_pid(AllocationState::Active, pid).next().is_some()
            || self.physical_allocations.for_pid(AllocationState::Deferred, pid).next().is_some()
            || self.vm_spaces.iter().any(|vm| vm.pid == pid
                && (vm.space.has_backed_pages(0, usize::MAX)
                    || vm.space.has_uncertain_mappings(0, usize::MAX)))
        {
            return Err(CapabilityError::IllegalOperation);
        }
        self.ensure_vm_space_for_pid(pid)
    }

    pub fn commit_private_exec(&mut self, pid: usize) -> Result<(), CapabilityError> {
        // The caller has completed prepare_private_exec and PCB configuration.
        // Never redirect a task to an unallocated tracker.
        if !self.vm_spaces.iter().any(|vm| vm.pid == pid) {
            return Err(CapabilityError::IllegalOperation);
        }
        self.entry_mut_by_pid(pid).ok_or(CapabilityError::InvalidArgument)?.memory_owner = pid;
        Ok(())
    }

    pub fn has_memory_peers(&self, pid: usize) -> bool {
        let owner = self.memory_owner_pid(pid);
        self.entries
            .iter()
            .any(|entry| entry.pid != pid && entry.memory_owner == owner)
    }

    pub fn defer_vm_reap(&mut self, pid: usize) {
        if let Some(entry) = self.entry_mut_by_pid(pid) {
            entry.deferred_vm_reap = true;
        }
    }

    pub fn entries(&self) -> impl Iterator<Item = ProcessEntry> + '_ {
        core::iter::once(self.alpha_entry)
            .chain(self.entries.iter().copied())
            .filter(|entry| entry.used)
    }

    pub fn statistics(&self) -> ProcessStatistics {
        let mut running = 0usize;
        let mut exited = 0usize;
        for entry in self.entries.iter() {
            if !entry.used {
                continue;
            }
            if entry.exited {
                exited += 1;
            } else {
                running += 1;
            }
        }
        ProcessStatistics { running, exited }
    }

    pub fn mark_exited(
        &mut self,
        pid: usize,
        is_ok: Word,
        exit_code: Word,
    ) -> Result<(), CapabilityError> {
        let entry = self
            .entry_mut_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        entry.exited = true;
        entry.exit_is_ok = is_ok;
        entry.exit_code = exit_code;
        Ok(())
    }

    pub fn reap_process(
        &mut self,
        pid: usize,
        release_root_slot: bool,
    ) -> Result<(), CapabilityError> {
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let Ok(index) = self.entries.binary_search_by_key(&pid, |entry| entry.pid) else {
            return Err(CapabilityError::InvalidArgument);
        };
        if !self.entries[index].used {
            return Err(CapabilityError::InvalidArgument);
        }
        if !self.entries[index].exited {
            return Err(CapabilityError::IllegalOperation);
        }
        if self.entries[index].resource_quarantined {
            return Err(CapabilityError::Fatal);
        }
        let root_slot = self.entries[index].root_slot;
        self.entries.remove(index);
        for hint in &mut self.entry_hints {
            if *hint == index + 1 { *hint = 0; }
            else if *hint > index + 1 { *hint -= 1; }
        }

        if release_root_slot {
            if let Some(slot_index) = self
                .used_root_slots
                .iter()
                .position(|slot| *slot == root_slot)
            {
                self.used_root_slots.swap_remove(slot_index);
            }
        }
        self.remove_vm_space(pid);
        self.frame_chunks.retain(|chunk| chunk.pid != pid);
        self.lazy_mappings.retain(|mapping| mapping.pid != pid);
        self.rebuild_lazy_mapping_buckets();
        self.drop_physical_allocations_for_pid(pid);
        Ok(())
    }

    pub fn assign_irq_to_pid(
        &mut self,
        pid: usize,
        irq_number: Word,
    ) -> Result<(), CapabilityError> {
        if self.alpha_entry.used
            && self.alpha_entry.pid != pid
            && self.alpha_entry.has_irq_number(irq_number)
        {
            return Err(CapabilityError::PermissionDenied);
        }
        for entry in self.entries.iter() {
            if entry.used && entry.pid != pid && entry.has_irq_number(irq_number) {
                return Err(CapabilityError::PermissionDenied);
            }
        }

        if let Some(entry) = self.entry_mut_by_pid(pid) {
            if entry.has_irq_number(irq_number) {
                return Ok(());
            }
            if entry.irq_count >= MAX_IRQS_PER_PROCESS {
                return Err(CapabilityError::PermissionDenied);
            }
            let slot = entry.irq_count;
            entry.irq_numbers[slot] = irq_number;
            entry.irq_count += 1;
            return Ok(());
        }

        Err(CapabilityError::InvalidArgument)
    }

    pub fn unassign_irq_from_pid(
        &mut self,
        pid: usize,
        irq_number: Word,
    ) -> Result<(), CapabilityError> {
        if let Some(entry) = self.entry_mut_by_pid(pid) {
            let mut j = 0usize;
            while j < entry.irq_count {
                if entry.irq_numbers[j] == irq_number {
                    let last = entry.irq_count - 1;
                    entry.irq_numbers[j] = entry.irq_numbers[last];
                    entry.irq_numbers[last] = INVALID_IRQ;
                    entry.irq_count -= 1;
                    return Ok(());
                }
                j += 1;
            }
            return Err(CapabilityError::InvalidArgument);
        }
        Err(CapabilityError::InvalidArgument)
    }

    pub fn add_io_range_to_pid(
        &mut self,
        pid: usize,
        min: Word,
        max: Word,
    ) -> Result<(), CapabilityError> {
        if min > max {
            return Err(CapabilityError::InvalidArgument);
        }

        if let Some(entry) = self.entry_mut_by_pid(pid) {
            if entry.io_range_count >= MAX_IO_RANGES_PER_PROCESS {
                return Err(CapabilityError::InvalidArgument);
            }

            let mut k = 0;
            while k < entry.io_range_count {
                let r = entry.io_ranges[k];
                let overlaps = !(max < r.min || r.max < min);
                if overlaps {
                    return Err(CapabilityError::PermissionDenied);
                }
                k += 1;
            }

            entry.io_ranges[entry.io_range_count] = IoPortRange { min, max };
            entry.io_range_count += 1;
            return Ok(());
        }

        Err(CapabilityError::InvalidArgument)
    }

    pub fn reserve_process_heap(
        &mut self,
        pid: usize,
        page_count: usize,
        page_size: usize,
        max_frame_slots: usize,
    ) -> Result<(CapabilityDescriptor, CapabilityDescriptor, usize, usize), CapabilityError> {
        self.reserve_process_heap_preferred_alignment(pid, page_count, page_size, max_frame_slots, 1)
    }

    /// Prefer a larger virtual alignment without consuming backing or slots
    /// for padding. If it cannot fit, preserve the ordinary reservation rather
    /// than rejecting a request that still fits at the current heap cursor.
    pub(crate) fn reserve_process_heap_preferred_alignment(
        &mut self,
        pid: usize,
        page_count: usize,
        page_size: usize,
        max_frame_slots: usize,
        alignment: usize,
    ) -> Result<(CapabilityDescriptor, CapabilityDescriptor, usize, usize), CapabilityError> {
        let pid = self.memory_owner_pid(pid);
        if page_count == 0 || page_size == 0 || !alignment.is_power_of_two() {
            return Err(CapabilityError::InvalidArgument);
        }
        let bytes = page_count
            .checked_mul(page_size)
            .ok_or(CapabilityError::InvalidArgument)?;

        if let Some(entry) = self.entry_mut_by_pid(pid) {
            let next_slot = entry.next_frame_slot.checked_add(page_count)
                .filter(|end| *end <= max_frame_slots)
                .ok_or(CapabilityError::InvalidArgument)?;
            if entry.user_heap_next_va == 0 || entry.user_heap_limit_va <= entry.user_heap_next_va {
                return Err(CapabilityError::InvalidArgument);
            }

            let heap_base = entry.user_heap_next_va.checked_add(alignment - 1)
                .map(|base| base & !(alignment - 1))
                .filter(|base| base.checked_add(bytes)
                    .is_some_and(|end| end <= entry.user_heap_limit_va))
                .unwrap_or(entry.user_heap_next_va);
            let heap_end = heap_base
                .checked_add(bytes)
                .ok_or(CapabilityError::InvalidArgument)?;
            if heap_end > entry.user_heap_limit_va {
                return Err(CapabilityError::InvalidArgument);
            }

            let start_slot = entry.next_frame_slot;
            entry.next_frame_slot = next_slot;
            entry.user_heap_next_va = heap_end;
            return Ok((entry.root_node, entry.address_space, heap_base, start_slot));
        }

        Err(CapabilityError::InvalidArgument)
    }

    pub fn reserve_process_heap_at(
        &mut self,
        pid: usize,
        base_va: usize,
        page_count: usize,
        page_size: usize,
        max_frame_slots: usize,
    ) -> Result<(CapabilityDescriptor, CapabilityDescriptor, usize), CapabilityError> {
        let pid = self.memory_owner_pid(pid);
        if base_va == 0 || page_count == 0 || page_size == 0 || (base_va & (page_size - 1)) != 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let bytes = page_count
            .checked_mul(page_size)
            .ok_or(CapabilityError::InvalidArgument)?;
        let end_va = base_va
            .checked_add(bytes)
            .ok_or(CapabilityError::InvalidArgument)?;

        if let Some(entry) = self.entry_mut_by_pid(pid) {
            if entry.next_frame_slot + page_count > max_frame_slots {
                return Err(CapabilityError::InvalidArgument);
            }
            let covers_cursor = base_va <= entry.user_heap_next_va && entry.user_heap_next_va < end_va;
            if covers_cursor && end_va > entry.user_heap_limit_va {
                return Err(CapabilityError::InvalidArgument);
            }
            let start_slot = entry.next_frame_slot;
            entry.next_frame_slot += page_count;
            // A fixed allocation covering the automatic cursor consumes that
            // range too. Never hand the next automatic allocation a VA inside
            // the newly reserved brk/private mapping. Disjoint fixed ranges
            // leave the cursor unchanged.
            if covers_cursor {
                entry.user_heap_next_va = end_va;
            }
            let result = (entry.root_node, entry.address_space, start_slot);
            self.exclude_shared_virtual_range(pid, base_va, page_count);
            return Ok(result);
        }

        Err(CapabilityError::InvalidArgument)
    }

    /// Exec's image slots were reserved before population. Reuse only slots
    /// with no active/deferred owner, without advancing the allocation cursor.
    pub fn preallocated_process_frame_slots(
        &self, pid: usize, first: usize, pages: usize, max_slots: usize,
    ) -> Option<(CapabilityDescriptor, CapabilityDescriptor)> {
        let entry = self.find_entry_by_pid(pid)?;
        let end = first.checked_add(pages)?;
        if pages == 0 || entry.memory_owner != pid || end > entry.next_frame_slot || end > max_slots {
            return None;
        }
        let occupied = self.physical_allocations.for_pid(AllocationState::Active, pid)
            .chain(self.physical_allocations.for_pid(AllocationState::Deferred, pid))
            .any(|allocation| allocation.start_slot.checked_add(allocation.page_count)
                .is_none_or(|limit| first < limit && end > allocation.start_slot));
        if occupied { return None; }
        Some((entry.root_node, entry.address_space))
    }

    pub fn reserve_process_virtual_gap(
        &mut self,
        pid: usize,
        page_count: usize,
        page_size: usize,
    ) -> Result<usize, CapabilityError> {
        let pid = self.memory_owner_pid(pid);
        if page_count == 0 || page_size == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let bytes = page_count
            .checked_mul(page_size)
            .ok_or(CapabilityError::InvalidArgument)?;

        if let Some(entry) = self.entry_mut_by_pid(pid) {
            if entry.user_heap_next_va == 0 || entry.user_heap_limit_va <= entry.user_heap_next_va {
                return Err(CapabilityError::InvalidArgument);
            }

            let gap_base = entry.user_heap_next_va;
            let gap_end = gap_base
                .checked_add(bytes)
                .ok_or(CapabilityError::InvalidArgument)?;
            if gap_end > entry.user_heap_limit_va {
                return Err(CapabilityError::InvalidArgument);
            }

            entry.user_heap_next_va = gap_end;
            return Ok(gap_base);
        }

        Err(CapabilityError::InvalidArgument)
    }
}
