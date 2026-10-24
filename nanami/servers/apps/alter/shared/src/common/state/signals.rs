use super::*;
use crate::common::signals::Signals;
use alloc::boxed::Box;

impl ManagedProcess {
    pub fn signals(&self) -> Option<&Signals> {
        if self.signal_state == 0 {
            return None;
        }
        Some(unsafe { &*(self.signal_state as *const Signals) })
    }

    pub fn signals_mut(&mut self) -> &mut Signals {
        if self.signal_state == 0 {
            self.signal_state = Box::into_raw(Box::new(Signals::new())) as Word;
        }
        unsafe { &mut *(self.signal_state as *mut Signals) }
    }

    pub fn signal_stopped(&self) -> bool {
        self.signals().is_some_and(|signals| signals.stopped)
    }

    pub fn parked_context(&self) -> Option<LinuxSyscallContext> {
        if self.signal_waiting {
            return Some(self.signal_context);
        }
        if self.terminal_read_waiting {
            return Some(self.terminal_read_context);
        }
        if self.network_waiting {
            return Some(self.network_wait_context);
        }
        if self.device_read_waiting {
            return Some(self.device_read_context);
        }
        if self.sleep_waiting {
            return Some(self.sleep_context);
        }
        self.readiness_wait.as_ref().map(|wait| wait.context)
    }

    pub fn cancel_signal_wait(&mut self) {
        self.signal_waiting = false;
        self.terminal_read_waiting = false;
        self.network_waiting = false;
        self.device_read_waiting = false;
        self.sleep_waiting = false;
        self.sleep_deadline = 0;
        self.readiness_wait = None;
    }
}

impl Runtime {
    pub fn is_signal_stopped(&self, pid: Word) -> bool {
        self.managed_process(pid)
            .is_some_and(ManagedProcess::signal_stopped)
    }

    pub fn inherit_signals(&mut self, parent: Word, child: Word) {
        let signals = self
            .managed_process(parent)
            .and_then(ManagedProcess::signals)
            .map(Signals::fork);
        if let (Some(signals), Some(child)) = (signals, self.managed_process_mut(child)) {
            *child.signals_mut() = signals;
        }
    }

    pub(super) fn release_signals(&mut self, index: usize) {
        let address = self.managed[index].signal_state;
        self.managed[index].signal_state = 0;
        if address != 0 {
            drop(unsafe { Box::from_raw(address as *mut Signals) });
        }
    }
}
