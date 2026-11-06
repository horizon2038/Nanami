use crate::Word;

pub const TIMER_SERVICE_REQUEST_SLEEP_MILLISECONDS: Word = 0x4001;

pub const TIMER_SERVICE_REQUEST_SLEEP_ASYNC_MILLISECONDS: Word = 0x4002;

pub const TIMER_SERVICE_REQUEST_INTERVAL_MILLISECONDS: Word = 0x4003;

pub const TIMER_SERVICE_REQUEST_MONOTONIC_TICKS: Word = 0x4004;

pub const TIMER_SERVICE_REQUEST_ALARM_TICKS: Word = 0x4005;

/// Optional native-client clocksource discovery. arg0 selects a counter kind;
/// reply details describe its physical block and counter period. Information
/// alone grants no mapping, interrupt or device-programming authority.
pub const TIMER_SERVICE_REQUEST_COUNTER_SOURCE: Word = 0x4006;
/// A running wide HPET: details are block base and femtoseconds per cycle.
/// Its clock is the same absolute nanosecond clock used by timer-service.
/// Narrow HPET/PIT/unsupported backends decline; clients must retain RPC.
pub const TIMER_COUNTER_SOURCE_HPET64: Word = 1;

pub const TIMER_NOTIFICATION_IDENTIFIER_BIT: Word =
    1usize << (core::mem::size_of::<Word>() * 8 - 1);
