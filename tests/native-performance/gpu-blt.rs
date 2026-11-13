//! Run the real Gen9 BCS/PPGTT driver against a strict command-stream model.
//! This is NOT Intel hardware, GT power-management or cache-coherency validation.
#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_runtime;
pub use nanami_uapi::RequestError;
pub type Word = usize;
use std::{
    alloc::{alloc_zeroed, dealloc, Layout as MemoryLayout},
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
};
#[macro_export]
macro_rules! println { ($($arg:tt)*) => {{ let _ = format_args!($($arg)*); }}; }
#[path = "../../nanami/servers/core-services/intel-graphics-server/src/blt.rs"]
mod blt;
#[path = "../../nanami/servers/core-services/intel-graphics-server/src/gen9.rs"]
mod gen9;
use gen9::Registers;
use nanami_uapi::gpu::{Layout, Rect};
const PAGE: usize = 4096;
struct Memory {
    physical: Word,
    local: Word,
    bytes: Word,
}
impl Drop for Memory {
    fn drop(&mut self) {
        unsafe {
            dealloc(
                self.local as *mut u8,
                MemoryLayout::from_size_align(self.bytes, PAGE).unwrap(),
            );
        }
    }
}
struct Model {
    memory: Vec<Memory>,
    next_physical: Word,
    regs: BTreeMap<Word, u32>,
    ggtt: Vec<u64>,
    writes: Vec<(Word, u32)>,
    elsp: Vec<u32>,
    ids: Vec<u32>,
    delay: usize,
    pending: Option<u64>,
    wake: bool,
    stuck: bool,
    completion: bool,
    fence: bool,
    sleeps: usize,
    reset_ready: bool,
    reset_ack: bool,
    expected_pat_bits: u64,
    pat_writable: bool,
    invalid_idle_event: bool,
}
impl Default for Model {
    fn default() -> Self {
        let mut ggtt = vec![0; 8192];
        ggtt[0] = 0xdead0001;
        Self {
            memory: Vec::new(),
            next_physical: 0x1000000,
            regs: [
                (0x2209c, 0),
                (0x223a0, 5),
                (0x40e0, 0x03080303),
                (0x40e4, 0x03030303),
            ]
            .into_iter()
            .collect(),
            ggtt,
            writes: Vec::new(),
            elsp: Vec::new(),
            ids: Vec::new(),
            delay: 0,
            pending: None,
            wake: true,
            stuck: false,
            completion: true,
            fence: true,
            sleeps: 0,
            reset_ready: true,
            reset_ack: true,
            expected_pat_bits: 0x10,
            pat_writable: true,
            invalid_idle_event: false,
        }
    }
}
impl Model {
    fn local(&self, physical: Word) -> Word {
        let block = self
            .memory
            .iter()
            .find(|block| physical >= block.physical && physical < block.physical + block.bytes)
            .expect("GPU DMA escaped owned memory");
        block.local + physical - block.physical
    }
    fn ggtt(&self, gpu: Word) -> Word {
        let pte = self.ggtt[gpu / PAGE];
        assert_ne!(pte & 1, 0, "unmapped GGTT access");
        self.local((pte as Word & !4095) + gpu % PAGE)
    }
    fn ppgtt(&self, root: Word, gpu: Word) -> Word {
        let mut table = root;
        for shift in [39, 30, 21, 12] {
            let pte =
                unsafe { ((self.local(table) as *const u64).add((gpu >> shift) & 511)).read() };
            assert_eq!(pte & 3, 3, "GPU access outside private PPGTT");
            if shift == 12 {
                assert_eq!(
                    pte & 0x98,
                    self.expected_pat_bits,
                    "PPGTT must select the validated UC private-PAT entry"
                );
            }
            table = pte as Word & !4095;
        }
        self.local(table + gpu % PAGE)
    }
    fn execute(&mut self, descriptor: u64) {
        assert_eq!(descriptor & 0x119, 0x119); // valid, privileged, 48-bit PPGTT.
        let context = self.ggtt((descriptor as Word & 0xfffff000) + PAGE);
        let regs = unsafe { std::slice::from_raw_parts(context as *const u32, PAGE / 4) };
        // Independently check Gen9's context register layout, not the older
        // Gen8 11-register prologue that the previous model also assumed.
        assert_eq!(regs[1], (0x22 << 23) | (1 << 12) | 27);
        assert_eq!(regs[0x21], (0x22 << 23) | (1 << 12) | 17);
        assert_eq!(regs[0x41], (0x22 << 23) | (1 << 12) | 1);
        assert_eq!(regs[0x42], 0x22200);
        assert_eq!(regs[0x51], (0x22 << 23) | (1 << 12) | 87);
        let group = [
            0x22028, 0x2209c, 0x220c0, 0x22178, 0x2217c, 0x22358, 0x22170, 0x22150, 0x22154,
            0x22158, 0x2241c,
        ];
        for (index, register) in group.into_iter().enumerate() {
            assert_eq!(regs[0x52 + index * 2], register);
        }
        for index in 0..32 {
            assert_eq!(regs[0x68 + index * 2], 0x22600 + index as u32 * 4);
        }
        assert_eq!(regs[0xa8], 0x22068);
        assert_eq!(regs[0xaa], 0x0a << 23);
        assert_eq!(regs[0x18], 0x221c0);
        assert_eq!(regs[0x1a], 0x221c4);
        assert_eq!(regs[0x1c], 0x221c8);
        assert_eq!(regs[0x54], 0x2209c);
        assert_eq!(regs[0x55] & (1 << 24), 1 << 24); // masked STOP_RING clear
        assert_eq!(regs[0x03] >> 16, 15); // explicitly disable RS ctx too
        assert_eq!(regs[0x02], 0x22244);
        assert_eq!(regs[0x23], 0);
        assert_eq!(regs[0x0b], 1);
        assert_eq!([regs[0x19], regs[0x1b], regs[0x1d]], [0; 3]); // no RCS indirect context
        let root = regs[0x33] as Word | ((regs[0x31] as Word) << 32);
        let tail = regs[0x07] as usize;
        assert!(tail <= PAGE && tail % 8 == 0);
        assert_eq!(regs[0x05], 0);
        let ring = unsafe {
            std::slice::from_raw_parts(self.ggtt(regs[0x09] as Word) as *const u32, tail / 4)
        }
        .to_vec();
        let id = (descriptor >> 32) as u32;
        self.ids.push(id);
        let mut cursor = 0;
        while cursor < ring.len() {
            let command = ring[cursor];
            match command {
                0 => cursor += 1,
                value if value == (0x08 << 23) | 1 => cursor += 1,
                value if value >> 23 == 0x26 => {
                    assert_eq!(value & 0xff, 2);
                    assert_ne!(value & (1 << 14), 0);
                    assert_eq!(ring[cursor + 1] & 7, 4);
                    let address = self.ggtt((ring[cursor + 1] & !7) as Word);
                    if self.fence {
                        unsafe {
                            (address as *mut u32).write(ring[cursor + 3]);
                        }
                    }
                    cursor += 4;
                }
                value if value >> 22 == (2 << 7) | 0x50 => {
                    assert_eq!(value & 255, 5);
                    assert_eq!(ring[cursor + 1] & 0xffff0000, (3 << 24) | (0xf0 << 16));
                    let pitch = (ring[cursor + 1] & 0xffff) as Word;
                    let x0 = (ring[cursor + 2] & 0xffff) as Word;
                    let y0 = (ring[cursor + 2] >> 16) as Word;
                    let x1 = (ring[cursor + 3] & 0xffff) as Word;
                    let y1 = (ring[cursor + 3] >> 16) as Word;
                    assert_eq!(ring[cursor + 5], 0);
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let destination =
                                self.ppgtt(root, ring[cursor + 4] as Word + y * pitch + x * 4);
                            unsafe {
                                (destination as *mut u32).write(ring[cursor + 6]);
                            }
                        }
                    }
                    cursor += 7;
                }
                value if value >> 22 == (2 << 7) | 0x53 => {
                    assert_eq!(value & 255, 8);
                    assert_eq!(ring[cursor + 1] & 0xffff0000, (3 << 24) | (0xcc << 16));
                    let pitch = (ring[cursor + 1] & 0xffff) as Word;
                    let sx = (ring[cursor + 6] & 0xffff) as Word;
                    let sy = (ring[cursor + 6] >> 16) as Word;
                    let tx = (ring[cursor + 2] & 0xffff) as Word;
                    let ty = (ring[cursor + 2] >> 16) as Word;
                    let width = (ring[cursor + 3] & 0xffff) as Word - tx;
                    let height = (ring[cursor + 3] >> 16) as Word - ty;
                    for row in 0..height {
                        for column in 0..width {
                            let source = self.ppgtt(
                                root,
                                ring[cursor + 8] as Word
                                    + (sy + row) * ring[cursor + 7] as Word
                                    + (sx + column) * 4,
                            );
                            let target = self.ppgtt(
                                root,
                                ring[cursor + 4] as Word + (ty + row) * pitch + (tx + column) * 4,
                            );
                            unsafe {
                                (target as *mut u32).write((source as *const u32).read());
                            }
                        }
                    }
                    cursor += 10;
                }
                _ => panic!("Unexpected BCS packet {command:#x}"),
            }
        }
        self.regs.insert(0x2209c, 1 << 9);
        self.regs.insert(0x22034, tail as u32);
        self.regs.insert(0x22030, tail as u32);
        if self.completion || self.invalid_idle_event {
            let index = ((self.regs[&0x223a0] & 7) + 1) % 6;
            let status = if self.invalid_idle_event {
                // Only bit 0 is defined. Deliberately give the other fields
                // misleading values, including our ID and CONTEXT_COMPLETE.
                1 | (1 << 4)
            } else {
                (1 << 4) | (1 << 3)
            };
            self.regs.insert(0x22370 + index as usize * 8, status);
            self.regs.insert(0x22374 + index as usize * 8, id);
            let previous = self.regs[&0x223a0];
            self.regs.insert(0x223a0, (previous & !7) | index);
        }
    }
}
thread_local! { static MODEL: Rc<RefCell<Model>> = Rc::new(RefCell::new(Model::default())); }
#[derive(Clone)]
struct Hardware(Rc<RefCell<Model>>);
fn reset() -> Hardware {
    MODEL.with(|model| {
        *model.borrow_mut() = Model::default();
        Hardware(model.clone())
    })
}
impl Registers for Hardware {
    fn read(&self, offset: Word) -> u32 {
        let model = self.0.borrow();
        if matches!(offset, 0x40e0 | 0x40e4)
            && model.regs.get(&0xd84).copied().unwrap_or(0) & 2 == 0
        {
            return u32::MAX; // PAT belongs to the render forcewake domain.
        }
        model.regs.get(&offset).copied().unwrap_or(0)
    }
    fn write(&mut self, offset: Word, value: u32) {
        let mut model = self.0.borrow_mut();
        model.writes.push((offset, value));
        if matches!(offset, 0x40e0 | 0x40e4) && !model.pat_writable {
            return;
        }
        if matches!(
            offset,
            0xa188 | 0xa278 | 0x220d0 | 0x2209c | 0x2229c | 0x223a0
        ) {
            let mask = value >> 16;
            let previous = model.regs.get(&offset).copied().unwrap_or(0);
            model
                .regs
                .insert(offset, (previous & !mask) | (value & mask));
            if matches!(offset, 0xa188 | 0xa278) && model.wake {
                let requested = model.regs[&offset] & 0xffff;
                let ack = if offset == 0xa188 { 0x130044 } else { 0xd84 };
                model.regs.insert(ack, requested);
            }
            if offset == 0x220d0 && value & 1 != 0 && model.reset_ready {
                *model.regs.get_mut(&offset).unwrap() |= 2;
            }
        } else if offset == 0x941c && model.reset_ack {
            assert_eq!(value, 8, "must only reset BCS, not display/other engines");
            assert_eq!(model.regs[&0x220d0] & 2, 2, "reset without READY handshake");
            model.regs.insert(offset, 0);
            for register in [0x2203c, 0x22030, 0x22034, 0x2209c] {
                model.regs.insert(register, 0);
            }
        } else {
            model.regs.insert(offset, value);
        }
        if offset == 0x22230 {
            model.elsp.push(value);
            if model.elsp.len() == 4 {
                let descriptor = (model.elsp[2] as u64) << 32 | model.elsp[3] as u64;
                assert_eq!(&model.elsp[..2], &[0, 0]);
                model.elsp.clear();
                model.regs.insert(0x2209c, 0);
                if model.stuck || model.delay != 0 {
                    model.pending = Some(descriptor);
                } else {
                    model.execute(descriptor);
                }
            }
        }
    }
    fn pte(&self, index: Word) -> u64 {
        self.0.borrow().ggtt[index]
    }
    fn write_pte(&mut self, index: Word, value: u64) {
        assert_ne!(index, 0);
        self.0.borrow_mut().ggtt[index] = value;
    }
}
pub fn request_dma(bytes: Word) -> Result<(Word, Word), RequestError> {
    MODEL.with(|model| {
        let mut model = model.borrow_mut();
        let physical = model.next_physical;
        model.next_physical += bytes;
        let local =
            unsafe { alloc_zeroed(MemoryLayout::from_size_align(bytes, PAGE).unwrap()) } as Word;
        assert_ne!(local, 0);
        model.memory.push(Memory {
            physical,
            local,
            bytes,
        });
        Ok((physical, local))
    })
}
pub mod cache {
    pub fn flush(_: usize, _: usize) {}
    pub fn flush_rect(_: usize, _: usize, _: nanami_uapi::gpu::Rect) {}
}
pub mod timer {
    pub fn timer_service_sleep_milliseconds(
        _: usize,
        milliseconds: usize,
    ) -> Result<(), crate::RequestError> {
        assert_eq!(milliseconds, 1);
        crate::MODEL.with(|model| {
            let mut model = model.borrow_mut();
            model.sleeps += 1;
            if !model.stuck && model.pending.is_some() {
                model.delay = model.delay.saturating_sub(1);
                if model.delay == 0 {
                    let descriptor = model.pending.take().unwrap();
                    model.execute(descriptor);
                }
            }
        });
        Ok(())
    }
}
fn surface(hardware: &mut Hardware, engine: &mut blt::Engine, layout: Layout) -> (Word, Word) {
    let (physical, local) = request_dma(layout.bytes).unwrap();
    let start = gen9::free_span(hardware, 8192, layout.bytes / PAGE).unwrap();
    gen9::bind(hardware, start, physical, layout.bytes / PAGE).unwrap();
    engine.map(start * PAGE, physical, layout.bytes).unwrap();
    (start * PAGE, local)
}

