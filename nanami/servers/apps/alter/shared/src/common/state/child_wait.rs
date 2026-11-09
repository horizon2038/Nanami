//! Read-only selection of a waitable child event.
use super::*;

#[cfg(test)]
static THREAD_CHECKS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
pub fn take_thread_checks() -> usize {
    THREAD_CHECKS.swap(0, core::sync::atomic::Ordering::Relaxed)
}

impl Runtime {
    pub fn child_wait_event(
        &self, parent: Word, target: Word, report_stops: bool, report_continues: bool,
    ) -> Option<(Word, u32, bool)> {
        let parent = self.thread_group(parent);
        let group = self.managed_process(parent)?.job.group;
        for child in &self.managed {
            if child.pid == 0
                || child.pid != child.tgid
                || child.parent_pid != parent
                || !wait_target_matches(child.pid, target, child.job.group, group)
            {
                continue;
            }
            let event = if child.exited {
                let signal = child.signals().map_or(0, |state| state.exit_signal);
                let status = if signal == 0 {
                    ((child.exit_status & 0xff) << 8) as u32
                } else {
                    signal as u32 & 0x7f
                };
                (child.pid, status, true)
            } else if let Some(status) = child.signals().and_then(|state| state.child_event) {
                if (status == 0xffff && report_continues) || (status != 0xffff && report_stops) {
                    (child.pid, status, false)
                } else {
                    continue;
                }
            } else {
                continue;
            };
            // Only a reportable event for the requested child can be blocked
            // by live threads. All checks are read-only; keep table order.
            if self.managed.iter().any(|other| {
                #[cfg(test)]
                THREAD_CHECKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                other.pid != child.pid && !other.exited && other.tgid == child.tgid
            }) {
                continue;
            }
            return Some(event);
        }
        None
    }
}
