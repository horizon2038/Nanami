use super::*;

impl Runtime {
    pub fn has_child(&self, parent_pid: Word, target_pid: Word) -> bool {
        let parent_pid = self.thread_group(parent_pid);
        let group = self
            .managed_process(parent_pid)
            .map_or(0, |process| process.job.group);
        let mut i = 0usize;
        while i < self.managed.len() {
            let child = self.managed[i];
            if child.pid != 0
                && child.pid == child.tgid
                && child.parent_pid == parent_pid
                && wait_target_matches(child.pid, target_pid, child.job.group, group)
            {
                return true;
            }
            i += 1;
        }
        false
    }

    pub fn remove_process(&mut self, pid: Word) -> bool {
        let mut i = 0usize;
        while i < self.managed.len() {
            if self.managed[i].pid == pid {
                #[cfg(feature = "syscall-scope")]
                if self.syscall_scope.retire(pid) {
                    nanami_runtime::println!("[syscall.scope.abort] owner={} reason=retire", pid);
                }
                self.release_signals(i);
                let process = self.managed[i];
                if !self
                    .managed
                    .iter()
                    .any(|other| other.pid != pid && other.files.same_storage(process.files))
                {
                    self.managed[i].files.release();
                }
                if !self
                    .managed
                    .iter()
                    .any(|other| other.pid != pid && other.mappings.same_storage(process.mappings))
                {
                    self.managed[i].mappings.release();
                }
                self.managed[i] = ManagedProcess::EMPTY;
                return true;
            }
            i += 1;
        }
        false
    }

    pub fn set_process_image_name(&mut self, pid: Word, image_name: &[u8]) -> bool {
        if image_name.is_empty() || image_name.len() > ALTER_IMAGE_NAME_MAX {
            return false;
        }
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        process.image_name = [0; ALTER_IMAGE_NAME_MAX];
        process.image_name[..image_name.len()].copy_from_slice(image_name);
        process.image_name_len = image_name.len();
        process.comm = [0; 16];
        process.comm[..image_name.len().min(15)]
            .copy_from_slice(&image_name[..image_name.len().min(15)]);
        true
    }

    pub fn reset_process_runtime_for_exec(&mut self, pid: Word) -> bool {
        #[cfg(feature = "syscall-scope")]
        if self.managed_process(pid).is_some() && self.syscall_scope.retire(pid) {
            nanami_runtime::println!("[syscall.scope.abort] owner={} reason=exec", pid);
        }
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        // EXEC_MEMORY has replaced the VM; no old private-fork alias survives.
        // The exec caller marks any potential new immutable main aliases next.
        process.vm_private_shared = false;
        process.vm_writable_private_shared = false;
        process.program_break = 0;
        process.argument_vector = 0;
        process.argument_count = 0;
        process.job.executed = true;
        if process.signal_state != 0 {
            process.signals_mut().exec();
        }
        process.mapped_break = 0;
        process.fs_base = 0;
        process.exited = false;
        process.exit_status = 0;
        process.signal_waiting = false;
        process.signal_wait_target = 0;
        process.signal_context = LinuxSyscallContext::EMPTY;
        process.last_syscall = 0;
        process.last_syscall_return = 0;
        process.terminal_line_read = 0;
        process.terminal_line_len = 0;
        process.terminal_line_ready = false;
        process.terminal_read_waiting = false;
        process.terminal_read_buffer = 0;
        process.terminal_read_len = 0;
        process.terminal_read_context = LinuxSyscallContext::EMPTY;
        process.network_waiting = false;
        process.network_wait_context = LinuxSyscallContext::EMPTY;
        process.device_read_waiting = false;
        process.device_read_fd = 0;
        process.device_read_buffer = 0;
        process.device_read_len = 0;
        process.device_read_context = LinuxSyscallContext::EMPTY;
        process.sleep_waiting = false;
        process.sleep_deadline = 0;
        process.sleep_context = LinuxSyscallContext::EMPTY;
        process.readiness_wait = None;
        process.mappings.clear();
        true
    }

    pub fn root_process_for_terminal(&self, terminal_id: Word) -> Option<ManagedProcess> {
        if terminal_id == 0 {
            return None;
        }
        let mut i = 0usize;
        while i < self.managed.len() {
            let process = self.managed[i];
            if process.pid != 0
                && process.parent_pid == 0
                && process.terminal_id == terminal_id
                && process.rootfs.leader == 0
            {
                return Some(process);
            }
            i += 1;
        }
        None
    }

