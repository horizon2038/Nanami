//! Bounded structural wait evidence, without clock queries or elapsed estimates.
use alloc::vec::Vec;

pub(crate) const LIMIT: usize = 64;
pub(crate) const OWNER_LIMIT: usize = 4;
pub(crate) const GAP_START: usize = 14;
pub(crate) const SYSCALL_BUCKETS: usize = 513;
pub(crate) const PATH_LIMIT: usize = 128;
pub(crate) const PATH_BYTES: usize = 192;

pub(crate) struct PathRow {
    pub bytes: [u8; PATH_BYTES],
    pub len: usize,
    pub truncated: bool,
    pub first_call: usize,
    pub hits: usize,
    pub ok: usize,
    pub errors: usize,
    pub last: Option<isize>,
}

#[derive(Clone, Copy)]
pub(crate) struct Owner {
    pub pid: usize,
    pub fd: usize,
    pub image: [u8; 32],
    pub image_len: usize,
    pub truncated: bool,
}
impl Owner {
    const EMPTY: Self = Self { pid: 0, fd: 0, image: [0; 32], image_len: 0, truncated: false };
}

pub(crate) struct Identity {
    pub pid: usize,
    pub socket: usize,
    pub peer: usize,
    pub sample_row: usize,
    pub address: [u8; 111],
    pub address_len: usize,
    pub address_truncated: bool,
    pub descriptors: usize,
    pub owners: [Owner; OWNER_LIMIT],
}
impl Identity {
    pub(crate) fn new(pid: usize, socket: usize, peer: usize, sample_row: usize, address: &[u8]) -> Self {
        let mut result = Self { pid, socket, peer, sample_row, address: [0; 111],
            address_len: address.len().min(111), address_truncated: address.len() > 111,
            descriptors: 0, owners: [Owner::EMPTY; OWNER_LIMIT] };
        result.address[..result.address_len].copy_from_slice(&address[..result.address_len]);
        result
    }

