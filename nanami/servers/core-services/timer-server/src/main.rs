#![no_std]
#![no_main]

use nanami_runtime::ipc::{ServiceEvent, ServiceRequest};
use nanami_runtime::{self, RequestError, Word};

#[path = "arch.rs"]
mod arch;

mod deadlines;
mod init_trace;
mod reply;
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
    nanami_runtime::print!("[timer-server] panic\n");
    let _ = nanami_runtime::request_exit();
    loop {}
}

fn nanami_main() -> nanami_runtime::NanamiResult {
    if let Err(e) = nanami_runtime::ipc::init_ipc_tls() {
        return Err(log_error("[timer-server] ipc tls init failed: ", e));
    }

    let mut prepared_timer = arch::prepare(SLOT_TIMER_RESOURCE)
        .map_err(|e| log_error("[timer-server] timer prepare failed: ", e))?;

    if let Err(e) =
        nanami_runtime::request_irq(prepared_timer.irq_number, SLOT_NOTIFICATION, SLOT_INTERRUPT)
    {
        return Err(log_error("[timer-server] request timer irq failed: ", e));
    }

    let notif_desc = nanami_runtime::ipc::process_slot_descriptor(SLOT_NOTIFICATION);
    let irq_desc = nanami_runtime::ipc::process_slot_descriptor(SLOT_INTERRUPT);
    if let Err(e) = nanami_runtime::ipc::bind_current_thread_notification(notif_desc) {
        return Err(log_error("[timer-server] bind notification failed: ", e));
    }

    nanami_client::registry::register_timer_service()
        .map_err(|e| log_error("[timer-server] service register failed: ", e))?;
    nanami_runtime::println!(
        "[timer-server] ready mode={} tick-hz={}",
        arch::MODE,
        arch::TICK_HZ
    );

    let service_port = nanami_runtime::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let mut state = TimerState::new();
    let mut pending_status = (nanami_runtime::OS_RESPONSE_OK, 0, 0);
    let mut has_pending_reply = false;

    loop {
        let used_reply_receive = has_pending_reply;
        let event = if used_reply_receive {
            match reply::receive_after_reply(
                &mut state.init_trace,
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
            match nanami_runtime::ipc::service_receive_event(service_port) {
                Ok(e) => e,
                Err(e) => return Err(log_error("[timer-server] receive failed: ", e)),
            }
        };
        if used_reply_receive {
            has_pending_reply = false;
        }

        match event {
            ServiceEvent::Request(request) => {
                pending_status = handle_request(request, &mut state, &mut prepared_timer, irq_desc);
                has_pending_reply = true;
            }
            ServiceEvent::Notification { .. } => {
                if let Err(e) = handle_notification(irq_desc, &mut prepared_timer) {
                    return Err(log_error("[timer-server] irq ack failed: ", e));
                }
            }
            ServiceEvent::Fault {
                identifier, reason, ..
            } => {
                nanami_runtime::print!("[timer-server] fault id=");
                nanami_runtime::print!("{}", identifier);
                nanami_runtime::print!(" reason=");
                nanami_runtime::print!("{:#x}", reason);
                nanami_runtime::print!("\n");
            }
        }
        complete_event(&mut state, &mut prepared_timer)
            .map_err(|e| log_error("[timer-server] arm failed: ", e))?;
    }
}

fn handle_request(
    request: ServiceRequest,
    state: &mut TimerState,
    timer: &mut arch::PreparedTimer,
    irq_desc: Word,
) -> (Word, Word, Word) {
    match request.code {
        nanami_client::timer::TIMER_SERVICE_REQUEST_SLEEP_MILLISECONDS
        | nanami_client::timer::TIMER_SERVICE_REQUEST_SLEEP_ASYNC_MILLISECONDS => {
            match schedule_timer(
                state,
                timer,
                irq_desc,
                request.identifier,
                request.arg1,
                request.arg0 as u64,
                0,
            ) {
                Ok(()) => (nanami_runtime::OS_RESPONSE_OK, request.arg0, 0),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_client::timer::TIMER_SERVICE_REQUEST_INTERVAL_MILLISECONDS => {
            match schedule_timer(
                state,
                timer,
                irq_desc,
                request.identifier,
                request.arg1,
                request.arg0 as u64,
                request.arg0 as u64,
            ) {
                Ok(()) => (nanami_runtime::OS_RESPONSE_OK, request.arg0, 0),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_client::timer::TIMER_SERVICE_REQUEST_MONOTONIC_TICKS => {
            match refresh_clock(state, timer, irq_desc) {
                Ok(()) => (
                    nanami_runtime::OS_RESPONSE_OK,
                    state.ticks as Word,
                    arch::TICK_HZ as Word,
                ),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_client::timer::TIMER_SERVICE_REQUEST_ALARM_TICKS => {
            match set_alarm(
                state,
                timer,
                irq_desc,
                request.identifier,
                request.arg1,
                (request.arg2 != 0).then_some(request.arg0 as u64),
            ) {
                Ok(()) => (nanami_runtime::OS_RESPONSE_OK, 0, 0),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        _ => (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_notification(
    irq_desc: Word,
    timer: &mut arch::PreparedTimer,
) -> Result<(), RequestError> {
    timer.on_interrupt();
    nanami_runtime::ipc::interrupt_ack(irq_desc)?;
    Ok(())
}

fn log_request_error(prefix: &str, err: RequestError) {
    nanami_runtime::println!("{}{}", prefix, err);
}

fn map_request_error_to_status(err: RequestError) -> Word {
    match err {
        RequestError::InvalidArgument => nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Transport => nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Protocol => nanami_runtime::OS_RESPONSE_FATAL,
        RequestError::Status(status) => status,
    }
}

fn log_error(prefix: &str, err: RequestError) -> nanami_runtime::NanamiError {
    log_request_error(prefix, err);
    err.into()
}

nanami_runtime::nanami_entry!(nanami_main);
