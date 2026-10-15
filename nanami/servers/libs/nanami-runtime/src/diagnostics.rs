//! Opt-in pipeline diagnostics. No clock IPC, timers, allocation or scheduling changes.
//! Ticks are local CPU counter units, NOT microseconds. Do not subtract timestamps
//! from different cores unless their counters have independently been verified to agree.
use core::sync::atomic::{AtomicU64, Ordering};

pub const ENABLED: bool = match option_env!("NANAMI_PIPELINE_TRACE") {
    Some(value) => value.len() == 1 && value.as_bytes()[0] == b'1',
    None => false,
};

#[inline]
pub fn stamp() -> Option<u64> {
    if !ENABLED {
        return None;
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let low: u32;
        let high: u32;
        // Compiler memory barrier as well: do not move queue/IPC accesses across
        // the sample. LFENCE orders the local timestamp, without modifying hardware.
        core::arch::asm!("lfence", "rdtsc", "lfence", out("eax") low, out("edx") high,
            options(nostack, preserves_flags));
        Some((u64::from(high) << 32) | u64::from(low))
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let ticks: u64;
        core::arch::asm!("isb", "mrs {}, cntvct_el0", out(reg) ticks,
            options(nostack, preserves_flags));
        Some(ticks)
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    None
}

pub fn elapsed(before: Option<u64>, after: Option<u64>) -> Option<u64> {
    before.zip(after).and_then(|(a, b)| b.checked_sub(a))
}

pub fn measure<T, E>(
    stage: RpcStage,
    bytes: usize,
    work: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let before = stamp();
    let result = work();
    rpc(stage, before, stamp(), bytes, result.is_err());
    result
}

pub fn clock_info(owner: &str) {
    if !ENABLED {
        return;
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let max = core::arch::x86_64::__cpuid(0).eax;
        let extended = core::arch::x86_64::__cpuid(0x8000_0000).eax;
        let invariant =
            extended >= 0x8000_0007 && core::arch::x86_64::__cpuid(0x8000_0007).edx & (1 << 8) != 0;
        let hz = if max >= 0x15 {
            let info = core::arch::x86_64::__cpuid(0x15);
            if info.eax != 0 && info.ebx != 0 && info.ecx != 0 {
                u64::from(info.ecx) * u64::from(info.ebx) / u64::from(info.eax)
            } else {
                0
            }
        } else {
            0
        };
        crate::println!(
            "[pipe.clock] owner={} counter=tsc tick-hz={} invariant={} cross-core-sync=unverified",
            owner,
            hz,
            invariant
        );
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let hz: u64;
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) hz, options(nostack, preserves_flags));
        crate::println!("[pipe.clock] owner={} counter=cntvct tick-hz={}", owner, hz);
    }
}

#[derive(Clone, Copy)]
#[repr(usize)]
pub enum RpcStage {
    VfsUsb,
    VfsServe,
    LoadStat,
    LoadHeap,
    LoadOpen,
    LoadRead,
    LoadCopy,
    LoadClose,
    LoadSpawn,
    ShellPresent,
    HonokaRender,
    PciScan,
}
const RPC_NAMES: [&str; 12] = [
    "vfs-usb",
    "vfs-serve",
    "load-stat",
    "load-heap",
    "load-open",
    "load-read",
    "load-copy",
    "load-close",
    "load-spawn",
    "shell-present",
    "honoka-render",
    "pci-scan",
];
struct Totals {
    calls: AtomicU64,
    ticks: AtomicU64,
    max: AtomicU64,
    bytes: AtomicU64,
    errors: AtomicU64,
    all_calls: AtomicU64,
    all_ticks: AtomicU64,
    all_bytes: AtomicU64,
}
impl Totals {
    const fn new() -> Self {
        Self {
            calls: AtomicU64::new(0),
            ticks: AtomicU64::new(0),
            max: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            all_calls: AtomicU64::new(0),
            all_ticks: AtomicU64::new(0),
            all_bytes: AtomicU64::new(0),
        }
    }
    fn record(&self, ticks: u64, bytes: u64, error: bool) -> u64 {
        self.ticks.fetch_add(ticks, Ordering::Relaxed);
        self.max.fetch_max(ticks, Ordering::Relaxed);
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
        self.errors.fetch_add(u64::from(error), Ordering::Relaxed);
        self.all_calls.fetch_add(1, Ordering::Relaxed);
        self.all_ticks.fetch_add(ticks, Ordering::Relaxed);
        self.all_bytes.fetch_add(bytes, Ordering::Relaxed);
        self.calls.fetch_add(1, Ordering::Relaxed) + 1
    }
}
static RPC: [Totals; RPC_NAMES.len()] = [const { Totals::new() }; RPC_NAMES.len()];

