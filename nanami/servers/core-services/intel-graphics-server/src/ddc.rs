//! Gen9 DDC transactions. Only an already-active output is probed: no power
//! well, panel-power, link-training, PLL or mode changes happen here.
use crate::{
    edid::{self, Edid},
    gen9::Registers,
};
use nanami_runtime::{RequestError, Word};

pub trait Delay {
    fn millisecond(&mut self) -> Result<(), RequestError>;
}
pub struct TimerDelay {
    pub port: Word,
    pub remaining: usize,
}
impl Delay for TimerDelay {
    fn millisecond(&mut self) -> Result<(), RequestError> {
        if self.remaining == 0 {
            return Err(RequestError::Transport);
        }
        self.remaining -= 1;
        nanami_client::timer::timer_service_sleep_milliseconds(self.port, 1)
    }
}

const GMBUS0: Word = 0xc5100;
const GMBUS1: Word = 0xc5104;
const GMBUS2: Word = 0xc5108;
const GMBUS3: Word = 0xc510c;
const GMBUS4: Word = 0xc5110;
const GMBUS5: Word = 0xc5120;
const CLOCK_GATE: Word = 0xc2020;
const GMBUS_GATE_DISABLE: u32 = 1 << 31;
const SW_CLEAR: u32 = 1 << 31;
const SW_READY: u32 = 1 << 30;
const CYCLE_WAIT: u32 = 1 << 25;
const CYCLE_INDEX: u32 = 2 << 25;
const CYCLE_STOP: u32 = 4 << 25;
const IN_USE: u32 = 1 << 15;
const WAIT_PHASE: u32 = 1 << 14;
const STALL_TIMEOUT: u32 = 1 << 13;
const HW_READY: u32 = 1 << 11;
const NAK: u32 = 1 << 10;
const ACTIVE: u32 = 1 << 9;

fn wait_gmbus(
    regs: &impl Registers,
    delay: &mut impl Delay,
    mask: u32,
    value: u32,
    errors: bool,
) -> Result<(), RequestError> {
    for attempt in 0..=50 {
        let status = regs.read(GMBUS2);
        if errors && status & (NAK | STALL_TIMEOUT) != 0 {
            return Err(RequestError::Transport);
        }
        if status & mask == value {
            return Ok(());
        }
        if attempt != 50 {
            delay.millisecond()?;
        }
    }
    Err(RequestError::Transport)
}

fn gmbus_block(
    regs: &mut impl Registers,
    delay: &mut impl Delay,
    offset: u8,
    output: &mut [u8; 128],
) -> Result<(), RequestError> {
    let command = SW_READY
        | CYCLE_INDEX
        | CYCLE_WAIT
        | (128 << 16)
        | ((offset as u32) << 8)
        | (0x50 << 1)
        | 1;
    regs.write(GMBUS1, command);
    for part in output.chunks_exact_mut(4) {
        wait_gmbus(regs, delay, HW_READY, HW_READY, true)?;
        part.copy_from_slice(&regs.read(GMBUS3).to_le_bytes());
    }
    wait_gmbus(regs, delay, WAIT_PHASE, WAIT_PHASE, true)?;
    regs.write(GMBUS1, CYCLE_STOP | SW_READY);
    wait_gmbus(regs, delay, ACTIVE, 0, false)
}

