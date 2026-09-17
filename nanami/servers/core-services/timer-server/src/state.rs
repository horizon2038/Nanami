use super::*;

#[derive(Clone, Copy)]
pub(super) struct ClientNotificationEntry {
    pub(super) used: bool,
    pub(super) pid: Word,
    pub(super) source_slot: Word,
    pub(super) descriptor: Word,
}

impl ClientNotificationEntry {
    pub(super) const EMPTY: Self = Self {
        used: false,
        pid: 0,
        source_slot: 0,
        descriptor: 0,
    };
}

#[derive(Clone, Copy)]
pub(super) struct PendingAsyncTimer {
    pub(super) target_tick: u64,
    pub(super) interval_ticks: u64,
    pub(super) notification_descriptor: Word,
    pub(super) alarm: bool,
}

impl PendingAsyncTimer {
    pub(super) const EMPTY: Self = Self {
        target_tick: 0,
        interval_ticks: 0,
        notification_descriptor: 0,
        alarm: false,
    };
}

pub(super) struct TimerState {
    pub(super) ticks: u64,
    pub(super) timer_started: bool,
    pub(super) schedule_count: usize,
    pub(super) fire_count: usize,
    pub(super) client_notifications: [ClientNotificationEntry; MAX_CLIENT_NOTIFICATIONS],
    pub(super) pending_timers: super::deadlines::Deadlines,
}

impl TimerState {
    pub(super) const fn new() -> Self {
        Self {
            ticks: 0,
            timer_started: false,
            schedule_count: 0,
            fire_count: 0,
            client_notifications: [ClientNotificationEntry::EMPTY; MAX_CLIENT_NOTIFICATIONS],
            pending_timers: super::deadlines::Deadlines::new(),
        }
    }
}
