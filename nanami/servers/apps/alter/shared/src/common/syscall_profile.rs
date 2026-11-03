//! Opt-in handler elapsed time. Includes time descheduled in service calls.
//! Bounded global totals for this single-threaded Alter event loop; no guest
//! memory, timers or scheduling are changed. Never use as exclusive CPU time.
use core::sync::atomic::{AtomicU64, Ordering};

const ENABLED: bool = match option_env!("NANAMI_ALTER_PROFILE") {
    Some(value) => value.len() == 1 && value.as_bytes()[0] == b'1',
    None => false,
};
const SYSCALLS: usize = 512;
static CALLS: [AtomicU64; SYSCALLS] = [const { AtomicU64::new(0) }; SYSCALLS];
static TICKS: [AtomicU64; SYSCALLS] = [const { AtomicU64::new(0) }; SYSCALLS];

pub fn stamp() -> Option<u64> {
    if !ENABLED {
        return None;
    }
    nanami_runtime::diagnostics::local_stamp()
}

pub fn record(pid: usize, number: usize, before: Option<u64>, exiting: bool) {
    if !ENABLED {
        return;
    }
    let after = stamp();
    if let Some(ticks) = nanami_runtime::diagnostics::elapsed(before, after) {
        if number < SYSCALLS {
            CALLS[number].fetch_add(1, Ordering::Relaxed);
            TICKS[number].fetch_add(ticks, Ordering::Relaxed);
        }
    }
    if !exiting {
        return;
    }
    // Flush outside the sampled handler. Totals can include other guests that
    // ran since the last exit; PID identifies the flush trigger, not ownership.
    for number in 0..SYSCALLS {
        let calls = CALLS[number].swap(0, Ordering::Relaxed);
        let ticks = TICKS[number].swap(0, Ordering::Relaxed);
        if calls != 0 {
            nanami_runtime::println!(
                "[alter.profile] trigger={} syscall={} calls={} ticks={}",
                pid,
                number,
                calls,
                ticks
            );
        }
    }
}
