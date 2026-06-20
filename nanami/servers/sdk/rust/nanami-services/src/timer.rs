use a9n_abi::CapabilityDescriptor;

use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

pub const TIMER_SERVICE_REQUEST_SLEEP_MILLISECONDS: Word = 0x4001;
pub const TIMER_SERVICE_REQUEST_SLEEP_ASYNC_MILLISECONDS: Word = 0x4002;
pub const TIMER_SERVICE_REQUEST_INTERVAL_MILLISECONDS: Word = 0x4003;
pub const TIMER_NOTIFICATION_IDENTIFIER_BIT: Word =
    1usize << (core::mem::size_of::<Word>() * 8 - 1);

pub fn timer_service_sleep_milliseconds(
    timer_service_port: CapabilityDescriptor,
    milliseconds: Word,
) -> Result<(), RequestError> {
    timer_service_sleep_async_on_notification_milliseconds(
        timer_service_port,
        milliseconds,
        crate::PROCESS_SLOT_NOTIFICATION,
    )?;
    let notification = crate::ipc::process_slot_descriptor(crate::PROCESS_SLOT_NOTIFICATION);
    wait_timer_notification(notification)
}

pub fn timer_service_sleep_on_notification_milliseconds(
    timer_service_port: CapabilityDescriptor,
    milliseconds: Word,
    notification_slot: Word,
) -> Result<(), RequestError> {
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
    let (status, _, _) = call_port(
        timer_service_port,
        TIMER_SERVICE_REQUEST_SLEEP_MILLISECONDS,
        milliseconds,
        0,
        0,
        0,
        2,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
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

pub fn timer_service_sleep_async_seconds(
    timer_service_port: CapabilityDescriptor,
    seconds: Word,
) -> Result<(), RequestError> {
    timer_service_sleep_async_milliseconds(timer_service_port, seconds.saturating_mul(1000))
}
