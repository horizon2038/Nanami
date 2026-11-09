#[path = "../../../nanami/servers/apps/net-server/src/app/backend_connect.rs"]
mod production;

use super::{RequestError, Word};

#[test]
fn absent_backend_preserves_every_attempt_backoff_and_late_success() {
    let attempts = core::cell::Cell::new(0usize);
    let sleeps = core::cell::Cell::new(0usize);
    let mut reports = Vec::new();
    let pid = production::connect(
        || {
            attempts.set(attempts.get() + 1);
            if attempts.get() <= 491 { Err(RequestError::Status(1)) } else { Ok(77) }
        },
        |error, count| reports.push((error, count, attempts.get(), sleeps.get())),
        || sleeps.set(sleeps.get() + 1),
    );
    assert_eq!(pid, 77);
    assert_eq!(attempts.get(), 492);
    assert_eq!(sleeps.get(), 491);
    assert_eq!(reports.len(), 9);
    for (index, (error, count, attempt, sleeps_before)) in reports.into_iter().enumerate() {
        assert_eq!(error, RequestError::Status(1));
        assert_eq!(count, 1usize << index);
        assert_eq!(attempt, count);
        assert_eq!(sleeps_before, count - 1);
    }
}

#[test]
fn changed_errors_are_reported_immediately_and_success_never_backs_off() {
    let errors = [RequestError::Status(1), RequestError::Status(1),
        RequestError::Status(1), RequestError::Transport, RequestError::Transport,
        RequestError::Protocol, RequestError::Status(1)];
    let mut attempts = 0;
    let mut sleeps = 0;
    let mut reports = Vec::new();
    assert_eq!(production::connect(
        || { let result = errors.get(attempts).copied().map_or(Ok(99 as Word), Err); attempts += 1; result },
        |error, count| reports.push((error, count)),
        || sleeps += 1,
    ), 99);
    assert_eq!(attempts, errors.len() + 1);
    assert_eq!(sleeps, errors.len());
    assert_eq!(reports, vec![(errors[0], 1), (errors[0], 2), (errors[3], 1),
        (errors[3], 2), (errors[5], 1), (errors[6], 1)]);
    let mut reports = 0;
    let mut sleeps = 0;
    assert_eq!(production::connect(|| Ok(12), |_, _| reports += 1, || sleeps += 1), 12);
    assert_eq!((reports, sleeps), (0, 0));
}
