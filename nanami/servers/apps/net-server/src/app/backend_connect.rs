//! Keep retry timing and every connection attempt; bound repeated diagnostics.
use nanami_runtime::{RequestError, Word};

pub(super) fn connect(
    mut attempt: impl FnMut() -> Result<Word, RequestError>,
    mut report: impl FnMut(RequestError, usize),
    mut backoff: impl FnMut(),
) -> Word {
    let mut previous = None;
    let mut consecutive = 0usize;
    loop {
        match attempt() {
            Ok(pid) => return pid,
            Err(error) => {
                consecutive = if previous == Some(error) {
                    consecutive.saturating_add(1)
                } else {
                    previous = Some(error);
                    1
                };
                // The first failure and every changed error are immediate.
                // Powers of two retain an unbounded liveness indication with
                // logarithmic output while an optional device stays absent.
                if consecutive.is_power_of_two() {
                    report(error, consecutive);
                }
                backoff();
            }
        }
    }
}
