pub use nanami_uapi::timer::*;

use a9n_abi::CapabilityDescriptor;

use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

pub fn timer_service_sleep_milliseconds(
    timer_service_port: CapabilityDescriptor,
    milliseconds: Word,
) -> Result<(), RequestError> {
    if milliseconds == 0 {
        return Ok(());
    }
    let (start, hz) = timer_service_monotonic_ticks(timer_service_port)?;
    if hz == 0 {
        return Err(RequestError::Protocol);
    }
    let duration = (milliseconds as u128 * hz as u128)
        .div_ceil(1000)
        .min(Word::MAX as u128) as Word;
    let deadline = start.saturating_add(duration);
    timer_service_sleep_async_on_notification_milliseconds(
        timer_service_port,
        milliseconds,
        crate::PROCESS_SLOT_NOTIFICATION,
    )?;
    let notification = crate::ipc::process_slot_descriptor(crate::PROCESS_SLOT_NOTIFICATION);
    let mut deferred = 0;
    let result = (|| loop {
        let identifier = crate::ipc::notification_wait(notification)?;
        deferred |= identifier & !TIMER_NOTIFICATION_IDENTIFIER_BIT;
        // The badge is shared by intervals and independent alarms. It is a
        // wakeup hint, not proof that this particular sleep has expired.
        if identifier & TIMER_NOTIFICATION_IDENTIFIER_BIT != 0
            && timer_service_monotonic_ticks(timer_service_port)?.0 >= deadline
        {
            return Ok(());
        }
    })();
    // Include combined IRQ/timer badges, and restore on failure as well.
    crate::ipc::preserve_interrupted_notification(deferred);
    result
}

/// Sleep on a notification reserved exclusively for sequential delay requests.
/// Unlike the bound notification, this must not receive intervals or IRQs.
pub fn timer_service_sleep_on_notification_milliseconds(
    timer_service_port: CapabilityDescriptor,
    milliseconds: Word,
    notification_slot: Word,
) -> Result<(), RequestError> {
    if notification_slot == crate::PROCESS_SLOT_NOTIFICATION {
        return timer_service_sleep_milliseconds(timer_service_port, milliseconds);
    }
    timer_service_sleep_async_on_notification_milliseconds(
        timer_service_port,
        milliseconds,
        notification_slot,
    )?;
    let notification = crate::ipc::process_slot_descriptor(notification_slot);
    wait_timer_notification(notification)
}

fn wait_timer_notification(notification: CapabilityDescriptor) -> Result<(), RequestError> {
    loop {
        let identifier = crate::ipc::notification_wait(notification)?;
        if (identifier & TIMER_NOTIFICATION_IDENTIFIER_BIT) != 0 {
            return Ok(());
        }
    }
}

pub fn timer_service_sleep_blocking_server_milliseconds(
    timer_service_port: CapabilityDescriptor,
    milliseconds: Word,
) -> Result<(), RequestError> {
    // The server acknowledges SLEEP immediately and signals expiration later.
    timer_service_sleep_milliseconds(timer_service_port, milliseconds)
}

pub fn timer_service_sleep_seconds(
    timer_service_port: CapabilityDescriptor,
    seconds: Word,
) -> Result<(), RequestError> {
    timer_service_sleep_milliseconds(timer_service_port, seconds.saturating_mul(1000))
}

pub fn timer_service_sleep_async_milliseconds(
    timer_service_port: CapabilityDescriptor,
    milliseconds: Word,
) -> Result<(), RequestError> {
    timer_service_sleep_async_on_notification_milliseconds(
        timer_service_port,
        milliseconds,
        crate::PROCESS_SLOT_NOTIFICATION,
    )
}

pub fn timer_service_sleep_async_on_notification_milliseconds(
    timer_service_port: CapabilityDescriptor,
    milliseconds: Word,
    notification_slot: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(
        timer_service_port,
        TIMER_SERVICE_REQUEST_SLEEP_ASYNC_MILLISECONDS,
        milliseconds,
        notification_slot,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn timer_service_interval_on_notification_milliseconds(
    timer_service_port: CapabilityDescriptor,
    milliseconds: Word,
    notification_slot: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(
        timer_service_port,
        TIMER_SERVICE_REQUEST_INTERVAL_MILLISECONDS,
        milliseconds,
        notification_slot,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn timer_service_monotonic_ticks(
    timer_service_port: CapabilityDescriptor,
) -> Result<(Word, Word), RequestError> {
    let (status, ticks, tick_hz) = call_port(
        timer_service_port,
        TIMER_SERVICE_REQUEST_MONOTONIC_TICKS,
        0,
        0,
        0,
        0,
        1,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((ticks, tick_hz))
}

/// Replace this notification's absolute one-shot alarm, or cancel it with None.
/// Deadlines use the units returned by `timer_service_monotonic_ticks`.
/// Independent sleep/interval requests are not affected. A notification that
/// has already been delivered cannot be recalled; callers must check the clock.
pub fn timer_service_set_alarm_ticks(
    timer_service_port: CapabilityDescriptor,
    notification_slot: Word,
    deadline: Option<Word>,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(
        timer_service_port,
        TIMER_SERVICE_REQUEST_ALARM_TICKS,
        deadline.unwrap_or(0),
        notification_slot,
        deadline.is_some() as Word,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn timer_service_sleep_async_seconds(
    timer_service_port: CapabilityDescriptor,
    seconds: Word,
) -> Result<(), RequestError> {
    timer_service_sleep_async_milliseconds(timer_service_port, seconds.saturating_mul(1000))
}
