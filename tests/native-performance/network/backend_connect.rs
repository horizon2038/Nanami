#[path = "../../../nanami/servers/apps/net-server/src/app/backend_connect.rs"]
mod production;

use super::{RequestError, Word};

#[test]
fn absent_backend_preserves_every_attempt_backoff_and_late_success() {
    let attempts = core::cell::Cell::new(0usize);
    let sleeps = core::cell::Cell::new(0usize);
    let mut reports = Vec::new();
    let pid = production::connect(
        500,
        || {
            attempts.set(attempts.get() + 1);
            if attempts.get() <= 491 {
                Err(RequestError::Status(1))
            } else {
                Ok(77)
            }
        },
        |error, count| reports.push((error, count, attempts.get(), sleeps.get())),
        || sleeps.set(sleeps.get() + 1),
    );
    assert_eq!(pid, Ok(77));
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
    let errors = [
        RequestError::Status(1),
        RequestError::Status(1),
        RequestError::Status(1),
        RequestError::Transport,
        RequestError::Transport,
        RequestError::Protocol,
        RequestError::Status(1),
    ];
    let mut attempts = 0;
    let mut sleeps = 0;
    let mut reports = Vec::new();
    assert_eq!(
        production::connect(
            errors.len(),
            || {
                let result = errors.get(attempts).copied().map_or(Ok(99 as Word), Err);
                attempts += 1;
                result
            },
            |error, count| reports.push((error, count)),
            || sleeps += 1,
        ),
        Ok(99)
    );
    assert_eq!(attempts, errors.len() + 1);
    assert_eq!(sleeps, errors.len());
    assert_eq!(
        reports,
        vec![
            (errors[0], 1),
            (errors[0], 2),
            (errors[3], 1),
            (errors[3], 2),
            (errors[5], 1),
            (errors[6], 1)
        ]
    );
    let mut reports = 0;
    let mut sleeps = 0;
    assert_eq!(
        production::connect(0, || Ok(12), |_, _| reports += 1, || sleeps += 1),
        Ok(12)
    );
    assert_eq!((reports, sleeps), (0, 0));
}

#[test]
fn absent_backend_times_out_after_fifty_waits_without_an_extra_backoff() {
    let mut attempts = 0;
    let mut sleeps = 0;
    let mut reports = Vec::new();
    let result = production::connect(
        50,
        || {
            attempts += 1;
            Err(RequestError::Status(1))
        },
        |error, count| reports.push((error, count)),
        || sleeps += 1,
    );
    assert_eq!(result, Err(RequestError::Status(1)));
    assert_eq!((attempts, sleeps), (51, 50));
    assert_eq!(
        reports,
        [1, 2, 4, 8, 16, 32].map(|count| (RequestError::Status(1), count)),
    );
}

#[test]
fn changing_errors_do_not_reset_the_timeout_budget() {
    let errors = [
        RequestError::Status(1),
        RequestError::Transport,
        RequestError::Protocol,
    ];
    let mut attempts = 0;
    let mut sleeps = 0;
    let result = production::connect(
        errors.len() - 1,
        || {
            let error = errors[attempts];
            attempts += 1;
            Err(error)
        },
        |_, _| {},
        || sleeps += 1,
    );
    assert_eq!(result, Err(RequestError::Protocol));
    assert_eq!((attempts, sleeps), (3, 2));
}

#[test]
fn zero_wait_budget_still_attempts_once_and_never_backs_off() {
    let mut attempts = 0;
    let mut sleeps = 0;
    let result = production::connect(
        0,
        || {
            attempts += 1;
            Err(RequestError::Status(1))
        },
        |_, _| {},
        || sleeps += 1,
    );
    assert_eq!(result, Err(RequestError::Status(1)));
    assert_eq!((attempts, sleeps), (1, 0));
}
