//! Bounded model for an opt-in, single Alter event-loop measurement scope.
//! Used only by the opt-in controller; runtime admission precedes attribution.
//! Tokens belong to one long-lived ledger; elapsed handlers include RPC waits.

pub(crate) const SYSCALLS: usize = 512;
pub(crate) const OUTSIDE: usize = SYSCALLS;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Total {
    pub calls: u64,
    pub ticks: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Count,
    Time,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    Busy,
    Inactive,
    NotOwner,
    EpochExhausted,
    StaleSample,
    MissingClock,
    NonMonotonic,
    Overlap,
    Overflow,
    NoMemory,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Sample {
    epoch: u64,
    before: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Summary {
    pub epoch: u64,
    pub mode: Mode,
    pub start: u64,
    pub end: u64,
    pub valid: bool,
}

pub(crate) struct Ledger {
    owner: Option<usize>,
    epoch: u64,
    mode: Mode,
    start: u64,
    last_end: u64,
    valid: bool,
    totals: [Total; SYSCALLS + 1],
}

impl Ledger {
    pub(crate) const fn new() -> Self {
        Self {
            owner: None, epoch: 0, mode: Mode::Count, start: 0, last_end: 0,
            valid: false, totals: [Total { calls: 0, ticks: 0 }; SYSCALLS + 1],
        }
    }

    pub(crate) fn start(&mut self, owner: usize, mode: Mode, now: u64) -> Result<u64, Error> {
        if self.owner.is_some() {
            return Err(Error::Busy);
        }
        let epoch = self.epoch.checked_add(1).ok_or(Error::EpochExhausted)?;
        self.totals.fill(Total::default());
        self.epoch = epoch;
        self.owner = Some(owner);
        self.mode = mode;
        self.start = now;
        self.last_end = now;
        self.valid = true;
        Ok(epoch)
    }

    pub(crate) fn needs_clock(&self) -> bool {
        self.owner.is_some() && self.mode == Mode::Time
    }

    pub(crate) fn sample(&self, before: Option<u64>) -> Option<Sample> {
        self.owner.map(|_| Sample { epoch: self.epoch, before })
    }

    pub(crate) fn record(&mut self, sample: Sample, number: usize, end: Option<u64>) -> Result<(), Error> {
        if self.owner.is_none() || sample.epoch != self.epoch {
            return Err(Error::StaleSample);
        }
        let elapsed = match self.mode {
            Mode::Count => Ok(0),
            Mode::Time => match (sample.before, end) {
                (Some(before), Some(end)) if before >= self.start && end >= before => {
                    if before < self.last_end {
                        Err(Error::Overlap)
                    } else {
                        Ok(end - before)
                    }
                }
                (Some(_), Some(_)) => Err(Error::NonMonotonic),
                _ => Err(Error::MissingClock),
            },
        };
        let result = elapsed.and_then(|ticks| {
            let row = &mut self.totals[number.min(OUTSIDE)];
            let calls = row.calls.checked_add(1).ok_or(Error::Overflow)?;
            let ticks = row.ticks.checked_add(ticks).ok_or(Error::Overflow)?;
            *row = Total { calls, ticks };
            Ok(())
        });
        if result.is_err() {
            self.valid = false;
        } else if self.mode == Mode::Time {
            // Successful timed records necessarily have an end counter.
            self.last_end = end.unwrap();
        }
        result
    }

    pub(crate) fn stop(&mut self, owner: usize, now: u64) -> Result<Summary, Error> {
        self.check_owner(owner)?;
        if now < self.last_end {
            self.valid = false;
        }
        self.owner = None;
        Ok(Summary {
            epoch: self.epoch, mode: self.mode, start: self.start, end: now,
            valid: self.valid,
        })
    }

    /// Owner retirement must not leave an arm for a reused PID/image identity.
    pub(crate) fn cancel(&mut self, owner: usize) -> Result<(), Error> {
        self.check_owner(owner)?;
        self.owner = None;
        self.valid = false;
        Ok(())
    }

    pub(crate) fn totals(&self) -> &[Total; SYSCALLS + 1] {
        &self.totals
    }

    fn check_owner(&self, owner: usize) -> Result<(), Error> {
        match self.owner {
            None => Err(Error::Inactive),
            Some(expected) if expected != owner => Err(Error::NotOwner),
            Some(_) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_does_not_partially_commit_or_reuse_epoch() {
        let mut ledger = Ledger::new();
        ledger.start(1, Mode::Time, 0).unwrap();
        ledger.totals[0] = Total { calls: 3, ticks: u64::MAX };
        let sample = ledger.sample(Some(0)).unwrap();
        assert_eq!(ledger.record(sample, 0, Some(1)), Err(Error::Overflow));
        assert_eq!(ledger.totals[0], Total { calls: 3, ticks: u64::MAX });
        assert!(!ledger.stop(1, 1).unwrap().valid);
        ledger.start(1, Mode::Count, 2).unwrap();
        ledger.totals[0] = Total { calls: u64::MAX, ticks: 8 };
        let sample = ledger.sample(None).unwrap();
        assert_eq!(ledger.record(sample, 0, None), Err(Error::Overflow));
        assert_eq!(ledger.totals[0], Total { calls: u64::MAX, ticks: 8 });
        assert!(!ledger.stop(1, 3).unwrap().valid);
        ledger.epoch = u64::MAX;
        assert_eq!(ledger.start(1, Mode::Count, 4), Err(Error::EpochExhausted));
        assert!(ledger.sample(None).is_none());
    }
}
