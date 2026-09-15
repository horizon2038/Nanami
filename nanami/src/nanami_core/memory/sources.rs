//! Untyped ranges retain large unused prefixes. RAM and MMIO use the same
//! demand-driven split; the buddy allocator independently tracks RAM ownership.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct PhysicalSource {
    end: usize,
    descriptor: CapabilityDescriptor,
}

impl MemoryManager {
    pub(super) fn initialize_physical_sources(&mut self) -> Result<(), CapabilityError> {
        for index in 0..self.initial_generic_count {
            let generic = self.initial_generics[index];
            if generic.size_radix < PAGE_BITS as u8 {
                continue;
            }
            let base = generic.address as usize;
            let end = base
                .checked_add(
                    checked_pow2(generic.size_radix as usize)
                        .ok_or(CapabilityError::InvalidArgument)?,
                )
                .ok_or(CapabilityError::InvalidArgument)?;
            let start = base
                .checked_add(self.initial_generic_delegated_bytes[index])
                .ok_or(CapabilityError::InvalidArgument)?;
            self.retain_source(
                index,
                start,
                PhysicalSource {
                    end,
                    descriptor: self.generic_descriptor_from_index(index),
                },
            );
        }
        Ok(())
    }

    fn retain_source(&mut self, index: usize, start: usize, source: PhysicalSource) {
        if start < source.end {
            self.physical_sources.insert((index, start), source);
        }
    }

    pub(super) fn source_range(&self, index: usize, address: usize) -> Option<(usize, usize)> {
        let (&(_, start), source) = self
            .physical_sources
            .range((index, 0)..=(index, address))
            .next_back()?;
        (address < source.end).then_some((start, source.end))
    }

    // The caller reserves metadata pool headroom before entering this method.
    // Advancing a source is committed after each successful conversion, so a
    // failed later allocation neither loses prefixes nor repeats conversions.
    pub(super) fn take_physical_source(
        &mut self,
        index: usize,
        address: usize,
        radix: usize,
    ) -> Result<CapabilityDescriptor, CapabilityError> {
        let bytes = checked_pow2(radix).ok_or(CapabilityError::InvalidArgument)?;
        let (mut cursor, end) = self
            .source_range(index, address)
            .ok_or(CapabilityError::InvalidArgument)?;
        if address & (bytes - 1) != 0 || address.checked_add(bytes).is_none_or(|limit| limit > end)
        {
            return Err(CapabilityError::InvalidArgument);
        }
        while cursor < address {
            let prefix_radix =
                prefix_radix(cursor, address).ok_or(CapabilityError::InvalidArgument)?;
            let source = self.physical_sources[&(index, cursor)];
            let slot = self.capability_directory.allocate(&mut self.node_pool)?;
            arch::generic::convert(
                source.descriptor,
                CapabilityType::Generic,
                prefix_radix as Word,
                1,
                slot.node,
                slot.index as Word,
            )?;
            let next = cursor + (1usize << prefix_radix);
            self.physical_sources.remove(&(index, cursor));
            self.retain_source(
                index,
                cursor,
                PhysicalSource {
                    end: next,
                    descriptor: slot.descriptor(),
                },
            );
            self.retain_source(index, next, source);
            cursor = next;
        }
        let source = self.physical_sources[&(index, cursor)];
        let slot = self.capability_directory.allocate(&mut self.node_pool)?;
        arch::generic::convert(
            source.descriptor,
            CapabilityType::Generic,
            radix as Word,
            1,
            slot.node,
            slot.index as Word,
        )?;
        self.physical_sources.remove(&(index, cursor));
        self.retain_source(index, cursor + bytes, source);
        Ok(slot.descriptor())
    }

    pub(super) fn ensure_metadata_capacity(&mut self) -> Result<(), CapabilityError> {
        if self.node_pool.remaining() >= directory::REFILL_RESERVE_BYTES {
            return Ok(());
        }
        // Use only still-untyped RAM, never a materialized frame or device
        // range. Reserve with the buddy allocator before converting it.
        let mut selected = None;
        'ranges: for (&(index, start), source) in self.physical_sources.iter().rev() {
            if self.initial_generics[index].is_device {
                continue;
            }
            let Some(mut base) = source
                .end
                .checked_sub(PHYSICAL_CHUNK_SIZE)
                .map(|value| value & !(PHYSICAL_CHUNK_SIZE - 1))
            else {
                continue;
            };
            while base >= start {
                if self
                    .physical_allocator
                    .as_mut()
                    .ok_or(CapabilityError::InvalidArgument)?
                    .allocate_at(base, PHYSICAL_CHUNK_SIZE, false)
                    .is_ok()
                {
                    selected = Some((index, base));
                    break 'ranges;
                }
                let Some(previous) = base.checked_sub(PHYSICAL_CHUNK_SIZE) else {
                    break;
                };
                base = previous;
            }
        }
        let (index, base) = selected.ok_or(CapabilityError::InvalidArgument)?;
        match self.take_physical_source(index, base, FRAME_LEAF_POOL_RADIX) {
            Ok(descriptor) => {
                self.node_pool = NodePool::new(descriptor);
                crate::info!(
                    "memory: capability metadata pool expanded paddr={:#x}",
                    base
                );
                Ok(())
            }
            Err(error) => {
                self.free_physical(base, PHYSICAL_CHUNK_SIZE)?;
                Err(error)
            }
        }
    }
}

fn prefix_radix(start: usize, end: usize) -> Option<usize> {
    let remaining = end.checked_sub(start)?;
    if remaining < PAGE_SIZE || start & (PAGE_SIZE - 1) != 0 || end & (PAGE_SIZE - 1) != 0 {
        return None;
    }
    Some((start.trailing_zeros() as usize).min(remaining.ilog2() as usize))
}

pub(super) fn containing_block(start: usize, end: usize, address: usize) -> Option<(usize, usize)> {
    for radix in (PAGE_BITS..=PHYSICAL_CHUNK_RADIX).rev() {
        let bytes = 1usize << radix;
        let base = address & !(bytes - 1);
        if base >= start && base.checked_add(bytes).is_some_and(|limit| limit <= end) {
            return Some((base, radix));
        }
    }
    None
}
