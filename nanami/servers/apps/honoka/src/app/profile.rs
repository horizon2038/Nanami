use nanami_runtime::Word;

/// Opt-in wall times, including preemption and timer IPC, not GPU/CPU timings.
#[derive(Default)]
pub struct Profile {
    port: Word,
    start: Option<u64>,
    renders: u64,
    rects: usize,
    compose: u64,
    present: u64,
    max_compose: u64,
    max_present: u64,
    #[cfg(feature = "honoka-frame-events")]
    events: FrameEvents,
    #[cfg(feature = "honoka-frame-events")]
    report_sequence: u64,
}

/// Counts between received RTC notifications, not a frame rate or wall time.
/// Diagnostic logging can perturb scheduling; no extra clock RPC is issued.
#[cfg(feature = "honoka-frame-events")]
#[derive(Default, Debug, PartialEq, Eq)]
struct FrameEvents {
    notifications: u64,
    presents: u64,
    timers: u64,
    scans: u64,
    damage_rects: u64,
    renders: u64,
    composed_rects: u64,
    overflowed: bool,
}

#[cfg(feature = "honoka-frame-events")]
impl FrameEvents {
    fn add(field: &mut u64, amount: u64, overflowed: &mut bool) {
        match field.checked_add(amount) {
            Some(value) => *field = value,
            None => {
                *field = u64::MAX;
                *overflowed = true;
            }
        }
    }
}

impl Profile {
    #[cfg(feature = "honoka-frame-events")]
    pub fn notification(&mut self, present: bool, timer: bool) {
        let e = &mut self.events;
        FrameEvents::add(&mut e.notifications, 1, &mut e.overflowed);
        FrameEvents::add(&mut e.presents, present as u64, &mut e.overflowed);
        FrameEvents::add(&mut e.timers, timer as u64, &mut e.overflowed);
    }

    #[cfg(feature = "honoka-frame-events")]
    pub fn fbdev_scan(&mut self) {
        FrameEvents::add(&mut self.events.scans, 1, &mut self.events.overflowed);
    }

    #[cfg(feature = "honoka-frame-events")]
    pub fn damage_rectangle(&mut self) {
        FrameEvents::add(&mut self.events.damage_rects, 1, &mut self.events.overflowed);
    }

    #[cfg(feature = "honoka-frame-events")]
    pub fn report_frame_events(&mut self) {
        // Diagnostic budget, not a time interval or an event-frequency cap.
        if self.report_sequence >= 64 {
            return;
        }
        let mut e = core::mem::take(&mut self.events);
        FrameEvents::add(&mut self.report_sequence, 1, &mut e.overflowed);
        nanami_runtime::println!(
            "[honoka.events] seq={} limit=64 notifications={} present-batches={} rtc-batches={} fbdev-scans={} damage-rects={} renders={} compose-rects={} overflow={}",
            self.report_sequence, e.notifications, e.presents, e.timers, e.scans,
            e.damage_rects, e.renders, e.composed_rects, e.overflowed,
        );
    }

    pub fn new(port: Word) -> Self {
        Self {
            port,
            ..Self::default()
        }
    }

    pub fn now(&self) -> Option<u64> {
        if cfg!(feature = "honoka-frame-events")
            || option_env!("NANAMI_FB_PROFILE") != Some("1") || self.port == 0
        {
            return None;
        }
        let (ticks, hz) = nanami_client::timer::timer_service_monotonic_ticks(self.port).ok()?;
        (hz != 0).then(|| (ticks as u128 * 1_000_000_000 / hz as u128) as u64)
    }

    pub fn record(&mut self, before: Option<u64>, composited: Option<u64>, rects: usize) {
        #[cfg(feature = "honoka-frame-events")]
        {
            FrameEvents::add(&mut self.events.renders, 1, &mut self.events.overflowed);
            FrameEvents::add(&mut self.events.composed_rects, rects as u64, &mut self.events.overflowed);
        }
        let Some((before, composited)) = before.zip(composited) else {
            return;
        };
        let Some(after) = self.now() else {
            return;
        };
        let compose = composited.saturating_sub(before);
        let present = after.saturating_sub(composited);
        let interval = after.saturating_sub(*self.start.get_or_insert(before));
        self.renders += 1;
        self.rects += rects;
        self.compose += compose;
        self.present += present;
        self.max_compose = self.max_compose.max(compose);
        self.max_present = self.max_present.max(present);
        if interval >= 2_000_000_000 {
            nanami_runtime::println!(
                "[honoka.perf] interval-ms={} renders={} rects={} compose-us={} present-us={} max-compose-us={} max-present-us={}",
                interval / 1_000_000, self.renders, self.rects,
                self.compose / 1000, self.present / 1000,
                self.max_compose / 1000, self.max_present / 1000
            );
            *self = Self {
                port: self.port,
                start: Some(after),
                ..Self::default()
            };
        }
    }
}

#[cfg(all(test, feature = "honoka-frame-events"))]
mod tests {
    use super::*;

    #[test]
    fn merged_notifications_and_real_render_records_are_counted_without_a_clock() {
        let mut p = Profile::new(1);
        assert_eq!(p.now(), None);
        p.notification(true, true);
        p.notification(true, false);
        p.fbdev_scan();
        p.damage_rectangle();
        p.damage_rectangle();
        p.record(None, None, 3);
        assert_eq!(p.events, FrameEvents { notifications: 2, presents: 2, timers: 1,
            scans: 1, damage_rects: 2, renders: 1, composed_rects: 3, overflowed: false });
        p.report_frame_events();
        assert_eq!(p.events, FrameEvents::default());
        assert_eq!(p.report_sequence, 1);
        p.notification(false, true);
        assert_eq!(p.events.notifications, 1);
        assert_eq!(p.events.presents, 0);
    }

    #[test]
    fn overflow_is_explicit_instead_of_silently_wrapping_event_evidence() {
        let mut p = Profile::new(0);
        p.events.scans = u64::MAX;
        p.fbdev_scan();
        assert_eq!(p.events.scans, u64::MAX);
        assert!(p.events.overflowed);
    }
}
