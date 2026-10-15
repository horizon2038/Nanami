use nanami_ui::Key;
pub const CAPACITY: usize = 120;
#[derive(Clone, Copy, Default, Debug)]
pub struct Sample {
    pub at_ms: u64,
    pub memory: Option<(u64, u64)>,
    pub processes: Option<(u64, u64)>,
    pub cores: Option<(usize, u64)>,
}
impl Sample {
    pub fn used(self) -> Option<u64> {
        self.memory.map(|(total, free)| total.saturating_sub(free))
    }
    pub fn percentage(self) -> Option<u64> {
        self.memory.map(|(total, free)| {
            if total == 0 {
                return 0;
            }
            ((total.saturating_sub(free) as u128 * 100) / total as u128).min(100) as u64
        })
    }
}
pub struct History {
    samples: [Sample; CAPACITY],
    start: usize,
    pub len: usize,
}
impl History {
    pub fn new() -> Self {
        Self {
            samples: [Sample::default(); CAPACITY],
            start: 0,
            len: 0,
        }
    }
    pub fn push(&mut self, sample: Sample) {
        if self.len < CAPACITY {
            self.samples[(self.start + self.len) % CAPACITY] = sample;
            self.len += 1;
        } else {
            self.samples[self.start] = sample;
            self.start = (self.start + 1) % CAPACITY;
        }
    }
    pub fn get(&self, i: usize) -> Sample {
        self.samples[(self.start + i) % CAPACITY]
    }
    pub fn latest(&self) -> Option<Sample> {
        if self.len == 0 {
            None
        } else {
            Some(self.get(self.len - 1))
        }
    }
    pub fn previous(&self) -> Option<Sample> {
        if self.len < 2 {
            None
        } else {
            Some(self.get(self.len - 2))
        }
    }
    pub fn memory_stats(&self) -> Option<(u64, u64, u64)> {
        let mut min = u64::MAX;
        let mut max = 0;
        let mut sum = 0u128;
        let mut count = 0;
        for i in 0..self.len {
            if let Some(value) = self.get(i).used() {
                min = min.min(value);
                max = max.max(value);
                sum += value as u128;
                count += 1;
            }
        }
        if count == 0 {
            None
        } else {
            Some((min, max, (sum / count) as u64))
        }
    }
    pub fn process_scale(&self) -> u64 {
        let mut peak = 1u64;
        for i in 0..self.len {
            if let Some((a, b)) = self.get(i).processes {
                peak = peak.max(a).max(b);
            }
        }
        peak.saturating_add(9)
            .saturating_div(10)
            .saturating_mul(10)
            .max(10)
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Memory,
    Processes,
    System,
}
pub struct Monitor {
    pub history: History,
    pub paused: bool,
    pub interval: usize,
    pub tab: Tab,
    pub refresh: bool,
}
impl Monitor {
    pub fn new() -> Self {
        Self {
            history: History::new(),
            paused: false,
            interval: 1,
            tab: Tab::Memory,
            refresh: true,
        }
    }
    pub fn interval_ms(&self) -> usize {
        [500, 1000, 2000][self.interval]
    }
    pub fn key(&mut self, key: Key) -> bool {
        match key {
            Key::Character(' ' | 'p' | 'P') => {
                self.paused = !self.paused;
            }
            Key::Character('r' | 'R') | Key::F5 => {
                self.refresh = true;
            }
            Key::Character('c' | 'C') => {
                self.history = History::new();
                self.refresh = true;
            }
            Key::Character('1') => {
                self.tab = Tab::Memory;
            }
            Key::Character('2') => {
                self.tab = Tab::Processes;
            }
            Key::Character('3') => {
                self.tab = Tab::System;
            }
            Key::Character('+' | '=') => {
                self.interval = self.interval.saturating_sub(1);
            }
            Key::Character('-') => {
                self.interval = (self.interval + 1).min(2);
            }
            _ => return false,
        }
        true
    }
    pub fn click(&mut self, x: i32, y: i32) -> bool {
        if (0..34).contains(&y) {
            match x {
                8..=98 => {
                    self.paused = !self.paused;
                }
                106..=167 => {
                    self.interval = 0;
                }
                174..=227 => {
                    self.interval = 1;
                }
                234..=287 => {
                    self.interval = 2;
                }
                302..=390 => {
                    self.refresh = true;
                }
                398..=488 => {
                    self.history = History::new();
                    self.refresh = true;
                }
                _ => return false,
            }
            return true;
        }
        if (36..68).contains(&y) && x >= 8 && x < 400 {
            self.tab = match x {
                8..=131 => Tab::Memory,
                132..=275 => Tab::Processes,
                _ => Tab::System,
            };
            return true;
        }
        false
    }
}
