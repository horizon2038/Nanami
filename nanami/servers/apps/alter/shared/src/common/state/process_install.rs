// Suspended process installation and initial resource ownership.
use super::*;

impl Runtime {
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
        self.install_managed_child_task(
            pid, parent_pid, owner_pid, pcb, terminal_id, image_name, false,
        )
    }

    /// The new task must remain suspended until its resources are installed.
    /// CLONE_FILES can borrow the parent's table immediately; rollback then
    /// follows the same shared-table ownership rules as a completed clone.
    pub fn install_managed_child_task(
        &mut self,
        pid: Word,
        parent_pid: Word,
        owner_pid: Word,
        pcb: Word,
        terminal_id: Word,
        image_name: &[u8],
        share_files: bool,
    ) -> bool {
        if image_name.is_empty() || image_name.len() > ALTER_IMAGE_NAME_MAX {
            return false;
        }
        // These inherited values are captured before any service request or
        // process mutation. One lookup supplies the same parent for all fields.
        let (rootfs, umask, comm, no_new_privs, argument_vector, argument_count, files) =
            if let Some(parent) = self.managed_process(parent_pid) {
                (parent.rootfs, parent.umask, parent.comm, parent.no_new_privs,
                 parent.argument_vector, parent.argument_count,
                 if share_files { Some(parent.files) } else { None })
            } else {
                if share_files {
                    return false;
                }
                let mut comm = [0; 16];
                let count = image_name.len().min(15);
                comm[..count].copy_from_slice(&image_name[..count]);
                (super::super::context::RootfsContext::DEFAULT, 0o022, comm, false, 0, 0, None)
            };
        // Sample once at creation, not from a stale syscall clock cache.
        // If the timer is unavailable, proc reports an unknown start time (0).
        if self.timer_port == 0
            && nanami_client::registry::connect_timer_service(crate::abi::SLOT_TIMER_SERVICE)
                .is_ok()
        {
            self.timer_port =
                nanami_runtime::ipc::process_slot_descriptor(crate::abi::SLOT_TIMER_SERVICE);
        }
        let started_at = if self.timer_port != 0 {
            nanami_client::timer::timer_service_monotonic_ticks(self.timer_port)
                .ok()
                .filter(|(_, hz)| *hz != 0)
                .map_or(0, |(ticks, hz)| (ticks as u128 * 100 / hz as u128) as Word)
        } else {
            0
        };
        let (terminal_canonical, terminal_echo, terminal_termios) = self
            .managed
            .iter()
            .find(|p| terminal_id != 0 && p.pid != 0 && !p.exited && p.terminal_id == terminal_id)
            .map(|p| (p.terminal_canonical, p.terminal_echo, p.terminal_termios))
            .unwrap_or((true, true, None));
        let mut i = 0usize;
        while i < self.managed.len() {
            if self.managed[i].pid == 0 {
                let mut stored_name = [0; ALTER_IMAGE_NAME_MAX];
                stored_name[..image_name.len()].copy_from_slice(image_name);
                self.managed[i] = ManagedProcess {
                    vm_private_shared: false,
                    vm_writable_private_shared: false,
                    pid,
                    tgid: pid,
                    vm_group: pid,
                    fs_group: pid,
                    umask,
                    parent_pid,
                    argument_vector,
                    argument_count,
                    started_at,
                    job: ProcessJobControl::EMPTY,
                    owner_pid,
                    rootfs,
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
                    signal_state: 0,
                    signal_waiting: false,
                    signal_wait_target: 0,
                    signal_context: LinuxSyscallContext::EMPTY,
                    last_syscall: 0,
                    last_syscall_return: 0,
                    personality: OsPersonality::Linux,
                    image_name: stored_name,
                    image_name_len: image_name.len(),
                    comm,
                    no_new_privs,
                    cwd: root_cwd(),
                    cwd_len: 1,
                    files: files.unwrap_or_else(|| FileTable::new(default_files(terminal_id))),
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
                    mappings: MappingTable::EMPTY,
                };
                self.initialize_job_control(pid, parent_pid);
                return true;
            }
            i += 1;
        }
        false
    }

}
