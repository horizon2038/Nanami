//! Read the timer owner's running HPET counter, without synchronous clock IPC
//! in every USB wakeup. This adapter NEVER writes HPET registers. Narrow or
//! unavailable counters retain the timer-service path (no ambiguous wrap epoch).
use nanami_runtime::{RequestError, Word};

#[derive(Clone, Copy)]
pub struct Clock {
    timer: Word,
    counter: Option<Counter>,
}

impl Clock {
    pub const fn rpc(timer: Word) -> Self {
        Self {
            timer,
            counter: None,
        }
    }

    pub fn prepare(manager: Word, timer: Word) -> Self {
        #[allow(unused_mut)]
        let mut clock = Self::rpc(timer);
        #[cfg(target_arch = "x86_64")]
        {
            // Start the timer owner before observing its counter. Its absolute
            // alarms and this read-only view must share the same epoch/units.
            let counter = (|| {
                let (_, hz) = nanami_client::timer::timer_service_monotonic_ticks(timer)?;
                if hz != 1_000_000_000 {
                    return Err(RequestError::Unsupported);
                }
                let physical = nanami_client::device::hpet_counter_base(manager)?;
                let offset = physical & 4095;
                if offset & 7 != 0 {
                    return Err(RequestError::Protocol);
                }
                let (_, mapping) = nanami_runtime::request_mmio(physical & !4095, 4096 + offset)?;
                unsafe { Counter::new(mapping + offset) }
            })();
            match counter {
                Ok(counter) => {
                    clock.counter = Some(counter);
                    nanami_runtime::println!("[usb-server] completion clock=local-hpet");
                }
                Err(error) => {
                    nanami_runtime::println!("[usb-server] completion clock=timer-rpc ({})", error)
                }
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = manager;
        clock
    }

    pub fn now(&self) -> Result<(Word, Word), RequestError> {
        if let Some(ns) = self.local_nanoseconds() {
            return Ok((ns, 1_000_000_000));
        }
        nanami_client::timer::timer_service_monotonic_ticks(self.timer)
    }

    /// Profiling must not add clock RPCs to the path it measures.
    pub fn local_nanoseconds(&self) -> Option<Word> {
        self.counter.and_then(|counter| counter.nanoseconds())
    }

    #[cfg(test)]
    pub(crate) unsafe fn test_counter(base: usize, timer: Word) -> Result<Self, RequestError> {
        Ok(Self {
            timer,
            counter: Some(Counter::new(base)?),
        })
    }
}

#[derive(Clone, Copy)]
struct Counter {
    base: usize,
    period_fs: u64,
}

impl Counter {
    unsafe fn new(base: usize) -> Result<Self, RequestError> {
        let capabilities = core::ptr::read_volatile(base as *const u64);
        let period_fs = capabilities >> 32;
        let enabled = core::ptr::read_volatile((base + 0x10) as *const u32) & 1 != 0;
        if capabilities & (1 << 13) == 0 || !enabled {
            return Err(RequestError::Unsupported);
        }
        if period_fs == 0 || period_fs > 100_000_000 {
            return Err(RequestError::Protocol);
        }
        Ok(Self { base, period_fs })
    }

    fn nanoseconds(self) -> Option<Word> {
        let raw = coherent_counter(|offset| unsafe {
            core::ptr::read_volatile((self.base + 0xf0 + offset) as *const u32)
        })?;
        Some(
            (u128::from(raw) * u128::from(self.period_fs) / 1_000_000).min(Word::MAX as u128)
                as Word,
        )
    }
}

fn coherent_counter(mut read: impl FnMut(usize) -> u32) -> Option<u64> {
    // Also bounded if a faulty device never returns a coherent sample.
    for _ in 0..8 {
        let high = read(4);
        let low = read(0);
        if high == read(4) {
            return Some(u64::from(high) << 32 | u64::from(low));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> Box<[u64; 512]> {
        let mut memory = Box::new([0; 512]);
        memory[0] = (41_666_667u64 << 32) | (1 << 13);
        memory[0x10 / 8] = 3;
        memory
    }

    #[test]
    fn counter_read_matches_timer_epoch_and_does_not_write_registers() {
        let mut memory = memory();
        memory[0xf0 / 8] = 24_000_000;
        let original = *memory;
        let clock = unsafe { Clock::test_counter(memory.as_ptr() as usize, 24) }.unwrap();
        assert_eq!(clock.now().unwrap(), (1_000_000_008, 1_000_000_000));
        assert_eq!(*memory, original);
        memory[0xf0 / 8] = 48_000_000;
        assert_eq!(clock.local_nanoseconds(), Some(2_000_000_016));
    }

    #[test]
    fn disabled_or_narrow_counter_cannot_supply_absolute_timer_deadlines() {
        for (wide, enabled) in [(false, true), (true, false)] {
            let mut memory = memory();
            if !wide {
                memory[0] &= !(1 << 13);
            }
            if !enabled {
                memory[0x10 / 8] = 0;
            }
            assert!(matches!(
                unsafe { Counter::new(memory.as_ptr() as usize) },
                Err(RequestError::Unsupported)
            ));
        }
    }

    #[test]
    fn invalid_period_is_rejected() {
        for period in [0u64, 100_000_001] {
            let mut memory = memory();
            memory[0] = period << 32 | (1 << 13);
            assert!(matches!(
                unsafe { Counter::new(memory.as_ptr() as usize) },
                Err(RequestError::Protocol)
            ));
        }
    }

    #[test]
    fn split_counter_read_retries_rollover_and_is_bounded() {
        let mut values = [1, u32::MAX, 2, 2, 7, 2].into_iter();
        assert_eq!(
            coherent_counter(|_| values.next().unwrap()),
            Some((2u64 << 32) | 7)
        );
        let mut reads = 0;
        assert_eq!(
            coherent_counter(|_| {
                reads += 1;
                reads
            }),
            None
        );
        assert_eq!(reads, 24);
    }
}
