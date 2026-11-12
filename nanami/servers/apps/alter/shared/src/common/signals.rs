//! Linux signal state. Runtime owns the lazy allocation; PCB metadata copies do not.
use alloc::rc::Rc;
use alloc::collections::VecDeque;
#[cfg(test)]
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};

pub const NSIG: usize = 64;
pub const SIGKILL: usize = 9;
pub const SIGTERM: usize = 15;
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

#[cfg(test)]
static SIGNAL_READS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub fn take_signal_reads() -> usize {
    SIGNAL_READS.swap(0, core::sync::atomic::Ordering::Relaxed)
}

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
        #[cfg(test)]
        SIGNAL_READS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
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

/// Records and their exact membership mask are mutated together. Keeping the
/// records private also protects the mask shared by threads in a task group.
#[derive(Clone, Default)]
pub struct Pending {
    records: VecDeque<Info>,
    mask: u64,
}

impl Pending {
    #[cfg(test)]
    pub fn record_addresses(&self) -> Vec<(Info, usize)> {
        self.records.iter().map(|info| (*info, info as *const Info as usize)).collect()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    fn retain(&mut self, keep: impl Fn(&Info) -> bool) {
        let mut mask = 0;
        self.records.retain(|info| {
            if keep(info) {
                mask |= bit(info.signal());
                true
            } else {
                false
            }
        });
        self.mask = mask;
    }

    fn enqueue(&mut self, info: Info) -> bool {
        let signal = info.signal();
        if signal < 32 && self.mask & bit(signal) != 0 {
            return true;
        }
        // Standard signals retain their finite headroom above the realtime cap.
        if signal >= 32 && self.len() >= PENDING_LIMIT {
            return false;
        }
        self.records.push_back(info);
        self.mask |= bit(signal);
        true
    }

    fn restore(&mut self, info: Info) {
        self.mask |= bit(info.signal());
        self.records.push_front(info);
    }

    fn take_signal(&mut self, signal: usize) -> Option<Info> {
        let index = self.records.iter().position(|info| info.signal() == signal)?;
        let info = self.records.remove(index)?;
        // The removed record was the first match. Earlier records cannot be
        // duplicates; any next match is now at index or later in logical order.
        // Realtime/restored duplicates retain membership until the last one.
        if !self.records.range(index..).any(|info| info.signal() == signal) {
            self.mask &= !bit(signal);
        }
        Some(info)
    }
}

#[derive(Clone)]
pub struct Signals {
    pub actions: Rc<RefCell<[Action; NSIG]>>,
    pub blocked: u64,
    pub pending: Pending,
    group_pending: Rc<RefCell<Pending>>,
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
            pending: Pending::default(),
            group_pending: Rc::new(RefCell::new(Pending::default())),
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
        self.clear_caught_handlers();
        self.altstack = AltStack::DISABLED;
        self.suspend_mask = None;
        self.wait_mask = None;
        self.trampoline = 0;
    }

    pub fn clear_caught_handlers(&mut self) {
        // exec unshares dispositions before resetting caught handlers.
        let actions = *self.actions.borrow();
        self.actions = Rc::new(RefCell::new(actions));
        for action in self.actions.borrow_mut().iter_mut() {
            if action.handler != SIG_IGN {
                *action = Action::default();
            }
        }
    }

    pub fn pending_mask(&self) -> u64 {
        self.pending.mask | self.group_pending.borrow().mask
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
        self.group_pending.borrow_mut().enqueue(info)
    }

    pub fn enqueue(&mut self, info: Info) -> bool {
        let signal = info.signal();
        if signal == SIGCONT {
            self.discard_stops();
        } else if stop_signal(signal) {
            self.discard(SIGCONT);
        }
        self.pending.enqueue(info)
    }

    pub fn take_matching(&mut self, mask: u64) -> Option<Info> {
        self.take_matching_record(mask).map(|(info, _)| info)
    }

    pub fn restore_pending(&mut self, info: Info, group: bool) {
        if group {
            self.group_pending.borrow_mut().restore(info);
        } else {
            self.pending.restore(info);
        }
    }

    pub fn take_matching_record(&mut self, mask: u64) -> Option<(Info, bool)> {
        let eligible = self.pending_mask() & mask;
        if eligible == 0 {
            return None;
        }
        let signal = eligible.trailing_zeros() as usize + 1;
        if self.pending.mask & bit(signal) != 0 {
            self.pending.take_signal(signal).map(|info| (info, false))
        } else {
            self.group_pending.borrow_mut().take_signal(signal).map(|info| (info, true))
        }
    }

    pub fn next(&mut self) -> Option<Info> {
        let mut eligible = !self.blocked;
        if self.stopped {
            eligible &= bit(SIGKILL) | bit(SIGCONT);
        }
        self.take_matching(eligible)
    }
}