#[test]
fn self_test_and_real_bcs_packets_fill_copy_and_keep_padding_unchanged() {
    let mut hardware = reset();
    let mut engine = blt::Engine::new(&mut hardware, 8192, 24).unwrap();
    let layout = Layout::new(80, 40, 384).unwrap();
    let (source, source_local) = surface(&mut hardware, &mut engine, layout);
    let (target, target_local) = surface(&mut hardware, &mut engine, layout);
    let rect = Rect {
        x: 2,
        y: 3,
        width: 7,
        height: 9,
    };
    engine
        .fill(
            &mut hardware,
            source,
            layout,
            source_local,
            rect,
            0x89abcdef,
        )
        .unwrap();
    engine
        .copy(
            &mut hardware,
            source,
            layout,
            source_local,
            target,
            layout,
            target_local,
            rect,
            11,
            12,
        )
        .unwrap();
    let pixels =
        unsafe { std::slice::from_raw_parts(target_local as *const u32, layout.bytes / 4) };
    for y in 0..40 {
        for x in 0..96 {
            let expected = if (12..21).contains(&y) && (11..18).contains(&x) {
                0x89abcdef
            } else {
                0
            };
            assert_eq!(pixels[y * 96 + x], expected);
        }
    }
    let model = hardware.0.borrow();
    assert_eq!(model.ids, [1, 2, 3, 4]);
    assert_eq!(model.ggtt[0], 0xdead0001);
    assert!(model.writes.iter().all(|(register, _)| matches!(
        *register,
        0xa188
            | 0xa278
            | 0x220d0
            | 0x941c
            | 0x101008
            | 0x22098
            | 0x220a8
            | 0x223a0
            | 0x22080
            | 0x2229c
            | 0x2209c
            | 0x22230
    )));
}

