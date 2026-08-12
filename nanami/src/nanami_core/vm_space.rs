use crate::nanami_utils::avl::AvlTree;
use crate::nanami_utils::static_avl::StaticAvlTree;
use nun::CapabilityDescriptor;

// Bootstrap mappings are recorded before the alpha heap is available, so this
// tracker intentionally remains fixed-size and allocation-free.
const BOOTSTRAP_VM_FRAME_MAPS: usize = 1 << 14;
const BOOTSTRAP_VM_PT_MAPS: usize = 256;

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
}

#[derive(Clone, Copy)]
pub struct BootstrapVmSpace {
    frame_by_va: StaticAvlTree<BOOTSTRAP_VM_FRAME_MAPS>,
    page_table_by_va: StaticAvlTree<BOOTSTRAP_VM_PT_MAPS>,
}

impl BootstrapVmSpace {
    pub const fn new() -> Self {
        Self {
            frame_by_va: StaticAvlTree::new(),
            page_table_by_va: StaticAvlTree::new(),
        }
    }
}

impl VmTracker for BootstrapVmSpace {
    fn record_frame(
        &mut self,
        virtual_address: usize,
        frame_descriptor: CapabilityDescriptor,
    ) -> Result<(), ()> {
        self.frame_by_va.insert(virtual_address, frame_descriptor)
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
}

pub struct VmSpace {
    frame_by_va: AvlTree,
    page_table_by_va: AvlTree,
}

impl VmSpace {
    pub const fn new() -> Self {
        Self {
            frame_by_va: AvlTree::new(),
            page_table_by_va: AvlTree::new(),
        }
    }

    pub fn forget_frame(&mut self, virtual_address: usize) -> Option<CapabilityDescriptor> {
        self.frame_by_va.remove(virtual_address)
    }
}

impl VmTracker for VmSpace {
    fn record_frame(
        &mut self,
        virtual_address: usize,
        frame_descriptor: CapabilityDescriptor,
    ) -> Result<(), ()> {
        self.frame_by_va.insert(virtual_address, frame_descriptor)
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
}
