use super::*;

impl Runtime {
    pub fn cached_fork_image(&self, path: &[u8]) -> Option<CachedElfImage> {
        self.fork_image_cache
            .iter()
            .flatten()
            .find(|image| image.path_len == path.len() && image.path[..image.path_len] == *path)
            .copied()
    }

    pub fn has_fork_image_cache_slot(&self) -> bool {
        self.fork_image_cache.iter().any(Option::is_none)
    }

    pub fn cache_fork_image(
        &mut self,
        path: &[u8],
        address: Word,
        size: Word,
        metadata: ElfMetadata,
    ) -> bool {
        if path.is_empty() || path.len() > ALTER_PATH_MAX {
            return false;
        }
        let Some(slot) = self
            .fork_image_cache
            .iter_mut()
            .find(|entry| entry.is_none())
        else {
            return false;
        };
        let mut cached_path = [0; ALTER_PATH_MAX];
        cached_path[..path.len()].copy_from_slice(path);
        *slot = Some(CachedElfImage {
            path: cached_path,
            path_len: path.len(),
            address,
            size,
            metadata,
        });
        true
    }

    pub fn install_managed_process(
        &mut self,
        pid: Word,
        owner_pid: Word,
        pcb: Word,
        terminal_id: Word,
        image_name: &[u8],
    ) -> bool {
        self.install_managed_child_process(pid, 0, owner_pid, pcb, terminal_id, image_name)
    }

    pub fn install_managed_child_process(
        &mut self,
        pid: Word,
        parent_pid: Word,
        owner_pid: Word,
        pcb: Word,
        terminal_id: Word,
        image_name: &[u8],
    ) -> bool {
        if image_name.is_empty() || image_name.len() > ALTER_IMAGE_NAME_MAX {
            return false;
        }
        let (terminal_canonical, terminal_echo, terminal_termios) = self.managed.iter()
            .find(|p| terminal_id != 0 && p.pid != 0 && !p.exited && p.terminal_id == terminal_id)
            .map(|p| (p.terminal_canonical, p.terminal_echo, p.terminal_termios))
            .unwrap_or((true, true, None));
        let mut i = 0usize;
        while i < self.managed.len() {
            if self.managed[i].pid == 0 {
                let mut stored_name = [0; ALTER_IMAGE_NAME_MAX];
                stored_name[..image_name.len()].copy_from_slice(image_name);
                self.managed[i] = ManagedProcess {
                    pid,
                    parent_pid,
                    owner_pid,
                    pcb,
                    program_break: 0,
                    mapped_break: 0,
                    fs_base: 0,
                    terminal_id,
                    trace_enabled: false,
                    diagnostics_enabled: false,
                    graphics_enabled: false,
                    framebuffer_size: FramebufferSize::DEFAULT,
                    graphics_session: 0,
                    exited: false,
                    exit_status: 0,
                    signal_waiting: false,
                    signal_wait_target: 0,
                    signal_context: LinuxSyscallContext::EMPTY,
                    last_syscall: 0,
                    last_syscall_return: 0,
                    personality: OsPersonality::Linux,
                    image_name: stored_name,
                    image_name_len: image_name.len(),
                    cwd: root_cwd(),
                    cwd_len: 1,
                    files: default_files(terminal_id),
                    terminal_line: [0; LINUX_TERMINAL_LINE_MAX],
                    terminal_line_read: 0,
                    terminal_line_len: 0,
                    terminal_line_ready: false,
                    terminal_canonical,
                    terminal_echo,
                    terminal_termios,
                    terminal_read_waiting: false,
                    terminal_read_buffer: 0,
                    terminal_read_len: 0,
                    terminal_read_context: LinuxSyscallContext::EMPTY,
                    network_waiting: false,
                    network_wait_context: LinuxSyscallContext::EMPTY,
                    device_read_waiting: false,
                    device_read_fd: 0,
                    device_read_buffer: 0,
                    device_read_len: 0,
                    device_read_context: LinuxSyscallContext::EMPTY,
                    sleep_waiting: false,
                    sleep_deadline: 0,
                    sleep_context: LinuxSyscallContext::EMPTY,
                    readiness_wait: None,
                    mappings: [ProcessMapping::EMPTY; ALTER_PROCESS_MAPPING_MAX],
                };
                return true;
            }
            i += 1;
        }
        false
    }

