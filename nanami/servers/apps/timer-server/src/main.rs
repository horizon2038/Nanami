#![no_std]
#![no_main]

use libnanami::ipc::{ServiceEvent, ServiceRequest};
use libnanami::{self, RequestError, Word};

const SLOT_IO_PIT: Word = 16;
const SLOT_NOTIFICATION: Word = 18;
const SLOT_INTERRUPT: Word = 19;
const SLOT_SERVICE_PORT: Word = 20;
const SLOT_CLIENT_NOTIFICATION_BASE: Word = 32;
const MAX_CLIENT_NOTIFICATIONS: usize = 128;
const MAX_PENDING_ASYNC_TIMERS: usize = 512;

const PIT_PORT_COUNTER0: Word = 0x40;
const PIT_PORT_COMMAND: Word = 0x43;
const PIT_COMMAND_RATE_GEN_LOHI: Word = 0x34;
const PIT_BASE_HZ: u64 = 1_193_182;
const PIT_TICK_HZ: u64 = 10;

#[derive(Clone, Copy)]
struct ClientNotificationEntry {
    used: bool,
    pid: Word,
    source_slot: Word,
    descriptor: Word,
}

impl ClientNotificationEntry {
    const EMPTY: Self = Self {
        used: false,
        pid: 0,
        source_slot: 0,
        descriptor: 0,
    };
}

#[derive(Clone, Copy)]
struct PendingAsyncTimer {
    used: bool,
    target_tick: u64,
    interval_ticks: u64,
    notification_descriptor: Word,
}

impl PendingAsyncTimer {
    const EMPTY: Self = Self {
        used: false,
        target_tick: 0,
        interval_ticks: 0,
        notification_descriptor: 0,
    };
}

struct TimerState {
    ticks: u64,
    schedule_count: usize,
    fire_count: usize,
    client_notifications: [ClientNotificationEntry; MAX_CLIENT_NOTIFICATIONS],
    pending_timers: [PendingAsyncTimer; MAX_PENDING_ASYNC_TIMERS],
}

impl TimerState {
    const fn new() -> Self {
        Self {
            ticks: 0,
            schedule_count: 0,
            fire_count: 0,
            client_notifications: [ClientNotificationEntry::EMPTY; MAX_CLIENT_NOTIFICATIONS],
            pending_timers: [PendingAsyncTimer::EMPTY; MAX_PENDING_ASYNC_TIMERS],
        }
    }
}

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

    if let Err(e) = libnanami::request_io_port(PIT_PORT_COUNTER0, PIT_PORT_COMMAND, SLOT_IO_PIT) {
        return Err(log_error("[timer-server] request pit io failed: ", e));
    }

    if let Err(e) = libnanami::request_irq(0, SLOT_NOTIFICATION, SLOT_INTERRUPT) {
        return Err(log_error("[timer-server] request irq0 failed: ", e));
    }

    let notif_desc = libnanami::ipc::process_slot_descriptor(SLOT_NOTIFICATION);
    let irq_desc = libnanami::ipc::process_slot_descriptor(SLOT_INTERRUPT);
    if let Err(e) = libnanami::ipc::bind_current_thread_notification(notif_desc) {
        return Err(log_error("[timer-server] bind notification failed: ", e));
    }

    let pit_desc = libnanami::ipc::process_slot_descriptor(SLOT_IO_PIT);
    if let Err(e) = pit_program_periodic(pit_desc, PIT_TICK_HZ) {
        return Err(log_error("[timer-server] pit init failed: ", e));
    }

    if let Err(e) = libnanami::ipc::interrupt_ack(irq_desc) {
        return Err(log_error("[timer-server] irq arm failed: ", e));
    }

    libnanami::print!("[timer-server] ready tick-hz=");
    libnanami::print!("{}", PIT_TICK_HZ as usize);
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
                pending_status = handle_request(request, &mut state);
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

