//! Append-only capability arena. Only the current radix-tree spine is needed:
//! published descriptors are stable, and old branches need no software index.
use super::*;

pub(super) const DIRECTORY_RADIX: usize = 8;
const DIRECTORY_LEVELS: usize = 4;
const DIRECTORY_SLOTS: usize = 1 << DIRECTORY_RADIX;
const DIRECTORY_NODE_BYTES: usize = 1 << kernel_object::node_memory_size_bits(DIRECTORY_RADIX);

// An arbitrary page-aligned interval needs at most two prefixes per address
// bit, plus two destination slots. It fits across at most two directory leaves.
const _: () = assert!(2 * (usize::BITS as usize - PAGE_BITS) + 2 < DIRECTORY_SLOTS);
pub(super) const REFILL_RESERVE_BYTES: usize = 2 * DIRECTORY_LEVELS * DIRECTORY_NODE_BYTES
    + 2 * (1 << kernel_object::node_memory_size_bits(PHYSICAL_LEAF_RADIX));

#[derive(Clone, Copy)]
pub(super) struct Slot {
    pub node: CapabilityDescriptor,
    pub index: usize,
}

impl Slot {
    pub fn descriptor(self) -> CapabilityDescriptor {
        make_child_slot_descriptor(self.node, DIRECTORY_RADIX, self.index)
    }
}

pub(super) struct CapabilityDirectory {
    spine: [CapabilityDescriptor; DIRECTORY_LEVELS],
    prefixes: [Option<usize>; DIRECTORY_LEVELS],
    next: usize,
}

impl CapabilityDirectory {
    pub fn new(root: CapabilityDescriptor) -> Self {
        Self {
            spine: [root, 0, 0, 0],
            prefixes: [Some(0), None, None, None],
            next: 0,
        }
    }

    pub fn allocate(&mut self, pool: &mut NodePool) -> Result<Slot, CapabilityError> {
        // Descriptor payload, not installed RAM, bounds the namespace. With a
        // 12-bit root: 12 + 4*8 + 11 = 55 bits, within A9N's 56-bit payload.
        let depth = crate::nanami_utils::descriptor::descriptor_depth(self.spine[0]);
        if depth + DIRECTORY_LEVELS * DIRECTORY_RADIX + PHYSICAL_LEAF_RADIX > nun::WORD_BITS
            || self.next >= (1usize << (DIRECTORY_LEVELS * DIRECTORY_RADIX))
        {
            return Err(CapabilityError::InvalidArgument);
        }
        for level in 1..DIRECTORY_LEVELS {
            let shift = (DIRECTORY_LEVELS - level) * DIRECTORY_RADIX;
            let prefix = self.next >> shift;
            if self.prefixes[level] != Some(prefix) {
                let slot = Slot {
                    node: self.spine[level - 1],
                    index: prefix & (DIRECTORY_SLOTS - 1),
                };
                pool.create_node(DIRECTORY_RADIX, slot)?;
                self.spine[level] = slot.descriptor();
                self.prefixes[level] = Some(prefix);
            }
        }
        let slot = Slot {
            node: self.spine[DIRECTORY_LEVELS - 1],
            index: self.next & (DIRECTORY_SLOTS - 1),
        };
        self.next += 1;
        Ok(slot)
    }
}

pub(super) struct NodePool {
    descriptor: CapabilityDescriptor,
    consumed: usize,
}

impl NodePool {
    pub fn new(descriptor: CapabilityDescriptor) -> Self {
        Self {
            descriptor,
            consumed: 0,
        }
    }

    pub fn remaining(&self) -> usize {
        (1 << FRAME_LEAF_POOL_RADIX) - self.consumed
    }

    pub fn create_node(&mut self, radix: usize, slot: Slot) -> Result<(), CapabilityError> {
        let bytes = 1usize << kernel_object::node_memory_size_bits(radix);
        let end = checked_align_up(self.consumed, bytes)
            .and_then(|start| start.checked_add(bytes))
            .filter(|end| *end <= 1 << FRAME_LEAF_POOL_RADIX)
            .ok_or(CapabilityError::InvalidArgument)?;
        arch::generic::convert(
            self.descriptor,
            CapabilityType::Node,
            radix as Word,
            1,
            slot.node,
            slot.index as Word,
        )?;
        self.consumed = end;
        Ok(())
    }
}
