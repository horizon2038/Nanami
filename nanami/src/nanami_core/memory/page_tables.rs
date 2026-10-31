//! Page-table backing pages have individual Generic owners. Revoking one owner
//! removes and resets its table without consuming another page of the bootstrap
//! pool. Only an unreachable address space may return its tables to this pool.
use super::*;

pub(super) struct PageTables {
    node: CapabilityDescriptor,
    generics: CapabilityDescriptor,
    initialized: usize,
    free: Vec<usize>,
    by_address_space: BTreeMap<CapabilityDescriptor, Vec<usize>>,
}

impl PageTables {
    pub(super) fn new(node: CapabilityDescriptor, generics: CapabilityDescriptor) -> Self {
        Self {
            node,
            generics,
            initialized: 0,
            free: Vec::new(),
            by_address_space: BTreeMap::new(),
        }
    }

    pub(super) fn allocate(
        &mut self,
        source: CapabilityDescriptor,
        address_space: CapabilityDescriptor,
        depth: usize,
        reclaimable: bool,
    ) -> Result<(CapabilityDescriptor, usize), CapabilityError> {
        let slot = if let Some(slot) = self.free.pop() {
            slot
        } else {
            if self.initialized == PAGE_TABLE_POOL_SLOTS {
                return Err(CapabilityError::InvalidArgument);
            }
            let slot = self.initialized;
            arch::generic::convert(
                source,
                CapabilityType::Generic,
                PAGE_BITS as Word,
                1,
                self.generics,
                slot as Word,
            )?;
            self.initialized += 1;
            slot
        };
        // Retain ownership even if conversion/mapping fails. Teardown can
        // safely revoke partially completed work; no live table is recycled.
        if reclaimable {
            self.by_address_space
                .entry(address_space)
                .or_default()
                .push(slot);
        }
        let generic = make_child_slot_descriptor(self.generics, PAGE_TABLE_POOL_RADIX, slot);
        arch::generic::convert(
            generic,
            CapabilityType::PageTable,
            depth as Word,
            1,
            self.node,
            slot as Word,
        )?;
        Ok((
            make_child_slot_descriptor(self.node, PAGE_TABLE_POOL_RADIX, slot),
            slot,
        ))
    }

    pub(super) fn release(&mut self, address_space: CapabilityDescriptor) -> CapabilityResult {
        let Some(slots) = self.by_address_space.get_mut(&address_space) else {
            return Ok(());
        };
        while let Some(&slot) = slots.last() {
            // Keep failed revocations owned and unavailable for reuse.
            arch::node::revoke(self.generics, slot as Word)?;
            slots.pop();
            self.free.push(slot);
        }
        self.by_address_space.remove(&address_space);
        Ok(())
    }
}
