//! One bounded contiguous DMA allocation, including every device-visible byte.
use super::*;

pub(super) const DMA_COMMAND_LIST_OFFSET: usize = 0x0000;
pub(super) const DMA_RECEIVED_FIS_OFFSET: usize = 0x0400;
pub(super) const DMA_COMMAND_TABLE_OFFSET: usize = 0x0500;
pub(super) const DMA_DATA_OFFSET: usize = 0x1000;
pub(super) const DMA_DATA_BYTES: usize = 16 * nanami_client::block::BLOCK_DEVICE_DEFAULT_SHM_BYTES;
pub(super) const DMA_FALLBACK_DATA_BYTES: usize = 4 * nanami_client::block::BLOCK_DEVICE_DEFAULT_SHM_BYTES;
pub(super) const DMA_TOTAL_BYTES: usize = DMA_DATA_OFFSET + DMA_DATA_BYTES;
pub(super) const COMMAND_TABLE_PRDT_OFFSET: usize = 0x80;
const COMMAND_HEADER_BYTES: usize = 32;
const COMMAND_TABLE_BYTES: usize = COMMAND_TABLE_PRDT_OFFSET + 16;

/// Zero asks the provider to choose its window. Explicit sizes stay exact.
pub(super) fn shared_window_bytes(requested: Word, available: Word) -> Word {
    if requested == 0 { available } else { requested }
}

/// Prefer a large command buffer, retaining the prior size under RAM pressure.
/// Nothing has been submitted to hardware while either allocation is attempted.
pub(super) fn allocate_command_memory(
    mut allocate: impl FnMut(usize) -> Result<(Word, Word), RequestError>,
) -> Result<(Word, Word, usize), RequestError> {
    let (physical, local, bytes) = match allocate(DMA_TOTAL_BYTES) {
        Ok((physical, local)) => (physical, local, DMA_DATA_BYTES),
        Err(_) => {
            let (physical, local) = allocate(DMA_DATA_OFFSET + DMA_FALLBACK_DATA_BYTES)?;
            (physical, local, DMA_FALLBACK_DATA_BYTES)
        }
    };
    Ok((physical, local, bytes))
}

pub(super) fn transfer_bytes(count: Word, offset: Word, capacity: Word, available: Word) -> Option<usize> {
    let bytes = count.checked_mul(BLOCK_BYTES)?;
    (count != 0 && bytes <= DMA_DATA_BYTES.min(available) && offset.checked_add(bytes)? <= capacity)
        .then_some(bytes)
}

pub(super) fn prepare_command(
    runtime: &AhciRuntime,
    command: u8,
    lba: u64,
    sectors: u16,
    bytes: usize,
    write: bool,
) -> Result<(), RequestError> {
    prepare_command_to(runtime, command, lba, sectors, bytes, write,
        runtime.dma_physical + DMA_DATA_OFFSET)
}

pub(super) fn prepare_command_to(
    runtime: &AhciRuntime,
    command: u8,
    lba: u64,
    sectors: u16,
    bytes: usize,
    write: bool,
    data_physical: Word,
) -> Result<(), RequestError> {
    if bytes > DMA_DATA_BYTES || bytes > runtime.dma_data_bytes
        || bytes > 0x0040_0000
        || (bytes != 0 && sectors == 0 && command != ATA_IDENTIFY_DEVICE)
        || (bytes != 0 && (data_physical & 1 != 0
            || data_physical.checked_add(bytes).is_none()))
    {
        return Err(RequestError::InvalidArgument);
    }
    unsafe {
        let command_list = (runtime.dma_virtual as usize + DMA_COMMAND_LIST_OFFSET) as *mut u32;
        let command_table = runtime.dma_virtual as usize + DMA_COMMAND_TABLE_OFFSET;
        // Only slot zero is issued. Reset its byte count, flags and reserved
        // header fields, plus the complete CFIS/ACMD/reserved/single-PRD table.
        // The whole DMA area was initialized before starting the controller.
        // Received FIS memory belongs to the running HBA.
        ptr::write_bytes(command_list.cast::<u8>(), 0, COMMAND_HEADER_BYTES);
        ptr::write_bytes(command_table as *mut u8, 0, COMMAND_TABLE_BYTES);
        let mut flags = 5u32;
        if write { flags |= 1 << 6; }
        if bytes != 0 { flags |= 1 << 16; }
        ptr::write_volatile(command_list, flags);
        ptr::write_volatile(command_list.add(1), 0);
        let command_table_physical = runtime.dma_physical as u64 + DMA_COMMAND_TABLE_OFFSET as u64;
        ptr::write_volatile(command_list.add(2), command_table_physical as u32);
        ptr::write_volatile(command_list.add(3), (command_table_physical >> 32) as u32);

        let fis = command_table as *mut u8;
        ptr::write(fis.add(0), 0x27);
        ptr::write(fis.add(1), 1 << 7);
        ptr::write(fis.add(2), command);
        ptr::write(fis.add(4), lba as u8);
        ptr::write(fis.add(5), (lba >> 8) as u8);
        ptr::write(fis.add(6), (lba >> 16) as u8);
        ptr::write(fis.add(7), 1 << 6);
        ptr::write(fis.add(8), (lba >> 24) as u8);
        ptr::write(fis.add(9), (lba >> 32) as u8);
        ptr::write(fis.add(10), (lba >> 40) as u8);
        ptr::write(fis.add(12), sectors as u8);
        ptr::write(fis.add(13), (sectors >> 8) as u8);

        if bytes != 0 {
            let prdt = (command_table + COMMAND_TABLE_PRDT_OFFSET) as *mut u32;
            let data_physical = data_physical as u64;
            ptr::write_volatile(prdt, data_physical as u32);
            ptr::write_volatile(prdt.add(1), (data_physical >> 32) as u32);
            ptr::write_volatile(prdt.add(2), 0);
            ptr::write_volatile(prdt.add(3), (bytes as u32 - 1) | (1 << 31));
        }
    }
    fence(Ordering::Release);
    Ok(())
}
