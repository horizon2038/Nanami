use crate::nanami_utils::static_avl::StaticAvlTree;

// Keep this in sync with PROCESS_FRAME_NODE_RADIX. The VM tracker must be able
// to record every mapped process frame; 1920x1080 fb-shm plus a 9MiB heap
// exceeds 4096 mappings.
pub const MAX_VM_FRAME_MAPS: usize = 1 << 14;
pub const MAX_VM_PT_MAPS: usize = 256;

#[derive(Clone, Copy)]
pub struct VmSpace {
    frame_by_va: StaticAvlTree<MAX_VM_FRAME_MAPS>,
    page_table_by_va: StaticAvlTree<MAX_VM_PT_MAPS>,
}

impl VmSpace {
    pub const fn new() -> Self {
        Self {
            frame_by_va: StaticAvlTree::new(),
            page_table_by_va: StaticAvlTree::new(),
        }
    }

    pub fn record_frame(
        &mut self,
        virtual_address: usize,
        frame_slot_index: usize,
    ) -> Result<(), ()> {
        self.frame_by_va.insert(virtual_address, frame_slot_index)
    }

    pub fn record_page_table(
        &mut self,
        virtual_address: usize,
        page_table_slot_index: usize,
    ) -> Result<(), ()> {
        self.page_table_by_va
            .insert(virtual_address, page_table_slot_index)
    }

    pub fn find_frame_slot(&self, virtual_address: usize) -> Option<usize> {
        self.frame_by_va.find(virtual_address)
    }

    pub fn find_page_table_slot(&self, virtual_address: usize) -> Option<usize> {
        self.page_table_by_va.find(virtual_address)
    }
}
