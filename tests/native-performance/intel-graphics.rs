//! The production Gen9 register logic and scanout with an in-memory MMIO model.
//! This checks software sequencing, not electrical/display hardware behavior.
#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_gpu;
extern crate self as nanami_runtime;
use std::cell::RefCell;
pub type Word = usize;
pub type CapabilityDescriptor = Word;
pub const OS_RESPONSE_OK: Word = 0;
pub const OS_RESPONSE_PERMISSION_DENIED: Word = 2;
#[path = "../../nanami/servers/libs/nanami-gpu/src/lib.rs"]
mod gpu_provider;
pub use gpu_provider::{cpu_copy, cpu_fill, Allocation, Buffer};
#[path = "../../nanami/servers/core-services/intel-graphics-server/src/blt.rs"]
mod blt;
use nanami_uapi::gfx::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    InvalidArgument,
    Unsupported,
    Protocol,
    Transport,
    Status(Word),
}
impl std::fmt::Display for RequestError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{self:?}")
    }
}
#[path = "../../nanami/servers/core-services/intel-graphics-server/src/connector.rs"]
mod connector;
#[path = "../../nanami/servers/core-services/intel-graphics-server/src/ddc.rs"]
mod ddc;
#[path = "../../nanami/servers/libs/nanami-client/src/gfx/display.rs"]
mod display_client;
#[path = "intel-graphics/display-tests.rs"]
mod display_tests;
#[path = "../../nanami/servers/core-services/intel-graphics-server/src/edid.rs"]
mod edid;
pub mod gfx {
    pub use nanami_uapi::gfx::*;
}
#[path = "../../nanami/servers/core-services/intel-graphics-server/src/vbt.rs"]
mod vbt;
#[macro_export]
macro_rules! println { ($($arg:tt)*) => {{ let _ = format_args!($($arg)*); }}; }
#[path = "../../nanami/servers/core-services/intel-graphics-server/src/gen9.rs"]
mod gen9;
#[path = "../../nanami/servers/uapi/src/intel_graphics.rs"]
mod generations;
#[path = "../../nanami/servers/core-services/intel-graphics-server/src/hardware.rs"]
mod hardware;
#[path = "../../nanami/servers/uapi/src/pci.rs"]
pub mod pci_uapi;
use gen9::{Rect, Registers};

fn call_port(
    port: Word,
    code: Word,
    a: Word,
    b: Word,
    c: Word,
    d: Word,
    words: Word,
) -> Result<(Word, Word, Word), RequestError> {
    display_tests::call_port([port, code, a, b, c, d, words])
}

const MMIO_BASE: Word = 0x8000_0000;
const APERTURE_BASE: Word = 0x9000_0000;
const DMA_BASE: Word = 0x500000;
const FRAME_BYTES: Word = 4096;
const STRIDE: Word = 128;
const WIDTH: Word = 24; // Deliberately padded rows.
const HEIGHT: Word = 16;
const CONTROL: Word = 0x70180;
const SURFACE: Word = 0x7019c;
const LIVE: Word = 0x701ac;
const FRAME: Word = 0x70040;

