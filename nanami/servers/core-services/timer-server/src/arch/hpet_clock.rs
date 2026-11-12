// HPET counter units are femtoseconds; service clock units are nanoseconds.
pub(super) struct Clock {
    pub(super) period_fs: u64,
    pub(super) wide: bool,
    cycles: u64,
    sampled: bool,
}

impl Clock {
    pub(super) fn new(period_fs: u64, wide: bool) -> Self {
        Self {
            period_fs,
            wide,
            cycles: 0,
            sampled: false,
        }
    }

    pub(super) fn sample(&mut self, mut read: impl FnMut(usize) -> u32) -> u64 {
        let raw = if self.wide {
            // A chipset may split even an aligned 64-bit MMIO access. The
            // previous confirmed high can bracket a new low with a fresh high
            // on a monotonic counter. Never reuse the low word or scaled time.
            let mut high = if self.sampled {
                (self.cycles >> 32) as u32
            } else {
                read(4)
            };
            loop {
                let low = read(0);
                let after = read(4);
                if high == after {
                    break (u64::from(high) << 32) | u64::from(low);
                }
                // A rollover invalidates the low, but this high is already
                // the leading observation for the next attempt.
                high = after;
            }
        } else {
            u64::from(read(0))
        };
        self.observe(raw)
    }

    pub(super) fn observe(&mut self, raw: u64) -> u64 {
        self.cycles = if self.wide {
            raw
        } else {
            self.cycles
                .saturating_add((raw as u32).wrapping_sub(self.cycles as u32) as u64)
        };
        self.sampled = true;
        self.cycles
    }

    pub(super) fn nanoseconds(&self, cycles: u64) -> u64 {
        (u128::from(cycles) * u128::from(self.period_fs) / 1_000_000).min(u64::MAX as u128) as u64
    }

    pub(super) fn last_cycles(&self) -> u64 {
        self.cycles
    }

    pub(super) fn cycles_ceil(&self, nanoseconds: u64) -> u64 {
        (u128::from(nanoseconds) * 1_000_000)
            .div_ceil(u128::from(self.period_fs))
            .min(u64::MAX as u128) as u64
    }
}
