use libnanami::Word;

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
}

impl Profile {
    pub fn new(port: Word) -> Self {
        Self {
            port,
            ..Self::default()
        }
    }

    pub fn now(&self) -> Option<u64> {
        if option_env!("NANAMI_FB_PROFILE") != Some("1") || self.port == 0 {
            return None;
        }
        let (ticks, hz) = nanami_services::timer::timer_service_monotonic_ticks(self.port).ok()?;
        (hz != 0).then(|| (ticks as u128 * 1_000_000_000 / hz as u128) as u64)
    }

    pub fn record(&mut self, before: Option<u64>, composited: Option<u64>, rects: usize) {
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
            libnanami::println!(
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
