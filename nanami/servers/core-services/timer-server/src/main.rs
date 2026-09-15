#![no_std]
#![no_main]

use libnanami::ipc::{ServiceEvent, ServiceRequest};
use libnanami::{self, RequestError, Word};

#[path = "arch.rs"]
mod arch;

mod state;
mod timers;
use state::*;
use timers::*;

const SLOT_TIMER_RESOURCE: Word = 16;
const SLOT_NOTIFICATION: Word = 18;
const SLOT_INTERRUPT: Word = 19;
const SLOT_SERVICE_PORT: Word = 20;
const SLOT_CLIENT_NOTIFICATION_BASE: Word = 32;
const MAX_CLIENT_NOTIFICATIONS: usize = 128;
const MAX_PENDING_ASYNC_TIMERS: usize = 512;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[timer-server] panic\n");
    let _ = libnanami::request_exit();
    loop {}
}

fn nanami_main() -> libnanami::NanamiResult {
    if let Err(e) = libnanami::ipc::init_ipc_tls() {
        return Err(log_error("[timer-server] ipc tls init failed: ", e));
    }

    if let Err(e) = nanami_services::registry::register_timer_service() {
        return Err(log_error("[timer-server] service register failed: ", e));
    }
    libnanami::print!("[timer-server] service registered: timer-service\n");

    let prepared_timer = arch::prepare(SLOT_TIMER_RESOURCE)
        .map_err(|e| log_error("[timer-server] timer prepare failed: ", e))?;

    if let Err(e) =
        libnanami::request_irq(prepared_timer.irq_number, SLOT_NOTIFICATION, SLOT_INTERRUPT)
    {
        return Err(log_error("[timer-server] request timer irq failed: ", e));
    }

    let notif_desc = libnanami::ipc::process_slot_descriptor(SLOT_NOTIFICATION);
    let irq_desc = libnanami::ipc::process_slot_descriptor(SLOT_INTERRUPT);
    if let Err(e) = libnanami::ipc::bind_current_thread_notification(notif_desc) {
        return Err(log_error("[timer-server] bind notification failed: ", e));
    }

    libnanami::print!("[timer-server] ready lazy tick-hz=");
    libnanami::print!("{}", arch::TICK_HZ as usize);
    libnanami::print!("\n");

    let service_port = libnanami::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let mut state = TimerState::new();
    let mut pending_status = (libnanami::OS_RESPONSE_OK, 0, 0);
    let mut has_pending_reply = false;

    loop {
        let used_reply_receive = has_pending_reply;
        let event = if used_reply_receive {
            match libnanami::ipc::service_reply_receive_event(
                service_port,
                pending_status.0,
                pending_status.1,
                pending_status.2,
            ) {
                Ok(e) => e,
                Err(e) => {
                    log_request_error("[timer-server] reply_receive failed: ", e);
                    has_pending_reply = false;
                    continue;
                }
            }
        } else {
            match libnanami::ipc::service_receive_event(service_port) {
                Ok(e) => e,
                Err(e) => return Err(log_error("[timer-server] receive failed: ", e)),
            }
        };
        if used_reply_receive {
            has_pending_reply = false;
        }

        match event {
            ServiceEvent::Request(request) => {
                pending_status =
                    handle_request(request, &mut state, prepared_timer.resource, irq_desc);
                has_pending_reply = true;
            }
            ServiceEvent::Notification { .. } => {
                if let Err(e) = handle_notification(irq_desc, &mut state) {
                    return Err(log_error("[timer-server] irq ack failed: ", e));
                }
            }
            ServiceEvent::Fault {
                identifier, reason, ..
            } => {
                libnanami::print!("[timer-server] fault id=");
                libnanami::print!("{}", identifier);
                libnanami::print!(" reason=");
                libnanami::print!("{:#x}", reason);
                libnanami::print!("\n");
            }
        }
    }
}

fn handle_request(
    request: ServiceRequest,
    state: &mut TimerState,
    timer_resource: Word,
    irq_desc: Word,
) -> (Word, Word, Word) {
    match request.code {
        nanami_services::timer::TIMER_SERVICE_REQUEST_SLEEP_MILLISECONDS => {
            match schedule_timer(
                state,
                timer_resource,
                irq_desc,
                request.identifier,
                request.arg1,
                request.arg0 as u64,
                0,
            ) {
                Ok(()) => (libnanami::OS_RESPONSE_OK, request.arg0, 0),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_services::timer::TIMER_SERVICE_REQUEST_SLEEP_ASYNC_MILLISECONDS => {
            match schedule_timer(
                state,
                timer_resource,
                irq_desc,
                request.identifier,
                request.arg1,
                request.arg0 as u64,
                0,
            ) {
                Ok(()) => (libnanami::OS_RESPONSE_OK, request.arg0, 0),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_services::timer::TIMER_SERVICE_REQUEST_INTERVAL_MILLISECONDS => {
            match schedule_timer(
                state,
                timer_resource,
                irq_desc,
                request.identifier,
                request.arg1,
                request.arg0 as u64,
                request.arg0 as u64,
            ) {
                Ok(()) => (libnanami::OS_RESPONSE_OK, request.arg0, 0),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_services::timer::TIMER_SERVICE_REQUEST_MONOTONIC_TICKS => {
            match ensure_timer_started(state, timer_resource, irq_desc) {
                Ok(()) => (
                    libnanami::OS_RESPONSE_OK,
                    state.ticks as Word,
                    arch::TICK_HZ as Word,
                ),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_notification(irq_desc: Word, state: &mut TimerState) -> Result<(), RequestError> {
    state.ticks = state.ticks.saturating_add(1);
    arch::rearm()?;
    libnanami::ipc::interrupt_ack(irq_desc)?;
    fire_expired_async_timers(state);
    Ok(())
}

fn log_request_error(prefix: &str, err: RequestError) {
    libnanami::println!("{}{}", prefix, err);
}

fn map_request_error_to_status(err: RequestError) -> Word {
    match err {
        RequestError::InvalidArgument => libnanami::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => libnanami::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Transport => libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Protocol => libnanami::OS_RESPONSE_FATAL,
        RequestError::Status(status) => status,
    }
}

fn log_error(prefix: &str, err: RequestError) -> libnanami::NanamiError {
    log_request_error(prefix, err);
    err.into()
}

libnanami::nanami_entry!(nanami_main);
