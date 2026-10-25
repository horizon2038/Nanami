use crate::gen9::{self, Mode, Rect, Registers, PAGE_BYTES};
use core::{
    ptr,
    sync::atomic::{fence, Ordering},
};
use nanami_client::pci::{self, Device};
use nanami_runtime::{RequestError, Word};

const MMIO_BYTES: Word = 2 * 1024 * 1024;
const GGTT_OFFSET: Word = 8 * 1024 * 1024;
const ADDRESS_LIMIT: Word = 1usize << 39;

pub struct Hardware {
    mmio: Word,
    table: Word,
    entries: Word,
}

impl Registers for Hardware {
    fn read(&self, offset: Word) -> u32 {
        assert!(offset <= MMIO_BYTES - 4 && offset & 3 == 0);
        unsafe { ptr::read_volatile((self.mmio + offset) as *const u32) }
    }
    fn write(&mut self, offset: Word, value: u32) {
        assert!(offset <= MMIO_BYTES - 4 && offset & 3 == 0);
        unsafe {
            ptr::write_volatile((self.mmio + offset) as *mut u32, value);
        }
    }
    fn pte(&self, index: Word) -> u64 {
        assert!(index < self.entries);
        unsafe { ptr::read_volatile((self.table + index * 8) as *const u64) }
    }
    fn write_pte(&mut self, index: Word, value: u64) {
        assert!(index < self.entries);
        unsafe {
            ptr::write_volatile((self.table + index * 8) as *mut u64, value);
        }
    }
}

/// DMA remains owned for the lifetime of scanout, even on a flip timeout.
pub struct Scanout {
    pub mode: Mode,
    pub output: Option<crate::connector::Connector>,
    hardware: Hardware,
    memory: Word,
    frame_bytes: Word,
    ggtt_start: Word,
    bound_pages: Word,
    front: usize,
    dirty: [Option<Rect>; 2],
    started: bool,
    failed: bool,
    timer: Word,
}

impl Scanout {
    pub fn new(port: Word, device: Device, timer: Word) -> Result<Self, RequestError> {
        let registers = pci::probe_bar(port, device.address, 0)?;
        let aperture = pci::probe_bar(port, device.address, 2)?;
        if registers.io || aperture.io || registers.bytes != 16 * 1024 * 1024 {
            return Err(RequestError::Unsupported);
        }
        let ggc = pci::read(port, device.address, 0x50, 2)?;
        let size_code = (ggc >> 6) & 3;
        if size_code == 0 {
            return Err(RequestError::Unsupported);
        }
        let table_bytes = (1usize << size_code) * 1024 * 1024;
        let (_, mmio) = nanami_runtime::request_mmio(registers.physical, MMIO_BYTES)?;
        let (_, table) =
            nanami_runtime::request_mmio(registers.physical + GGTT_OFFSET, table_bytes)?;
        let mut hardware = Hardware {
            mmio,
            table,
            entries: table_bytes / 8,
        };
        let mode = gen9::discover(&hardware, aperture.bytes)?;
        // An enabled-looking but stopped pipe cannot complete a flip.
        let frame = gen9::frame(&hardware, mode);
        nanami_client::timer::timer_service_sleep_milliseconds(timer, 40)?;
        if gen9::frame(&hardware, mode) == frame {
            return Err(RequestError::Unsupported);
        }
        let frame_bytes = mode
            .bytes
            .checked_add(PAGE_BYTES - 1)
            .ok_or(RequestError::Protocol)?
            & !(PAGE_BYTES - 1);
        let pages = (frame_bytes / PAGE_BYTES) * 2;
        // Only touch PTEs that firmware actually left empty; never infer
        // ownership from identical/scratch mappings or overwrite active pages.
        // BAR2 limits CPU aperture access, not the display engine's GGTT
        // address space. CPU uploads use the DMA mapping, so scanout buffers
        // may live above BAR2's aperture (within PLANE_SURF's 32-bit range).
        let scanout_entries = hardware.entries.min((1usize << 32) / PAGE_BYTES);
        let start = gen9::free_span(&hardware, scanout_entries, pages)?;
        let (physical, memory) = nanami_runtime::request_dma(frame_bytes * 2)?;
        if physical
            .checked_add(frame_bytes * 2)
            .is_none_or(|end| end > ADDRESS_LIMIT)
        {
            return Err(RequestError::Unsupported);
        }
        let (_, original) = nanami_runtime::request_mmio(
            aperture.physical + mode.original_surface as Word,
            frame_bytes,
        )?;
        unsafe {
            ptr::copy_nonoverlapping(original as *const u8, memory as *mut u8, mode.bytes);
            ptr::copy_nonoverlapping(
                original as *const u8,
                (memory + frame_bytes) as *mut u8,
                mode.bytes,
            );
        }
        crate::cache::flush(memory, frame_bytes * 2);
        // No fallible setup remains after the PTE commit. A failed service
        // publication may still unbind safely, before the first PLANE_SURF.
        gen9::bind(&mut hardware, start, physical, pages)?;
        nanami_runtime::println!(
            "[intel-graphics] native scanout pipe={} {}x{} stride={} GGTT={:#x} buffers=2",
            mode.pipe,
            mode.width,
            mode.height,
            mode.stride,
            start * PAGE_BYTES
        );
        Ok(Self {
            mode,
            output: None,
            hardware,
            memory,
            frame_bytes,
            ggtt_start: start,
            bound_pages: pages,
            front: 0,
            dirty: [None; 2],
            started: false,
            failed: false,
            timer,
        })
    }

