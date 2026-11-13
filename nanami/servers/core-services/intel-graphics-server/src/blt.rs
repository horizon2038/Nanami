//! Gen9 BCS, one trusted logical context and one synchronous submission at a
//! time. No raw batches, legacy ring submission, global GPU reset or IRQs.
#[path = "blt/commands.rs"]
pub mod commands;
#[path = "blt/ppgtt.rs"]
mod ppgtt;
use crate::gen9::{self, Registers};
use nanami_runtime::{RequestError, Word};
use nanami_uapi::gpu::{Layout, Rect};
const PAGE: Word = 4096;
const BLOCK_BYTES: Word = 6 * PAGE; // HWSP, context, ring, status, two scratch pages.
const BASE: Word = 0x22000;
const FORCEWAKE: Word = 0xa188;
const FORCEWAKE_ACK: Word = 0x130044;
const RENDER_FORCEWAKE: Word = 0xa278;
const RENDER_ACK: Word = 0xd84;
// Multi-threaded forcewake has separate kernel/user request/ACK bits. Do not
// mistake firmware's bit 0 for ownership of our user's bit 1, or clear it.
const WAKE_CLIENT: u32 = 1 << 1;
const MI_MODE: Word = BASE + 0x9c;
const MODE: Word = BASE + 0x29c;
const ELSP: Word = BASE + 0x230;
const CSB_PTR: Word = BASE + 0x3a0;
const CSB_BASE: Word = BASE + 0x370;
const IDLE: u32 = 1 << 9;
const STOP_RING: u32 = 1 << 8;
const RUN_LIST: u32 = 1 << 15;
const CONTEXT_COMPLETE: u32 = 1 << 4;
const IDLE_TO_ACTIVE: u32 = 1;
const LRI: u32 = (0x22 << 23) | (1 << 12);

pub struct Engine {
    pub failed: bool,
    memory: Word,
    gpu: Word,
    tables: ppgtt::PageTables,
    context_id: u32,
    csb_head: u32,
    timer: Word,
}
fn sleep(timer: Word) -> Result<(), RequestError> {
    nanami_client::timer::timer_service_sleep_milliseconds(timer, 1)
}
fn wait(
    regs: &impl Registers,
    timer: Word,
    register: Word,
    mask: u32,
    expected: u32,
) -> Result<(), RequestError> {
    for _ in 0..50 {
        let value = regs.read(register);
        if value == u32::MAX {
            return Err(RequestError::Protocol);
        }
        if value & mask == expected {
            return Ok(());
        }
        sleep(timer)?;
    }
    nanami_runtime::println!(
        "[intel-graphics] BCS wait timeout register={:#x} mask={:#x} expected={:#x} actual={:#x}",
        register,
        mask,
        expected,
        regs.read(register)
    );
    Err(RequestError::Transport)
}
fn pat(regs: &mut impl Registers, retired_gt: bool) -> Result<u64, RequestError> {
    let low = regs.read(0x40e0);
    let high = regs.read(0x40e4);
    if low == u32::MAX || high == u32::MAX {
        return Err(RequestError::Protocol);
    }
    let entries = low as u64 | ((high as u64) << 32);
    for index in 0..8 {
        if (entries >> (index * 8)) & 3 == 0 {
            // PPGTT PAT index: PAT bit 7, PCD bit 4, PWT bit 3.
            return Ok(((index & 4) << 5) | ((index & 2) << 3) | ((index & 1) << 3));
        }
    }
    if retired_gt {
        // A full acknowledged firmware-GT retirement gives us ownership of
        // every command context. Its reset/default PAT may be all WB (0x03).
        // Reserve entry 7 for our UC PPGTT leaves BEFORE submitting anything.
        // This is not allowed on the path that merely borrows vacant GGTT
        // entries: other inherited contexts could still use the same index.
        let next = high & 0x00ff_ffff;
        regs.write(0x40e4, next);
        if regs.read(0x40e4) != next {
            regs.write(0x40e4, high);
            return Err(RequestError::Protocol);
        }
        nanami_runtime::println!("[intel-graphics] owned GT private PAT entry 7 initialized UC");
        return Ok(0x98); // PAT=1, PCD=1, PWT=1: index 7
    }
    nanami_runtime::println!(
        "[intel-graphics] BCS no UC memory type in private PAT={:#x}",
        entries
    );
    Err(RequestError::Unsupported)
}