struct Model {
    mmio: Vec<u32>,
    table: Vec<u64>,
    original: Vec<u8>,
    dma: Vec<u8>,
    sleeps: Vec<Word>,
    flushed: Vec<(Word, Word)>,
    running: bool,
    latch: bool,
    timer_error: bool,
}
impl Default for Model {
    fn default() -> Self {
        let mut mmio = vec![0; 2 * 1024 * 1024 / 4];
        mmio[CONTROL / 4] = (1 << 31) | (4 << 24);
        mmio[0x70188 / 4] = (STRIDE / 64) as u32;
        mmio[0x70190 / 4] = ((HEIGHT - 1) << 16 | (WIDTH - 1)) as u32;
        mmio[0x6001c / 4] = ((WIDTH - 1) << 16 | (HEIGHT - 1)) as u32;
        let mut table = vec![0; 2 * 1024 * 1024 / 8];
        table[0] = 0x100001; // Firmware scanout; must never be overwritten.
        Self {
            mmio,
            table,
            original: vec![0x35; FRAME_BYTES],
            dma: Vec::new(),
            sleeps: Vec::new(),
            flushed: Vec::new(),
            running: true,
            latch: true,
            timer_error: false,
        }
    }
}
thread_local! { static MODEL: RefCell<Model> = RefCell::new(Model::default()); }
fn reset() {
    MODEL.with(|model| *model.borrow_mut() = Model::default());
}
pub fn request_mmio(physical: Word, bytes: Word) -> Result<(Word, Word), RequestError> {
    MODEL.with(|model| {
        let mut model = model.borrow_mut();
        let pointer = match physical {
            MMIO_BASE => {
                assert_eq!(bytes, model.mmio.len() * 4);
                model.mmio.as_mut_ptr() as Word
            }
            value if value == MMIO_BASE + 8 * 1024 * 1024 => {
                assert_eq!(bytes, model.table.len() * 8);
                model.table.as_mut_ptr() as Word
            }
            APERTURE_BASE => {
                assert_eq!(bytes, FRAME_BYTES);
                model.original.as_mut_ptr() as Word
            }
            _ => return Err(RequestError::Protocol),
        };
        Ok((physical, pointer))
    })
}
pub fn request_dma(bytes: Word) -> Result<(Word, Word), RequestError> {
    MODEL.with(|model| {
        let mut model = model.borrow_mut();
        assert!(model.dma.is_empty());
        model.dma = vec![0; bytes];
        Ok((DMA_BASE, model.dma.as_mut_ptr() as Word))
    })
}
pub fn request_shared_memory(_: Word, _: Word) -> Result<(Word, Word), RequestError> {
    panic!("unexpected shared buffer allocation");
}
pub fn request_mapping_share(_: Word, _: Word, _: Word) -> Result<Word, RequestError> {
    panic!("unexpected DMA share");
}
pub fn request_mapping_release(_: Word, _: Word) -> Result<(), RequestError> {
    panic!("unexpected DMA release");
}
pub mod cache {
    pub fn flush_rect(address: usize, stride: usize, rect: nanami_uapi::gpu::Rect) {
        for row in rect.y..rect.y + rect.height {
            flush(address + row * stride + rect.x * 4, rect.width * 4);
        }
    }
    pub fn flush(address: usize, bytes: usize) {
        super::MODEL.with(|model| model.borrow_mut().flushed.push((address, bytes)));
    }
}
pub mod pci {
    pub use crate::pci_uapi::*;
    pub fn enumerate(_: usize, _: usize) -> Result<Option<Device>, crate::RequestError> {
        Ok(None)
    }
    pub fn write(
        _: usize,
        _: Address,
        _: usize,
        _: usize,
        _: u32,
    ) -> Result<(), crate::RequestError> {
        panic!("unexpected PCI command write");
    }
    pub fn probe_bar(_: usize, _: Address, index: usize) -> Result<Bar, crate::RequestError> {
        let physical = match index {
            0 => super::MMIO_BASE,
            2 => super::APERTURE_BASE,
            _ => return Err(crate::RequestError::InvalidArgument),
        };
        Ok(Bar {
            physical,
            bytes: 16 * 1024 * 1024,
            io: false,
            prefetchable: index == 2,
        })
    }
    pub fn read(
        _: usize,
        _: Address,
        offset: usize,
        width: usize,
    ) -> Result<u32, crate::RequestError> {
        assert_eq!((offset, width), (0x50, 2));
        Ok(1 << 6)
    }
}
pub mod timer {
    pub fn timer_service_sleep_milliseconds(
        _: usize,
        milliseconds: usize,
    ) -> Result<(), crate::RequestError> {
        super::MODEL.with(|model| {
            let mut model = model.borrow_mut();
            model.sleeps.push(milliseconds);
            if model.timer_error {
                return Err(crate::RequestError::Transport);
            }
            if model.running {
                model.mmio[super::FRAME / 4] += 1;
            }
            if milliseconds == 1 && model.latch {
                model.mmio[super::LIVE / 4] = model.mmio[super::SURFACE / 4];
            }
            Ok(())
        })
    }
}
fn device() -> pci::Device {
    pci::Device {
        address: pci::Address::new(0, 2, 0),
        vendor_id: 0x8086,
        device_id: 0x3e92,
        class: 0x030000,
        header: 0,
    }
}
fn scanout() -> hardware::Scanout {
    hardware::Scanout::new(16, device(), 24).unwrap()
}
fn rect(x: Word, y: Word) -> Rect {
    Rect {
        x,
        y,
        width: 1,
        height: 1,
    }
}