    pub fn has_child(&self, parent_pid: Word, target_pid: Word) -> bool {
        let mut i = 0usize;
        while i < self.managed.len() {
            let child = self.managed[i];
            if child.pid != 0
                && child.parent_pid == parent_pid
                && wait_target_matches(child.pid, target_pid)
            {
                return true;
            }
            i += 1;
        }
        false
    }

    pub fn exited_child(&self, parent_pid: Word, target_pid: Word) -> Option<(Word, Word)> {
        let mut i = 0usize;
        while i < self.managed.len() {
            let child = self.managed[i];
            if child.pid != 0
                && child.parent_pid == parent_pid
                && child.exited
                && wait_target_matches(child.pid, target_pid)
            {
                return Some((child.pid, child.exit_status));
            }
            i += 1;
        }
        None
    }

    pub fn remove_process(&mut self, pid: Word) -> bool {
        let mut i = 0usize;
        while i < self.managed.len() {
            if self.managed[i].pid == pid {
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
        true
    }

    pub fn reset_process_runtime_for_exec(&mut self, pid: Word) -> bool {
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        process.program_break = 0;
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
        process.mappings = [ProcessMapping::EMPTY; ALTER_PROCESS_MAPPING_MAX];
        true
    }

    pub fn deepest_process_in_tree(&self, root_pid: Word) -> Option<ManagedProcess> {
        if root_pid == 0 {
            return None;
        }
        let mut best = ManagedProcess::EMPTY;
        let mut best_depth = 0usize;
        let mut i = 0usize;
        while i < self.managed.len() {
            let entry = self.managed[i];
            if entry.pid != 0 {
                if let Some(depth) = self.tree_depth(entry.pid, root_pid) {
                    if best.pid == 0 || depth >= best_depth {
                        best = entry;
                        best_depth = depth;
                    }
                }
            }
            i += 1;
        }
        if best.pid == 0 {
            None
        } else {
            Some(best)
        }
    }

    pub fn root_process_for_terminal(&self, terminal_id: Word) -> Option<ManagedProcess> {
        if terminal_id == 0 {
            return None;
        }
        let mut i = 0usize;
        while i < self.managed.len() {
            let process = self.managed[i];
            if process.pid != 0 && process.parent_pid == 0 && process.terminal_id == terminal_id {
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
            if process.pid != 0 && process.exited {
                return Some(process);
            }
            i += 1;
        }
        None
    }

    fn tree_depth(&self, pid: Word, root_pid: Word) -> Option<usize> {
        let mut current = pid;
        let mut depth = 0usize;
        while depth <= self.managed.len() {
            if current == root_pid {
                return Some(depth);
            }
            let process = self.managed_process(current)?;
            if process.parent_pid == 0 {
                return None;
            }
            current = process.parent_pid;
            depth += 1;
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
            let mut i = 0usize;
            while i < self.managed.len() {
                let process = &self.managed[i];
                if process.pid == identifier && process.pcb != 0 {
                    return Some(FaultProcess {
                        pid: process.pid,
                        pcb: process.pcb,
                        personality: process.personality,
                    });
                }
                i += 1;
            }
            return None;
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
        let mut i = 0usize;
        while i < self.managed.len() {
            let entry = &self.managed[i];
            if entry.pid == pid && entry.pcb != 0 {
                return Some(entry);
            }
            i += 1;
        }
        None
    }

    pub fn managed_process_mut(&mut self, pid: Word) -> Option<&mut ManagedProcess> {
        let mut i = 0usize;
        while i < self.managed.len() {
            if self.managed[i].pid == pid && self.managed[i].pcb != 0 {
                return Some(&mut self.managed[i]);
            }
            i += 1;
        }
        None
    }

    pub fn mark_process_exited(&mut self, pid: Word, status: Word) {
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