fn reset_bcs(regs: &mut impl Registers, timer: Word) -> Result<(), RequestError> {
    let control = regs.read(BASE + 0x3c);
    if control == u32::MAX {
        return Err(RequestError::Protocol);
    }
    // A disabled ring has no valid context to drain. An enabled firmware
    // ring must be idle; never force a reset through an executing batch.
    if control & 1 != 0
        && (regs.read(MI_MODE) & IDLE == 0
            || regs.read(BASE + 0x30) & 0x1ffff8 != regs.read(BASE + 0x34) & 0x1ffff8)
    {
        nanami_runtime::println!(
            "[intel-graphics] BCS firmware ring still executing control={:#x} mode={:#x}",
            control,
            regs.read(MI_MODE)
        );
        return Err(RequestError::Unsupported);
    }
    if regs.read(BASE + 0xd0) & 4 != 0 {
        return Err(RequestError::Protocol);
    }
    regs.write(BASE + 0xd0, (1 << 16) | 1);
    let result = (|| {
        wait(regs, timer, BASE + 0xd0, 2, 2)?;
        // Gen9 BCS domain only. Do not reset display, RCS, media or GuC.
        // Repeat acknowledged reset and settle before programming registers.
        for _ in 0..2 {
            regs.write(0x941c, 1 << 3);
            wait(regs, timer, 0x941c, 1 << 3, 0)?;
        }
        sleep(timer)?;
        if regs.read(BASE + 0x3c) & 1 != 0 {
            return Err(RequestError::Protocol);
        }
        Ok(())
    })();
    regs.write(BASE + 0xd0, 1 << 16);
    result
}
impl Engine {
    pub fn new(
        regs: &mut impl Registers,
        entries: Word,
        timer: Word,
    ) -> Result<Self, RequestError> {
        Self::initialize(regs, entries, timer, false)
    }

    /// Only after the full GT retirement handshake, before any new context
    /// submission. Does not itself reset render/media/display or acquire them.
    pub fn after_gt_retirement(
        regs: &mut impl Registers,
        entries: Word,
        timer: Word,
    ) -> Result<Self, RequestError> {
        Self::initialize(regs, entries, timer, true)
    }