    pub(crate) fn add_owner(&mut self, pid: usize, fd: usize, image: &[u8]) {
        if self.descriptors < OWNER_LIMIT {
            let image = image.rsplit(|byte| *byte == b'/').next().unwrap_or(&[]);
            let owner = &mut self.owners[self.descriptors];
            owner.pid = pid;
            owner.fd = fd;
            owner.image_len = image.len().min(owner.image.len());
            owner.truncated = image.len() > owner.image.len();
            owner.image[..owner.image_len].copy_from_slice(&image[..owner.image_len]);
        }
        self.descriptors = self.descriptors.saturating_add(1);
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Row {
    pub pid: usize,
    pub syscall: usize,
    pub phase: u8,
    pub cached_ticks: usize,
    pub hz: usize,
    pub deadline: Option<usize>,
    pub timeout_ns: Option<u128>,
    pub ready: Option<usize>,
    pub interests: usize,
    pub trigger: usize,
    pub expired: bool,
    pub result: isize,
    pub count: usize,
    pub first_fd: i32,
    pub socket: usize,
    pub peer: usize,
    pub lower: Option<usize>,
    pub upper: Option<usize>,
}

pub(crate) struct Trace {
    pub rows: Vec<Row>,
    pub pid: Option<usize>,
    pub dropped: usize,
    pub allocation_failed: bool,
    pub identity: Option<Identity>,
    pub clock_consistent: bool,
    pub gap_start: Option<usize>,
    pub gap_end: Option<usize>,
    pub gap_calls: [usize; SYSCALL_BUCKETS],
    pub gap_overflow: bool,
    pub paths: Vec<PathRow>,
    pub path_total: usize,
    pub path_dropped: usize,
    pub path_allocation_failed: bool,
    pub path_overflow: bool,
    pub path_steps: [usize; 3],
    pub path_step_overflow: bool,
    current_call: Option<usize>,
    current_path: Option<usize>,
    last_clock: Option<(usize, usize)>,
    closed: usize,
    report_at: Option<usize>,
    finished: bool,
}

impl Trace {
    pub(crate) const fn new() -> Self {
        Self { rows: Vec::new(), pid: None, dropped: 0, allocation_failed: false,
               identity: None, clock_consistent: true, last_clock: None, closed: 0,
               gap_start: None, gap_end: None, gap_calls: [0; SYSCALL_BUCKETS], gap_overflow: false,
               paths: Vec::new(), path_total: 0, path_dropped: 0,
               path_allocation_failed: false, path_overflow: false, current_call: None, current_path: None,
               path_steps: [0; 3], path_step_overflow: false,
               report_at: None, finished: false }
    }

    pub(crate) fn syscall(&mut self, pid: usize, number: usize) {
        if !self.in_gap(pid) {
            return;
        }
        self.current_call = Some(number);
        self.current_path = None;
        let count = &mut self.gap_calls[number.min(SYSCALL_BUCKETS - 1)];
        match count.checked_add(1) {
            Some(next) => *count = next,
            None => self.gap_overflow = true,
        }
    }

    pub(crate) fn in_gap(&self, pid: usize) -> bool {
        self.pid == Some(pid) && self.active_for(pid) && self.gap_start.is_some() && self.gap_end.is_none()
    }

    pub(crate) fn path_step(&mut self, pid: usize, step: usize) {
        if !self.in_gap(pid) || step >= self.path_steps.len() { return; }
        match self.path_steps[step].checked_add(1) {
            Some(next) => self.path_steps[step] = next,
            None => self.path_step_overflow = true,
        }
    }

    pub(crate) fn path(&mut self, pid: usize, bytes: &[u8]) {
        if !self.in_gap(pid) { return; }
        self.current_path = None;
        let Some(number) = self.current_call else { return; };
        match self.path_total.checked_add(1) {
            Some(next) => self.path_total = next,
            None => self.path_overflow = true,
        }
        // Never merge truncated prefixes: distinct long names can share one.
        if bytes.len() <= PATH_BYTES {
            if let Some(index) = self.paths.iter().position(|row| !row.truncated && &row.bytes[..row.len] == bytes) {
                let row = &mut self.paths[index];
                match row.hits.checked_add(1) {
                    Some(next) => row.hits = next,
                    None => self.path_overflow = true,
                }
                self.current_path = Some(index);
                return;
            }
        }
        if self.paths.len() == PATH_LIMIT || self.path_allocation_failed {
            self.path_dropped = self.path_dropped.saturating_add(1);
            return;
        }
        if self.paths.capacity() == 0 && self.paths.try_reserve_exact(PATH_LIMIT).is_err() {
            self.path_allocation_failed = true;
            self.path_dropped = self.path_dropped.saturating_add(1);
            return;
        }
        let mut row = PathRow { bytes: [0; PATH_BYTES], len: bytes.len().min(PATH_BYTES),
            truncated: bytes.len() > PATH_BYTES, first_call: number, hits: 1, ok: 0, errors: 0, last: None };
        row.bytes[..row.len].copy_from_slice(&bytes[..row.len]);
        self.current_path = Some(self.paths.len());
        self.paths.push(row);
    }

    pub(crate) fn path_result(&mut self, pid: usize, number: usize, result: Option<isize>) {
        if !self.in_gap(pid) || self.current_call != Some(number) { return; }
        let Some(index) = self.current_path.take() else { return; };
        let Some(value) = result else { return; };
        let row = &mut self.paths[index];
        let count = if value < 0 { &mut row.errors } else { &mut row.ok };
        match count.checked_add(1) {
            Some(next) => *count = next,
            None => self.path_overflow = true,
        }
        row.last = Some(value);
    }

    // Called only after an already-required successful clock refresh. Each
    // buffered event is closed exactly once by its first later sample.
    pub(crate) fn sample(&mut self, ticks: usize, hz: usize) {
        if self.finished { return; }
        if hz == 0 || self.last_clock.is_some_and(|(previous, rate)| ticks < previous || hz != rate) {
            self.clock_consistent = false;
        }
        self.last_clock = Some((ticks, hz));
        for row in &mut self.rows[self.closed..] {
            if row.lower.is_some_and(|lower| lower <= ticks) && row.hz == hz {
                row.upper = Some(ticks);
            } else {
                self.clock_consistent = false;
            }
        }
        self.closed = self.rows.len();
    }

    pub(crate) fn active_for(&self, pid: usize) -> bool {
        !self.finished && self.pid.is_none_or(|selected| selected == pid)
    }

    pub(crate) fn needs_identity(&self, pid: usize, socket: usize, peer: usize) -> bool {
        self.active_for(pid) && self.identity.is_none() && socket != 0 && peer != 0
            && self.rows.first().is_some_and(|row| row.pid == pid && row.socket == socket && row.peer == peer)
    }

    pub(crate) fn capture_identity(&mut self, identity: Identity) {
        if identity.descriptors != 0 && identity.sample_row < self.rows.len()
            && self.needs_identity(identity.pid, identity.socket, identity.peer) {
            self.identity = Some(identity);
        }
    }

    pub(crate) fn record(&mut self, mut row: Row) {
        if !self.active_for(row.pid) {
            return;
        }
        self.pid = Some(row.pid);
        if self.rows.len() == LIMIT || self.allocation_failed {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        if self.rows.capacity() == 0 && self.rows.try_reserve_exact(LIMIT).is_err() {
            self.allocation_failed = true;
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        row.lower = self.last_clock.and_then(|(ticks, hz)| {
            if ticks == row.cached_ticks && hz == row.hz && hz != 0 {
                Some(ticks)
            } else {
                self.clock_consistent = false;
                None
            }
        });
        row.upper = None;
        // Prospectively chosen from the repeated prior captured gap. Do not
        // move the window after seeing which calls or durations it contains.
        if self.rows.len() == GAP_START && row.phase == 3 {
            self.gap_start = Some(GAP_START);
        } else if self.gap_start.is_some() && self.gap_end.is_none() && matches!(row.phase, 0 | 1) {
            self.gap_end = Some(self.rows.len());
        }
        self.rows.push(row);
    }

    // Start the reporting delay at an existing fresh timer notification, not
    // at the potentially stale cached tick attached to a wait event.
    pub(crate) fn due(&mut self, now: usize, hz: usize) -> bool {
        if self.finished || self.pid.is_none() || hz == 0 {
            return false;
        }
        let at = *self.report_at.get_or_insert_with(|| now.saturating_add(hz.saturating_mul(2)));
        if now < at {
            return false;
        }
        self.finished = true;
        true
    }
}
