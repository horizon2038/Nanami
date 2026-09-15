use libnanami::RequestError;

/// Mappings deliberately have no Drop: a timeout must never free storage that
/// a controller may still DMA into. Slot pages are reused only after Disable
/// Slot completion; controller-wide pages live for the driver's lifetime.
#[derive(Clone, Copy)]
pub struct Dma {
    pub physical: u64,
    pub virtual_address: usize,
    pub bytes: usize,
}
impl Dma {
    pub fn allocate(bytes: usize, address64: bool) -> Result<Self, RequestError> {
        let bytes = bytes
            .checked_add(4095)
            .ok_or(RequestError::InvalidArgument)?
            & !4095;
        let (physical, virtual_address) = libnanami::request_dma(bytes).map_err(|error| {
            libnanami::println!(
                "[usb-server] DMA allocation bytes={:#x} failed: {}",
                bytes,
                error
            );
            error
        })?;
        if physical & 4095 != 0
            || virtual_address & 4095 != 0
            || (!address64
                && physical
                    .checked_add(bytes)
                    .is_none_or(|end| end > 0x1_0000_0000))
        {
            return Err(RequestError::Unsupported);
        }
        unsafe { core::ptr::write_bytes(virtual_address as *mut u8, 0, bytes) };
        Ok(Self {
            physical: physical as u64,
            virtual_address,
            bytes,
        })
    }
    pub fn physical_at(self, offset: usize) -> u64 {
        assert!(offset < self.bytes);
        self.physical + offset as u64
    }
    pub fn virtual_at(self, offset: usize) -> usize {
        assert!(offset < self.bytes);
        self.virtual_address + offset
    }
    pub fn clear(self) {
        unsafe { core::ptr::write_bytes(self.virtual_address as *mut u8, 0, self.bytes) }
    }
}
