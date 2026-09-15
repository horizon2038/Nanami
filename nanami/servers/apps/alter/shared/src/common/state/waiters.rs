use super::*;

impl Runtime {
    pub fn park_signal_waiter(
        &mut self,
        pid: Word,
        target_pid: Word,
        context: LinuxSyscallContext,
    ) -> bool {
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        process.signal_waiting = true;
        process.signal_wait_target = target_pid;
        process.signal_context = context;
        true
    }

    pub fn park_terminal_reader(
        &mut self,
        pid: Word,
        buffer: Word,
        len: Word,
        context: LinuxSyscallContext,
    ) -> bool {
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        process.terminal_read_waiting = true;
        process.terminal_read_buffer = buffer;
        process.terminal_read_len = len;
        process.terminal_read_context = context;
        true
    }

    pub fn park_network_waiter(&mut self, pid: Word, context: LinuxSyscallContext) -> bool {
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        process.network_waiting = true;
        process.network_wait_context = context;
        true
    }

    pub fn park_device_reader(
        &mut self,
        pid: Word,
        fd: Word,
        buffer: Word,
        len: Word,
        context: LinuxSyscallContext,
    ) -> bool {
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        process.device_read_waiting = true;
        process.device_read_fd = fd;
        process.device_read_buffer = buffer;
        process.device_read_len = len;
        process.device_read_context = context;
        true
    }

    pub fn take_signal_waiter_for_child(
        &mut self,
        child_pid: Word,
    ) -> Option<(Word, Word, LinuxSyscallContext)> {
        let mut parent_pid = 0;
        let mut i = 0usize;
        while i < self.managed.len() {
            let process = self.managed[i];
            if process.pid == child_pid && process.pcb != 0 {
                parent_pid = process.parent_pid;
                break;
            }
            i += 1;
        }
        if parent_pid == 0 {
            return None;
        }
        i = 0;
        while i < self.managed.len() {
            let process = self.managed[i];
            if process.pid == parent_pid
                && process.pcb != 0
                && process.signal_waiting
                && wait_target_matches(child_pid, process.signal_wait_target)
            {
                self.managed[i].signal_waiting = false;
                self.managed[i].signal_wait_target = 0;
                self.managed[i].signal_context = LinuxSyscallContext::EMPTY;
                return Some((process.pid, process.pcb, process.signal_context));
            }
            i += 1;
        }
        None
    }
}
