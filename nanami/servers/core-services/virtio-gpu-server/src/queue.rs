//! One synchronous, fenced request on a modern PCI split control virtqueue.
//! DMA is explicitly released only after reset acknowledgement or detach.
use crate::wire;
use core::{
    ptr,
    sync::atomic::{fence, Ordering},
};
use nanami_runtime::{RequestError, Word};

pub const PAGE: Word = 4096;
const DMA_BYTES: Word = 3 * PAGE;
const AVAIL: Word = 128;
const USED: Word = 256;
const ACKNOWLEDGE: u32 = 1;
const DRIVER: u32 = 2;
const DRIVER_OK: u32 = 4;
const FEATURES_OK: u32 = 8;
const NEEDS_RESET: u32 = 64;
const FAILED: u32 = 128;
const DEVICE_FEATURE_SELECT: Word = 0;
const DEVICE_FEATURE: Word = 4;
const DRIVER_FEATURE_SELECT: Word = 8;
const DRIVER_FEATURE: Word = 12;
const CONFIG_VECTOR: Word = 16;
const NUM_QUEUES: Word = 18;
const STATUS: Word = 20;
const QUEUE_SELECT: Word = 22;
const QUEUE_SIZE: Word = 24;
const QUEUE_VECTOR: Word = 26;
const QUEUE_ENABLE: Word = 28;
const QUEUE_NOTIFY_OFFSET: Word = 30;
const QUEUE_DESCRIPTORS: Word = 32;
const QUEUE_AVAILABLE: Word = 40;
const QUEUE_USED: Word = 48;
const DESCRIPTOR_NEXT: u16 = 1;
const DESCRIPTOR_WRITE: u16 = 2;
const AVAILABLE_NO_INTERRUPT: u16 = 1;

pub trait Transport {
    fn read(&self, offset: Word, width: Word) -> u32;
    fn write(&mut self, offset: Word, width: Word, value: u32);
    fn notify(&mut self, offset: Word) -> Result<(), RequestError>;
    fn notify_offset(&self, queue_offset: u32) -> Result<Word, RequestError>;
}

