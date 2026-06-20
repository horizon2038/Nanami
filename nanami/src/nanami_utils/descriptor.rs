use nun::{CapabilityDescriptor, BYTE_BITS, WORD_BITS};

#[inline(always)]
pub fn descriptor_depth(descriptor: CapabilityDescriptor) -> usize {
    let common_offset_bit = WORD_BITS - BYTE_BITS;
    let raw = (descriptor >> common_offset_bit) & ((1usize << BYTE_BITS) - 1);
    raw + BYTE_BITS
}

#[inline(always)]
pub fn make_root_slot_descriptor(root_radix: usize, slot_index: usize) -> CapabilityDescriptor {
    let common_offset_bit = WORD_BITS - BYTE_BITS;
    // Root-child descriptor layout matches InitSlotOffset::as_descriptor():
    // - top 8 bits store "encoded depth" (= root radix for root children)
    // - payload stores slot index at (56 - root_radix)
    let encoded_depth = root_radix;
    let slot_shift = common_offset_bit.saturating_sub(root_radix);

    (encoded_depth << common_offset_bit) | (slot_index << slot_shift)
}

#[inline(always)]
pub fn make_child_slot_descriptor(
    node_descriptor: CapabilityDescriptor,
    node_radix: usize,
    slot_index: usize,
) -> CapabilityDescriptor {
    let common_offset_bit = WORD_BITS - BYTE_BITS;
    let parent_depth = descriptor_depth(node_descriptor);
    let new_depth = parent_depth + node_radix;
    let slot_shift = WORD_BITS.saturating_sub(new_depth);

    let depth_mask = !(((1usize << BYTE_BITS) - 1) << common_offset_bit);
    let slot_mask = !(((1usize << node_radix) - 1) << slot_shift);

    (node_descriptor & depth_mask & slot_mask)
        | ((new_depth - BYTE_BITS) << common_offset_bit)
        | (slot_index << slot_shift)
}
