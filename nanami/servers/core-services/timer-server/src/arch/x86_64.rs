use libnanami::{RequestError, Word};

pub const TICK_HZ: u64 = 100;
pub const MODE: &str = "pit-periodic-fallback";

pub struct PreparedTimer {
    pub resource: Word,
    pub irq_number: Word,
    ticks: u64,
}

const PIT_PORT_COUNTER0: Word = 0x40;
const PIT_PORT_COMMAND: Word = 0x43;
const PIT_COMMAND_RATE_GEN_LOHI: Word = 0x34;
const PIT_BASE_HZ: u64 = 1_193_182;

pub fn prepare(timer_resource_slot: Word) -> Result<PreparedTimer, RequestError> {
    libnanami::request_io_port(PIT_PORT_COUNTER0, PIT_PORT_COMMAND, timer_resource_slot)?;
    Ok(PreparedTimer {
        resource: libnanami::ipc::process_slot_descriptor(timer_resource_slot),
        irq_number: 0,
        ticks: 0,
    })
}

pub fn start(timer_resource: Word) -> Result<(), RequestError> {
    let mut divisor = PIT_BASE_HZ / TICK_HZ;
    if divisor == 0 {
        divisor = 1;
    }
    if divisor > u16::MAX as u64 {
        divisor = u16::MAX as u64;
    }

    let divisor = divisor as u16;
    libnanami::io::io_write(
        timer_resource,
        PIT_PORT_COMMAND,
        1,
        PIT_COMMAND_RATE_GEN_LOHI,
    )?;
    libnanami::io::io_write(
        timer_resource,
        PIT_PORT_COUNTER0,
        1,
        (divisor & 0x00ff) as Word,
    )?;
    libnanami::io::io_write(
        timer_resource,
        PIT_PORT_COUNTER0,
        1,
        ((divisor >> 8) & 0x00ff) as Word,
    )?;
    Ok(())
}

impl PreparedTimer {
    pub fn start(&mut self) -> Result<(), RequestError> {
        start(self.resource)
    }
    pub fn now(&mut self) -> u64 {
        self.ticks
    }
    pub fn on_interrupt(&mut self) {
        self.ticks = self.ticks.saturating_add(1);
    }
    // Without an independent clocksource PIT must keep counting periodic ticks.
    pub fn arm(&mut self, _deadline: Option<u64>) -> Result<(), RequestError> {
        Ok(())
    }
}