pub struct Queue<T> {
    transport: T,
    claimed: bool,
    pub failed: bool,
    physical: Word,
    local: Word,
    size: u16,
    available: u16,
    used: u16,
    sequence: u64,
    notify: Word,
    timer: Word,
}
unsafe fn store16(address: Word, value: u16) {
    ptr::write_volatile(address as *mut u16, value.to_le());
}
unsafe fn store32(address: Word, value: u32) {
    ptr::write_volatile(address as *mut u32, value.to_le());
}
unsafe fn store64(address: Word, value: u64) {
    ptr::write_volatile(address as *mut u64, value.to_le());
}
unsafe fn load16(address: Word) -> u16 {
    u16::from_le(ptr::read_volatile(address as *const u16))
}
unsafe fn load32(address: Word) -> u32 {
    u32::from_le(ptr::read_volatile(address as *const u32))
}
impl<T: Transport> Queue<T> {
    pub fn new(transport: T, timer: Word) -> Self {
        Self {
            transport,
            claimed: false,
            failed: false,
            physical: 0,
            local: 0,
            size: 0,
            available: 0,
            used: 0,
            sequence: 0,
            notify: 0,
            timer,
        }
    }
    fn pause(&self) -> Result<(), RequestError> {
        nanami_client::timer::timer_service_sleep_milliseconds(self.timer, 1)
    }
    pub fn ensure_healthy(&self) -> Result<(), RequestError> {
        if self.failed || self.size == 0 {
            return Err(RequestError::Transport);
        }
        Ok(())
    }
    /// Acknowledged reset is the only way to retire an uncertain virtqueue.
    pub fn reset(&mut self) -> Result<(), RequestError> {
        self.failed = true;
        self.size = 0;
        // Discovery may decline a firmware-owned device before takeover.
        // Do not destroy its existing scanout merely to report inapplicability.
        if !self.claimed {
            return Ok(());
        }
        self.transport.write(STATUS, 1, 0);
        for _ in 0..100 {
            if self.transport.read(STATUS, 1) == 0 {
                if self.local != 0 {
                    nanami_runtime::request_mapping_release(self.local, DMA_BYTES)?;
                    self.local = 0;
                }
                return Ok(());
            }
            self.pause()?;
        }
        Err(RequestError::Transport)
    }
    pub fn initialize(&mut self) -> Result<(), RequestError> {
        if self.transport.read(STATUS, 1) & DRIVER_OK != 0 {
            let selector = self.transport.read(DEVICE_FEATURE_SELECT, 4);
            self.transport.write(DEVICE_FEATURE_SELECT, 4, 1);
            let high = self.transport.read(DEVICE_FEATURE, 4);
            self.transport.write(DEVICE_FEATURE_SELECT, 4, selector);
            if high & 1 == 0 || high & 2 != 0 {
                return Err(RequestError::Unsupported);
            }
        }
        self.claimed = true;
        self.reset()?;
        self.transport.write(STATUS, 1, ACKNOWLEDGE | DRIVER);
        self.transport.write(DEVICE_FEATURE_SELECT, 4, 1);
        let high = self.transport.read(DEVICE_FEATURE, 4);
        // VERSION_1 is required. ACCESS_PLATFORM needs an IOMMU/DMA mapper,
        // which this pc99 driver does not implement.
        if high & 1 == 0 || high & 2 != 0 {
            return Err(RequestError::Unsupported);
        }
        self.transport.write(DRIVER_FEATURE_SELECT, 4, 0);
        self.transport.write(DRIVER_FEATURE, 4, 0); // No optional GPU/ring features.
        self.transport.write(DRIVER_FEATURE_SELECT, 4, 1);
        self.transport.write(DRIVER_FEATURE, 4, 1); // VIRTIO_F_VERSION_1.
        self.transport
            .write(STATUS, 1, ACKNOWLEDGE | DRIVER | FEATURES_OK);
        if self.transport.read(STATUS, 1) != ACKNOWLEDGE | DRIVER | FEATURES_OK {
            return Err(RequestError::Unsupported);
        }
        if self.transport.read(NUM_QUEUES, 2) < 2 {
            return Err(RequestError::Unsupported);
        }
        self.transport.write(CONFIG_VECTOR, 2, 0xffff);
        self.transport.write(QUEUE_SELECT, 2, 0); // controlq; cursorq stays unused.
        let maximum = self.transport.read(QUEUE_SIZE, 2) as u16;
        if maximum < 2 || !maximum.is_power_of_two() || self.transport.read(QUEUE_ENABLE, 2) != 0 {
            return Err(RequestError::Unsupported);
        }
        let size = maximum.min(8);
        self.notify = self
            .transport
            .notify_offset(self.transport.read(QUEUE_NOTIFY_OFFSET, 2))?;
        let (physical, local) = nanami_runtime::request_dma(DMA_BYTES)?;
        self.physical = physical;
        self.local = local;
        if physical & (PAGE - 1) != 0
            || local == 0
            || local & (PAGE - 1) != 0
            || physical.checked_add(DMA_BYTES).is_none()
        {
            return Err(RequestError::Protocol);
        }
        unsafe {
            ptr::write_bytes(local as *mut u8, 0, DMA_BYTES);
            store16(local + AVAIL, AVAILABLE_NO_INTERRUPT);
        }
        self.transport.write(QUEUE_SIZE, 2, size as u32);
        self.transport.write(QUEUE_VECTOR, 2, 0xffff);
        for (offset, address) in [
            (QUEUE_DESCRIPTORS, physical),
            (QUEUE_AVAILABLE, physical + AVAIL),
            (QUEUE_USED, physical + USED),
        ] {
            self.transport.write(offset, 4, address as u32);
            self.transport
                .write(offset + 4, 4, (address as u64 >> 32) as u32);
        }
        fence(Ordering::SeqCst);
        self.transport.write(QUEUE_ENABLE, 2, 1);
        if self.transport.read(QUEUE_ENABLE, 2) != 1 {
            return Err(RequestError::Protocol);
        }
        self.transport
            .write(STATUS, 1, ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK);
        if self.transport.read(STATUS, 1) != ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK {
            return Err(RequestError::Protocol);
        }
        self.available = 0;
        self.used = 0;
        self.failed = false;
        self.size = size;
        Ok(())
    }
    pub fn execute(
        &mut self,
        command: &wire::Command,
        response: u32,
        length: usize,
    ) -> Result<&[u8], RequestError> {
        self.ensure_healthy()?;
        if !(wire::HEADER_BYTES..=PAGE).contains(&length) {
            return Err(RequestError::InvalidArgument);
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(RequestError::Unsupported)?;
        let result = self.submit(command, response, length);
        if let Err(error) = result {
            self.failed = true;
            nanami_runtime::println!(
                "[virtio-gpu] controlq failed: {}; retaining DMA and resources",
                error
            );
            return Err(error);
        }
        Ok(unsafe { core::slice::from_raw_parts((self.local + 2 * PAGE) as *const u8, length) })
    }
    fn submit(
        &mut self,
        command: &wire::Command,
        response: u32,
        length: usize,
    ) -> Result<(), RequestError> {
        let request = self.local + PAGE;
        let reply = self.local + 2 * PAGE;
        unsafe {
            ptr::write_bytes(reply as *mut u8, 0, length);
            ptr::copy_nonoverlapping(
                command.bytes().as_ptr(),
                request as *mut u8,
                command.bytes().len(),
            );
            store32(request + 4, wire::FENCE);
            store64(request + 8, self.sequence);
            // Descriptor 0: read-only request, followed by device-write reply.
            store64(self.local, (self.physical + PAGE) as u64);
            store32(self.local + 8, command.bytes().len() as u32);
            store16(self.local + 12, DESCRIPTOR_NEXT);
            store16(self.local + 14, 1);
            store64(self.local + 16, (self.physical + 2 * PAGE) as u64);
            store32(self.local + 24, length as u32);
            store16(self.local + 28, DESCRIPTOR_WRITE);
            store16(self.local + 30, 0);
            store16(
                self.local + AVAIL + 4 + (self.available % self.size) as Word * 2,
                0,
            );
            fence(Ordering::Release);
            self.available = self.available.wrapping_add(1);
            store16(self.local + AVAIL + 2, self.available);
        }
        fence(Ordering::SeqCst);
        self.transport.notify(self.notify)?;
        for _ in 0..250 {
            if self.transport.read(STATUS, 1) & (NEEDS_RESET | FAILED) != 0 {
                return Err(RequestError::Transport);
            }
            let used = unsafe { load16(self.local + USED + 2) };
            if used != self.used {
                fence(Ordering::Acquire);
                let element = self.local + USED + 4 + (self.used % self.size) as Word * 8;
                let id = unsafe { load32(element) };
                let bytes = unsafe { load32(element + 4) } as usize;
                if used != self.used.wrapping_add(1) || id != 0 || bytes != length {
                    return Err(RequestError::Protocol);
                }
                let header = unsafe { core::slice::from_raw_parts(reply as *const u8, length) };
                if wire::read32(header, 0) != response
                    || wire::read32(header, 4) != wire::FENCE
                    || wire::read64(header, 8) != self.sequence
                    || wire::read32(header, 16) != 0
                    || wire::read32(header, 20) != 0
                {
                    return Err(RequestError::Protocol);
                }
                self.used = used;
                return Ok(());
            }
            self.pause()?;
        }
        Err(RequestError::Transport)
    }
    pub fn command(&mut self, command: wire::Command) -> Result<(), RequestError> {
        self.execute(&command, wire::OK, wire::HEADER_BYTES)
            .map(|_| ())
    }
}