#[test]
fn completed_contexts_allow_repeated_submission_and_csb_wrap() {
    let mut hardware = reset();
    let mut engine = blt::Engine::new(&mut hardware, 8192, 24).unwrap();
    let layout = Layout::new(16, 16, 64).unwrap();
    let (gpu, local) = surface(&mut hardware, &mut engine, layout);
    hardware.0.borrow_mut().delay = 2;
    for color in 0..18 {
        engine
            .fill(
                &mut hardware,
                gpu,
                layout,
                local,
                Rect {
                    x: 0,
                    y: 0,
                    width: 16,
                    height: 16,
                },
                color,
            )
            .unwrap();
        assert_eq!(unsafe { (local as *const u32).read() }, color);
    }
    assert_eq!(hardware.0.borrow().ids.len(), 20);
    assert!(!engine.failed);
}

#[test]
fn neither_a_breadcrumb_nor_context_completion_alone_retires_dma() {
    for fence in [true, false] {
        let mut hardware = reset();
        let mut engine = blt::Engine::new(&mut hardware, 8192, 24).unwrap();
        let layout = Layout::new(16, 16, 64).unwrap();
        let (gpu, local) = surface(&mut hardware, &mut engine, layout);
        hardware.0.borrow_mut().fence = fence;
        hardware.0.borrow_mut().completion = !fence;
        let before = hardware.0.borrow().ggtt.clone();
        assert!(engine
            .fill(
                &mut hardware,
                gpu,
                layout,
                local,
                Rect {
                    x: 0,
                    y: 0,
                    width: 16,
                    height: 16
                },
                1
            )
            .is_err());
        assert!(engine.failed);
        assert!(engine.unmap(gpu, layout.bytes).is_err());
        assert_eq!(hardware.0.borrow().ggtt, before);
    }
}