pub fn gmbus(
    regs: &mut impl Registers,
    delay: &mut impl Delay,
    pin: u8,
) -> Result<Edid, RequestError> {
    if !(1..=6).contains(&pin) {
        return Err(RequestError::Unsupported);
    }
    // Do not reset/reprogram a transfer left active by another owner.
    if regs.read(GMBUS2) & (ACTIVE | IN_USE) != 0 {
        return Err(RequestError::Transport);
    }
    let saved = [
        regs.read(GMBUS0),
        regs.read(GMBUS4),
        regs.read(GMBUS5),
        regs.read(CLOCK_GATE),
    ];
    // Display WA #0868 applies to Sunrise/Kaby/Cannon Point PCH GMBUS.
    regs.write(CLOCK_GATE, saved[3] | GMBUS_GATE_DISABLE);
    let _ = regs.read(CLOCK_GATE);
    regs.write(GMBUS4, 0); // Polling: never enable an unhandled interrupt.
    regs.write(GMBUS5, 0);
    regs.write(GMBUS1, SW_CLEAR);
    regs.write(GMBUS1, 0);
    regs.write(GMBUS0, pin as u32); // 100 kHz, no byte-count override.
    let result = read_blocks(|offset, output| gmbus_block(regs, delay, offset, output));
    // NAK must become idle before SW_CLEAR. If the owned engine is still busy,
    // leave its pin/clock/data alone; a failed EDID must not abort other work.
    let idle = wait_gmbus(regs, delay, ACTIVE, 0, false);
    if idle.is_ok() {
        regs.write(GMBUS1, SW_CLEAR);
        regs.write(GMBUS1, 0);
        regs.write(GMBUS5, saved[2]);
        regs.write(GMBUS0, saved[0]);
        regs.write(GMBUS4, saved[1]);
        regs.write(CLOCK_GATE, saved[3]);
    }
    idle?;
    result
}

const AUX_BUSY: u32 = 1 << 31;
const AUX_DONE: u32 = 1 << 30;
const AUX_IRQ: u32 = 1 << 29;
const AUX_TIMEOUT: u32 = 1 << 28;
const AUX_TIMEOUT_MAX: u32 = 3 << 26;
const AUX_RX_ERROR: u32 = 1 << 25;
const AUX_STATUS: u32 = AUX_DONE | AUX_TIMEOUT | AUX_RX_ERROR;
const AUX_SIZE_MASK: u32 = 31 << 20;
const AUX_SYNC: u32 = (17 << 5) | 31; // Gen9 FW sync=18, normal sync=32.

fn aux_packet(
    regs: &mut impl Registers,
    delay: &mut impl Delay,
    channel: u8,
    send: &[u8],
    receive: &mut [u8; 20],
) -> Result<usize, RequestError> {
    if channel > 4 || send.len() < 3 || send.len() > 20 {
        return Err(RequestError::InvalidArgument);
    }
    let ctl = 0x64010 + channel as Word * 0x100;
    let data = ctl + 4;
    let saved_ctl = regs.read(ctl);
    if saved_ctl & AUX_BUSY != 0 {
        return Err(RequestError::Transport);
    }
    let mut saved_data = [0; 5];
    for (index, value) in saved_data.iter_mut().enumerate() {
        *value = regs.read(data + index * 4);
    }
    let result = (|| {
        // DP transport errors are retried at least three times, with a bounded
        // delay. Each retry reloads the request, since receive overwrites DATA.
        for _ in 0..5 {
            for (index, part) in send.chunks(4).enumerate() {
                let mut bytes = [0; 4];
                bytes[..part.len()].copy_from_slice(part);
                regs.write(data + index * 4, u32::from_be_bytes(bytes));
            }
            regs.write(
                ctl,
                AUX_BUSY | AUX_STATUS | AUX_TIMEOUT_MAX | AUX_SYNC | ((send.len() as u32) << 20),
            );
            let mut status = regs.read(ctl);
            for _ in 0..5 {
                if status & AUX_BUSY == 0 {
                    break;
                }
                delay.millisecond()?;
                status = regs.read(ctl);
            }
            if status & AUX_BUSY != 0 {
                return Err(RequestError::Transport);
            }
            regs.write(ctl, (status & !(AUX_BUSY | AUX_IRQ)) | AUX_STATUS);
            if status & (AUX_TIMEOUT | AUX_RX_ERROR) != 0 || status & AUX_DONE == 0 {
                delay.millisecond()?;
                continue;
            }
            let count = ((status & AUX_SIZE_MASK) >> 20) as usize;
            if !(1..=20).contains(&count) {
                return Err(RequestError::Protocol);
            }
            for (index, part) in receive[..count].chunks_mut(4).enumerate() {
                part.copy_from_slice(&regs.read(data + index * 4).to_be_bytes()[..part.len()]);
            }
            return Ok(count);
        }
        Err(RequestError::Transport)
    })();
    if regs.read(ctl) & AUX_BUSY == 0 {
        for (index, value) in saved_data.into_iter().enumerate() {
            regs.write(data + index * 4, value);
        }
        regs.write(ctl, saved_ctl & !(AUX_BUSY | AUX_STATUS));
    }
    result
}

