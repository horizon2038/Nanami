use super::*;

pub(super) fn schedule_timer(
    state: &mut TimerState,
    timer_resource: Word,
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
    ensure_timer_started(state, timer_resource, irq_desc)?;
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
            state.next_deadline = Some(
                state
                    .next_deadline
                    .map_or(target_tick, |next| next.min(target_tick)),
            );
            return Ok(());
        }
        i += 1;
    }

    Err(RequestError::Unsupported)
}

pub(super) fn ensure_timer_started(
    state: &mut TimerState,
    timer_resource: Word,
    irq_desc: Word,
) -> Result<(), RequestError> {
    if state.timer_started {
        return Ok(());
    }
    arch::start(timer_resource)?;
    libnanami::ipc::interrupt_ack(irq_desc)?;
    state.timer_started = true;
    Ok(())
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
    if state
        .next_deadline
        .is_none_or(|deadline| state.ticks < deadline)
    {
        return;
    }
    state.next_deadline = None;
    let mut expired = [0; MAX_PENDING_ASYNC_TIMERS];
    let mut expired_count = 0usize;

    let mut i = 0usize;
    while i < MAX_PENDING_ASYNC_TIMERS {
        let timer = state.pending_timers[i];
        if timer.used && state.ticks >= timer.target_tick {
            if timer.interval_ticks == 0 {
                state.pending_timers[i].used = false;
            } else {
                // Skip missed periods without a loop, preserving the original phase.
                // At u64::MAX no future tick is representable; fire once and retire.
                let delay =
                    timer.interval_ticks - (state.ticks - timer.target_tick) % timer.interval_ticks;
                let next_tick = state.ticks.saturating_add(delay);
                if next_tick > state.ticks {
                    state.pending_timers[i].target_tick = next_tick;
                } else {
                    state.pending_timers[i].used = false;
                }
            }
            if expired_count < MAX_PENDING_ASYNC_TIMERS {
                expired[expired_count] = timer.notification_descriptor;
                expired_count += 1;
            }
            state.fire_count = state.fire_count.wrapping_add(1);
        }
        if state.pending_timers[i].used {
            let deadline = state.pending_timers[i].target_tick;
            state.next_deadline = Some(
                state
                    .next_deadline
                    .map_or(deadline, |next| next.min(deadline)),
            );
        }
        i += 1;
    }

    let mut j = 0usize;
    while j < expired_count {
        if let Err(e) = libnanami::ipc::notification_notify(expired[j]) {
            log_request_error("[timer-server] async notify failed: ", e);
            retire_notification_descriptor(state, expired[j]);
        }
        j += 1;
    }
}

fn retire_notification_descriptor(state: &mut TimerState, descriptor: Word) {
    state.next_deadline = None;
    let mut i = 0usize;
    while i < MAX_PENDING_ASYNC_TIMERS {
        if state.pending_timers[i].notification_descriptor == descriptor {
            state.pending_timers[i] = PendingAsyncTimer::EMPTY;
        }
        if state.pending_timers[i].used {
            let deadline = state.pending_timers[i].target_tick;
            state.next_deadline = Some(
                state
                    .next_deadline
                    .map_or(deadline, |next| next.min(deadline)),
            );
        }
        i += 1;
    }

    let mut j = 0usize;
    while j < MAX_CLIENT_NOTIFICATIONS {
        if state.client_notifications[j].descriptor == descriptor {
            state.client_notifications[j] = ClientNotificationEntry::EMPTY;
        }
        j += 1;
    }
}

fn milliseconds_to_ticks(wait_ms: u64) -> u64 {
    wait_ms.saturating_mul(arch::TICK_HZ).saturating_add(999) / 1000
}