#[test]
fn undefined_fields_in_idle_to_active_csb_do_not_retire_the_context() {
    let mut hardware = reset();
    let mut engine = blt::Engine::new(&mut hardware, 8192, 24).unwrap();
    let layout = Layout::new(16, 16, 64).unwrap();
    let (gpu, local) = surface(&mut hardware, &mut engine, layout);
    hardware.0.borrow_mut().completion = false;
    hardware.0.borrow_mut().invalid_idle_event = true;
    let before = hardware.0.borrow().ggtt.clone();
    let rect = Rect {
        x: 0,
        y: 0,
        width: 16,
        height: 16,
    };
    assert!(engine
        .fill(&mut hardware, gpu, layout, local, rect, 1)
        .is_err());
    assert!(engine.failed);
    assert!(engine.unmap(gpu, layout.bytes).is_err());
    assert_eq!(hardware.0.borrow().ggtt, before);
}

#[test]
fn gen9_blt_packets_match_literal_prm_vectors_and_signed_pitch_bounds() {
    // KBL Vol 2a XY_COLOR_BLT/XY_SRC_COPY_BLT, with linear-byte pitch
    // cross-checked against Intel IGT intel_blt_copy (tiled pitch differs).
    let source = Layout::new(80, 40, 384).unwrap();
    let target = Layout::new(96, 48, 512).unwrap();
    let rect = Rect {
        x: 2,
        y: 3,
        width: 7,
        height: 9,
    };
    assert_eq!(
        blt::commands::fill(0x02000000, source, rect, 0x12345678).unwrap(),
        [0x54300005, 0x03f00180, 0x00030002, 0x000c0009, 0x02000000, 0, 0x12345678]
    );
    assert_eq!(
        blt::commands::copy(0x02000000, source, 0x03000000, target, rect, 11, 12).unwrap(),
        [
            0x54f00008, 0x03cc0200, 0x000c000b, 0x00150012, 0x03000000, 0, 0x00030002, 384,
            0x02000000, 0
        ]
    );
    assert_eq!(
        blt::commands::flush(0x04000200, 7, true),
        [0x13044002, 0x04000204, 0, 7]
    );
    assert_eq!(
        blt::commands::flush(0x04000200, 8, false),
        [0x13004002, 0x04000204, 0, 8]
    );
    assert!(Layout::new(16, 16, 32704).is_some());
    assert!(Layout::new(16, 16, 32768).is_none()); // negative signed 16-bit pitch
    let forged = Layout {
        width: 16,
        height: 16,
        stride: 32768,
        bytes: 524288,
    };
    assert!(blt::commands::fill(0x02000000, forged, rect, 0).is_err());
    assert!(blt::commands::copy(0x02000000, forged, 0x03000000, target, rect, 0, 0).is_err());
    assert!(blt::commands::copy(0x02000000, source, 0x03000000, forged, rect, 0, 0).is_err());
    assert!(blt::commands::fill(0xfffff000, source, rect, 0).is_err());
}

