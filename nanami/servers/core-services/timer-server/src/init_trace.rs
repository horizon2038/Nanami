//! One-shot diagnostics for the first 10 ms interval on the main notification.
//! No clocks, RPCs, IRQ acknowledgements or extra timers are used for tracing.
//! Serial output can change timing; reply-enter does not prove delivery.
//! An additional opt-in flag splits only this first selected reply/receive.

use nanami_runtime::Word;

const ENABLED: bool = match option_env!("NANAMI_USB_INIT_TRACE") {
    Some(value) => matches!(value.as_bytes(), [b'1']),
    None => false,
};

const SPLIT_REPLY: bool = match option_env!("NANAMI_TIMER_SPLIT_REPLY") {
    Some(value) => matches!(value.as_bytes(), [b'1']),
    None => false,
};

pub(super) struct Trace {
    seen: bool,
    active: bool,
}

impl Trace {
    pub(super) const fn new() -> Self {
        Self {
            seen: false,
            active: false,
        }
    }

    pub(super) fn begin(&mut self, pid: Word, notification: Word, milliseconds: u64) {
        // Match the USB startup request without depending on a hard-coded PID.
        // The first matching caller's actual PID is printed for verification.
        if !ENABLED
            || self.seen
            || milliseconds != 10
            || notification != nanami_runtime::PROCESS_SLOT_NOTIFICATION
        {
            return;
        }
        self.seen = true;
        self.active = true;
        self.record(format_args!(
            "stage=interval.receive pid={} notification={} ms={}",
            pid, notification, milliseconds
        ));
    }

    #[inline]
    pub(super) fn record(&self, fields: core::fmt::Arguments<'_>) {
        if ENABLED && self.active {
            nanami_runtime::println!("[timer.init] {}", fields);
        }
    }

    pub(super) fn reply_enter(&mut self, status: Word) -> bool {
        let split = ENABLED && SPLIT_REPLY && self.active;
        if split {
            self.record(format_args!(
                "stage=reply-enter status={:#x} path=split",
                status
            ));
        } else {
            self.record(format_args!("stage=reply-enter status={:#x}", status));
        }
        // Consume the selector before either syscall, even if Reply fails.
        // Later replies and periodic interrupts retain the original path.
        self.active = false;
        split
    }
}