fn aux_i2c(
    regs: &mut impl Registers,
    delay: &mut impl Delay,
    channel: u8,
    request: u8,
    payload: &[u8],
    read: &mut [u8],
) -> Result<(), RequestError> {
    let size = payload.len().max(read.len());
    if size > 16 || (!payload.is_empty() && !read.is_empty()) {
        return Err(RequestError::InvalidArgument);
    }
    let mut packet = [0; 20];
    packet[0] = request << 4;
    packet[2] = 0x50; // EDID's seven-bit I2C address, not shifted to 0xa0.
    let mut count = 3;
    if size != 0 {
        packet[3] = (size - 1) as u8;
        count = 4;
    }
    if !payload.is_empty() {
        packet[count..count + payload.len()].copy_from_slice(payload);
        count += payload.len();
    }
    let mut copied = 0;
    let mut deferred = 0;
    loop {
        let mut reply = [0; 20];
        let got = aux_packet(regs, delay, channel, &packet[..count], &mut reply)?;
        match reply[0] >> 4 {
            0 => {
                if !payload.is_empty() {
                    if got != 1 && !(got == 2 && reply[1] as usize == payload.len()) {
                        return Err(RequestError::Protocol);
                    }
                    return Ok(());
                }
                if read.is_empty() {
                    return if got == 1 {
                        Ok(())
                    } else {
                        Err(RequestError::Protocol)
                    };
                }
                if got <= 1 || got - 1 > read.len() - copied {
                    return Err(RequestError::Protocol);
                }
                read[copied..copied + got - 1].copy_from_slice(&reply[1..got]);
                copied += got - 1;
                if copied == read.len() {
                    return Ok(());
                }
                packet[3] = (read.len() - copied - 1) as u8;
                deferred = 0;
            }
            2 | 8 => {
                deferred += 1;
                if deferred == 7 {
                    return Err(RequestError::Transport);
                }
                delay.millisecond()?; // Native or I2C DEFER, never a busy loop.
            }
            _ => return Err(RequestError::Transport),
        }
    }
}

fn aux_block(
    regs: &mut impl Registers,
    delay: &mut impl Delay,
    channel: u8,
    offset: u8,
    output: &mut [u8; 128],
) -> Result<(), RequestError> {
    let result = (|| {
        aux_i2c(regs, delay, channel, 4, &[offset], &mut [])?; // WRITE + MOT
        for part in output.chunks_exact_mut(16) {
            aux_i2c(regs, delay, channel, 5, &[], part)?;
        }
        Ok(())
    })();
    // Release I2C MOT on both success and failure. AUX_BUSY protection prevents
    // this cleanup from overwriting a timed-out, still-running transaction.
    let stop = aux_i2c(regs, delay, channel, 1, &[], &mut []);
    result?;
    stop
}

pub fn aux(
    regs: &mut impl Registers,
    delay: &mut impl Delay,
    channel: u8,
) -> Result<Edid, RequestError> {
    read_blocks(|offset, output| aux_block(regs, delay, channel, offset, output))
}

fn read_blocks(
    mut read: impl FnMut(u8, &mut [u8; 128]) -> Result<(), RequestError>,
) -> Result<Edid, RequestError> {
    let mut bytes = [0; 256];
    read(0, (&mut bytes[..128]).try_into().unwrap())?;
    let extensions = edid::base_extensions(&bytes[..128])?;
    if extensions != 0 {
        // The first extension does not require the E-DDC segment pointer.
        // On a failed extension retain a valid base, and report truncation.
        if read(128, (&mut bytes[128..]).try_into().unwrap()).is_ok()
            && edid::valid_block(&bytes[128..])
        {
            if let Ok(complete) = Edid::parse(&bytes) {
                return Ok(complete);
            }
        }
    }
    Edid::parse(&bytes[..128])
}