    pub fn inspect_output(&mut self, pci: Word, device: Device) {
        match crate::connector::inspect(&mut self.hardware, self.mode, pci, device, self.timer) {
            Ok(output) => self.output = Some(output),
            Err(error) => {
                nanami_runtime::println!("[intel-graphics] output discovery unavailable: {}", error)
            }
        }
    }

    pub fn capabilities(&self) -> Word {
        use nanami_client::gfx::*;
        let mut flags = DISPLAY_CAP_NATIVE_SCANOUT | DISPLAY_CAP_VBLANK_FLIP;
        if let Some(output) = &self.output {
            flags |= DISPLAY_CAP_CONNECTOR_INFO;
            if output.edid.len != 0 {
                flags |= DISPLAY_CAP_EDID | DISPLAY_CAP_MONITOR_MODES;
            }
        }
        flags
    }

    pub fn detach_before_flip(&mut self) {
        assert!(!self.started);
        gen9::unbind(&mut self.hardware, self.ggtt_start, self.bound_pages);
    }

    pub fn copy_current(&self, destination: Word) {
        unsafe {
            ptr::copy_nonoverlapping(
                (self.memory + self.front * self.frame_bytes) as *const u8,
                destination as *mut u8,
                self.mode.bytes,
            );
        }
    }

    pub fn present(&mut self, source: Word, rect: Rect) -> Result<Word, RequestError> {
        if self.failed {
            return Err(RequestError::Transport);
        }
        let back = if self.started { 1 - self.front } else { 0 };
        let damage = self.dirty[back].map_or(rect, |old| old.union(rect));
        let destination = self.memory + back * self.frame_bytes;
        fence(Ordering::Acquire);
        for row in damage.y..damage.y + damage.height {
            let offset = row * self.mode.stride + damage.x * 4;
            let bytes = damage.width * 4;
            unsafe {
                ptr::copy_nonoverlapping(
                    (source + offset) as *const u8,
                    (destination + offset) as *mut u8,
                    bytes,
                );
            }
        }
        let first = damage.y * self.mode.stride + damage.x * 4;
        let span = (damage.height - 1) * self.mode.stride + damage.width * 4;
        crate::cache::flush(destination + first, span);
        let surface = self.ggtt_start * PAGE_BYTES + back * self.frame_bytes;
        gen9::flip(&mut self.hardware, self.mode, surface)?;
        self.started = true;
        // Waiting on the timer yields the CPU; no unbounded MMIO spin loop.
        // SURFLIVE, not the double-buffered SURF readback, proves ownership.
        for _ in 0..200 {
            if gen9::latched(&self.hardware, self.mode, surface) {
                self.front = back;
                self.dirty[back] = None;
                self.dirty[1 - back] =
                    Some(self.dirty[1 - back].map_or(rect, |old| old.union(rect)));
                fence(Ordering::Release);
                return Ok(gen9::frame(&self.hardware, self.mode) as Word);
            }
            if nanami_client::timer::timer_service_sleep_milliseconds(self.timer, 1).is_err() {
                break;
            }
        }
        self.failed = true;
        nanami_runtime::println!(
            "[intel-graphics] flip timeout; retaining scanout DMA and GGTT mappings"
        );
        Err(RequestError::Transport)
    }
}