#[test]
fn busy_engine_or_existing_forcewake_owner_is_not_reprogrammed() {
    for existing_owner in [false, true] {
        let mut hardware = reset();
        if existing_owner {
            hardware.0.borrow_mut().regs.insert(0xa188, 2);
        } else {
            hardware.0.borrow_mut().regs.insert(0x2209c, 0);
            hardware.0.borrow_mut().regs.insert(0x2203c, 1);
        }
        assert!(blt::Engine::new(&mut hardware, 8192, 24).is_err());
        assert!(hardware.0.borrow().memory.is_empty());
        assert!(hardware
            .0
            .borrow()
            .writes
            .iter()
            .all(|(register, _)| matches!(*register, 0xa188 | 0xa278)));
    }
}

#[test]
fn firmware_kernel_forcewake_is_preserved_while_user_client_initializes_disabled_bcs() {
    let mut hardware = reset();
    hardware.0.borrow_mut().regs.extend([
        (0xa188, 1),
        (0x130044, 1),
        (0xa278, 1),
        (0xd84, 1),
        (0x203c, 0x1234),
        (0x7019c, 0x4000),
    ]);
    blt::Engine::new(&mut hardware, 8192, 24).unwrap();
    let model = hardware.0.borrow();
    assert_eq!(model.regs[&0xa188], 3);
    assert_eq!(model.regs[&0xa278], 3);
    assert_eq!(model.regs[&0x203c], 0x1234); // other engine left untouched
    assert_eq!(model.regs[&0x7019c], 0x4000); // display untouched
    assert_eq!(model.writes.iter().filter(|(r, _)| *r == 0x941c).count(), 2);
}

