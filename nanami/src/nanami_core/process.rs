use alloc::{boxed::Box, vec::Vec};

use crate::nanami_core::vm_space::{BootstrapVmSpace, VmSpace};
use nun::{CapabilityDescriptor, CapabilityError, Word};

pub const PROCESS_ROOT_SLOT_BASE: usize = 200;
pub const MAX_IO_RANGES_PER_PROCESS: usize = 16;
pub const MAX_IRQS_PER_PROCESS: usize = 8;

const INVALID_IRQ: Word = usize::MAX;

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
    pub used: bool,
    pub pid: usize,
    pub reaper_pid: usize,
    pub root_slot: usize,
    pub root_node: CapabilityDescriptor,
    pub pcb: CapabilityDescriptor,
    pub address_space: CapabilityDescriptor,
    pub os_port: CapabilityDescriptor,
    pub os_port_identifier: Word,
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

pub struct ProcessManager {
    alpha_entry: ProcessEntry,
    alpha_vm_space: *mut BootstrapVmSpace,
    next_pid: usize,
    root_slot_limit: usize,
    reserved_root_slots: &'static [usize],
    entries: Vec<ProcessEntry>,
    used_root_slots: Vec<usize>,
    vm_spaces: Vec<ProcessVmSpace>,
    frame_chunks: Vec<ProcessFrameChunk>,
    physical_allocations: Vec<ProcessPhysicalAllocationEntry>,
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
pub struct ProcessPhysicalAllocation {
    pub base_va: usize,
    pub start_slot: usize,
    pub base_page: usize,
    pub page_count: usize,
}

struct ProcessPhysicalAllocationEntry {
    pid: usize,
    allocation: ProcessPhysicalAllocation,
}

impl ProcessManager {
    pub fn new_alpha(
        alpha_root_node: CapabilityDescriptor,
        alpha_address_space: CapabilityDescriptor,
        alpha_os_port: CapabilityDescriptor,
        root_slot_limit: usize,
        reserved_root_slots: &'static [usize],
    ) -> Self {
        crate::info!("process: ProcessManager::new_alpha");
        let alpha_entry = ProcessEntry {
            used: true,
            pid: 0,
            reaper_pid: 0,
            root_slot: 0,
            root_node: alpha_root_node,
            pcb: 0,
            address_space: alpha_address_space,
            os_port: alpha_os_port,
            os_port_identifier: 0,
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
        Self {
            alpha_entry,
            alpha_vm_space,
            next_pid: 1,
            root_slot_limit,
            reserved_root_slots,
            entries: Vec::new(),
            used_root_slots: Vec::new(),
            vm_spaces: Vec::new(),
            frame_chunks: Vec::new(),
            physical_allocations: Vec::new(),
        }
    }

    pub fn alpha_vm_space_mut(&mut self) -> &mut BootstrapVmSpace {
        unsafe { &mut *self.alpha_vm_space }
    }

    pub fn alpha_entry(&self) -> ProcessEntry {
        self.alpha_entry
    }

    pub fn vm_space_mut(&mut self, pid: usize) -> Option<&mut VmSpace> {
        for vm in self.vm_spaces.iter_mut() {
            if vm.pid == pid {
                return Some(vm.space.as_mut());
            }
        }
        None
    }

    pub fn ensure_vm_space_for_pid(&mut self, pid: usize) -> Result<(), CapabilityError> {
        if pid == 0 {
            return Ok(());
        }
        if self.vm_space_mut(pid).is_some() {
            return Ok(());
        }
        self.vm_spaces.push(ProcessVmSpace {
            pid,
            space: Box::new(VmSpace::new()),
        });
        Ok(())
    }

    fn entry_mut_by_pid(&mut self, pid: usize) -> Option<&mut ProcessEntry> {
        if pid == 0 {
            return Some(&mut self.alpha_entry);
        }
        self.entries
            .iter_mut()
            .find(|entry| entry.used && entry.pid == pid)
    }

    pub fn has_frame_chunk(&self, pid: usize, chunk_index: usize) -> bool {
        self.frame_chunks
            .iter()
            .any(|chunk| chunk.pid == pid && chunk.chunk_index == chunk_index)
    }

    pub fn register_frame_chunk(
        &mut self,
        pid: usize,
        chunk_index: usize,
    ) -> Result<(), CapabilityError> {
        if self.has_frame_chunk(pid, chunk_index) {
            return Ok(());
        }
        self.frame_chunks
            .push(ProcessFrameChunk { pid, chunk_index });
        Ok(())
    }

    pub fn register_physical_allocation(
        &mut self,
        pid: usize,
        base_va: usize,
        start_slot: usize,
        base_page: usize,
        page_count: usize,
    ) -> Result<(), CapabilityError> {
        if pid == 0 || page_count == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        self.physical_allocations
            .push(ProcessPhysicalAllocationEntry {
                pid,
                allocation: ProcessPhysicalAllocation {
                    base_va,
                    start_slot,
                    base_page,
                    page_count,
                },
            });
        Ok(())
    }

    pub fn releasable_physical_allocations_for_pid(
        &self,
        pid: usize,
    ) -> Vec<ProcessPhysicalAllocation> {
        let mut allocations = Vec::new();
        for entry in self.physical_allocations.iter() {
            if entry.pid == pid {
                let mut ref_count = 0usize;
                for candidate in self.physical_allocations.iter() {
                    if candidate.allocation.base_page == entry.allocation.base_page
                        && candidate.allocation.page_count == entry.allocation.page_count
                    {
                        ref_count += 1;
                    }
                }
                if ref_count == 1 {
                    allocations.push(entry.allocation);
                }
            }
        }
        allocations
    }

    pub fn drop_physical_allocations_for_pid(&mut self, pid: usize) {
        self.physical_allocations.retain(|entry| entry.pid != pid);
    }

    pub fn find_active_physical_allocation_reference(
        &self,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) -> Option<ProcessPhysicalAllocation> {
        for entry in self.physical_allocations.iter() {
            if entry.pid == pid
                && entry.allocation.base_va == base_va
                && entry.allocation.page_count == page_count
            {
                return Some(entry.allocation);
            }
        }
        None
    }

    pub fn release_physical_allocation_reference(
        &mut self,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) -> Result<(ProcessPhysicalAllocation, bool), CapabilityError> {
        let Some(index) = self.physical_allocations.iter().position(|entry| {
            entry.pid == pid
                && entry.allocation.base_va == base_va
                && entry.allocation.page_count == page_count
        }) else {
            return Err(CapabilityError::InvalidArgument);
        };

        let allocation = self.physical_allocations[index].allocation;
        let mut ref_count = 0usize;
        for entry in self.physical_allocations.iter() {
            if entry.allocation.base_page == allocation.base_page
                && entry.allocation.page_count == allocation.page_count
            {
                ref_count += 1;
            }
        }
        self.physical_allocations.swap_remove(index);
        Ok((allocation, ref_count == 1))
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
        next_frame_slot: usize,
        user_heap_next_va: usize,
        user_heap_limit_va: usize,
    ) -> Result<(), CapabilityError> {
        let entry = ProcessEntry {
            used: true,
            pid,
            reaper_pid,
            root_slot,
            root_node,
            pcb,
            address_space,
            os_port,
            os_port_identifier,
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

        if let Some(existing) = self.entry_mut_by_pid(pid) {
            *existing = entry;
            return Ok(());
        }

        self.entries.push(entry);
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

    fn is_root_slot_reserved(&self, slot: usize) -> bool {
        self.reserved_root_slots
            .iter()
            .any(|reserved| *reserved == slot)
    }

    pub fn find_entry_by_pid(&self, pid: usize) -> Option<ProcessEntry> {
        if pid == 0 && self.alpha_entry.used {
            return Some(self.alpha_entry);
        }
        for entry in self.entries.iter() {
            if entry.used && entry.pid == pid {
                return Some(*entry);
            }
        }
        None
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
        let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.used && entry.pid == pid)
        else {
            return Err(CapabilityError::InvalidArgument);
        };
        if !self.entries[index].exited {
            return Err(CapabilityError::IllegalOperation);
        }
        let root_slot = self.entries[index].root_slot;
        self.entries.swap_remove(index);

        if release_root_slot {
            if let Some(slot_index) = self
                .used_root_slots
                .iter()
                .position(|slot| *slot == root_slot)
            {
                self.used_root_slots.swap_remove(slot_index);
            }
        }
        if let Some(vm_index) = self.vm_spaces.iter().position(|vm| vm.pid == pid) {
            self.vm_spaces.swap_remove(vm_index);
        }
        self.frame_chunks.retain(|chunk| chunk.pid != pid);
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
        if page_count == 0 || page_size == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let bytes = page_count
            .checked_mul(page_size)
            .ok_or(CapabilityError::InvalidArgument)?;

        if let Some(entry) = self.entry_mut_by_pid(pid) {
            if entry.next_frame_slot + page_count > max_frame_slots {
                return Err(CapabilityError::InvalidArgument);
            }
            if entry.user_heap_next_va == 0 || entry.user_heap_limit_va <= entry.user_heap_next_va {
                return Err(CapabilityError::InvalidArgument);
            }

            let heap_base = entry.user_heap_next_va;
            let heap_end = heap_base
                .checked_add(bytes)
                .ok_or(CapabilityError::InvalidArgument)?;
            if heap_end > entry.user_heap_limit_va {
                return Err(CapabilityError::InvalidArgument);
            }

            let start_slot = entry.next_frame_slot;
            entry.next_frame_slot += page_count;
            entry.user_heap_next_va = heap_end;
            return Ok((entry.root_node, entry.address_space, heap_base, start_slot));
        }

        Err(CapabilityError::InvalidArgument)
    }

    pub fn reserve_process_virtual_gap(
        &mut self,
        pid: usize,
        page_count: usize,
        page_size: usize,
    ) -> Result<usize, CapabilityError> {
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
