//! Keep every device-visible byte inside the single contiguous DMA allocation.
pub const QUEUE_MEM_BYTES: usize = 16384;
pub const MAX_TRANSFER_BYTES: usize = nanami_client::block::BLOCK_DEVICE_DEFAULT_SHM_BYTES as usize;
pub const DMA_QUEUE_OFFSET: usize = 0;
pub const DMA_HEADER_OFFSET: usize = DMA_QUEUE_OFFSET + QUEUE_MEM_BYTES;
pub const DMA_DATA_OFFSET: usize = DMA_HEADER_OFFSET + core::mem::size_of::<VirtioBlkReqHeader>();
pub const DMA_STATUS_OFFSET: usize = DMA_DATA_OFFSET + MAX_TRANSFER_BYTES;
pub const DMA_TOTAL_BYTES: usize = (DMA_STATUS_OFFSET + 1).next_multiple_of(4096);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtioBlkReqHeader {
    pub request_type: u32,
    pub reserved: u32,
    pub sector: u64,
}
