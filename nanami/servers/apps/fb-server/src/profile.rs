use libnanami::Word;

/// Opt-in service-wall-time measurements; no timer IPC in normal builds.
#[derive(Default)]
pub struct Profile {
    port: Option<Word>,
    start: u64,
    presents: u64,
    changed: u64,
    requested: u64,
    written: u64,
    elapsed: u64,
    longest: u64,
}

impl Profile {
    pub fn new() -> Self {
        let mut profile = Self::default();
        if option_env!("NANAMI_FB_PROFILE") == Some("1")
            && nanami_services::registry::connect_timer_service(24).is_ok()
        {
            profile.port = Some(libnanami::ipc::process_slot_descriptor(24));
            profile.start = profile.now().unwrap_or(0);
        }
        profile
    }

    pub fn now(&self) -> Option<u64> {
        if option_env!("NANAMI_FB_PROFILE") != Some("1") {
            return None;
        }
        let (ticks, hz) = nanami_services::timer::timer_service_monotonic_ticks(self.port?).ok()?;
        (hz != 0).then(|| (ticks as u128 * 1_000_000_000 / hz as u128) as u64)
    }

    pub fn record(&mut self, before: Option<u64>, pixels: usize, written: usize) {
        let Some((before, after)) = before.zip(self.now()) else {
            return;
        };
        let elapsed = after.saturating_sub(before);
        self.presents += 1;
        self.changed += u64::from(written != 0);
        self.requested += pixels as u64 * 4;
        self.written += written as u64;
        self.elapsed += elapsed;
        self.longest = self.longest.max(elapsed);
        let interval = after.saturating_sub(self.start);
        if interval >= 2_000_000_000 {
            libnanami::println!(
                "[fb.perf] interval-ms={} presents={} changed={} requested-kib={} written-kib={} service-us={} max-us={}",
                interval / 1_000_000, self.presents, self.changed, self.requested / 1024,
                self.written / 1024, self.elapsed / 1000, self.longest / 1000
            );
            *self = Self {
                port: self.port,
                start: after,
                ..Self::default()
            };
        }
    }
}
