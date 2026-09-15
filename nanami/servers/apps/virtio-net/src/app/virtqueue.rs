//! Split virtqueue layout shared by the RX and TX paths.
const VIRTIO_QUEUE_ALIGN: usize = 4096;

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct VirtqDesc {
    pub(super) addr: u64,
    pub(super) len: u32,
    pub(super) flags: u16,
    pub(super) next: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct VirtqUsedElem {
    pub(super) id: u32,
    pub(super) len: u32,
}

pub(super) fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

pub(super) fn used_offset(queue_size: u16) -> usize {
    let q = queue_size as usize;
    let avail_bytes = 2 + 2 + q * 2 + 2;
    align_up(
        q * core::mem::size_of::<VirtqDesc>() + avail_bytes,
        VIRTIO_QUEUE_ALIGN,
    )
}

pub(super) fn total_queue_bytes(queue_size: u16) -> usize {
    let q = queue_size as usize;
    used_offset(queue_size) + (2 + 2 + q * core::mem::size_of::<VirtqUsedElem>() + 2)
}

pub(super) unsafe fn desc_ptr(base: *mut u8) -> *mut VirtqDesc {
    base as *mut VirtqDesc
}

pub(super) unsafe fn avail_idx_ptr(base: *mut u8, queue_size: u16) -> *mut u16 {
    base.add(core::mem::size_of::<VirtqDesc>() * queue_size as usize + 2) as *mut u16
}

pub(super) unsafe fn avail_flags_ptr(base: *mut u8, queue_size: u16) -> *mut u16 {
    base.add(core::mem::size_of::<VirtqDesc>() * queue_size as usize) as *mut u16
}

pub(super) unsafe fn avail_ring_ptr(base: *mut u8, queue_size: u16) -> *mut u16 {
    base.add(core::mem::size_of::<VirtqDesc>() * queue_size as usize + 4) as *mut u16
}

pub(super) unsafe fn used_idx_ptr(base: *mut u8, queue_size: u16) -> *mut u16 {
    base.add(used_offset(queue_size) + 2) as *mut u16
}

pub(super) unsafe fn used_flags_ptr(base: *mut u8, queue_size: u16) -> *mut u16 {
    base.add(used_offset(queue_size)) as *mut u16
}

pub(super) unsafe fn used_ring_ptr(base: *mut u8, queue_size: u16) -> *mut VirtqUsedElem {
    base.add(used_offset(queue_size) + 4) as *mut VirtqUsedElem
}