    pub fn exited_process(&self) -> Option<ManagedProcess> {
        let mut i = 0usize;
        while i < self.managed.len() {
            let process = self.managed[i];
            if process.pid != 0 && process.exited && process.rootfs.root_len == 0 {
                if process.pid == process.tgid && self.has_live_threads(process.pid) {
                    i += 1;
                    continue;
                }
                return Some(process);
            }
            i += 1;
        }
        None
    }

    pub fn pcb_for_pid(&self, pid: Word) -> Option<Word> {
        if pid == 0 {
            return None;
        }
        let mut i = 0usize;
        while i < self.managed.len() {
            let entry = &self.managed[i];
            if entry.pid == pid && entry.pcb != 0 {
                return Some(entry.pcb);
            }
            i += 1;
        }
        None
    }

    pub fn process_for_fault_identifier(&self, identifier: Word) -> Option<FaultProcess> {
        if identifier != 0 {
            let process = self.managed_process(identifier)?;
            return Some(FaultProcess {
                pid: process.pid,
                pcb: process.pcb,
                personality: process.personality,
            });
        }
        self.single_managed_fault_process()
    }

    fn single_managed_fault_process(&self) -> Option<FaultProcess> {
        let mut found = None;
        let mut count = 0usize;
        let mut i = 0usize;
        while i < self.managed.len() {
            let entry = &self.managed[i];
            if entry.pid != 0 && entry.pcb != 0 {
                found = Some(FaultProcess {
                    pid: entry.pid,
                    pcb: entry.pcb,
                    personality: entry.personality,
                });
                count += 1;
            }
            i += 1;
        }
        if count == 1 {
            found
        } else {
            None
        }
    }

    pub fn next_pcb_slot(&self) -> Option<Word> {
        let mut i = 0usize;
        while i < self.managed.len() {
            if self.managed[i].pid == 0 {
                return Some(crate::abi::ALTER_MANAGED_PCB_SLOT_BASE + i as Word);
            }
            i += 1;
        }
        None
    }

    pub fn managed_process(&self, pid: Word) -> Option<&ManagedProcess> {
        let index = self
            .process_index
            .find(&self.managed, pid, |p| (p.pcb != 0).then_some(p.pid))?;
        self.managed.get(index)
    }

    pub fn managed_process_mut(&mut self, pid: Word) -> Option<&mut ManagedProcess> {
        let index = self
            .process_index
            .find(&self.managed, pid, |p| (p.pcb != 0).then_some(p.pid))?;
        self.managed.get_mut(index)
    }

    pub fn mark_process_exited(&mut self, pid: Word, status: Word) {
        #[cfg(feature = "syscall-scope")]
        if self.syscall_scope.retire(pid) {
            nanami_runtime::println!("[syscall.scope.abort] owner={} reason=exit", pid);
        }
        let group = self.thread_group(pid);
        if self
            .managed_process(group)
            .is_some_and(|process| process.job.session == group)
            && !self.has_live_threads(pid)
        {
            self.release_session_terminal(group);
        }
        if let Some(process) = self.managed_process_mut(pid) {
            process.exited = true;
            process.readiness_wait = None;
            process.exit_status = status;
        }
    }

    pub fn set_trace_enabled(&mut self, pid: Word, enabled: bool) -> bool {
        if enabled {
            self.trace_ever_enabled = true;
        }
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        process.trace_enabled = enabled;
        true
    }

    pub fn set_diagnostics_enabled(&mut self, pid: Word, enabled: bool) -> bool {
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        process.diagnostics_enabled = enabled;
        true
    }

    pub fn configure_graphics(&mut self, pid: Word, enabled: bool, size: FramebufferSize) -> bool {
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        process.graphics_enabled = enabled;
        process.framebuffer_size = size;
        true
    }

    pub fn set_graphics_session(&mut self, pid: Word, session: Word) -> bool {
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        process.graphics_session = session;
        true
    }

    pub fn set_personality(&mut self, pid: Word, personality: OsPersonality) -> bool {
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        process.personality = personality;
        true
    }

    pub fn set_fs_base(&mut self, pid: Word, fs_base: Word) -> bool {
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        process.fs_base = fs_base;
        true
    }
}