    fn initialize(
        regs: &mut impl Registers,
        entries: Word,
        timer: Word,
        retired_gt: bool,
    ) -> Result<Self, RequestError> {
        let domains = [(FORCEWAKE, FORCEWAKE_ACK), (RENDER_FORCEWAKE, RENDER_ACK)];
        let mut acquired = 0;
        let result = (|| {
            for (request, ack) in domains {
                if regs.read(request) & WAKE_CLIENT != 0 {
                    nanami_runtime::println!(
                        "[intel-graphics] BCS forcewake client already active register={:#x}",
                        request
                    );
                    return Err(RequestError::Unsupported);
                }
                wait(regs, timer, ack, WAKE_CLIENT, 0)?;
                regs.write(request, (WAKE_CLIENT << 16) | WAKE_CLIENT);
                acquired += 1;
                wait(regs, timer, ack, WAKE_CLIENT, WAKE_CLIENT)?;
            }
            reset_bcs(regs, timer)?;
            Self::prepare(regs, entries, timer, retired_gt)
        })();
        if result.is_err() {
            // Prepare can only fail before submission. Self-test failures are
            // handled below, retaining the engine's DMA once ELSP was written.
            for (request, _) in domains[..acquired].iter().rev() {
                regs.write(*request, WAKE_CLIENT << 16);
            }
        }
        let mut engine = result?;
        if let Err(error) = engine.self_test(regs) {
            nanami_runtime::println!(
                "[intel-graphics] BCS self-test failed: {}; retaining private engine DMA",
                error
            );
            return Err(error);
        }
        nanami_runtime::println!("[intel-graphics] Gen9 BCS accelerated fill/copy/present ready");
        Ok(engine)
    }
    fn prepare(
        regs: &mut impl Registers,
        entries: Word,
        timer: Word,
        retired_gt: bool,
    ) -> Result<Self, RequestError> {
        // reset_bcs acknowledged and retired the inherited context. MI_MODE's
        // idle indication need not be asserted on a reset/disabled streamer.
        let tables = ppgtt::PageTables::new(pat(regs, retired_gt)?)?;
        let start = gen9::free_span(regs, entries.min((1usize << 32) / PAGE), BLOCK_BYTES / PAGE)?;
        let (physical, memory) = ppgtt::dma(BLOCK_BYTES)?;
        crate::cache::flush(memory, BLOCK_BYTES);
        gen9::bind(regs, start, physical, BLOCK_BYTES / PAGE)?;
        let gpu = start * PAGE;
        let mut engine = Self {
            failed: false,
            memory,
            gpu,
            tables,
            context_id: 0,
            csb_head: 5,
            timer,
        };
        if let Err(error) = engine.tables.map(gpu, physical, BLOCK_BYTES) {
            gen9::unbind(regs, start, BLOCK_BYTES / PAGE);
            return Err(error);
        }
        // Mask BCS interrupt sources. Poll CSB and a post-sync memory fence.
        regs.write(BASE + 0x98, u32::MAX);
        regs.write(BASE + 0xa8, u32::MAX);
        regs.write(CSB_PTR, (7 << 16) | 5 | (7 << 24) | (5 << 8));
        regs.write(BASE + 0x80, (gpu + 3 * PAGE) as u32);
        regs.write(MODE, (RUN_LIST << 16) | RUN_LIST);
        regs.write(MI_MODE, STOP_RING << 16);
        let _ = regs.read(MI_MODE);
        Ok(engine)
    }
    fn context(&self, tail: u32) {
        let regs =
            unsafe { core::slice::from_raw_parts_mut((self.memory + PAGE) as *mut u32, PAGE / 4) };
        regs.fill(0);
        regs[1] = LRI | 27; // Gen9: 14 register/value pairs, not Gen8's 11.
        let values = [
            (0x02, BASE + 0x244, (15 << 16) | 9),
            (0x04, BASE + 0x34, 0),
            (0x06, BASE + 0x30, tail),
            (0x08, BASE + 0x38, (self.gpu + 2 * PAGE) as u32),
            (0x0a, BASE + 0x3c, 1),
            (0x0c, BASE + 0x168, 0),
            (0x0e, BASE + 0x140, 0),
            (0x10, BASE + 0x110, 1 << 5),
            (0x12, BASE + 0x11c, 0),
            (0x14, BASE + 0x114, 0),
            (0x16, BASE + 0x118, 0),
            (0x18, BASE + 0x1c0, 0),
            (0x1a, BASE + 0x1c4, 0),
            // Gen9 register-image layout includes these slots, but the
            // RCS indirect-context facility is unsupported on BCS. Keep its
            // pointer/length/offset zero; do not import RCS's 0x26 offset.
            (0x1c, BASE + 0x1c8, 0),
        ];
        for (index, register, value) in values {
            regs[index] = register as u32;
            regs[index + 1] = value;
        }
        regs[0x21] = LRI | 17; // timestamp and four upper/lower PDP pairs.
        regs[0x22] = (BASE + 0x3a8) as u32;
        for (index, pdp) in (0x24..=0x30).step_by(4).zip((0..4).rev()) {
            regs[index] = (BASE + 0x274 + pdp * 8) as u32;
            regs[index + 2] = (BASE + 0x270 + pdp * 8) as u32;
        }
        regs[0x31] = (self.tables.root() >> 32) as u32;
        regs[0x33] = self.tables.root() as u32;
        regs[0x41] = LRI | 1;
        regs[0x42] = (BASE + 0x200) as u32;
        // Gen9 saves/restores MI_MODE at fixed context offset 0x54. Supply
        // its complete register group so a subsequent hardware restore cannot
        // inherit STOP_RING or load registers from a Gen8-shaped image.
        regs[0x51] = LRI | 87; // 44 pairs
        let fixed = [
            0x28, 0x9c, 0xc0, 0x178, 0x17c, 0x358, 0x170, 0x150, 0x154, 0x158, 0x41c,
        ];
        for (index, offset) in fixed
            .into_iter()
            .chain((0x600..0x680).step_by(4))
            .chain(core::iter::once(0x68))
            .enumerate()
        {
            regs[0x52 + index * 2] = (BASE + offset) as u32;
        }
        regs[0x55] = STOP_RING << 16;
        regs[0xaa] = 0x0a << 23; // MI_BATCH_BUFFER_END
        crate::cache::flush(self.memory + PAGE, PAGE);
    }
    fn submit(&mut self, regs: &mut impl Registers, command: &[u32]) -> Result<(), RequestError> {
        if self.failed {
            return Err(RequestError::Transport);
        }
        if command.len() > 64 {
            return Err(RequestError::InvalidArgument);
        }
        // The prior context is fully retired and its CSB entries consumed.
        // A different ID prevents lite-restore; wrap instead of exhausting
        // the finite hardware ID space during a long-running desktop session.
        self.context_id = self.context_id % 0xfffff + 1;
        let mut words = [0; 80];
        words[0] = commands::MI_ARB_ENABLE;
        let fence = self.gpu + 3 * PAGE + 0x200;
        words[1..5].copy_from_slice(&commands::flush(fence + 64, 0, true));
        words[5..5 + command.len()].copy_from_slice(command);
        let end = 5 + command.len();
        words[end..end + 4].copy_from_slice(&commands::flush(fence, self.context_id, false));
        let count = (end + 4 + 1) & !1;
        // The previous full context-complete event, not merely the breadcrumb,
        // permits rebuilding this private context and ring for the next job.
        crate::cache::flush(self.memory, 3 * PAGE);
        unsafe {
            core::ptr::copy_nonoverlapping(
                words.as_ptr(),
                (self.memory + 2 * PAGE) as *mut u32,
                count,
            );
            core::ptr::write_volatile((self.memory + 3 * PAGE + 0x200) as *mut u32, 0);
        }
        crate::cache::flush(self.memory + 2 * PAGE, PAGE * 2);
        self.context((count * 4) as u32);
        let descriptor =
            self.gpu as u64 | 1 | (3 << 3) | (1 << 8) | ((self.context_id as u64) << 32);
        // ELSP consumes descriptor 1 first, then descriptor 0, high DWORD first.
        for value in [0, 0, (descriptor >> 32) as u32, descriptor as u32] {
            regs.write(ELSP, value);
        }
        let mut completed = false;
        for _ in 0..200 {
            let tail = regs.read(CSB_PTR) & 7;
            if tail >= 6 {
                break;
            }
            let mut steps = 0;
            while self.csb_head != tail && steps < 6 {
                self.csb_head = (self.csb_head + 1) % 6;
                let offset = CSB_BASE + self.csb_head as Word * 8;
                let status = regs.read(offset);
                let id = regs.read(offset + 4);
                // PRM Vol 6, Context Status: when IDLE_TO_ACTIVE is set the
                // ID and every other status field are undefined. Such an
                // entry is not evidence that our context finished saving.
                completed |= id == self.context_id
                    && status & (IDLE_TO_ACTIVE | CONTEXT_COMPLETE) == CONTEXT_COMPLETE;
                steps += 1;
            }
            regs.write(CSB_PTR, (7 << 24) | (self.csb_head << 8));
            crate::cache::flush(self.memory + 3 * PAGE + 0x200, 64);
            let value =
                unsafe { core::ptr::read_volatile((self.memory + 3 * PAGE + 0x200) as *const u32) };
            if completed && value == self.context_id && regs.read(MI_MODE) & IDLE != 0 {
                return Ok(());
            }
            if sleep(self.timer).is_err() {
                break;
            }
        }
        self.failed = true;
        nanami_runtime::println!(
            "[intel-graphics] BCS timeout; keeping DMA, mappings and context pinned"
        );
        Err(RequestError::Transport)
    }
    fn self_test(&mut self, regs: &mut impl Registers) -> Result<(), RequestError> {
        // Nonzero origins and padded rows expose pitch/coordinate mistakes;
        // untouched pixels and the rest of each scratch page are canaries.
        let layout = Layout::new(16, 16, 128).unwrap();
        let rect = Rect {
            x: 2,
            y: 3,
            width: 7,
            height: 9,
        };
        self.fill(
            regs,
            self.gpu + 4 * PAGE,
            layout,
            self.memory + 4 * PAGE,
            rect,
            0x12345678,
        )?;
        self.copy(
            regs,
            self.gpu + 4 * PAGE,
            layout,
            self.memory + 4 * PAGE,
            self.gpu + 5 * PAGE,
            layout,
            self.memory + 5 * PAGE,
            rect,
            4,
            5,
        )?;
        crate::cache::flush(self.memory + 4 * PAGE, 2 * PAGE);
        for (page, x, y) in [(4, 2, 3), (5, 4, 5)] {
            let pixels = unsafe {
                core::slice::from_raw_parts((self.memory + page * PAGE) as *const u32, PAGE / 4)
            };
            for (index, &pixel) in pixels.iter().enumerate() {
                let column = index % (layout.stride / 4);
                let row = index / (layout.stride / 4);
                let expected = if (x..x + rect.width).contains(&column)
                    && (y..y + rect.height).contains(&row)
                {
                    0x12345678
                } else {
                    0
                };
                if pixel != expected {
                    self.failed = true;
                    return Err(RequestError::Protocol);
                }
            }
        }
        Ok(())
    }
    pub fn map(&mut self, gpu: Word, physical: Word, bytes: Word) -> Result<(), RequestError> {
        if self.failed {
            return Err(RequestError::Transport);
        }
        self.tables.map(gpu, physical, bytes)
    }
    pub fn unmap(&mut self, gpu: Word, bytes: Word) -> Result<(), RequestError> {
        if self.failed {
            return Err(RequestError::Transport);
        }
        self.tables.unmap(gpu, bytes);
        Ok(())
    }
    pub fn fill(
        &mut self,
        regs: &mut impl Registers,
        gpu: Word,
        layout: Layout,
        local: Word,
        rect: Rect,
        color: u32,
    ) -> Result<(), RequestError> {
        if self.failed {
            return Err(RequestError::Transport);
        }
        let command = commands::fill(gpu, layout, rect, color)?;
        crate::cache::flush_rect(local, layout.stride, rect);
        self.submit(regs, &command)?;
        crate::cache::flush_rect(local, layout.stride, rect);
        Ok(())
    }
    pub fn copy(
        &mut self,
        regs: &mut impl Registers,
        source: Word,
        source_layout: Layout,
        source_local: Word,
        target: Word,
        target_layout: Layout,
        target_local: Word,
        rect: Rect,
        x: Word,
        y: Word,
    ) -> Result<(), RequestError> {
        if self.failed {
            return Err(RequestError::Transport);
        }
        let command = commands::copy(source, source_layout, target, target_layout, rect, x, y)?;
        let destination = Rect {
            x,
            y,
            width: rect.width,
            height: rect.height,
        };
        crate::cache::flush_rect(source_local, source_layout.stride, rect);
        crate::cache::flush_rect(target_local, target_layout.stride, destination);
        self.submit(regs, &command)?;
        crate::cache::flush_rect(target_local, target_layout.stride, destination);
        Ok(())
    }
}