/// Capture both endpoints BEFORE logging. Values include descheduling while
/// blocked in IPC. Reports are bounded batches; they are not a benchmark.
pub fn rpc(stage: RpcStage, before: Option<u64>, after: Option<u64>, bytes: usize, error: bool) {
    if !ENABLED {
        return;
    }
    let Some(ticks) = elapsed(before, after) else {
        return;
    };
    let index = stage as usize;
    let calls = RPC[index].record(ticks, bytes as u64, error);
    if calls >= 32 || ticks >= 100_000_000 {
        flush_rpc(stage);
    }
}

pub fn flush_rpc(stage: RpcStage) {
    if !ENABLED {
        return;
    }
    let index = stage as usize;
    let stats = &RPC[index];
    let calls = stats.calls.swap(0, Ordering::Relaxed);
    if calls == 0 {
        return;
    }
    crate::println!("[pipe.rpc] stage={} calls={} bytes={} total-ticks={} max-ticks={} errors={} cumulative-calls={} cumulative-ticks={} cumulative-bytes={}",
        RPC_NAMES[index], calls, stats.bytes.swap(0, Ordering::Relaxed),
        stats.ticks.swap(0, Ordering::Relaxed), stats.max.swap(0, Ordering::Relaxed),
        stats.errors.swap(0, Ordering::Relaxed), stats.all_calls.load(Ordering::Relaxed),
        stats.all_ticks.load(Ordering::Relaxed), stats.all_bytes.load(Ordering::Relaxed));
}

#[derive(Clone, Copy)]
#[repr(usize)]
pub enum InputStage {
    Usb,
    Input,
    Honoka,
    Window,
    Shell,
}
const INPUT_NAMES: [&str; 5] = [
    "usb-publish",
    "input-distribute",
    "honoka-pop",
    "honoka-route",
    "shell-pop",
];
static KEYS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
static KEY_ERRORS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
static WAKES: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

// Per-process, single event-loop deferred records. Keep counter sampling and
// queue handling separate from console output; never spin on another process.
struct Deferred {
    count: AtomicU64,
    first: AtomicU64,
    last: AtomicU64,
    work: AtomicU64,
    tag: AtomicU64,
    notifies: AtomicU64,
}
impl Deferred {
    const fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            first: AtomicU64::new(0),
            last: AtomicU64::new(0),
            work: AtomicU64::new(0),
            tag: AtomicU64::new(0),
            notifies: AtomicU64::new(0),
        }
    }
    fn record(&self, count: u64, first: u64, last: u64, work: u64, tag: u64) {
        if self.count.load(Ordering::Relaxed) == 0 {
            self.first.store(first, Ordering::Relaxed);
        }
        self.last.store(last, Ordering::Relaxed);
        self.work.fetch_add(work, Ordering::Relaxed);
        self.tag.fetch_or(tag, Ordering::Relaxed);
        self.count.fetch_add(count, Ordering::Relaxed);
    }
}
static DEFERRED_KEYS: [Deferred; 5] = [const { Deferred::new() }; 5];
static DEFERRED_WAKES: [Deferred; 5] = [const { Deferred::new() }; 5];

/// Record actual processing now; flush on an already existing periodic event.
/// A combined record's work is the SUM of measured work, not the idle interval
/// between its first and last events. No console syscall occurs here.
pub fn defer_keys(
    stage: InputStage,
    count: u64,
    tag: usize,
    before: Option<u64>,
    after: Option<u64>,
    errors: usize,
    notifies: usize,
) {
    if !ENABLED || count == 0 {
        return;
    }
    KEYS[stage as usize].fetch_add(count, Ordering::Relaxed);
    KEY_ERRORS[stage as usize].fetch_add(errors as u64, Ordering::Relaxed);
    DEFERRED_KEYS[stage as usize].record(
        count,
        before.unwrap_or(0),
        after.unwrap_or(0),
        elapsed(before, after).unwrap_or(0),
        0,
    );
    DEFERRED_KEYS[stage as usize]
        .tag
        .store((tag & 255) as u64, Ordering::Relaxed);
    DEFERRED_KEYS[stage as usize]
        .notifies
        .fetch_add(notifies as u64, Ordering::Relaxed);
}

pub fn flush_keys(stage: InputStage) {
    if !ENABLED {
        return;
    }
    let record = &DEFERRED_KEYS[stage as usize];
    let count = record.count.swap(0, Ordering::Relaxed);
    if count == 0 {
        return;
    }
    let work = record.work.swap(0, Ordering::Relaxed);
    let notifies = record.notifies.swap(0, Ordering::Relaxed);
    let total = KEYS[stage as usize].load(Ordering::Relaxed);
    let errors = KEY_ERRORS[stage as usize].load(Ordering::Relaxed);
    if report_batch(total, count) || errors != 0 {
        crate::println!("[pipe.key] stage={} batch={} total={} tag={} start-tick={} end-tick={} work-ticks={} notifies={} notify-errors={} deferred=true",
            INPUT_NAMES[stage as usize], count, total, record.tag.load(Ordering::Relaxed), record.first.load(Ordering::Relaxed),
            record.last.load(Ordering::Relaxed), work, notifies, errors);
    }
}

