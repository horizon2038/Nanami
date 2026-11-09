//! Feature-gated observer control. One ledger lives for the event-loop lifetime.
use alloc::vec::Vec;
use super::syscall_scope::{Error, Ledger, Mode, Sample, Summary, Total, SYSCALLS};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Ignored,
    Started { epoch: u64, mode: Mode },
    Stopped(Summary),
}

pub(crate) struct Controller {
    // Never remove/recreate a ledger: its epoch must survive stop/cancel.
    ledger: Vec<Ledger>,
    completed: Option<(usize, Summary)>,
}

pub(crate) const REPORT_MAGIC: u64 = 0x0031_5043_534e_414e;
pub(crate) const REPORT_BYTES: usize = 64 + (SYSCALLS + 1) * 16;

impl Controller {
    pub(crate) const fn new() -> Self {
        Self { ledger: Vec::new(), completed: None }
    }

    pub(crate) fn command(
        &mut self, owner: usize, name: &[u8], clock: impl FnOnce() -> Option<u64>,
    ) -> Result<Command, Error> {
        let name = name.split(|byte| *byte == 0).next().unwrap_or(&[]);
        let mode = match name {
            b"nscope.count" => Some(Mode::Count),
            b"nscope.time" => Some(Mode::Time),
            b"nscope.stop" => None,
            _ => return Ok(Command::Ignored),
        };
        if let Some(mode) = mode {
            if self.ledger.is_empty() {
                self.ledger.try_reserve_exact(1).map_err(|_| Error::NoMemory)?;
                self.ledger.push(Ledger::new());
            }
            let now = clock().ok_or(Error::MissingClock)?;
            let epoch = self.ledger[0].start(owner, mode, now)?;
            self.completed = None;
            Ok(Command::Started { epoch, mode })
        } else {
            let ledger = self.ledger.first_mut().ok_or(Error::Inactive)?;
            let now = clock().ok_or(Error::MissingClock)?;
            let summary = ledger.stop(owner, now)?;
            self.completed = Some((owner, summary));
            Ok(Command::Stopped(summary))
        }
    }

    pub(crate) fn sample(&self, clock: impl FnOnce() -> Option<u64>) -> Option<Sample> {
        let ledger = self.ledger.first()?;
        let now = if ledger.needs_clock() { clock() } else { None };
        ledger.sample(now)
    }

    pub(crate) fn record(
        &mut self, sample: Option<Sample>, number: usize, clock: impl FnOnce() -> Option<u64>,
    ) -> Result<(), Error> {
        let Some(sample) = sample else { return Ok(()); };
        let ledger = self.ledger.first_mut().ok_or(Error::StaleSample)?;
        let now = if ledger.needs_clock() { clock() } else { None };
        ledger.record(sample, number, now)
    }

    pub(crate) fn retire(&mut self, owner: usize) -> bool {
        if self.completed.is_some_and(|(expected, _)| expected == owner) {
            self.completed = None;
        }
        self.ledger.first_mut().is_some_and(|ledger| ledger.cancel(owner).is_ok())
    }

    /// Bounded owned LE packet, copied with the ordinary checked guest writer.
    /// No native address, mapping authority or pointer is placed in the packet.
    pub(crate) fn report(&self, owner: usize) -> Result<Vec<u8>, Error> {
        let (expected, summary) = self.completed.ok_or(Error::Inactive)?;
        if owner != expected {
            return Err(Error::NotOwner);
        }
        let mut packet = Vec::new();
        packet.try_reserve_exact(REPORT_BYTES).map_err(|_| Error::NoMemory)?;
        let mode = if summary.mode == Mode::Count { 0 } else { 1 };
        for value in [REPORT_MAGIC, summary.epoch, mode, summary.start, summary.end,
                      u64::from(summary.valid), (SYSCALLS + 1) as u64,
                      1 /* local counter identity/rate NOT verified */] {
            packet.extend_from_slice(&value.to_le_bytes());
        }
        let rows = self.totals().ok_or(Error::Inactive)?;
        for row in rows {
            packet.extend_from_slice(&row.calls.to_le_bytes());
            packet.extend_from_slice(&row.ticks.to_le_bytes());
        }
        Ok(packet)
    }

    pub(crate) fn totals(&self) -> Option<&[Total; SYSCALLS + 1]> {
        self.ledger.first().map(Ledger::totals)
    }
}
