//! Optional native clock reads. Timer ownership/alarm delivery stay in the
//! service. No guest mapping, stale NOW cache or CPU-counter interpolation.
use super::{
    timer_service_counter_source, timer_service_monotonic_ticks, TIMER_COUNTER_SOURCE_HPET64,
};
use crate::{RequestError, Word};

/// Bind this reader to a live provider capability. Replacing that capability
/// (even in the same slot) requires a new reader. Unsupported providers retain
/// one fresh clock RPC per sample, without repeating optional discovery.
pub struct MonotonicReader {
    provider: Word,
    attempted: bool,
    counter: Option<Counter>,
    last: Word,
}

impl MonotonicReader {
    pub const fn new() -> Self {
        Self {
            provider: 0,
            attempted: false,
            counter: None,
            last: 0,
        }
    }

    pub fn uses_local_counter(&self) -> bool {
        self.counter.is_some()
    }

    pub fn sample(&mut self, provider: Word) -> Result<(Word, Word), RequestError> {
        if provider == 0 {
            return Err(RequestError::InvalidArgument);
        }
        if self.provider != provider {
            self.counter = None;
            self.provider = provider;
            self.attempted = false;
            self.last = 0;
        }
        if !self.attempted {
            let before = timer_service_monotonic_ticks(provider)?;
            if before.1 == 0 {
                return Err(RequestError::Protocol);
            }
            self.attempted = true;
            #[cfg(any(target_arch = "x86_64", test))]
            if before.1 == 1_000_000_000 {
                if let Ok((counter, after)) = Counter::prepare(provider, before.0) {
                    self.counter = Some(counter);
                    self.last = after;
                    return Ok((after, 1_000_000_000));
                }
            }
            // Preparation can perform IPC/MMIO work. Return a fresh sample,
            // never the older pre-discovery reference as the current time.
            return self.rpc(provider);
        }
        if let Some(counter) = &mut self.counter {
            if let Some(now) = counter.nanoseconds().filter(|now| *now >= self.last) {
                self.last = now;
                return Ok((now, 1_000_000_000));
            }
            // Disabled/reset/incoherent hardware is not a cached clock hit.
            self.counter = None;
        }
        self.rpc(provider)
    }

    fn rpc(&mut self, provider: Word) -> Result<(Word, Word), RequestError> {
        let sample = timer_service_monotonic_ticks(provider)?;
        if sample.1 == 0 {
            return Err(RequestError::Protocol);
        }
        self.last = sample.0;
        Ok(sample)
    }
}

impl Default for MonotonicReader {
    fn default() -> Self {
        Self::new()
    }
}

struct Counter {
    mapping: Word,
    base: Word,
    period_fs: u64,
    high: Option<u32>,
}

impl Counter {
    fn prepare(provider: Word, before: Word) -> Result<(Self, Word), RequestError> {
        let (physical, period) =
            timer_service_counter_source(provider, TIMER_COUNTER_SOURCE_HPET64)?;
        let offset = physical & 4095;
        if physical == 0
            || physical & 7 != 0
            || offset > 4096 - 0xf8
            || period == 0
            || period > 100_000_000
        {
            return Err(RequestError::Protocol);
        }
        let page = physical & !4095;
        let (actual, mapping) = nanami_runtime::request_mmio(page, 4096)?;
        if mapping == 0 || mapping & 4095 != 0 {
            return Err(RequestError::Protocol);
        }
        if actual != page || mapping.checked_add(4096).is_none() {
            let _ = nanami_runtime::request_mapping_release(mapping, 4096);
            return Err(RequestError::Protocol);
        }
        let mut counter = Self {
            mapping,
            base: mapping + offset,
            period_fs: period as u64,
            high: None,
        };
        // The mapping uses existing native Uncached MMIO attributes. This
        // adapter only reads; it does not change comparator/configuration or
        // use ordinary protection APIs that could replace the cache attribute.
        let capabilities = unsafe { core::ptr::read_volatile(counter.base as *const u64) };
        if capabilities & (1 << 13) == 0 || capabilities >> 32 != period as u64 {
            return Err(RequestError::Protocol);
        }
        let local = counter.nanoseconds().ok_or(RequestError::Protocol)?;
        let (after, hz) = timer_service_monotonic_ticks(provider)?;
        // Same global source/epoch/units, bracketed on one client CPU. No TSC
        // synchronization/frequency assumptions are needed across cores.
        if hz != 1_000_000_000 || local < before || local > after {
            return Err(RequestError::Protocol);
        }
        Ok((counter, after))
    }

    fn nanoseconds(&mut self) -> Option<Word> {
        if unsafe { core::ptr::read_volatile((self.base + 0x10) as *const u32) } & 1 == 0 {
            return None;
        }
        let raw = coherent_counter(self.high, |offset| unsafe {
            core::ptr::read_volatile((self.base + 0xf0 + offset) as *const u32)
        })?;
        self.high = Some((raw >> 32) as u32);
        Some(scale_counter(raw, self.period_fs))
    }
}

pub(super) fn scale_counter(raw: u64, period_fs: u64) -> Word {
    // Integral-nanosecond periods need no wide fractional conversion. Saturate
    // before narrowing exactly as in the general femtosecond calculation.
    if period_fs % 1_000_000 == 0 {
        raw.saturating_mul(period_fs / 1_000_000).min(Word::MAX as u64) as Word
    } else {
        (u128::from(raw) * u128::from(period_fs) / 1_000_000).min(Word::MAX as u128) as Word
    }
}

impl Drop for Counter {
    fn drop(&mut self) {
        // Alpha retains failed cleanup in its VM accounting. Never recycle
        // device backing as RAM or hand the alias to a Linux guest.
        let _ = nanami_runtime::request_mapping_release(self.mapping, 4096);
    }
}

pub(super) fn coherent_counter(previous_high: Option<u32>, mut read: impl FnMut(usize) -> u32) -> Option<u64> {
    // HPET 1.0a section2.4.7: even a64-bit access can be split by the chipset.
    // Keep the last confirmed high read as the leading half of high/low/high.
    // On a monotonic counter, equal bracketing highs prove the fresh low valid
    // even when the first high came from the previous sample. Never reuse low
    // bits or a scaled time. Configuration, epoch and backwards-time checks
    // remain live, and replacing/dropping the source discards this hint.
    let mut high = previous_high.unwrap_or_else(|| read(4));
    for _ in 0..8 {
        let low = read(0);
        let after = read(4);
        if high == after {
            return Some(u64::from(high) << 32 | u64::from(low));
        }
        // The failed attempt's trailing high precedes the next low as well.
        high = after;
    }
    None
}
