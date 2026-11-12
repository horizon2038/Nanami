//! Bound startup retries for an optional backend and repeated diagnostics.
use nanami_runtime::{RequestError, Word};

pub(super) fn connect(
    max_waits: usize,
    mut attempt: impl FnMut() -> Result<Word, RequestError>,
    mut report: impl FnMut(RequestError, usize),
    mut backoff: impl FnMut(),
) -> Result<Word, RequestError> {
    let mut previous = None;
    let mut consecutive = 0usize;
    let mut completed_waits = 0usize;
    loop {
        match attempt() {
            Ok(pid) => return Ok(pid),
            Err(error) => {
                consecutive = if previous == Some(error) {
                    consecutive.saturating_add(1)
                } else {
                    previous = Some(error);
                    1
                };
                // The first failure and every changed error are immediate.
                // Powers of two keep output logarithmic while waiting.
                if consecutive.is_power_of_two() {
                    report(error, consecutive);
                }
                // Count total waits independently of the error so changing
                // failures cannot extend the startup budget. This also bounds
                // retries when the timer is unavailable.
                if completed_waits >= max_waits {
                    return Err(error);
                }
                backoff();
                completed_waits += 1;
            }
        }
    }
}