#[test]
fn generation_dispatch_does_not_guess_from_the_vendor_alone() {
    use generations::{generation, Generation::*};
    assert_eq!(generation(0x8086, 0x1912, 0x030000), Some(Skylake));
    assert_eq!(generation(0x8086, 0x5912, 0x030000), Some(KabyLake));
    assert_eq!(generation(0x8086, 0x3e92, 0x030000), Some(CoffeeLake));
    assert_eq!(generation(0x8086, 0x1234, 0x030000), None);
    assert_eq!(generation(0x1234, 0x3e92, 0x030000), None);
    assert_eq!(generation(0x8086, 0x3e92, 0x020000), None);
}

#[test]
fn native_setup_preserves_firmware_and_copies_the_inherited_picture() {
    reset();
    let mut display = scanout();
    let mut shared = vec![0u8; display.mode.bytes];
    display.copy_current(shared.as_mut_ptr() as Word).unwrap();
    assert!(shared.iter().all(|byte| *byte == 0x35));
    MODEL.with(|model| {
        let model = model.borrow();
        assert_eq!(
            model.table[..3],
            [
                0x100001,
                DMA_BASE as u64 | 1,
                (DMA_BASE + FRAME_BYTES) as u64 | 1
            ]
        );
        assert_eq!(model.mmio[SURFACE / 4], 0);
        assert_eq!(model.sleeps, [40]);
        assert_eq!(model.mmio[gen9::GGTT_FLUSH_CONTROL / 4], 1);
    });
    display.detach_before_flip();
    MODEL.with(|model| assert_eq!(model.borrow().table[..3], [0x100001, 0, 0]));
}

#[test]
fn alternating_partial_presents_repair_stale_back_buffers_and_preserve_padding() {
    reset();
    let mut display = scanout();
    let mut shared = vec![0x35; display.mode.bytes];
    for (x, y, value) in [(1, 2, 0x11), (5, 4, 0x22), (3, 3, 0x33), (7, 6, 0x44)] {
        let offset = y * STRIDE + x * 4;
        shared[offset..offset + 4].fill(value);
        display
            .present(shared.as_ptr() as Word, rect(x, y))
            .unwrap();
        MODEL.with(|model| {
            let model = model.borrow();
            let surface = model.mmio[LIVE / 4] as Word;
            let back = surface / FRAME_BYTES - 1;
            assert_eq!(
                &model.dma[back * FRAME_BYTES..back * FRAME_BYTES + shared.len()],
                &shared
            );
            assert_eq!(model.table[0], 0x100001);
        });
    }
}

#[test]
fn written_surface_is_not_proof_of_a_completed_flip() {
    reset();
    let mut display = scanout();
    let shared = vec![0x77; display.mode.bytes];
    MODEL.with(|model| model.borrow_mut().latch = false);
    assert_eq!(
        display.present(shared.as_ptr() as Word, rect(0, 0)),
        Err(RequestError::Transport)
    );
    MODEL.with(|model| {
        let model = model.borrow();
        assert_eq!(model.mmio[SURFACE / 4], FRAME_BYTES as u32);
        assert_eq!(model.mmio[LIVE / 4], 0);
        assert_eq!(model.sleeps.len(), 201);
        assert_ne!(model.table[1], 0); // Timeout must not unmap a possibly active buffer.
    });
    let before = MODEL.with(|model| model.borrow().dma.clone());
    assert_eq!(
        display.present(shared.as_ptr() as Word, rect(1, 1)),
        Err(RequestError::Transport)
    );
    MODEL.with(|model| assert_eq!(model.borrow().dma, before));
}

#[test]
fn timer_failure_poisoning_also_retains_dma() {
    reset();
    let mut display = scanout();
    MODEL.with(|model| model.borrow_mut().timer_error = true);
    let shared = vec![0; display.mode.bytes];
    assert_eq!(
        display.present(shared.as_ptr() as Word, rect(0, 0)),
        Err(RequestError::Transport)
    );
    MODEL.with(|model| assert_ne!(model.borrow().table[1], 0));
}

#[test]
fn filled_ggtt_and_stopped_pipe_fail_before_dma_allocation() {
    for filled in [false, true] {
        reset();
        MODEL.with(|model| {
            let mut model = model.borrow_mut();
            if filled {
                model.table.fill(0x700001);
            } else {
                model.running = false;
            }
        });
        assert!(matches!(
            hardware::Scanout::new(16, device(), 24),
            Err(RequestError::Unsupported)
        ));
        MODEL.with(|model| assert!(model.borrow().dma.is_empty()));
    }
}