pub fn defer_wake(stage: InputStage, badge: usize, received: Option<u64>) {
    if !ENABLED {
        return;
    }
    WAKES[stage as usize].fetch_add(1, Ordering::Relaxed);
    DEFERRED_WAKES[stage as usize].record(
        1,
        received.unwrap_or(0),
        received.unwrap_or(0),
        0,
        badge as u64,
    );
}

pub fn flush_wake(stage: InputStage) {
    if !ENABLED {
        return;
    }
    let record = &DEFERRED_WAKES[stage as usize];
    let batch = record.count.swap(0, Ordering::Relaxed);
    if batch == 0 {
        return;
    }
    let badge = record.tag.swap(0, Ordering::Relaxed);
    let count = WAKES[stage as usize].load(Ordering::Relaxed);
    // Always retain an input wake even when the initial log budget is exhausted.
    if report_count(count) || badge & (1 << 49) != 0 {
        crate::println!(
            "[pipe.wake] stage={} count={} badge={:#x} tick={} last-tick={} batch={} deferred=true",
            INPUT_NAMES[stage as usize],
            count,
            badge,
            record.first.load(Ordering::Relaxed),
            record.last.load(Ordering::Relaxed),
            batch
        );
    }
}

fn report_count(count: u64) -> bool {
    count <= 16 || count.is_power_of_two()
}

fn report_batch(total: u64, batch: u64) -> bool {
    let previous = total.saturating_sub(batch);
    previous < 16
        || previous
            .saturating_add(1)
            .checked_next_power_of_two()
            .is_some_and(|next| next <= total)
}

/// Only key counts and source sequence badges, never typed characters/keycodes.
/// Caller reports AFTER enqueue/notify, so diagnostics do not precede delivery.
pub fn keys(
    stage: InputStage,
    count: u64,
    tag: usize,
    before: Option<u64>,
    errors: usize,
    notifies: usize,
) {
    if !ENABLED || count == 0 {
        return;
    }
    let after = stamp();
    let index = stage as usize;
    let total = KEYS[index].fetch_add(count, Ordering::Relaxed) + count;
    let failures = KEY_ERRORS[index].fetch_add(errors as u64, Ordering::Relaxed) + errors as u64;
    if report_batch(total, count) || errors != 0 {
        crate::println!("[pipe.key] stage={} batch={} total={} tag={} start-tick={} end-tick={} work-ticks={} notifies={} notify-errors={}",
            INPUT_NAMES[index], count, total, tag & 255, before.unwrap_or(0), after.unwrap_or(0),
            elapsed(before, after).unwrap_or(0), notifies, failures);
    }
}

pub fn wake(stage: InputStage, badge: usize, received: Option<u64>) {
    if !ENABLED {
        return;
    }
    let count = WAKES[stage as usize].fetch_add(1, Ordering::Relaxed) + 1;
    if report_count(count) {
        crate::println!(
            "[pipe.wake] stage={} count={} badge={:#x} tick={}",
            INPUT_NAMES[stage as usize],
            count,
            badge,
            received.unwrap_or(0)
        );
    }
}

pub fn notify_error(stage: &str) {
    if ENABLED {
        crate::println!(
            "[pipe.notify] stage={} failed=true tick={}",
            stage,
            stamp().unwrap_or(0)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamps_reject_missing_or_backwards_samples() {
        assert_eq!(elapsed(Some(10), Some(15)), Some(5));
        assert_eq!(elapsed(Some(15), Some(10)), None);
        assert_eq!(elapsed(None, Some(10)), None);
    }
    #[test]
    fn totals_keep_count_bytes_max_and_errors_without_clock_calls() {
        let totals = Totals::new();
        assert_eq!(totals.record(5, 1024, false), 1);
        assert_eq!(totals.record(8, 4096, true), 2);
        assert_eq!(totals.ticks.load(Ordering::Relaxed), 13);
        assert_eq!(totals.max.load(Ordering::Relaxed), 8);
        assert_eq!(totals.bytes.load(Ordering::Relaxed), 5120);
        assert_eq!(totals.errors.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn key_logs_are_bounded_after_initial_samples() {
        assert!(report_count(16));
        assert!(!report_count(17));
        assert!(report_count(32));
        assert!(!report_count(33));
        assert!(report_batch(20, 20));
        assert!(report_batch(65, 2));
        assert!(!report_batch(66, 1));
    }
    #[test]
    fn deferred_work_excludes_idle_time_and_keeps_all_wake_badges() {
        let record = Deferred::new();
        record.record(1, 100, 105, 5, 1);
        record.record(1, 1000, 1008, 8, 4);
        assert_eq!(record.count.load(Ordering::Relaxed), 2);
        assert_eq!(record.first.load(Ordering::Relaxed), 100);
        assert_eq!(record.last.load(Ordering::Relaxed), 1008);
        assert_eq!(record.work.load(Ordering::Relaxed), 13);
        assert_eq!(record.tag.load(Ordering::Relaxed), 5);
    }
}
