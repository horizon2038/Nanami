// HPET counter units are femtoseconds; service clock units are nanoseconds.
pub(super) struct Clock {
    pub(super) period_fs: u64,
    pub(super) wide: bool,
    cycles: u64,
}

impl Clock {
    pub(super) fn new(period_fs: u64, wide: bool) -> Self {
        Self {
            period_fs,
            wide,
            cycles: 0,
        }
    }

    pub(super) fn observe(&mut self, raw: u64) -> u64 {
        self.cycles = if self.wide {
            raw
        } else {
            self.cycles
                .saturating_add((raw as u32).wrapping_sub(self.cycles as u32) as u64)
        };
        self.cycles
    }

    pub(super) fn nanoseconds(&self, cycles: u64) -> u64 {
        (u128::from(cycles) * u128::from(self.period_fs) / 1_000_000).min(u64::MAX as u128) as u64
    }

    pub(super) fn cycles_ceil(&self, nanoseconds: u64) -> u64 {
        (u128::from(nanoseconds) * 1_000_000)
            .div_ceil(u128::from(self.period_fs))
            .min(u64::MAX as u128) as u64
    }
}
