use super::*;

pub(super) fn schedule_timer(
    state: &mut TimerState,
    timer: &mut arch::PreparedTimer,
    irq_desc: Word,
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
    refresh_clock(state, timer, irq_desc)?;
    let interval_ticks = if interval_ms == 0 {
        0
    } else {
        milliseconds_to_ticks(interval_ms).max(1)
    };
    let target_tick = state.ticks.saturating_add(wait_ticks);
    state.schedule_count = state.schedule_count.wrapping_add(1);

    if state.pending_timers.push(PendingAsyncTimer {
        target_tick,
        interval_ticks,
        notification_descriptor: descriptor,
        alarm: false,
    }) {
        Ok(())
    } else {
        Err(RequestError::Unsupported)
    }
}

pub(super) fn refresh_clock(
    state: &mut TimerState,
    timer: &mut arch::PreparedTimer,
    irq_desc: Word,
) -> Result<(), RequestError> {
    if !state.timer_started {
        timer.start()?;
        libnanami::ipc::interrupt_ack(irq_desc)?;
        state.timer_started = true;
    }
    state.ticks = timer.now();
    Ok(())
}

// One replaceable absolute alarm per client notification. Existing sleep and
// interval requests remain independent, including on the same notification.
pub(super) fn set_alarm(
    state: &mut TimerState,
    timer: &mut arch::PreparedTimer,
    irq_desc: Word,
    requester_pid: Word,
    source_slot: Word,
    deadline: Option<u64>,
) -> Result<(), RequestError> {
    if requester_pid == 0 {
        return Err(RequestError::InvalidArgument);
    }
    let source_slot = if source_slot == 0 {
        libnanami::PROCESS_SLOT_NOTIFICATION
    } else {
        source_slot
    };
    let descriptor = ensure_client_notification_descriptor(state, requester_pid, source_slot)?;
    if deadline.is_some() {
        refresh_clock(state, timer, irq_desc)?;
    }
    state.pending_timers.remove_alarm(descriptor);
    let Some(target_tick) = deadline else {
        return Ok(());
    };
    if target_tick <= state.ticks {
        return libnanami::ipc::notification_notify(descriptor);
    }
    if state.pending_timers.push(PendingAsyncTimer {
        target_tick,
        interval_ticks: 0,
        notification_descriptor: descriptor,
        alarm: true,
    }) {
        Ok(())
    } else {
        Err(RequestError::Unsupported)
    }
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

pub(super) fn fire_expired_async_timers(state: &mut TimerState) {
    while let Some(mut timer) = state.pending_timers.pop_due(state.ticks) {
        if timer.interval_ticks != 0 {
            // Preserve phase; coalesce missed periods into one notification.
            let delay =
                timer.interval_ticks - (state.ticks - timer.target_tick) % timer.interval_ticks;
            timer.target_tick = state.ticks.saturating_add(delay);
            if timer.target_tick > state.ticks {
                state.pending_timers.push(timer);
            }
        }
        state.fire_count = state.fire_count.wrapping_add(1);
        if let Err(e) = libnanami::ipc::notification_notify(timer.notification_descriptor) {
            log_request_error("[timer-server] async notify failed: ", e);
            retire_notification_descriptor(state, timer.notification_descriptor);
        }
    }
}

fn retire_notification_descriptor(state: &mut TimerState, descriptor: Word) {
    state.pending_timers.retire(descriptor);

    let mut j = 0usize;
    while j < MAX_CLIENT_NOTIFICATIONS {
        if state.client_notifications[j].descriptor == descriptor {
            state.client_notifications[j] = ClientNotificationEntry::EMPTY;
        }
        j += 1;
    }
}

fn milliseconds_to_ticks(wait_ms: u64) -> u64 {
    (u128::from(wait_ms) * u128::from(arch::TICK_HZ))
        .div_ceil(1000)
        .min(u64::MAX as u128) as u64
}
