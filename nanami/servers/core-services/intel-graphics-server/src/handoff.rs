//! Boot-only ownership transfer, before publishing any GPU client interface.
//! Retire firmware command contexts before changing shared GT policy (PAT),
//! independently of whether scanout found vacant GGTT entries. A filled GGTT
//! additionally requires a bounded, non-display allocation arena.
//! Register definitions/sequencing: Intel Gen9 PRM; i915 intel_reset.c.
use crate::gen9::{Mode, Registers, PAGE_BYTES};
use core::ops::Range;
use nanami_runtime::Word;

const ENGINES: [Word; 4] = [0x2000, 0x22000, 0x12000, 0x1a000];
const FORCEWAKE: [(Word, Word); 3] = [(0xa188, 0x130044), (0xa278, 0xd84), (0xa270, 0xd88)];
const RESET: Word = 0x941c;
const ARENA_PAGES: Word = 128 * 1024 * 1024 / PAGE_BYTES;
const TOP_RESERVED_PAGES: Word = 32 * 1024 * 1024 / PAGE_BYTES;

/// Keep the whole CPU aperture (including firmware's primary scanout) and
/// every enabled hardware cursor. Never assume identical PTEs are scratch.
pub fn arena(
    regs: &impl Registers,
    entries: Word,
    aperture_bytes: Word,
) -> Result<Range<Word>, &'static str> {
    let low = aperture_bytes / PAGE_BYTES;
    let high = entries
        .min((1usize << 32) / PAGE_BYTES)
        .checked_sub(TOP_RESERVED_PAGES)
        .ok_or("GGTT too small")?;
    let cursors = cursors(regs);
    let mut end = high;
    loop {
        let start = end
            .checked_sub(ARENA_PAGES)
            .ok_or("GGTT arena unavailable")?;
        if start < low {
            return Err("GGTT arena overlaps CPU aperture");
        }
        if let Some(cursor) = cursors
            .iter()
            .find(|range| start < range.end && range.start < end)
        {
            end = cursor.start;
            continue;
        }
        return Ok(start..end);
    }
}

pub fn cursors(regs: &impl Registers) -> [Range<Word>; 3] {
    core::array::from_fn(|pipe| {
        let step = pipe * 0x1000;
        if regs.read(0x70080 + step) & 0x3f == 0 {
            return 0..0;
        }
        // Gen9's largest supported ARGB cursor is 256x256 (64 pages).
        let start = (regs.read(0x70084 + step) as Word & !0xfff) / PAGE_BYTES;
        start..start + 64
    })
}

fn wait(
    regs: &impl Registers,
    sleep: &mut impl FnMut() -> Result<(), &'static str>,
    register: Word,
    mask: u32,
    expected: u32,
) -> Result<(), &'static str> {
    for _ in 0..50 {
        let value = regs.read(register);
        if value == u32::MAX {
            return Err("GT register inaccessible");
        }
        if value & mask == expected {
            return Ok(());
        }
        sleep()?;
    }
    Err("GT handshake timeout")
}

fn domain_reset(
    regs: &mut impl Registers,
    sleep: &mut impl FnMut() -> Result<(), &'static str>,
    domain: u32,
) -> Result<(), &'static str> {
    // Repeat the acknowledged request to serialize reset completion; settle
    // afterwards. No PCI FLR, PLL/link reset, or display register programming.
    for _ in 0..2 {
        regs.write(RESET, domain);
        wait(regs, sleep, RESET, domain, 0)?;
    }
    sleep()
}

/// A deliberately limited initial engine inventory: Coffee Lake GT1/GT2.
/// GT3 has a second VCS and needs a different reset inventory. Unknown/fused
/// configurations must not silently skip a command engine during takeover.
pub fn known_inventory(device: u16) -> bool {
    matches!(
        device,
        0x3e90
            | 0x3e93
            | 0x3e99
            | 0x3e91
            | 0x3e92
            | 0x3e96
            | 0x3e98
            | 0x3e9a
            | 0x3e9c
            | 0x3e94
            | 0x3e9b
    )
}

pub fn retire_firmware(
    regs: &mut impl Registers,
    device: u16,
    mode: Mode,
    mut sleep: impl FnMut() -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    if !known_inventory(device) {
        return Err("GT takeover requires a known engine inventory");
    }
    let mut acquired = 0;
    let result = (|| {
        for (request, ack) in FORCEWAKE {
            if regs.read(request) & 1 != 0 {
                return Err("GT forcewake already owned");
            }
            wait(regs, &mut sleep, ack, 1, 0)?;
            regs.write(request, (1 << 16) | 1);
            acquired += 1;
            wait(regs, &mut sleep, ack, 1, 1)?;
        }
        // Never force-reset an executing firmware batch (KBL/CFL hang risk).
        for base in ENGINES {
            let control = regs.read(base + 0x3c);
            let state = regs.read(base + 0x9c);
            if control == u32::MAX || state == u32::MAX {
                return Err("GT engine inaccessible");
            }
            if control & 1 != 0
                && (state & (1 << 9) == 0
                    || regs.read(base + 0x30) & 0x1ffff8 != regs.read(base + 0x34) & 0x1ffff8)
            {
                return Err("firmware GT engine still executing");
            }
        }
        // Stop autonomous submissions, then get each command streamer's
        // READY_TO_RESET acknowledgement. Never bypass a failed handshake.
        domain_reset(regs, &mut sleep, 1 << 5)?; // GuC, not display
        let reset = (|| {
            for base in ENGINES {
                if regs.read(base + 0xd0) & (1 << 2) != 0 {
                    return Err("GT engine catastrophic error");
                }
                regs.write(base + 0xd0, (1 << 16) | 1);
                wait(regs, &mut sleep, base + 0xd0, 2, 2)?;
            }
            domain_reset(regs, &mut sleep, 1)?; // full GT, excludes display
            Ok(())
        })();
        for base in ENGINES {
            regs.write(base + 0xd0, 1 << 16);
        }
        reset?;
        for base in ENGINES {
            if regs.read(base + 0x3c) & 1 != 0 {
                return Err("GT ring survived reset");
            }
        }
        let frame = crate::gen9::frame(regs, mode);
        for _ in 0..40 {
            sleep()?;
            if crate::gen9::frame(regs, mode) != frame {
                return Ok(());
            }
        }
        Err("display stopped after GT reset")
    })();
    for (request, _) in FORCEWAKE[..acquired].iter().rev() {
        regs.write(*request, 1 << 16);
    }
    result
}
