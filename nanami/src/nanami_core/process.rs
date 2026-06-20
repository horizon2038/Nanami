use crate::nanami_core::vm_space::VmSpace;
use nun::{CapabilityDescriptor, CapabilityError, Word};

pub const PROCESS_ROOT_SLOT_BASE: usize = 200;
pub const PROCESS_ROOT_SLOT_LIMIT: usize = 1024;
pub const TOTAL_PROCESS_SLOTS: usize = PROCESS_ROOT_SLOT_LIMIT - PROCESS_ROOT_SLOT_BASE;
pub const MAX_ACTIVE_PROCESSES: usize = 64;
pub const MAX_IO_RANGES_PER_PROCESS: usize = 16;
pub const MAX_IRQS_PER_PROCESS: usize = 8;

const INVALID_PID: usize = usize::MAX;
const INVALID_IRQ: Word = usize::MAX;

static mut VM_SPACE_POOL: [VmSpace; MAX_ACTIVE_PROCESSES] = [VmSpace::new(); MAX_ACTIVE_PROCESSES];

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
}

impl ProcessEntry {
    pub const EMPTY: Self = Self {
        used: false,
        pid: INVALID_PID,
        root_node: 0,
        pcb: 0,
        address_space: 0,
        os_port: 0,
        os_port_identifier: 0,
        irq_count: 0,
        irq_numbers: [INVALID_IRQ; MAX_IRQS_PER_PROCESS],
        io_range_count: 0,
        io_ranges: [IoPortRange::EMPTY; MAX_IO_RANGES_PER_PROCESS],
        next_frame_slot: 0,
        user_heap_next_va: 0,
        user_heap_limit_va: 0,
    };

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
    pub entries: [ProcessEntry; MAX_ACTIVE_PROCESSES],
    used_slots: [bool; TOTAL_PROCESS_SLOTS],
    vm_owner: [usize; MAX_ACTIVE_PROCESSES],
}

impl ProcessManager {
    pub fn new_alpha(
        alpha_root_node: CapabilityDescriptor,
        alpha_address_space: CapabilityDescriptor,
        alpha_os_port: CapabilityDescriptor,
    ) -> Self {
        crate::info!("process: ProcessManager::new_alpha");
        let mut entries = [ProcessEntry::EMPTY; MAX_ACTIVE_PROCESSES];
        entries[0] = ProcessEntry {
            used: true,
            pid: 0,
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
        };
        let mut used_slots = [false; TOTAL_PROCESS_SLOTS];
        used_slots[0] = true;
        let mut vm_owner = [INVALID_PID; MAX_ACTIVE_PROCESSES];
        vm_owner[0] = 0;
        unsafe {
            VM_SPACE_POOL[0] = VmSpace::new();
        }
        Self {
            entries,
            used_slots,
            vm_owner,
        }
    }

    pub fn alpha_vm_space_mut(&mut self) -> &mut VmSpace {
        unsafe { &mut VM_SPACE_POOL[0] }
    }

    pub fn alpha_entry(&self) -> ProcessEntry {
        self.entries[0]
    }

    pub fn vm_space_mut(&mut self, pid: usize) -> Option<&mut VmSpace> {
        let mut i = 0;
        while i < MAX_ACTIVE_PROCESSES {
            if self.vm_owner[i] == pid {
                unsafe {
                    return Some(&mut VM_SPACE_POOL[i]);
                }
            }
            i += 1;
        }
        None
    }

    pub fn ensure_vm_space_for_pid(&mut self, pid: usize) -> Result<(), CapabilityError> {
        if self.vm_space_mut(pid).is_some() {
            return Ok(());
        }
        let mut i = 0;
        while i < MAX_ACTIVE_PROCESSES {
            if self.vm_owner[i] == INVALID_PID {
                self.vm_owner[i] = pid;
                unsafe {
                    VM_SPACE_POOL[i] = VmSpace::new();
                }
                return Ok(());
            }
            i += 1;
        }
        Err(CapabilityError::InvalidArgument)
    }

    pub fn install_process(
        &mut self,
        pid: usize,
        root_node: CapabilityDescriptor,
        pcb: CapabilityDescriptor,
        address_space: CapabilityDescriptor,
        os_port: CapabilityDescriptor,
        os_port_identifier: Word,
        next_frame_slot: usize,
        user_heap_next_va: usize,
        user_heap_limit_va: usize,
    ) -> Result<(), CapabilityError> {
        if pid >= TOTAL_PROCESS_SLOTS {
            return Err(CapabilityError::InvalidArgument);
        }

        let mut i = 0;
        while i < MAX_ACTIVE_PROCESSES {
            if self.entries[i].used && self.entries[i].pid == pid {
                self.entries[i] = ProcessEntry {
                    used: true,
                    pid,
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
                };
                return Ok(());
            }
            i += 1;
        }

        let mut j = 0;
        while j < MAX_ACTIVE_PROCESSES {
            if !self.entries[j].used {
                self.entries[j] = ProcessEntry {
                    used: true,
                    pid,
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
                };
                return Ok(());
            }
            j += 1;
        }
        Err(CapabilityError::InvalidArgument)
    }

    pub fn alloc_process_slot(&mut self) -> Result<usize, CapabilityError> {
        let mut pid = 1usize;
        while pid < TOTAL_PROCESS_SLOTS {
            if !self.used_slots[pid] {
                self.used_slots[pid] = true;
                return Ok(pid);
            }
            pid += 1;
        }
        Err(CapabilityError::InvalidArgument)
    }

