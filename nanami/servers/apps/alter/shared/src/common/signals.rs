//! Linux signal state. Runtime owns the lazy allocation; PCB metadata copies do not.
use alloc::vec::Vec;

pub const NSIG: usize = 64;
pub const SIGKILL: usize = 9;
pub const SIGCHLD: usize = 17;
pub const SIGCONT: usize = 18;
pub const SIGSTOP: usize = 19;
pub const SIG_DFL: usize = 0;
pub const SIG_IGN: usize = 1;
pub const SA_NOCLDSTOP: usize = 1;
pub const SA_NOCLDWAIT: usize = 2;
pub const SA_SIGINFO: usize = 4;
pub const SA_RESTORER: usize = 0x0400_0000;
pub const SA_ONSTACK: usize = 0x0800_0000;
pub const SA_RESTART: usize = 0x1000_0000;
pub const SA_NODEFER: usize = 0x4000_0000;
pub const SA_RESETHAND: usize = 0x8000_0000;
pub const SS_ONSTACK: usize = 1;
pub const SS_DISABLE: usize = 2;
pub const SS_AUTODISARM: usize = 0x8000_0000;
const PENDING_LIMIT: usize = 128;

pub const fn bit(signal: usize) -> u64 {
    1u64 << (signal - 1)
}

pub const fn blockable(mask: u64) -> u64 {
    mask & !(bit(SIGKILL) | bit(SIGSTOP))
}

pub fn default_ignored(signal: usize) -> bool {
    matches!(signal, SIGCHLD | SIGCONT | 23 | 28)
}

pub fn stop_signal(signal: usize) -> bool {
    matches!(signal, SIGSTOP | 20 | 21 | 22)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Action {
    pub handler: usize,
    pub flags: usize,
    pub restorer: usize,
    pub mask: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AltStack {
    pub sp: usize,
    pub flags: usize,
    pub size: usize,
}

impl AltStack {
    pub const DISABLED: Self = Self {
        sp: 0,
        flags: SS_DISABLE,
        size: 0,
    };

    pub fn contains(self, sp: usize) -> bool {
        self.flags & SS_DISABLE == 0 && sp >= self.sp && sp.wrapping_sub(self.sp) < self.size
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub bytes: [u8; 128],
}

impl Info {
    pub fn new(signal: usize, code: i32, sender: usize) -> Self {
        let mut info = Self { bytes: [0; 128] };
        info.bytes[..4].copy_from_slice(&(signal as u32).to_le_bytes());
        info.bytes[8..12].copy_from_slice(&code.to_le_bytes());
        info.bytes[16..20].copy_from_slice(&(sender as u32).to_le_bytes());
        info
    }

    pub fn signal(self) -> usize {
        u32::from_le_bytes(self.bytes[..4].try_into().unwrap()) as usize
    }

    pub fn fault(signal: usize, code: i32, address: usize) -> Self {
        let mut info = Self::new(signal, code, 0);
        info.bytes[16..24].copy_from_slice(&(address as u64).to_le_bytes());
        info
    }

    pub fn child(pid: usize, code: i32, status: usize) -> Self {
        let mut info = Self::new(SIGCHLD, code, pid);
        info.bytes[24..28].copy_from_slice(&(status as u32).to_le_bytes());
        info
    }
}

#[derive(Clone)]
pub struct Signals {
    pub actions: [Action; NSIG],
    pub blocked: u64,
    pub pending: Vec<Info>,
    pub altstack: AltStack,
    pub suspend_mask: Option<u64>,
    pub wait_mask: Option<u64>,
    pub stopped: bool,
    // A completed syscall or suspended user context held by a default stop action.
    pub stopped_context: Option<crate::process::HardwareContext>,
    pub child_event: Option<u32>,
    pub child_notify: bool,
    pub exit_signal: usize,
    pub trampoline: usize,
    pub timer_deadline: Option<usize>,
    pub timer_interval: usize,
}

impl Signals {
    pub fn new() -> Self {
        Self {
            actions: [Action::default(); NSIG],
            blocked: 0,
            pending: Vec::new(),
            altstack: AltStack::DISABLED,
            suspend_mask: None,
            wait_mask: None,
            stopped: false,
            stopped_context: None,
            child_event: None,
            child_notify: false,
            exit_signal: 0,
            trampoline: 0,
            timer_deadline: None,
            timer_interval: 0,
        }
    }

    pub fn fork(&self) -> Self {
        Self {
            actions: self.actions,
            blocked: self.blocked,
            altstack: self.altstack,
            trampoline: self.trampoline,
            ..Self::new()
        }
    }

    pub fn exec(&mut self) {
        for action in &mut self.actions {
            if action.handler != SIG_IGN {
                *action = Action::default();
            }
        }
        self.altstack = AltStack::DISABLED;
        self.suspend_mask = None;
        self.wait_mask = None;
        self.trampoline = 0;
    }

    pub fn pending_mask(&self) -> u64 {
        self.pending
            .iter()
            .fold(0, |mask, info| mask | bit(info.signal()))
    }

    pub fn discard(&mut self, signal: usize) {
        self.pending.retain(|info| info.signal() != signal);
    }

    pub fn enqueue(&mut self, info: Info) -> bool {
        let signal = info.signal();
        if signal == SIGCONT {
            self.pending.retain(|info| !stop_signal(info.signal()));
        } else if stop_signal(signal) {
            self.discard(SIGCONT);
        }
        if signal < 32 && self.pending.iter().any(|queued| queued.signal() == signal) {
            return true;
        }
        // Standard signals have at most one record per number. Reserve their
        // finite headroom rather than dropping an already queued realtime signal
        // when SIGSTOP/SIGALRM arrives at a full realtime queue.
        if signal >= 32 && self.pending.len() >= PENDING_LIMIT {
            return false;
        }
        self.pending.push(info);
        true
    }

    pub fn take_matching(&mut self, mask: u64) -> Option<Info> {
        let index = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, info)| mask & bit(info.signal()) != 0)
            .min_by_key(|(_, info)| info.signal())
            .map(|(index, _)| index)?;
        Some(self.pending.remove(index))
    }

    pub fn next(&mut self) -> Option<Info> {
        let mut eligible = !self.blocked;
        if self.stopped {
            eligible &= bit(SIGKILL) | bit(SIGCONT);
        }
        self.take_matching(eligible)
    }
}
