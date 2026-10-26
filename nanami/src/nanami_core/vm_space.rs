use crate::nanami_utils::avl::AvlTree;
use crate::nanami_utils::static_avl::StaticAvlTree;
use alloc::collections::BTreeMap;
use nun::CapabilityDescriptor;

// Bootstrap mappings are recorded before the alpha heap is available, so this
// tracker intentionally remains fixed-size and allocation-free.
const BOOTSTRAP_VM_FRAME_MAPS: usize = 1 << 14;
const BOOTSTRAP_VM_PT_MAPS: usize = 256;

// Both supported architectures use 4-KiB pages and 512 entries per leaf table.
// Frame unmapping does not remove page tables. Remember only the most recently
// mapped 2-MiB region; destroying/replacing the VmSpace discards this hint too.
const PAGE_TABLE_REGION_SHIFT: u32 = 12 + 9;
const NO_PAGE_TABLE_REGION: usize = usize::MAX;

pub trait VmTracker {
    fn record_frame(
        &mut self,
        virtual_address: usize,
        frame_descriptor: CapabilityDescriptor,
    ) -> Result<(), ()>;
    fn record_page_table(
        &mut self,
        virtual_address: usize,
        page_table_slot_index: usize,
    ) -> Result<(), ()>;
    fn find_frame(&self, virtual_address: usize) -> Option<CapabilityDescriptor>;
    fn find_page_table_slot(&self, virtual_address: usize) -> Option<usize>;
    fn forget_frame(&mut self, virtual_address: usize) -> Option<CapabilityDescriptor>;
    fn page_tables_ready(&self, virtual_address: usize) -> bool;
}

#[derive(Clone, Copy)]
pub struct BootstrapVmSpace {
    frame_by_va: StaticAvlTree<BOOTSTRAP_VM_FRAME_MAPS>,
    page_table_by_va: StaticAvlTree<BOOTSTRAP_VM_PT_MAPS>,
    page_table_region: usize,
}

impl BootstrapVmSpace {
    pub const fn new() -> Self {
        Self {
            frame_by_va: StaticAvlTree::new(),
            page_table_by_va: StaticAvlTree::new(),
            page_table_region: NO_PAGE_TABLE_REGION,
        }
    }
}

impl VmTracker for BootstrapVmSpace {
    fn record_frame(
        &mut self,
        virtual_address: usize,
        frame_descriptor: CapabilityDescriptor,
    ) -> Result<(), ()> {
        self.frame_by_va.insert(virtual_address, frame_descriptor)?;
        self.page_table_region = virtual_address >> PAGE_TABLE_REGION_SHIFT;
        Ok(())
    }

    fn record_page_table(
        &mut self,
        virtual_address: usize,
        page_table_slot_index: usize,
    ) -> Result<(), ()> {
        self.page_table_by_va
            .insert(virtual_address, page_table_slot_index)
    }

    fn find_frame(&self, virtual_address: usize) -> Option<CapabilityDescriptor> {
        self.frame_by_va.find(virtual_address)
    }

    fn find_page_table_slot(&self, virtual_address: usize) -> Option<usize> {
        self.page_table_by_va.find(virtual_address)
    }

    fn forget_frame(&mut self, virtual_address: usize) -> Option<CapabilityDescriptor> {
        self.frame_by_va.remove(virtual_address)
    }

    fn page_tables_ready(&self, virtual_address: usize) -> bool {
        self.page_table_region == virtual_address >> PAGE_TABLE_REGION_SHIFT
    }
}

pub struct VmSpace {
    frame_by_va: AvlTree,
    page_table_by_va: AvlTree,
    page_table_region: usize,
    // Only non-default permissions need metadata. PROT_NONE retains its Frame
    // record and allocation reference while the hardware leaf is absent.
    permissions: BTreeMap<usize, usize>,
}

impl VmSpace {
    pub const fn new() -> Self {
        Self {
            frame_by_va: AvlTree::new(),
            page_table_by_va: AvlTree::new(),
            page_table_region: NO_PAGE_TABLE_REGION,
            permissions: BTreeMap::new(),
        }
    }

    pub fn forget_frame(&mut self, virtual_address: usize) -> Option<CapabilityDescriptor> {
        self.permissions.remove(&virtual_address);
        self.frame_by_va.remove(virtual_address)
    }

    pub fn permissions(&self, virtual_address: usize) -> usize {
        self.permissions.get(&virtual_address).copied().unwrap_or(7)
    }

    pub fn set_permissions(&mut self, virtual_address: usize, permissions: usize) {
        if permissions == 7 {
            self.permissions.remove(&virtual_address);
        } else {
            self.permissions.insert(virtual_address, permissions);
        }
    }

    pub fn backed_pages(&self, start: usize, end: usize) -> alloc::vec::Vec<(usize, usize)> {
        let mut pages = alloc::vec::Vec::new();
        self.frame_by_va
            .visit_range(start, end, |va, _| pages.push((va, self.permissions(va))));
        pages
    }
}

impl VmTracker for VmSpace {
    fn record_frame(
        &mut self,
        virtual_address: usize,
        frame_descriptor: CapabilityDescriptor,
    ) -> Result<(), ()> {
        self.frame_by_va.insert(virtual_address, frame_descriptor)?;
        self.page_table_region = virtual_address >> PAGE_TABLE_REGION_SHIFT;
        Ok(())
    }

    fn record_page_table(
        &mut self,
        virtual_address: usize,
        page_table_slot_index: usize,
    ) -> Result<(), ()> {
        self.page_table_by_va
            .insert(virtual_address, page_table_slot_index)
    }

    fn find_frame(&self, virtual_address: usize) -> Option<CapabilityDescriptor> {
        self.frame_by_va.find(virtual_address)
    }

    fn find_page_table_slot(&self, virtual_address: usize) -> Option<usize> {
        self.page_table_by_va.find(virtual_address)
    }

    fn forget_frame(&mut self, virtual_address: usize) -> Option<CapabilityDescriptor> {
        VmSpace::forget_frame(self, virtual_address)
    }

    fn page_tables_ready(&self, virtual_address: usize) -> bool {
        self.page_table_region == virtual_address >> PAGE_TABLE_REGION_SHIFT
    }
}
