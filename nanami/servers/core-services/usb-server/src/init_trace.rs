//! Bounded, opt-in controller diagnostics for the first enumeration only.
//!
//! The USB server has one event loop. A session encloses its initial poll and
//! IRQ enable, not initialization, hotplug, storage requests or maintenance.
//! The service separately logs four startup IPC boundaries with the same flag.
//! Serial output changes timing; this is not a benchmark or a timeout fix.

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::xhci::registers::{read, MFINDEX, USBCMD, USBSTS};

pub const ENABLED: bool = match option_env!("NANAMI_USB_INIT_TRACE") {
    Some(value) => matches!(value.as_bytes(), [b'1']),
    None => false,
};
const MAX_RECORDS: usize = 128;
// Zero means inactive/exhausted. One reserves the single truncation record.
static REMAINING: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, PartialEq, Eq)]
enum Permit {
    Record,
    Truncated,
    Skip,
}

fn take(budget: &AtomicUsize) -> Permit {
    match budget.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
        remaining.checked_sub(1)
    }) {
        Ok(1) => Permit::Truncated,
        Ok(_) => Permit::Record,
        Err(_) => Permit::Skip,
    }
}

/// The closure is never evaluated when tracing is disabled or out of budget.
#[inline]
pub fn record(output: impl FnOnce()) {
    if !ENABLED {
        return;
    }
    match take(&REMAINING) {
        Permit::Record => output(),
        Permit::Truncated => nanami_runtime::println!(
            "[usb.init] stage=trace.truncated limit={} later-stages-not-recorded=true",
            MAX_RECORDS
        ),
        Permit::Skip => {}
    }
}

#[must_use = "keep the session alive through the initial poll and IRQ enable"]
pub struct Session {
    controller: usize,
}

impl Session {
    pub fn begin(controller: usize) -> Self {
        if ENABLED {
            REMAINING.store(MAX_RECORDS + 1, Ordering::Relaxed);
            nanami_runtime::println!(
                "[usb.init] controller={} stage=initial.begin limit={}",
                controller,
                MAX_RECORDS
            );
        }
        Self { controller }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if ENABLED {
            let remaining = REMAINING.swap(0, Ordering::Relaxed);
            nanami_runtime::println!(
                "[usb.init] controller={} stage=initial.end truncated={}",
                self.controller,
                remaining == 0
            );
        }
    }
}

/// Reads only ordinary status/control registers; no W1C, EOI or ring changes.
#[derive(Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub command: u32,
    pub status: u32,
    pub iman: u32,
    pub mfindex: u32,
}

impl Snapshot {
    /// All bases must be valid mapped xHCI register addresses.
    pub unsafe fn capture(op: usize, interrupter: usize, runtime: usize) -> Self {
        Self {
            command: read(op, USBCMD),
            status: read(op, USBSTS),
            iman: read(interrupter, 0),
            mfindex: read(runtime, MFINDEX) & 0x3fff,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inactive_session_never_grants_a_record() {
        let budget = AtomicUsize::new(0);
        assert_eq!(take(&budget), Permit::Skip);
        assert_eq!(take(&budget), Permit::Skip);
        assert_eq!(budget.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn budget_limits_records_and_reports_truncation_once() {
        let budget = AtomicUsize::new(MAX_RECORDS + 1);
        for _ in 0..MAX_RECORDS {
            assert_eq!(take(&budget), Permit::Record);
        }
        assert_eq!(take(&budget), Permit::Truncated);
        for _ in 0..1000 {
            assert_eq!(take(&budget), Permit::Skip);
        }
    }

    #[test]
    fn status_snapshot_is_read_only() {
        let op = [5_u32, 0x10];
        let interrupter = [2_u32];
        let runtime = [0xabcd_u32];
        let snapshot = unsafe {
            Snapshot::capture(
                op.as_ptr() as usize,
                interrupter.as_ptr() as usize,
                runtime.as_ptr() as usize,
            )
        };
        assert_eq!(
            snapshot,
            Snapshot {
                command: 5,
                status: 0x10,
                iman: 2,
                mfindex: 0xabcd & 0x3fff,
            }
        );
        assert_eq!(op, [5, 0x10]);
        assert_eq!(interrupter, [2]);
        assert_eq!(runtime, [0xabcd]);
    }

    #[test]
    fn session_gates_snapshot_work_and_ends_before_steady_state() {
        let mut work = 0;
        // No live session, even in a trace-enabled build.
        record(|| work += 1);
        assert_eq!(work, 0);
        {
            let _session = Session::begin(0);
            record(|| work += 1);
            assert_eq!(work, usize::from(ENABLED));
            for _ in 0..MAX_RECORDS + 100 {
                record(|| work += 1);
            }
            assert_eq!(work, if ENABLED { MAX_RECORDS } else { 0 });
        }
        record(|| panic!("steady-state diagnostics must not evaluate their arguments"));
        {
            let _session = Session::begin(1);
            record(|| work += 1);
        }
        assert_eq!(work, if ENABLED { MAX_RECORDS + 1 } else { 0 });
        record(|| panic!("hotplug diagnostics must remain inactive"));
    }
}