fn handle_request(request: ServiceRequest, state: &mut TimerState) -> (Word, Word, Word) {
    match request.code {
        nanami_services::timer::TIMER_SERVICE_REQUEST_SLEEP_MILLISECONDS => {
            match schedule_timer(
                state,
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
                request.identifier,
                request.arg1,
                request.arg0 as u64,
                request.arg0 as u64,
            ) {
                Ok(()) => (libnanami::OS_RESPONSE_OK, request.arg0, 0),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_notification(irq_desc: Word, state: &mut TimerState) -> Result<(), RequestError> {
    state.ticks = state.ticks.saturating_add(1);
    if state.ticks <= 4 {
        libnanami::print!("[timer-server] tick=");
        libnanami::print!("{}", state.ticks as usize);
        libnanami::print!("\n");
    }
    libnanami::ipc::interrupt_ack(irq_desc)?;
    fire_expired_async_timers(state);
    Ok(())
}

fn schedule_timer(
    state: &mut TimerState,
    requester_pid: Word,
    source_notification_slot: Word,
    wait_ms: u64,
    interval_ms: u64,
) -> Result<(), RequestError> {
    if requester_pid == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let source_slot = if source_notification_slot == 0 {
        libnanami::PROCESS_SLOT_NOTIFICATION
    } else {
        source_notification_slot
    };
    let descriptor = ensure_client_notification_descriptor(state, requester_pid, source_slot)?;
    let wait_ticks = milliseconds_to_ticks(wait_ms);
    if wait_ticks == 0 {
        return libnanami::ipc::notification_notify(descriptor);
    }
    let interval_ticks = if interval_ms == 0 {
        0
    } else {
        milliseconds_to_ticks(interval_ms).max(1)
    };
    let target_tick = state.ticks.saturating_add(wait_ticks);
    state.schedule_count = state.schedule_count.wrapping_add(1);

    let mut i = 0usize;
    while i < MAX_PENDING_ASYNC_TIMERS {
        if !state.pending_timers[i].used {
            state.pending_timers[i] = PendingAsyncTimer {
                used: true,
                target_tick,
                interval_ticks,
                notification_descriptor: descriptor,
            };
            if state.schedule_count <= 8 {
                if interval_ticks == 0 {
                    libnanami::print!("[timer-server] schedule pid=");
                } else {
                    libnanami::print!("[timer-server] interval pid=");
                }
                libnanami::print!("{}", requester_pid as usize);
                libnanami::print!(" slot=");
                libnanami::print!("{}", source_slot as usize);
                libnanami::print!(" ms=");
                libnanami::print!("{}", wait_ms as usize);
                libnanami::print!(" now=");
                libnanami::print!("{}", state.ticks as usize);
                libnanami::print!(" target=");
                libnanami::print!("{}", target_tick as usize);
                libnanami::print!(" count=");
                libnanami::print!("{}", state.schedule_count);
                libnanami::print!("\n");
            }
            return Ok(());
        }
        i += 1;
    }

    Err(RequestError::Unsupported)
}

fn ensure_client_notification_descriptor(
    state: &mut TimerState,
    requester_pid: Word,
    source_notification_slot: Word,
) -> Result<Word, RequestError> {
    let mut i = 0usize;
    while i < MAX_CLIENT_NOTIFICATIONS {
        let entry = state.client_notifications[i];
        if entry.used && entry.pid == requester_pid && entry.source_slot == source_notification_slot
        {
            return Ok(entry.descriptor);
        }
        i += 1;
    }

    let mut free_index = None;
    let mut j = 0usize;
    while j < MAX_CLIENT_NOTIFICATIONS {
        if !state.client_notifications[j].used {
            free_index = Some(j);
            break;
        }
        j += 1;
    }
    let index = free_index.ok_or(RequestError::Unsupported)?;
    let destination_slot = SLOT_CLIENT_NOTIFICATION_BASE + index as Word;

    libnanami::request_notification_port_copy(
        requester_pid,
        source_notification_slot,
        destination_slot,
        nanami_services::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT,
    )?;

    let descriptor = libnanami::ipc::process_slot_descriptor(destination_slot);
    state.client_notifications[index] = ClientNotificationEntry {
        used: true,
        pid: requester_pid,
        source_slot: source_notification_slot,
        descriptor,
    };
    Ok(descriptor)
}

fn fire_expired_async_timers(state: &mut TimerState) {
    let mut expired = [0; MAX_PENDING_ASYNC_TIMERS];
    let mut expired_count = 0usize;

    let mut i = 0usize;
    while i < MAX_PENDING_ASYNC_TIMERS {
        let timer = state.pending_timers[i];
        if timer.used && state.ticks >= timer.target_tick {
            if timer.interval_ticks == 0 {
                state.pending_timers[i].used = false;
            } else {
                let mut next_tick = timer.target_tick;
                while next_tick <= state.ticks {
                    next_tick = next_tick.saturating_add(timer.interval_ticks);
                }
                state.pending_timers[i].target_tick = next_tick;
            }
            if expired_count < MAX_PENDING_ASYNC_TIMERS {
                expired[expired_count] = timer.notification_descriptor;
                expired_count += 1;
            }
            state.fire_count = state.fire_count.wrapping_add(1);
            if state.fire_count <= 8 {
                libnanami::print!("[timer-server] fire target=");
                libnanami::print!("{}", timer.target_tick as usize);
                libnanami::print!(" now=");
                libnanami::print!("{}", state.ticks as usize);
                libnanami::print!(" count=");
                libnanami::print!("{}", state.fire_count);
                libnanami::print!("\n");
            }
        }
        i += 1;
    }

    let mut j = 0usize;
    while j < expired_count {
        if let Err(e) = libnanami::ipc::notification_notify(expired[j]) {
            log_request_error("[timer-server] async notify failed: ", e);
        }
        j += 1;
    }
}

fn milliseconds_to_ticks(wait_ms: u64) -> u64 {
    wait_ms.saturating_mul(PIT_TICK_HZ).saturating_add(999) / 1000
}

fn pit_program_periodic(pit_desc: Word, tick_hz: u64) -> Result<(), RequestError> {
    if tick_hz == 0 || tick_hz > PIT_BASE_HZ {
        return Err(RequestError::InvalidArgument);
    }

    let mut divisor = PIT_BASE_HZ / tick_hz;
    if divisor == 0 {
        divisor = 1;
    }
    if divisor > u16::MAX as u64 {
        divisor = u16::MAX as u64;
    }

    let divisor_u16 = divisor as u16;
    libnanami::io::io_write(pit_desc, PIT_PORT_COMMAND, 1, PIT_COMMAND_RATE_GEN_LOHI)?;
    libnanami::io::io_write(
        pit_desc,
        PIT_PORT_COUNTER0,
        1,
        (divisor_u16 & 0x00ff) as Word,
    )?;
    libnanami::io::io_write(
        pit_desc,
        PIT_PORT_COUNTER0,
        1,
        ((divisor_u16 >> 8) & 0x00ff) as Word,
    )?;
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
