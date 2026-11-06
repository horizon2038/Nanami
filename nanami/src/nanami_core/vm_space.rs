use crate::nanami_utils::avl::AvlTree;
use crate::nanami_utils::static_avl::StaticAvlTree;
use alloc::collections::{BTreeMap, BTreeSet};
use nun::CapabilityDescriptor;

// Bootstrap mappings are recorded before the alpha heap is available, so this
// tracker intentionally remains fixed-size and allocation-free.
const BOOTSTRAP_VM_FRAME_MAPS: usize = 1 << 14;
const BOOTSTRAP_VM_PT_MAPS: usize = 256;
// Alpha alternates between several temporary mapping windows. Remember their
// completed leaf-table regions without another kernel depth query. This is a
// metadata hint, not additional mapped memory or a limit on address coverage.
const BOOTSTRAP_PT_REGIONS: usize = 8;

// Both supported architectures use 4-KiB pages and 512 entries per leaf table.
// Frame unmapping does not remove page tables. Readiness hints are local to
// their tracker and are discarded when that address space is replaced.
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
    page_table_regions: [usize; BOOTSTRAP_PT_REGIONS],
    next_page_table_region: usize,
}

impl BootstrapVmSpace {
    pub const fn new() -> Self {
        Self {
            frame_by_va: StaticAvlTree::new(),
            page_table_by_va: StaticAvlTree::new(),
            page_table_regions: [NO_PAGE_TABLE_REGION; BOOTSTRAP_PT_REGIONS],
            next_page_table_region: 0,
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
        let region = virtual_address >> PAGE_TABLE_REGION_SHIFT;
        if !self.page_table_regions.contains(&region) {
            self.page_table_regions[self.next_page_table_region] = region;
            self.next_page_table_region = (self.next_page_table_region + 1) % BOOTSTRAP_PT_REGIONS;
        }
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
        self.page_table_regions
            .contains(&(virtual_address >> PAGE_TABLE_REGION_SHIFT))
    }
}

pub struct VmSpace {
    frame_by_va: AvlTree,
    page_table_by_va: AvlTree,
    // Ordinary processes retain the one-region sequential-mapping hint.
    page_table_region: usize,
    // Only non-default permissions need metadata. PROT_NONE retains its Frame
    // record and allocation reference while the hardware leaf is absent.
    permissions: BTreeMap<usize, usize>,
    // Private fork aliases, never packed into opaque descriptors or R/W/X bits.
    shared_private: BTreeSet<usize>,
    // A capability error may follow a PTE update but precede its shootdown ACK.
    // Separate from R/W/X: unknown presence must neither grant checked access
    // nor allow backing retirement. Nodes are allocated only on error.
    uncertain_mappings: BTreeSet<usize>,
}

impl VmSpace {
    pub const fn new() -> Self {
        Self {
            frame_by_va: AvlTree::new(),
            page_table_by_va: AvlTree::new(),
            page_table_region: NO_PAGE_TABLE_REGION,
            permissions: BTreeMap::new(),
            shared_private: BTreeSet::new(),
            uncertain_mappings: BTreeSet::new(),
        }
    }

    pub fn forget_frame(&mut self, virtual_address: usize) -> Option<CapabilityDescriptor> {
        self.permissions.remove(&virtual_address);
        self.shared_private.remove(&virtual_address);
        self.uncertain_mappings.remove(&virtual_address);
        self.frame_by_va.remove(virtual_address)
    }

    pub fn private_page_shared(&self, va: usize) -> bool {
        self.shared_private.contains(&va)
    }

    pub fn mark_private_page_shared(&mut self, va: usize, shared: bool) {
        if shared {
            self.shared_private.insert(va);
        } else {
            self.shared_private.remove(&va);
        }
    }

    pub fn shared_private_pages(&self, start: usize, end: usize) -> alloc::vec::Vec<usize> {
        self.shared_private.range(start..end).copied().collect()
    }

    pub fn private_range_shared(&self, start: usize, end: usize) -> bool {
        self.shared_private.range(start..end).next().is_some()
    }

    /// Confirmed access only. Zero also covers an uncertain PTE; callers must
    /// use frame_may_be_present rather than infer absence from permissions.
    pub fn permissions(&self, virtual_address: usize) -> usize {
        if self.mapping_is_uncertain(virtual_address) {
            return 0;
        }
        self.permissions.get(&virtual_address).copied().unwrap_or(7)
    }

    pub fn mapping_is_uncertain(&self, virtual_address: usize) -> bool {
        self.uncertain_mappings.contains(&virtual_address)
    }

    pub fn has_uncertain_mappings(&self, start: usize, end: usize) -> bool {
        self.uncertain_mappings.range(start..end).next().is_some()
    }

    pub fn mark_mapping_uncertain(&mut self, virtual_address: usize) {
        self.uncertain_mappings.insert(virtual_address);
    }

    pub fn confirm_unmapped(&mut self, virtual_address: usize) {
        self.uncertain_mappings.remove(&virtual_address);
        self.set_permissions(virtual_address, 0);
    }

    pub fn frame_may_be_present(&self, virtual_address: usize) -> bool {
        self.mapping_is_uncertain(virtual_address)
            || (self.find_frame(virtual_address).is_some()
                && self.permissions.get(&virtual_address).copied().unwrap_or(7) != 0)
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

    pub fn all_backed_pages(
        &self,
        start: usize,
        end: usize,
        mut predicate: impl FnMut(usize) -> bool,
    ) -> bool {
        if start >= end {
            return true;
        }
        let mut valid = true;
        self.frame_by_va.visit_range(start, end, |va, _| {
            valid = valid && predicate(va);
        });
        valid
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
