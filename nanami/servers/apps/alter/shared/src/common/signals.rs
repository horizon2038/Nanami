//! Linux signal state. Runtime owns the lazy allocation; PCB metadata copies do not.
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};

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
    pub actions: Rc<RefCell<[Action; NSIG]>>,
    pub blocked: u64,
    pub pending: Vec<Info>,
    group_pending: Rc<RefCell<Vec<Info>>>,
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
    pub timer: Rc<Cell<Timer>>,
}

#[derive(Clone, Copy, Default)]
pub struct Timer {
    pub deadline: Option<usize>,
    pub interval: usize,
}

impl Signals {
    pub fn new() -> Self {
        Self {
            actions: Rc::new(RefCell::new([Action::default(); NSIG])),
            blocked: 0,
            pending: Vec::new(),
            group_pending: Rc::new(RefCell::new(Vec::new())),
            altstack: AltStack::DISABLED,
            suspend_mask: None,
            wait_mask: None,
            stopped: false,
            stopped_context: None,
            child_event: None,
            child_notify: false,
            exit_signal: 0,
            trampoline: 0,
            timer: Rc::new(Cell::new(Timer::default())),
        }
    }

    pub fn fork(&self) -> Self {
        Self {
            actions: Rc::new(RefCell::new(*self.actions.borrow())),
            blocked: self.blocked,
            altstack: self.altstack,
            trampoline: self.trampoline,
            ..Self::new()
        }
    }

    pub fn exec(&mut self) {
        // exec unshares dispositions before resetting caught handlers.
        let actions = *self.actions.borrow();
        self.actions = Rc::new(RefCell::new(actions));
        for action in self.actions.borrow_mut().iter_mut() {
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
            .chain(self.group_pending.borrow().iter())
            .fold(0, |mask, info| mask | bit(info.signal()))
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty() || !self.group_pending.borrow().is_empty()
    }

    pub fn discard(&mut self, signal: usize) {
        self.pending.retain(|info| info.signal() != signal);
        self.group_pending
            .borrow_mut()
            .retain(|info| info.signal() != signal);
    }

    pub fn action(&self, signal: usize) -> Action {
        self.actions.borrow()[signal - 1]
    }

    pub fn set_action(&mut self, signal: usize, action: Action) {
        self.actions.borrow_mut()[signal - 1] = action;
    }

    pub fn clone_task(&self, shared_handlers: bool, thread: bool) -> Self {
        let mut child = self.fork();
        child.altstack = AltStack::DISABLED;
        if shared_handlers {
            child.actions = self.actions.clone();
        }
        if thread {
            child.group_pending = self.group_pending.clone();
            child.timer = self.timer.clone();
        }
        child
    }

    pub fn discard_stops(&mut self) {
        self.pending.retain(|info| !stop_signal(info.signal()));
        self.group_pending
            .borrow_mut()
            .retain(|info| !stop_signal(info.signal()));
    }

    pub fn enqueue_group(&mut self, info: Info) -> bool {
        Self::enqueue_to(&mut self.group_pending.borrow_mut(), info)
    }

    pub fn enqueue(&mut self, info: Info) -> bool {
        let signal = info.signal();
        if signal == SIGCONT {
            self.discard_stops();
        } else if stop_signal(signal) {
            self.discard(SIGCONT);
        }
        Self::enqueue_to(&mut self.pending, info)
    }

    fn enqueue_to(pending: &mut Vec<Info>, info: Info) -> bool {
        let signal = info.signal();
        if signal < 32 && pending.iter().any(|queued| queued.signal() == signal) {
            return true;
        }
        // Standard signals have at most one record per number. Reserve their
        // finite headroom rather than dropping an already queued realtime signal
        // when SIGSTOP/SIGALRM arrives at a full realtime queue.
        if signal >= 32 && pending.len() >= PENDING_LIMIT {
            return false;
        }
        pending.push(info);
        true
    }

    pub fn take_matching(&mut self, mask: u64) -> Option<Info> {
        self.take_matching_record(mask).map(|(info, _)| info)
    }

    pub fn restore_pending(&mut self, info: Info, group: bool) {
        if group {
            self.group_pending.borrow_mut().insert(0, info);
        } else {
            self.pending.insert(0, info);
        }
    }

    pub fn take_matching_record(&mut self, mask: u64) -> Option<(Info, bool)> {
        let local = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, info)| mask & bit(info.signal()) != 0)
            .min_by_key(|(_, info)| info.signal())
            .map(|(index, info)| (index, info.signal()));
        let group = self
            .group_pending
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, info)| mask & bit(info.signal()) != 0)
            .min_by_key(|(_, info)| info.signal())
            .map(|(index, info)| (index, info.signal()));
        if let Some((index, signal)) = local {
            if group.is_none_or(|(_, group_signal)| signal <= group_signal) {
                return Some((self.pending.remove(index), false));
            }
        }
        group.map(|(index, _)| (self.group_pending.borrow_mut().remove(index), true))
    }

    pub fn next(&mut self) -> Option<Info> {
        let mut eligible = !self.blocked;
        if self.stopped {
            eligible &= bit(SIGKILL) | bit(SIGCONT);
        }
        self.take_matching(eligible)
    }
}