#[test]
fn reset_failures_do_not_allocate_dma_and_release_only_our_forcewake_bits() {
    for ready in [false, true] {
        let mut hardware = reset();
        {
            let mut model = hardware.0.borrow_mut();
            model.reset_ready = ready;
            model.reset_ack = !ready;
            model
                .regs
                .extend([(0xa188, 1), (0x130044, 1), (0xa278, 1), (0xd84, 1)]);
        }
        assert!(blt::Engine::new(&mut hardware, 8192, 24).is_err());
        let model = hardware.0.borrow();
        assert!(model.memory.is_empty());
        assert_eq!(model.regs[&0xa188], 1);
        assert_eq!(model.regs[&0xa278], 1);
        assert!(model.sleeps <= 50);
    }
}

#[test]
fn missing_wake_or_uc_pat_never_submits_a_context() {
    for missing_wake in [false, true] {
        let mut hardware = reset();
        {
            let mut model = hardware.0.borrow_mut();
            model.wake = !missing_wake;
            if !missing_wake {
                model.regs.insert(0x40e0, 0x03030303);
            }
        }
        assert!(blt::Engine::new(&mut hardware, 8192, 24).is_err());
        let model = hardware.0.borrow();
        assert!(model.memory.is_empty());
        assert!(model.ids.is_empty());
        assert!(!model.writes.iter().any(|(reg, _)| *reg == 0x22230));
        assert_eq!(model.regs.get(&0xa188).copied().unwrap_or(0), 0);
        assert_eq!(model.regs.get(&0xa278).copied().unwrap_or(0), 0);
    }
}