    pub fn find_entry_by_pid(&self, pid: usize) -> Option<ProcessEntry> {
        let mut i = 0;
        while i < MAX_ACTIVE_PROCESSES {
            let e = self.entries[i];
            if e.used && e.pid == pid {
                return Some(e);
            }
            i += 1;
        }
        None
    }

    pub fn assign_irq_to_pid(
        &mut self,
        pid: usize,
        irq_number: Word,
    ) -> Result<(), CapabilityError> {
        let mut i = 0;
        while i < MAX_ACTIVE_PROCESSES {
            if self.entries[i].used
                && self.entries[i].pid != pid
                && self.entries[i].has_irq_number(irq_number)
            {
                return Err(CapabilityError::PermissionDenied);
            }
            i += 1;
        }

        let mut j = 0;
        while j < MAX_ACTIVE_PROCESSES {
            if self.entries[j].used && self.entries[j].pid == pid {
                if self.entries[j].has_irq_number(irq_number) {
                    return Ok(());
                }
                if self.entries[j].irq_count >= MAX_IRQS_PER_PROCESS {
                    return Err(CapabilityError::PermissionDenied);
                }
                let slot = self.entries[j].irq_count;
                self.entries[j].irq_numbers[slot] = irq_number;
                self.entries[j].irq_count += 1;
                return Ok(());
            }
            j += 1;
        }

        Err(CapabilityError::InvalidArgument)
    }

    pub fn unassign_irq_from_pid(
        &mut self,
        pid: usize,
        irq_number: Word,
    ) -> Result<(), CapabilityError> {
        let mut i = 0usize;
        while i < MAX_ACTIVE_PROCESSES {
            if self.entries[i].used && self.entries[i].pid == pid {
                let mut j = 0usize;
                while j < self.entries[i].irq_count {
                    if self.entries[i].irq_numbers[j] == irq_number {
                        let last = self.entries[i].irq_count - 1;
                        self.entries[i].irq_numbers[j] = self.entries[i].irq_numbers[last];
                        self.entries[i].irq_numbers[last] = INVALID_IRQ;
                        self.entries[i].irq_count -= 1;
                        return Ok(());
                    }
                    j += 1;
                }
                return Err(CapabilityError::InvalidArgument);
            }
            i += 1;
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

        let mut i = 0;
        while i < MAX_ACTIVE_PROCESSES {
            if self.entries[i].used && self.entries[i].pid == pid {
                if self.entries[i].io_range_count >= MAX_IO_RANGES_PER_PROCESS {
                    return Err(CapabilityError::InvalidArgument);
                }

                let mut k = 0;
                while k < self.entries[i].io_range_count {
                    let r = self.entries[i].io_ranges[k];
                    let overlaps = !(max < r.min || r.max < min);
                    if overlaps {
                        return Err(CapabilityError::PermissionDenied);
                    }
                    k += 1;
                }

                self.entries[i].io_ranges[self.entries[i].io_range_count] =
                    IoPortRange { min, max };
                self.entries[i].io_range_count += 1;
                return Ok(());
            }
            i += 1;
        }

        Err(CapabilityError::InvalidArgument)
    }

    pub fn process_root_slot_for_pid(pid: usize) -> Result<usize, CapabilityError> {
        let slot = PROCESS_ROOT_SLOT_BASE + pid;
        if slot >= PROCESS_ROOT_SLOT_LIMIT {
            return Err(CapabilityError::InvalidArgument);
        }
        Ok(slot)
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

        let mut i = 0usize;
        while i < MAX_ACTIVE_PROCESSES {
            if self.entries[i].used && self.entries[i].pid == pid {
                if self.entries[i].next_frame_slot + page_count > max_frame_slots {
                    return Err(CapabilityError::InvalidArgument);
                }
                if self.entries[i].user_heap_next_va == 0
                    || self.entries[i].user_heap_limit_va <= self.entries[i].user_heap_next_va
                {
                    return Err(CapabilityError::InvalidArgument);
                }

                let heap_base = self.entries[i].user_heap_next_va;
                let heap_end = heap_base
                    .checked_add(bytes)
                    .ok_or(CapabilityError::InvalidArgument)?;
                if heap_end > self.entries[i].user_heap_limit_va {
                    return Err(CapabilityError::InvalidArgument);
                }

                let start_slot = self.entries[i].next_frame_slot;
                self.entries[i].next_frame_slot += page_count;
                self.entries[i].user_heap_next_va = heap_end;
                return Ok((
                    self.entries[i].root_node,
                    self.entries[i].address_space,
                    heap_base,
                    start_slot,
                ));
            }
            i += 1;
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

        let mut i = 0usize;
        while i < MAX_ACTIVE_PROCESSES {
            if self.entries[i].used && self.entries[i].pid == pid {
                if self.entries[i].user_heap_next_va == 0
                    || self.entries[i].user_heap_limit_va <= self.entries[i].user_heap_next_va
                {
                    return Err(CapabilityError::InvalidArgument);
                }

                let gap_base = self.entries[i].user_heap_next_va;
                let gap_end = gap_base
                    .checked_add(bytes)
                    .ok_or(CapabilityError::InvalidArgument)?;
                if gap_end > self.entries[i].user_heap_limit_va {
                    return Err(CapabilityError::InvalidArgument);
                }

                self.entries[i].user_heap_next_va = gap_end;
                return Ok(gap_base);
            }
            i += 1;
        }

        Err(CapabilityError::InvalidArgument)
    }
}
