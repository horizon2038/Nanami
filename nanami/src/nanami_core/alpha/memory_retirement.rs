//! Deferred Frame aliases outlive unmapping, but never their RAM ownership.
use super::*;

impl Alpha {
    pub(super) fn free_deferred_process_allocations(
        &mut self,
        pid: usize,
        process_root: Option<CapabilityDescriptor>,
    ) -> Result<(), CapabilityError> {
        let memory = &mut self.memory;
        self.processes
            .retire_deferred_physical_allocations(pid, |allocation, last| {
                // Every local alias loses its Frame capability; only the final
                // active/deferred owner can release the RAM. Commit each successful
                // record so a later failure cannot leave already-freed RAM owned.
                if let Some(root_node) = process_root.filter(|_| allocation.owns_frame_cap) {
                    for page in 0..allocation.page_count {
                        let slot = allocation.start_slot + page;
                        arch::node::remove(
                            process_frame_chunk_descriptor(
                                root_node,
                                slot / PROCESS_FRAME_CHUNK_PAGES,
                            ),
                            (slot % PROCESS_FRAME_CHUNK_PAGES) as Word,
                        )?;
                    }
                }
                if last {
                    memory.free_physical(
                        allocation.base_page * PAGE_SIZE,
                        allocation.page_count * PAGE_SIZE,
                    )?;
                }
                Ok(())
            })
    }
}