#[test]
fn scanout_can_use_empty_ggtt_entries_above_the_cpu_aperture() {
    reset();
    const APERTURE_PAGES: usize = 16 * 1024 * 1024 / 4096;
    MODEL.with(|model| model.borrow_mut().table[..APERTURE_PAGES].fill(0x700001));
    let mut display = scanout();
    let shared = vec![0x44u8; display.mode.bytes];
    display
        .present(shared.as_ptr() as Word, rect(0, 0))
        .unwrap();
    MODEL.with(|model| {
        let model = model.borrow();
        assert!(model.table[..APERTURE_PAGES]
            .iter()
            .all(|pte| *pte == 0x700001));
        assert_eq!(model.mmio[LIVE / 4] as usize, APERTURE_PAGES * 4096);
    });
}

struct FakeRegisters {
    registers: std::collections::BTreeMap<Word, u32>,
    table: Vec<u64>,
    corrupt: Option<Word>,
}
impl Registers for FakeRegisters {
    fn read(&self, offset: Word) -> u32 {
        self.registers.get(&offset).copied().unwrap_or(0)
    }
    fn write(&mut self, offset: Word, value: u32) {
        self.registers.insert(offset, value);
    }
    fn pte(&self, index: Word) -> u64 {
        self.table[index]
    }
    fn write_pte(&mut self, index: Word, value: u64) {
        self.table[index] = value;
        if self.corrupt == Some(index) && value != 0 {
            self.table[index] ^= 0x1000;
        }
    }
}
fn registers() -> FakeRegisters {
    let model = Model::default();
    FakeRegisters {
        registers: model
            .mmio
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, value)| *value != 0)
            .map(|(index, value)| (index * 4, value))
            .collect(),
        table: vec![0; 16],
        corrupt: None,
    }
}

#[test]
fn unsafe_inherited_modes_are_refused_without_register_writes() {
    for (offset, bit) in [
        (CONTROL, 1 << 10),
        (CONTROL, 1 << 15),
        (CONTROL, 1 << 9),
        (CONTROL, 1 << 20),
        (0x43208, 1 << 31),
        (0x64800, 1 << 31),
        (0x45504, 1),
        (0x7018c, 1),
        (0x701a4, 1),
        (0x70280, 1 << 31),
        (0x71180, (1 << 31) | (4 << 24)),
    ] {
        let mut registers = registers();
        *registers.registers.entry(offset).or_default() |= bit;
        let before = registers.registers.clone();
        assert_eq!(
            gen9::discover(&registers, 16 * 1024 * 1024),
            Err(RequestError::Unsupported)
        );
        assert_eq!(registers.registers, before);
    }
}

#[test]
fn ggtt_never_overwrites_present_entries_and_rolls_back_failed_readback() {
    let mut registers = registers();
    registers.table[3] = 0x800001;
    assert_eq!(gen9::free_span(&registers, 16, 4), Ok(4));
    assert_eq!(
        gen9::bind(&mut registers, 2, DMA_BASE, 4),
        Err(RequestError::Protocol)
    );
    assert_eq!(registers.table[2], 0);
    registers.corrupt = Some(6);
    assert_eq!(
        gen9::bind(&mut registers, 4, DMA_BASE, 4),
        Err(RequestError::Protocol)
    );
    assert_eq!(&registers.table[4..8], &[0; 4]);
    assert_eq!(registers.table[3], 0x800001);
    assert_eq!(
        gen9::bind(&mut registers, 4, 1 << 39, 1),
        Err(RequestError::Unsupported)
    );
    assert_eq!(
        gen9::bind(&mut registers, 4, DMA_BASE + 1, 1),
        Err(RequestError::Unsupported)
    );
    assert_eq!(
        gen9::bind(&mut registers, 4, DMA_BASE, usize::MAX),
        Err(RequestError::Unsupported)
    );
}

#[test]
fn damage_rectangles_validate_clip_and_union_without_overflow() {
    let mode = gen9::discover(&registers(), 16 * 1024 * 1024).unwrap();
    assert_eq!(
        Rect::checked(mode, WIDTH, 1 | (1 << 32)),
        Err(RequestError::InvalidArgument)
    );
    assert_eq!(
        Rect::checked(mode, 0, 0),
        Err(RequestError::InvalidArgument)
    );
    let damage = Rect::checked(mode, 2 | (3 << 32), Word::MAX).unwrap();
    assert_eq!((damage.width, damage.height), (WIDTH - 2, HEIGHT - 3));
    assert_eq!(
        rect(1, 2).union(rect(3, 4)),
        Rect {
            x: 1,
            y: 2,
            width: 3,
            height: 3
        }
    );
}
