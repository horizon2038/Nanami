use nun::Word;

const PAGE_BITS: usize = 12;
const PAGE_SIZE: usize = 1 << PAGE_BITS;
const ENTRY_DATA_MAX: usize = 3;

const WORD_BYTES: usize = core::mem::size_of::<Word>();
const WORD_BITS: usize = WORD_BYTES * 8;

#[derive(Clone, Copy)]
pub enum KernelObjectKind {
    Node,
    Generic,
    AddressSpace,
    PageTable,
    Frame,
    ProcessControlBlock,
    IpcPort,
    NotificationPort,
    VirtualCpu,
}

pub const fn memory_size_bits(kind: KernelObjectKind, specific_bits: usize) -> Option<usize> {
    match kind {
        KernelObjectKind::Node => Some(node_memory_size_bits(specific_bits)),
        KernelObjectKind::Generic => Some(specific_bits),
        KernelObjectKind::AddressSpace | KernelObjectKind::PageTable => Some(PAGE_BITS),
        KernelObjectKind::Frame => {
            if is_valid_frame_size_bits(specific_bits) {
                Some(specific_bits)
            } else {
                None
            }
        }
        KernelObjectKind::ProcessControlBlock => Some(radix_ceil(process_control_block_size())),
        KernelObjectKind::IpcPort => Some(radix_ceil(ipc_port_size())),
        KernelObjectKind::NotificationPort => Some(radix_ceil(notification_port_size())),
        KernelObjectKind::VirtualCpu => Some(radix_ceil(virtual_cpu_capability_size())),
    }
}

pub const fn node_memory_size_bits(radix_bits: usize) -> usize {
    let node_alignment = 1usize << radix_ceil(capability_node_size());
    let slot_alignment = 1usize << radix_ceil(capability_slot_size());
    let aligned_node_size = align_up(capability_node_size(), node_alignment);
    let slot_array_base = align_up(aligned_node_size, slot_alignment);
    let slot_array_size = capability_slot_size() * (1usize << radix_bits);
    radix_ceil(slot_array_base + slot_array_size)
}

const fn capability_component_size() -> usize {
    WORD_BYTES // vptr
}

const fn capability_slot_size() -> usize {
    let raw = WORD_BYTES  // component*
        + WORD_BYTES      // reserved/type-rights union
        + ENTRY_DATA_MAX * WORD_BYTES
        + WORD_BYTES      // next_slot*
        + WORD_BYTES      // preview_slot*
        + WORD_BYTES; // depth
    align_up(raw, WORD_BITS)
}

const fn capability_node_size() -> usize {
    capability_component_size()
        + WORD_BYTES // ignore_bits
        + WORD_BYTES // radix_bits
        + WORD_BYTES // capability_slots*
}

const fn process_control_block_size() -> usize {
    capability_component_size() + process_size()
}

const fn process_size() -> usize {
    let hardware_context = 22 * WORD_BYTES;
    let floating_context = align_up(128 * WORD_BYTES, WORD_BITS);
    let scalar_fields = 11 * WORD_BYTES;
    let capability_slots = 5 * capability_slot_size();
    let queue_links_and_ports = 8 * WORD_BYTES;
    let reply_state = 5 * WORD_BYTES;
    let name = 128;
    align_up(
        hardware_context
            + floating_context
            + scalar_fields
            + capability_slots
            + queue_links_and_ports
            + reply_state
            + name,
        WORD_BITS,
    )
}

const fn ipc_port_size() -> usize {
    capability_component_size()
        + WORD_BYTES // queue_head*
        + WORD_BYTES // queue_tail*
        + WORD_BYTES // state enum
}

const fn notification_port_size() -> usize {
    capability_component_size()
        + WORD_BYTES * 2 // notification
        + WORD_BYTES * 2 // queue_head/tail
        + WORD_BYTES     // binded_process
        + WORD_BYTES // state enum
}

const fn virtual_cpu_capability_size() -> usize {
    capability_component_size() + PAGE_SIZE * 4
}

const fn is_valid_frame_size_bits(bits: usize) -> bool {
    bits >= PAGE_BITS
}

const fn align_up(value: usize, alignment: usize) -> usize {
    if alignment == 0 {
        return value;
    }
    let mask = alignment - 1;
    (value + mask) & !mask
}

const fn radix_ceil(value: usize) -> usize {
    if value <= 1 {
        return 0;
    }
    let mut radix = 0usize;
    let mut n = value - 1;
    while n != 0 {
        radix += 1;
        n >>= 1;
    }
    radix
}
