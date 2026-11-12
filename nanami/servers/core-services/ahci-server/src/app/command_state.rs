//! One exclusive command slot; a successful completion establishes readiness.
use super::RequestError;
use core::cell::Cell;

pub(super) struct State {
    completed: Cell<bool>,
}

impl State {
    pub(super) const fn new() -> Self {
        Self { completed: Cell::new(false) }
    }

    pub(super) fn invalidate(&self) {
        self.completed.set(false);
    }

    #[inline]
    pub(super) fn submit(
        &self,
        wait_ready: impl FnOnce() -> Result<(), RequestError>,
        issue: impl FnOnce(),
        complete: impl FnOnce() -> Result<(), RequestError>,
    ) -> Result<(), RequestError> {
        // Non-NCQ completion clears CI after BSY/DRQ clear; the caller also
        // verifies latched TFES. Only this owner issues to this one slot.
        // Consume that proof before any callback: failures stay cold. Port
        // stop/reconfiguration also invalidates it before touching hardware.
        let warm = self.completed.replace(false);
        if !warm {
            wait_ready()?;
        }
        issue();
        let result = complete();
        self.completed.set(result.is_ok());
        #[cfg(not(test))]
        if result.is_ok() && option_env!("NANAMI_AHCI_SUBMIT_DIAG") == Some("1") {
            use core::sync::atomic::{AtomicUsize, Ordering};
            static COMMANDS: AtomicUsize = AtomicUsize::new(0);
            static COLD: AtomicUsize = AtomicUsize::new(0);
            let cold = COLD.fetch_add(usize::from(!warm), Ordering::Relaxed) + usize::from(!warm);
            let commands = COMMANDS.fetch_add(1, Ordering::Relaxed) + 1;
            if commands <= 8 || commands.is_power_of_two() {
                nanami_runtime::println!("[ahci.submit] commands={} cold={}", commands, cold);
            }
        }
        result
    }
}
