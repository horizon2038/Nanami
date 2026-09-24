#![allow(dead_code)]
extern crate self as libnanami;
extern crate self as nanami_services;
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
    timer.on_interrupt();
    timer.arm(None).unwrap();
    assert_eq!(mmio[0x100 / 8] & (1 << 2), 0);
    assert_eq!(mmio[0x20 / 8], 1); // Only timer 0's W1C status bit.
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