#[test]
fn a_posted_self_test_timeout_keeps_all_dma_mappings_and_forcewake_pinned() {
    let mut hardware = reset();
    hardware.0.borrow_mut().stuck = true;
    assert!(blt::Engine::new(&mut hardware, 8192, 24).is_err());
    let model = hardware.0.borrow();
    assert!(model.pending.is_some());
    assert!(!model.memory.is_empty());
    assert!(model.ggtt.iter().filter(|pte| **pte & 1 != 0).count() >= 7);
    assert_eq!(model.regs[&0xa188] & 2, 2);
    assert_eq!(model.regs[&0xa278] & 2, 2);
    assert!(model.sleeps <= 201);
}

#[test]
fn all_wb_pat_is_initialized_only_with_exclusive_retired_gt_ownership() {
    for retired in [false, true] {
        let mut hardware = reset();
        {
            let mut model = hardware.0.borrow_mut();
            model.regs.insert(0x40e0, 0x03030303);
            model.expected_pat_bits = 0x98;
        }
        if retired {
            blt::Engine::after_gt_retirement(&mut hardware, 8192, 24).unwrap();
        } else {
            assert!(blt::Engine::new(&mut hardware, 8192, 24).is_err());
        }
        let model = hardware.0.borrow();
        assert_eq!(model.regs[&0x40e0], 0x03030303);
        if retired {
            assert_eq!(model.regs[&0x40e4], 0x00030303);
            assert_eq!(model.ids, [1, 2]); // actual fill/copy canaries passed
        } else {
            assert_eq!(model.regs[&0x40e4], 0x03030303);
            assert!(model.memory.is_empty());
            assert!(!model
                .writes
                .iter()
                .any(|(r, _)| matches!(*r, 0x40e0 | 0x40e4)));
        }
    }
}

#[test]
fn each_private_pat_index_uses_the_prm_pat_pcd_pwt_leaf_bits() {
    for (index, bits) in [0, 8, 16, 24, 128, 136, 144, 152].into_iter().enumerate() {
        let mut hardware = reset();
        // Only this byte is UC, with nonzero target-cache bits. Those bits
        // must not be confused with the low two memory-type bits.
        let pat = (0x0303030303030303u64 & !(255 << (index * 8))) | (12 << (index * 8));
        {
            let mut model = hardware.0.borrow_mut();
            model.regs.insert(0x40e0, pat as u32);
            model.regs.insert(0x40e4, (pat >> 32) as u32);
            model.expected_pat_bits = bits;
        }
        blt::Engine::new(&mut hardware, 8192, 24).unwrap();
        let model = hardware.0.borrow();
        assert_eq!(model.ids.len(), 2); // real fill/copy interpreted through PPGTT
        assert!(!model
            .writes
            .iter()
            .any(|(register, _)| matches!(*register, 0x40e0 | 0x40e4 | 0x40e8)));
    }
}

#[test]
fn failed_private_pat_readback_aborts_before_any_dma_or_submission() {
    let mut hardware = reset();
    {
        let mut model = hardware.0.borrow_mut();
        model.regs.insert(0x40e0, 0x03030303);
        model.pat_writable = false;
    }
    assert!(blt::Engine::after_gt_retirement(&mut hardware, 8192, 24).is_err());
    let model = hardware.0.borrow();
    assert!(model.memory.is_empty());
    assert!(model.ids.is_empty());
    assert_eq!(model.regs[&0x40e4], 0x03030303);
    assert_eq!(model.regs[&0xa188] & 2, 0);
    assert_eq!(model.regs[&0xa278] & 2, 0);
}

#[test]
fn private_ppgtt_rejects_double_mapping_and_can_unmap_then_rebind() {
    let mut hardware = reset();
    let mut engine = blt::Engine::new(&mut hardware, 8192, 24).unwrap();
    let layout = Layout::new(16, 16, 64).unwrap();
    let (gpu, _) = surface(&mut hardware, &mut engine, layout);
    let (physical, _) = request_dma(PAGE).unwrap();
    assert!(engine.map(gpu, physical, PAGE).is_err());
    engine.unmap(gpu, PAGE).unwrap();
    engine.map(gpu, physical, PAGE).unwrap();
    assert!(engine.map(1usize << 32, physical, PAGE).is_err());
    assert!(engine.map(gpu + 1, physical, PAGE).is_err());
}
