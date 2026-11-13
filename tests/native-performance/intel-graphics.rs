//! The production Gen9 register logic and scanout with an in-memory MMIO model.
//! This checks software sequencing, not electrical/display hardware behavior.
#![allow(dead_code)]
extern crate alloc;
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
#[path = "../../nanami/servers/core-services/intel-graphics-server/src/handoff.rs"]
mod handoff;
#[path = "../../nanami/servers/core-services/intel-graphics-server/src/hardware.rs"]
mod hardware;
#[path = "../../nanami/servers/core-services/intel-graphics-server/src/pci_layout.rs"]
mod pci_layout;
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
    gt_handshake: bool,
    resets: Vec<u32>,
    stuck_reset: u32,
    stuck_ready: bool,
    fail_engine_dma: bool,
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
            gt_handshake: false,
            resets: Vec::new(),
            stuck_reset: 0,
            stuck_ready: false,
            fail_engine_dma: false,
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
        if !model.dma.is_empty() && model.fail_engine_dma {
            return Err(RequestError::Transport);
        }
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
        offset: usize,
        width: usize,
        value: u32,
    ) -> Result<(), crate::RequestError> {
        assert_eq!((offset, width, value), (4, 2, 6));
        Ok(())
    }
    pub fn probe_bar(_: usize, _: Address, _: usize) -> Result<Bar, crate::RequestError> {
        panic!("must not size BARs on an active firmware GPU");
    }
    pub fn read(
        _: usize,
        _: Address,
        offset: usize,
        width: usize,
    ) -> Result<u32, crate::RequestError> {
        match (offset, width) {
            (4, 2) => Ok(6),
            (0x10, 4) => Ok(super::MMIO_BASE as u32 | 4),
            (0x14 | 0x1c, 4) => Ok(0),
            (0x18, 4) => Ok(super::APERTURE_BASE as u32 | 12),
            (0x50, 2) => super::MODEL
                .with(|model| Ok((model.borrow().table.len() * 8 / (1024 * 1024)).ilog2() << 6)),
            (0x62, 1) => Ok(0), // 128 MiB, no destructive sizing writes.
            _ => panic!("unexpected PCI register {offset:#x}/{width}"),
        }
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
            if model.gt_handshake {
                for (request, ack) in [(0xa188, 0x130044), (0xa278, 0xd84), (0xa270, 0xd88)] {
                    model.mmio[ack / 4] = model.mmio[request / 4] & 3;
                }
                let reset = model.mmio[0x941c / 4];
                if reset != 0 && reset != model.stuck_reset {
                    model.resets.push(reset);
                    model.mmio[0x941c / 4] = 0;
                }
                for base in [0x2000, 0x22000, 0x12000, 0x1a000] {
                    if model.mmio[(base + 0xd0) / 4] & 1 != 0 && !model.stuck_ready {
                        model.mmio[(base + 0xd0) / 4] |= 2;
                    }
                    if reset & 1 != 0 {
                        model.mmio[(base + 0x3c) / 4] = 0;
                        model.mmio[(base + 0x9c) / 4] = 1 << 9;
                    }
                }
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
fn stopped_pipe_fails_before_dma_allocation() {
    reset();
    MODEL.with(|model| model.borrow_mut().running = false);
    assert!(matches!(
        hardware::Scanout::new(16, device(), 24),
        Err(RequestError::Unsupported)
    ));
    MODEL.with(|model| assert!(model.borrow().dma.is_empty()));
}

#[test]
fn filled_ggtt_takeover_requires_ack_and_preserves_pte_contents_on_timeout() {
    reset();
    MODEL.with(|model| model.borrow_mut().table.fill(0x700001));
    assert!(matches!(
        hardware::Scanout::new(16, device(), 24),
        Err(RequestError::Unsupported)
    ));
    MODEL.with(|model| {
        let model = model.borrow();
        assert!(model.table.iter().all(|pte| *pte == 0x700001));
        assert_eq!(model.mmio[SURFACE / 4], 0);
        assert_eq!(model.mmio[0xa188 / 4], 1 << 16); // our forcewake released
        assert!(model.resets.is_empty());
        assert!(model.sleeps.len() < 60);
    });
}

#[test]
fn physical_8mib_filled_ggtt_is_claimed_only_after_gt_reset_and_can_be_restored() {
    reset();
    MODEL.with(|model| {
        let mut model = model.borrow_mut();
        model.table = vec![0x700001; 8 * 1024 * 1024 / 8];
        model.gt_handshake = true;
    });
    let mut display = scanout();
    const ARENA: usize = 0xf6000000 / 4096;
    MODEL.with(|model| {
        let model = model.borrow();
        assert_eq!(model.resets, [32, 32, 1, 1]);
        assert!(model.table[..ARENA].iter().all(|pte| *pte == 0x700001));
        assert_eq!(model.table[ARENA], DMA_BASE as u64 | 1);
        assert_eq!(model.table[ARENA + 1], (DMA_BASE + FRAME_BYTES) as u64 | 1);
        assert_eq!(model.table[ARENA + 2], 0);
        assert_eq!(*model.table.last().unwrap(), 0x700001); // top firmware hole
        assert_eq!(model.mmio[SURFACE / 4], 0); // no display programming by reset
    });
    display.detach_before_flip();
    MODEL.with(|model| assert!(model.borrow().table.iter().all(|pte| *pte == 0x700001)));
}

#[test]
fn vacant_ggtt_also_acquires_gt_ownership_before_initializing_default_all_wb_pat() {
    reset();
    MODEL.with(|model| {
        let mut model = model.borrow_mut();
        model.gt_handshake = true;
        model.fail_engine_dma = true; // stop after PAT setup, before engine allocation
                                      // Match the physical trace: 8 MiB GGTT table, first vacant PTE at
                                      // GPU address 32 MiB, and the documented all-WB PAT default.
        model.table = vec![0; 8 * 1024 * 1024 / 8];
        model.table[..32 * 1024 * 1024 / 4096].fill(0x700001);
        model.mmio[0x40e0 / 4] = 0x03030303;
        model.mmio[0x40e4 / 4] = 0x03030303;
    });
    let mut display = scanout();
    MODEL.with(|model| {
        assert_eq!(
            model.borrow().table[32 * 1024 * 1024 / 4096],
            DMA_BASE as u64 | 1
        )
    });
    let table = MODEL.with(|model| model.borrow().table.clone());
    display.initialize_acceleration(16, device());
    MODEL.with(|model| {
        let model = model.borrow();
        assert_eq!(model.resets, [32, 32, 1, 1, 8, 8]);
        assert_eq!(model.mmio[0x40e0 / 4], 0x03030303);
        assert_eq!(model.mmio[0x40e4 / 4], 0x00030303);
        assert_eq!(model.table, table); // no filled-table arena reclamation
        assert_eq!(model.mmio[SURFACE / 4], 0);
        assert_eq!(model.mmio[0x22230 / 4], 0); // no context submitted on DMA failure
    });
    assert_eq!(display.accelerated(), 0);
    let shared = vec![0x44; display.mode.bytes];
    display
        .present(shared.as_ptr() as Word, rect(0, 0))
        .unwrap();
}

#[test]
fn vacant_ggtt_does_not_change_pat_when_gt_ownership_cannot_be_acquired() {
    reset();
    MODEL.with(|model| {
        let mut model = model.borrow_mut();
        model.mmio[0x40e0 / 4] = 0x03030303;
        model.mmio[0x40e4 / 4] = 0x03030303;
    });
    let mut display = scanout();
    display.initialize_acceleration(16, device());
    MODEL.with(|model| {
        let model = model.borrow();
        assert!(model.resets.is_empty());
        assert_eq!(model.mmio[0x40e4 / 4], 0x03030303);
    });
    assert_eq!(display.accelerated(), 0);
}

#[test]
fn busy_firmware_command_engine_is_not_reset_and_no_pte_is_changed() {
    reset();
    MODEL.with(|model| {
        let mut model = model.borrow_mut();
        model.table.fill(0x700001);
        model.gt_handshake = true;
        model.mmio[0x203c / 4] = 1;
        model.mmio[0x2030 / 4] = 8;
    });
    assert!(hardware::Scanout::new(16, device(), 24).is_err());
    MODEL.with(|model| {
        let model = model.borrow();
        assert!(model.resets.is_empty());
        assert!(model.table.iter().all(|pte| *pte == 0x700001));
    });
}

#[test]
fn takeover_never_claims_ptes_after_reset_or_ready_handshake_failure() {
    for ready in [false, true] {
        reset();
        MODEL.with(|model| {
            let mut model = model.borrow_mut();
            model.table.fill(0x700001);
            model.gt_handshake = true;
            model.stuck_ready = ready;
            if !ready {
                model.stuck_reset = 1;
            }
        });
        assert!(hardware::Scanout::new(16, device(), 24).is_err());
        MODEL.with(|model| {
            let model = model.borrow();
            assert!(model.table.iter().all(|pte| *pte == 0x700001));
            assert_eq!(model.resets, [32, 32]);
            for base in [0x2000, 0x22000, 0x12000, 0x1a000] {
                assert_eq!(model.mmio[(base + 0xd0) / 4], 1 << 16);
            }
            assert_eq!(model.mmio[SURFACE / 4], 0);
            assert!(model.sleeps.len() < 110);
        });
    }
}

#[test]
fn scanout_can_use_empty_ggtt_entries_above_the_cpu_aperture() {
    reset();
    const APERTURE_PAGES: usize = 128 * 1024 * 1024 / 4096;
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

#[test]
fn inherited_pci_layout_uses_msac_and_reads_64bit_bars_without_writes() {
    for code in [0, 1, 3, 7, 15, 31] {
        let layout = pci_layout::read(|offset, width| match (offset, width) {
            (4, 2) => Ok(6),
            (0x10, 4) => Ok(0x8000_0004),
            (0x14, 4) => Ok(0),
            (0x18, 4) => Ok(12),
            (0x1c, 4) => Ok(1),           // Aperture above 4 GiB.
            (0x62, 1) => Ok(code | 0xe0), // Ignore scratch bits.
            _ => panic!("unexpected config access"),
        })
        .unwrap();
        assert_eq!(layout.registers, 0x8000_0000);
        assert_eq!(layout.aperture, 1usize << 32);
        assert_eq!(
            layout.aperture_bytes,
            (code as usize + 1) * 128 * 1024 * 1024
        );
    }
}

#[test]
fn inherited_pci_layout_rejects_disabled_decode_illegal_sizes_and_unassigned_bars() {
    for (command, size, base) in [
        (0, 1, 0x9000_000c),
        (6, 2, 0x9000_000c),
        (6, 1, 12),
        (6, 3, 0x9000_000c),
        (6, 1, 0x9000_0001),
    ] {
        assert!(pci_layout::read(|offset, width| match (offset, width) {
            (4, 2) => Ok(command),
            (0x10, 4) => Ok(0x8000_0004),
            (0x14 | 0x1c, 4) => Ok(0),
            (0x18, 4) => Ok(base),
            (0x62, 1) => Ok(size),
            _ => panic!("unexpected config access"),
        })
        .is_err());
    }
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
fn ggtt_nonzero_invalid_entries_are_not_mistaken_for_live_mappings() {
    let mut regs = registers();
    regs.table.fill(0xabc000); // Present=0, stale physical-address bits.
    regs.table[2] = 0x123001;
    assert_eq!(gen9::free_span(&regs, 16, 4), Ok(3));
    gen9::bind(&mut regs, 3, DMA_BASE, 4).unwrap();
    assert_eq!(regs.table[2], 0x123001);
    assert_eq!(regs.table[3], DMA_BASE as u64 | 1);
}

#[test]
fn takeover_arena_skips_aperture_active_cursors_and_top_firmware_reservation() {
    let mut regs = registers();
    let pages = (1usize << 32) / 4096;
    let aperture = 128 * 1024 * 1024;
    let original = handoff::arena(&regs, pages, aperture).unwrap();
    regs.registers.insert(0x71080, 0x27); // enabled cursor on pipe B
    regs.registers
        .insert(0x71084, (original.start * 4096) as u32);
    let arena = handoff::arena(&regs, pages, aperture).unwrap();
    assert_eq!(arena.end, original.start);
    assert!(arena.start * 4096 >= aperture);
    assert!(arena.end < pages - 32 * 1024 * 1024 / 4096);
    assert!(handoff::arena(&regs, pages, 1usize << 32).is_err());
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
