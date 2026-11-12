#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_runtime;
pub use std::println;
pub type Word = usize;
#[derive(Debug)]
pub enum RequestError {
    Protocol,
    Unsupported,
    Transport,
}
thread_local! { static MMIO: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
pub fn connect_service_by_name(_: &str, _: Word) -> Result<(), RequestError> {
    Ok(())
}
pub fn request_mmio(_: Word, _: Word) -> Result<(Word, Word), RequestError> {
    Ok((0, MMIO.with(|address| address.get())))
}
pub mod ipc {
    pub fn process_slot_descriptor(slot: usize) -> usize {
        slot
    }
}
pub mod device {
    pub const DEVICE_MANAGER_SERVICE: &str = "device-manager";
    pub fn hpet_mmio_base(_: usize) -> Result<usize, super::RequestError> {
        Ok(0x1000)
    }
}
#[path = "../../nanami/servers/core-services/timer-server/src/arch/hpet_clock.rs"]
mod clock_math;
#[path = "../../nanami/servers/core-services/timer-server/src/arch/x86_64_hpet.rs"]
mod hpet;

fn device(wide: bool) -> Box<[u64; 512]> {
    let mut mmio = Box::new([0; 512]);
    mmio[0] = (10_000_000u64 << 32) | (1 << 15) | if wide { 1 << 13 } else { 0 };
    // Not periodic-capable: one-shot must still work. Leave hostile firmware
    // configuration behind to check that initialization resets all mode bits.
    mmio[0x100 / 8] = (1 << 1) | (1 << 3) | (1 << 6) | (1 << 14) | if wide { 1 << 5 } else { 0 };
    MMIO.with(|address| address.set(mmio.as_mut_ptr() as usize));
    mmio
}

#[test]
fn optional_counter_source_requires_running_wide_owner() {
    let _memory = device(true);
    let mut timer = hpet::prepare(16).unwrap();
    assert_eq!(timer.counter_source(), None);
    timer.start().unwrap();
    assert_eq!(timer.counter_source(), Some((0x1000, 10_000_000)));
    let _memory = device(false);
    let mut timer = hpet::prepare(16).unwrap();
    timer.start().unwrap();
    assert_eq!(timer.counter_source(), None);
}

#[test]
fn clock_read_alone_does_not_enable_interrupts() {
    let mut mmio = device(true);
    let mut timer = hpet::prepare(16).unwrap();
    timer.start().unwrap();
    assert_eq!(
        mmio[0x100 / 8] & ((1 << 1) | (1 << 2) | (1 << 3) | (1 << 6) | (1 << 14)),
        0
    );
    assert_eq!(mmio[0x10 / 8], 3); // Counter enabled, legacy routing selected.
    mmio[0xf0 / 8] = 123_456;
    assert_eq!(timer.now(), 1_234_560);
    timer.arm(None).unwrap();
    assert_eq!(mmio[0x100 / 8] & (1 << 2), 0);
}

#[test]
fn one_shot_disarms_when_queue_becomes_empty() {
    let mut mmio = device(true);
    let mut timer = hpet::prepare(16).unwrap();
    timer.start().unwrap();
    timer.arm(Some(1_000_000)).unwrap();
    assert_eq!(mmio[0x108 / 8], 100_000);
    assert_ne!(mmio[0x100 / 8] & (1 << 2), 0);
    assert_eq!(mmio[0x100 / 8] & (1 << 3), 0);
    mmio[0xf0 / 8] = 100_001;
    mmio[0x20 / 8] = 0; // Edge-mode status is always zero on hardware.
    timer.on_interrupt();
    timer.arm(None).unwrap();
    assert_eq!(mmio[0x100 / 8] & (1 << 2), 0);
    assert_eq!(mmio[0x20 / 8], 0); // No runtime W1C for an edge-mode IRQ.
}

#[test]
fn past_deadline_uses_future_comparator_not_equality_already_missed() {
    let mut mmio = device(true);
    let mut timer = hpet::prepare(16).unwrap();
    timer.start().unwrap();
    mmio[0xf0 / 8] = 500_000;
    timer.arm(Some(1_000_000)).unwrap();
    assert!(mmio[0x108 / 8] >= 501_000);
    assert_eq!(mmio[0xf0 / 8], 500_000); // Never reset a running clock.
}

#[test]
fn counter_wrap_maintenance_is_bounded_and_monotonic() {
    let mut mmio = device(false);
    let mut timer = hpet::prepare(16).unwrap();
    timer.start().unwrap();
    assert_ne!(mmio[0x100 / 8] & (1 << 8), 0);
    timer.arm(None).unwrap();
    assert_eq!(mmio[0x108 / 8], 1 << 30);
    for step in 1..=5u64 {
        mmio[0xf0 / 8] = (step << 30) as u32 as u64;
        timer.on_interrupt();
        assert_eq!(timer.now(), (step << 30) * 10);
        timer.arm(None).unwrap();
        assert_eq!(mmio[0x108 / 8], ((step + 1) << 30) as u32 as u64);
    }
}

#[test]
fn wide_clock_with_narrow_comparator_chunks_long_waits_but_can_idle() {
    let mut mmio = device(true);
    mmio[0x100 / 8] &= !(1 << 5);
    let mut timer = hpet::prepare(16).unwrap();
    timer.start().unwrap();
    timer.arm(Some(1_000_000_000_000)).unwrap();
    assert_eq!(mmio[0x108 / 8], 1 << 30);
    timer.arm(None).unwrap();
    assert_eq!(mmio[0x100 / 8] & (1 << 2), 0);
}

#[test]
fn unchanged_alarm_uses_event_sample_and_fired_alarm_can_be_rearmed() {
    let mut mmio = device(true);
    let mut timer = hpet::prepare(16).unwrap();
    timer.start().unwrap();
    timer.arm(Some(1_000_000)).unwrap();
    mmio[0xf0 / 8] = 50_000;
    assert_eq!(timer.now(), 500_000);
    // A later MMIO value must not be read by the unchanged-alarm path. The
    // real service handles the pending interrupt in its next event.
    mmio[0xf0 / 8] = 100_001;
    timer.arm(Some(1_000_000)).unwrap();
    assert_eq!(mmio[0x108 / 8], 100_000);
    timer.on_interrupt();
    timer.now();
    timer.arm(Some(1_000_000)).unwrap();
    assert!(mmio[0x108 / 8] > 100_001);
}

#[test]
fn physical_24mhz_period_arms_ten_milliseconds_without_resetting_the_epoch() {
    let mut mmio = device(true);
    mmio[0] = (41_666_667u64 << 32) | (1 << 15) | (1 << 13);
    let mut timer = hpet::prepare(16).unwrap();
    timer.start().unwrap();
    mmio[0xf0 / 8] = 48_000_000;
    let now = timer.now();
    assert_eq!(now, 2_000_000_016);
    let deadline = now + 10_000_000;
    timer.arm(Some(deadline)).unwrap();
    let comparator = mmio[0x108 / 8];
    let comparator_ns = u128::from(comparator) * 41_666_667 / 1_000_000;
    assert!(comparator_ns >= u128::from(deadline));
    assert_eq!(mmio[0xf0 / 8], 48_000_000);
    assert_ne!(mmio[0x100 / 8] & (1 << 2), 0);
    assert_eq!(mmio[0x100 / 8] & (1 << 3), 0);
    assert_eq!(mmio[0x10 / 8], 3);
}

#[test]
fn exact_time_conversion_matches_wide_math_on_both_sides_of_product_overflow() {
    for period in [
        1,
        10,
        1_000_000,
        10_000_000,
        41_666_667,
        100_000_000,
        u64::MAX,
    ] {
        let clock = clock_math::Clock::new(period, true);
        let boundary = u64::MAX / period;
        for cycles in [
            0,
            1,
            999_999,
            1_000_000,
            1_000_001,
            boundary.saturating_sub(1),
            boundary,
            boundary.saturating_add(1),
            u64::MAX,
        ] {
            let expected = (u128::from(cycles) * u128::from(period) / 1_000_000)
                .min(u128::from(u64::MAX)) as u64;
            assert_eq!(
                clock.nanoseconds(cycles),
                expected,
                "period={period} cycles={cycles}"
            );
        }
        let boundary = u64::MAX / 1_000_000;
        for nanos in [
            0,
            1,
            9,
            10,
            999_999,
            1_000_000,
            1_000_000_000,
            boundary - 1,
            boundary,
            boundary + 1,
            u64::MAX,
        ] {
            let expected = (u128::from(nanos) * 1_000_000)
                .div_ceil(u128::from(period))
                .min(u128::from(u64::MAX)) as u64;
            assert_eq!(
                clock.cycles_ceil(nanos),
                expected,
                "period={period} nanos={nanos}"
            );
        }
    }
}

#[test]
fn valid_hpet_periods_keep_exact_floor_and_ceil_for_small_and_large_timestamps() {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..10_000 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let period = seed % 100_000_000 + 1;
        let clock = clock_math::Clock::new(period, true);
        for value in [seed, seed % 1_000_000_000] {
            let nanos = (u128::from(value) * u128::from(period) / 1_000_000)
                .min(u128::from(u64::MAX)) as u64;
            let cycles = (u128::from(value) * 1_000_000)
                .div_ceil(u128::from(period))
                .min(u128::from(u64::MAX)) as u64;
            assert_eq!(clock.nanoseconds(value), nanos);
            assert_eq!(clock.cycles_ceil(value), cycles);
        }
    }
}

#[test]
fn conversion_keeps_bounded_clock_metadata_and_observed_counter() {
    assert!(std::mem::size_of::<clock_math::Clock>() <= 3 * std::mem::size_of::<u64>());
    let mut clock = clock_math::Clock::new(41_666_667, true);
    for raw in [0, 1, 24_000_000, u64::MAX] {
        assert_eq!(clock.observe(raw), raw);
        let expected = (u128::from(raw) * 41_666_667 / 1_000_000).min(u128::from(u64::MAX)) as u64;
        assert_eq!(clock.nanoseconds(clock.last_cycles()), expected);
    }
}
