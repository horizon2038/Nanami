use libnanami::{RequestError, Word};

use crate::abi::{
    ALTER_IMAGE_NAME_MAX, ALTER_MANAGED_PROCESS_MAX, ALTER_PATH_MAX, ALTER_PROCESS_MAPPING_MAX,
};
use crate::elf::ElfMetadata;
use crate::process::LinuxSyscallContext;

pub const LINUX_FD_MAX: usize = 64;
pub const LINUX_CWD_MAX: usize = 128;
pub const LINUX_TERMINAL_LINE_MAX: usize = 256;
pub const LINUX_PIPE_MAX: usize = 16;
pub const LINUX_PIPE_BYTES: usize = 1024;
const FORK_IMAGE_CACHE_ENTRIES: usize = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OsPersonality {
    Linux,
    FreeBsd,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LinuxFileKind {
    Empty,
    Posix,
    Terminal,
    PipeRead,
    PipeWrite,
    SocketUdp,
    SocketTcp,
    SocketTcpListener,
    SocketIcmp,
    SocketNetlink,
}

#[derive(Clone, Copy)]
pub struct LinuxFile {
    pub kind: LinuxFileKind,
    pub posix_fd: Word,
    pub flags: Word,
    pub local_port: u16,
    pub peer_port: u16,
    pub peer_ip: u32,
}

impl LinuxFile {
    pub const EMPTY: Self = Self {
        kind: LinuxFileKind::Empty,
        posix_fd: 0,
        flags: 0,
        local_port: 0,
        peer_port: 0,
        peer_ip: 0,
    };

    pub const fn posix(posix_fd: Word, flags: Word) -> Self {
        Self {
            kind: LinuxFileKind::Posix,
            posix_fd,
            flags,
            local_port: 0,
            peer_port: 0,
            peer_ip: 0,
        }
    }

    pub const fn terminal() -> Self {
        Self {
            kind: LinuxFileKind::Terminal,
            posix_fd: 0,
            flags: 0,
            local_port: 0,
            peer_port: 0,
            peer_ip: 0,
        }
    }

    pub const fn pipe_read(pipe_id: Word, flags: Word) -> Self {
        Self {
            kind: LinuxFileKind::PipeRead,
            posix_fd: pipe_id,
            flags,
            local_port: 0,
            peer_port: 0,
            peer_ip: 0,
        }
    }

    pub const fn pipe_write(pipe_id: Word, flags: Word) -> Self {
        Self {
            kind: LinuxFileKind::PipeWrite,
            posix_fd: pipe_id,
            flags,
            local_port: 0,
            peer_port: 0,
            peer_ip: 0,
        }
    }

    pub const fn socket_udp(flags: Word) -> Self {
        Self {
            kind: LinuxFileKind::SocketUdp,
            posix_fd: 0,
            flags,
            local_port: 0,
            peer_port: 0,
            peer_ip: 0,
        }
    }

    pub const fn socket_tcp(flags: Word) -> Self {
        Self {
            kind: LinuxFileKind::SocketTcp,
            posix_fd: 0,
            flags,
            local_port: 0,
            peer_port: 0,
            peer_ip: 0,
        }
    }

    pub const fn socket_icmp(flags: Word) -> Self {
        Self {
            kind: LinuxFileKind::SocketIcmp,
            posix_fd: 0,
            flags,
            local_port: 0,
            peer_port: 0,
            peer_ip: 0,
        }
    }

    pub const fn socket_netlink(flags: Word) -> Self {
        Self {
            kind: LinuxFileKind::SocketNetlink,
            posix_fd: 0,
            flags,
            local_port: 0,
            peer_port: 0,
            peer_ip: 0,
        }
    }

    pub fn is_open(self) -> bool {
        self.kind != LinuxFileKind::Empty
    }
}

#[derive(Clone, Copy)]
pub struct LinuxPipe {
    pub active: bool,
    pub readers: Word,
    pub writers: Word,
    pub buffer: [u8; LINUX_PIPE_BYTES],
    pub read: usize,
    pub write: usize,
    pub len: usize,
}

impl LinuxPipe {
    pub const EMPTY: Self = Self {
        active: false,
        readers: 0,
        writers: 0,
        buffer: [0; LINUX_PIPE_BYTES],
        read: 0,
        write: 0,
        len: 0,
    };
}

#[derive(Clone, Copy)]
pub struct CachedElfImage {
    pub path: [u8; ALTER_PATH_MAX],
    pub path_len: usize,
    pub address: Word,
    pub size: Word,
    pub metadata: ElfMetadata,
}

#[derive(Clone, Copy)]
pub struct ProcessMapping {
    pub base: Word,
    pub size: Word,
    pub prot: Word,
}

impl ProcessMapping {
    pub const EMPTY: Self = Self {
        base: 0,
        size: 0,
        prot: 0,
    };
}

#[derive(Clone, Copy)]
pub struct ManagedProcess {
    pub pid: Word,
    pub parent_pid: Word,
    pub owner_pid: Word,
    pub pcb: Word,
    pub program_break: Word,
    pub mapped_break: Word,
    pub fs_base: Word,
    pub terminal_id: Word,
    pub trace_enabled: bool,
    pub diagnostics_enabled: bool,
    pub exited: bool,
    pub exit_status: Word,
    pub signal_waiting: bool,
    pub signal_wait_target: Word,
    pub signal_context: LinuxSyscallContext,
    pub last_syscall: Word,
    pub last_syscall_return: isize,
    pub personality: OsPersonality,
    pub image_name: [u8; ALTER_IMAGE_NAME_MAX],
    pub image_name_len: usize,
    pub cwd: [u8; LINUX_CWD_MAX],
    pub cwd_len: usize,
    pub files: [LinuxFile; LINUX_FD_MAX],
    pub terminal_line: [u8; LINUX_TERMINAL_LINE_MAX],
    pub terminal_line_read: usize,
    pub terminal_line_len: usize,
    pub terminal_line_ready: bool,
    pub terminal_canonical: bool,
    pub terminal_echo: bool,
    pub terminal_read_waiting: bool,
    pub terminal_read_buffer: Word,
    pub terminal_read_len: Word,
    pub terminal_read_context: LinuxSyscallContext,
    pub network_waiting: bool,
    pub network_wait_context: LinuxSyscallContext,
    pub mappings: [ProcessMapping; ALTER_PROCESS_MAPPING_MAX],
}

impl ManagedProcess {
    pub const EMPTY: Self = Self {
        pid: 0,
        parent_pid: 0,
        owner_pid: 0,
        pcb: 0,
        program_break: 0,
        mapped_break: 0,
        fs_base: 0,
        terminal_id: 0,
        trace_enabled: false,
        diagnostics_enabled: false,
        exited: false,
        exit_status: 0,
        signal_waiting: false,
        signal_wait_target: 0,
        signal_context: LinuxSyscallContext::EMPTY,
        last_syscall: 0,
        last_syscall_return: 0,
        personality: OsPersonality::Linux,
        image_name: [0; ALTER_IMAGE_NAME_MAX],
        image_name_len: 0,
        cwd: [0; LINUX_CWD_MAX],
        cwd_len: 0,
        files: [LinuxFile::EMPTY; LINUX_FD_MAX],
        terminal_line: [0; LINUX_TERMINAL_LINE_MAX],
        terminal_line_read: 0,
        terminal_line_len: 0,
        terminal_line_ready: false,
        terminal_canonical: true,
        terminal_echo: true,
        terminal_read_waiting: false,
        terminal_read_buffer: 0,
        terminal_read_len: 0,
        terminal_read_context: LinuxSyscallContext::EMPTY,
        network_waiting: false,
        network_wait_context: LinuxSyscallContext::EMPTY,
        mappings: [ProcessMapping::EMPTY; ALTER_PROCESS_MAPPING_MAX],
    };
}

#[derive(Clone, Copy)]
pub struct Runtime {
    pub posix_port: Word,
    pub posix_shm: Word,
    pub posix_shm_size: Word,
    pub posix_direct_shm: Word,
    pub posix_direct_shm_size: Word,
    pub terminal_port: Word,
    pub terminal_shm: Word,
    pub terminal_shm_size: Word,
    pub terminal_input_notification_id: Word,
    pub network_port: Word,
    pub network_shm: Word,
    pub network_shm_size: Word,
    pub next_ephemeral_port: u16,
    pub client_shm: Word,
    pub client_shm_size: Word,
    pub loaded_entry: Word,
    pub loaded_segment_count: Word,
    pub exec_image_buffer: Word,
    pub exec_image_buffer_size: Word,
    pub exec_snapshot_buffer: Word,
    pub exec_snapshot_buffer_size: Word,
    pub exec_stack_buffer: Word,
    pub exec_stack_buffer_size: Word,
    pub fork_image_cache: [Option<CachedElfImage>; FORK_IMAGE_CACHE_ENTRIES],
    pub trapped_faults: Word,
    pub managed: [ManagedProcess; ALTER_MANAGED_PROCESS_MAX],
    pub pipes: [LinuxPipe; LINUX_PIPE_MAX],
}

impl Runtime {
    pub const fn new(
        posix_port: Word,
        posix_shm: Word,
        posix_shm_size: Word,
        posix_direct_shm: Word,
        posix_direct_shm_size: Word,
        terminal_port: Word,
        terminal_shm: Word,
        terminal_shm_size: Word,
    ) -> Self {
        Self {
            posix_port,
            posix_shm,
            posix_shm_size,
            posix_direct_shm,
            posix_direct_shm_size,
            terminal_port,
            terminal_shm,
            terminal_shm_size,
            terminal_input_notification_id: 0,
            network_port: 0,
            network_shm: 0,
            network_shm_size: 0,
            next_ephemeral_port: 49152,
            client_shm: 0,
            client_shm_size: 0,
            loaded_entry: 0,
            loaded_segment_count: 0,
            exec_image_buffer: 0,
            exec_image_buffer_size: 0,
            exec_snapshot_buffer: 0,
            exec_snapshot_buffer_size: 0,
            exec_stack_buffer: 0,
            exec_stack_buffer_size: 0,
            fork_image_cache: [None; FORK_IMAGE_CACHE_ENTRIES],
            trapped_faults: 0,
            managed: [ManagedProcess::EMPTY; ALTER_MANAGED_PROCESS_MAX],
            pipes: [LinuxPipe::EMPTY; LINUX_PIPE_MAX],
        }
    }

    pub fn posix_read_buffer_size(&self) -> Word {
        if self.posix_direct_shm != 0 && self.posix_direct_shm_size != 0 {
            self.posix_direct_shm_size
        } else {
            self.posix_shm_size
        }
    }

    pub fn read_posix(
        &self,
        fd: Word,
        out_offset: Word,
        len: Word,
    ) -> Result<(Word, Word), RequestError> {
        if self.posix_direct_shm != 0
            && out_offset
                .checked_add(len)
                .filter(|end| *end <= self.posix_direct_shm_size)
                .is_some()
        {
            match nanami_services::posix::posix_read_direct(self.posix_port, fd, out_offset, len) {
                Ok(bytes) => return Ok((bytes, self.posix_direct_shm + out_offset)),
                Err(RequestError::Status(libnanami::OS_RESPONSE_ILLEGAL_OPERATION)) => {}
                Err(error) => return Err(error),
            }
        }
        let bytes = nanami_services::posix::posix_read(self.posix_port, fd, out_offset, len)?;
        Ok((bytes, self.posix_shm + out_offset))
    }

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
                    terminal_canonical: true,
                    terminal_echo: true,
                    terminal_read_waiting: false,
                    terminal_read_buffer: 0,
                    terminal_read_len: 0,
                    terminal_read_context: LinuxSyscallContext::EMPTY,
                    network_waiting: false,
                    network_wait_context: LinuxSyscallContext::EMPTY,
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
        process.mappings = [ProcessMapping::EMPTY; ALTER_PROCESS_MAPPING_MAX];
        true
    }

    pub fn linux_file(&self, pid: Word, fd: Word) -> Option<LinuxFile> {
        if fd as usize >= LINUX_FD_MAX {
            return None;
        }
        let file = self.managed_process(pid)?.files[fd as usize];
        if file.is_open() {
            Some(file)
        } else {
            None
        }
    }

    pub fn set_linux_file(&mut self, pid: Word, fd: Word, file: LinuxFile) -> bool {
        if fd as usize >= LINUX_FD_MAX {
            return false;
        }
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        process.files[fd as usize] = file;
        true
    }

    pub fn clear_linux_file(&mut self, pid: Word, fd: Word) -> Option<LinuxFile> {
        if fd as usize >= LINUX_FD_MAX {
            return None;
        }
        let process = self.managed_process_mut(pid)?;
        let old = process.files[fd as usize];
        process.files[fd as usize] = LinuxFile::EMPTY;
        if old.is_open() {
            Some(old)
        } else {
            None
        }
    }

    pub fn allocate_linux_file(
        &mut self,
        pid: Word,
        file: LinuxFile,
        min_fd: Word,
    ) -> Option<Word> {
        let process = self.managed_process_mut(pid)?;
        let mut fd = min_fd as usize;
        if fd >= process.files.len() {
            return None;
        }
        while fd < process.files.len() {
            if !process.files[fd].is_open() {
                process.files[fd] = file;
                return Some(fd as Word);
            }
            fd += 1;
        }
        None
    }

    pub fn set_cwd(&mut self, pid: Word, path: &[u8]) -> bool {
        if path.is_empty() || path.len() >= LINUX_CWD_MAX || path[0] != b'/' {
            return false;
        }
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        process.cwd = [0; LINUX_CWD_MAX];
        process.cwd[..path.len()].copy_from_slice(path);
        process.cwd_len = path.len();
        true
    }

    pub fn inherit_process_files(&mut self, parent_pid: Word, child_pid: Word) -> bool {
        let Some(parent) = self.managed_process(parent_pid) else {
            return false;
        };
        let Some(child) = self.managed_process_mut(child_pid) else {
            return false;
        };
        child.cwd = parent.cwd;
        child.cwd_len = parent.cwd_len;
        child.files = parent.files;
        true
    }

    pub fn allocate_pipe(&mut self) -> Option<Word> {
        let mut i = 0usize;
        while i < self.pipes.len() {
            if !self.pipes[i].active {
                self.pipes[i] = LinuxPipe {
                    active: true,
                    readers: 1,
                    writers: 1,
                    buffer: [0; LINUX_PIPE_BYTES],
                    read: 0,
                    write: 0,
                    len: 0,
                };
                return Some(i as Word);
            }
            i += 1;
        }
        None
    }

    pub fn pipe(&self, pipe_id: Word) -> Option<LinuxPipe> {
        let index = pipe_id as usize;
        if index >= self.pipes.len() || !self.pipes[index].active {
            return None;
        }
        Some(self.pipes[index])
    }

    pub fn pipe_mut(&mut self, pipe_id: Word) -> Option<&mut LinuxPipe> {
        let index = pipe_id as usize;
        if index >= self.pipes.len() || !self.pipes[index].active {
            return None;
        }
        Some(&mut self.pipes[index])
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
            let entry = self.managed[i];
            if entry.pid == pid && entry.pcb != 0 {
                return Some(entry.pcb);
            }
            i += 1;
        }
        None
    }

    pub fn process_for_fault_identifier(&self, identifier: Word) -> Option<ManagedProcess> {
        if let Some(process) = self.managed_process(identifier) {
            Some(process)
        } else if identifier == 0 {
            self.single_managed_process()
        } else {
            None
        }
    }

    fn single_managed_process(&self) -> Option<ManagedProcess> {
        let mut found = ManagedProcess::EMPTY;
        let mut count = 0usize;
        let mut i = 0usize;
        while i < self.managed.len() {
            let entry = self.managed[i];
            if entry.pid != 0 && entry.pcb != 0 {
                found = entry;
                count += 1;
            }
            i += 1;
        }
        if count == 1 {
            Some(found)
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

    pub fn managed_process(&self, pid: Word) -> Option<ManagedProcess> {
        let mut i = 0usize;
        while i < self.managed.len() {
            let entry = self.managed[i];
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
            process.exit_status = status;
        }
    }

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

    pub fn set_trace_enabled(&mut self, pid: Word, enabled: bool) -> bool {
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

    pub fn reset_stack_mapping(&mut self, pid: Word, base: Word, size: Word, prot: Word) -> bool {
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        let mut i = 0usize;
        while i < process.mappings.len() {
            let mapping = process.mappings[i];
            if mapping.base != 0 {
                let Some(mapping_end) = mapping.base.checked_add(mapping.size) else {
                    return false;
                };
                let Some(end) = base.checked_add(size) else {
                    return false;
                };
                if ranges_touch_or_overlap(base, end, mapping.base, mapping_end) {
                    process.mappings[i] = ProcessMapping::EMPTY;
                }
            }
            i += 1;
        }
        self.add_mapping(pid, base, size, prot)
    }

    pub fn add_mapping(&mut self, pid: Word, base: Word, size: Word, prot: Word) -> bool {
        if base == 0 || size == 0 {
            return false;
        }
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        let mut new_base = base;
        let Some(mut new_end) = base.checked_add(size) else {
            return false;
        };
        let mut i = 0usize;
        while i < process.mappings.len() {
            let mapping = process.mappings[i];
            if mapping.base != 0 && mapping.prot == prot {
                let Some(mapping_end) = mapping.base.checked_add(mapping.size) else {
                    return false;
                };
                if ranges_touch_or_overlap(new_base, new_end, mapping.base, mapping_end) {
                    new_base = ::core::cmp::min(new_base, mapping.base);
                    new_end = ::core::cmp::max(new_end, mapping_end);
                    process.mappings[i] = ProcessMapping::EMPTY;
                }
            }
            i += 1;
        }
        i = 0;
        while i < process.mappings.len() {
            let mapping = process.mappings[i];
            if mapping.base == 0 {
                process.mappings[i] = ProcessMapping {
                    base: new_base,
                    size: new_end - new_base,
                    prot,
                };
                return true;
            }
            i += 1;
        }
        false
    }

    pub fn remove_mapping(&mut self, pid: Word, base: Word, size: Word) -> bool {
        if base == 0 || size == 0 {
            return false;
        }
        let Some(end) = base.checked_add(size) else {
            return false;
        };
        if !self.has_mapping(pid, base, size) {
            return false;
        }
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };

        let mut cursor = base;
        while cursor < end {
            let mut found = false;
            let mut i = 0usize;
            while i < process.mappings.len() {
                let mapping = process.mappings[i];
                if mapping.base != 0 {
                    let Some(mapping_end) = mapping.base.checked_add(mapping.size) else {
                        return false;
                    };
                    if mapping.base <= cursor && cursor < mapping_end {
                        let chunk_end = ::core::cmp::min(mapping_end, end);
                        if !remove_mapping_fragment(process, cursor, chunk_end - cursor) {
                            return false;
                        }
                        cursor = chunk_end;
                        found = true;
                        break;
                    }
                }
                i += 1;
            }
            if !found {
                return false;
            }
        }
        true
    }

    pub fn mapping_prot(&self, pid: Word, base: Word, size: Word) -> Option<Word> {
        if base == 0 || size == 0 {
            return None;
        }
        let end = base.checked_add(size)?;
        let process = self.managed_process(pid)?;
        let mut cursor = base;
        let mut out = None;
        while cursor < end {
            let mut found = false;
            let mut i = 0usize;
            while i < process.mappings.len() {
                let mapping = process.mappings[i];
                if mapping.base != 0 {
                    let mapping_end = mapping.base.checked_add(mapping.size)?;
                    if mapping.base <= cursor && cursor < mapping_end {
                        if let Some(prot) = out {
                            if prot != mapping.prot {
                                return None;
                            }
                        } else {
                            out = Some(mapping.prot);
                        }
                        cursor = ::core::cmp::min(mapping_end, end);
                        found = true;
                        break;
                    }
                }
                i += 1;
            }
            if !found {
                return None;
            }
        }
        out
    }

    pub fn protect_mapping(&mut self, pid: Word, base: Word, size: Word, prot: Word) -> bool {
        if !self.has_mapping(pid, base, size) {
            return false;
        }
        self.remove_mapping(pid, base, size) && self.add_mapping(pid, base, size, prot)
    }

    pub fn has_mapping(&self, pid: Word, base: Word, size: Word) -> bool {
        if base == 0 || size == 0 {
            return false;
        }
        let Some(end) = base.checked_add(size) else {
            return false;
        };
        let Some(process) = self.managed_process(pid) else {
            return false;
        };
        let mut cursor = base;
        while cursor < end {
            let mut found = false;
            let mut i = 0usize;
            while i < process.mappings.len() {
                let mapping = process.mappings[i];
                if mapping.base != 0 {
                    let Some(mapping_end) = mapping.base.checked_add(mapping.size) else {
                        return false;
                    };
                    if mapping.base <= cursor && cursor < mapping_end {
                        cursor = ::core::cmp::min(mapping_end, end);
                        found = true;
                        break;
                    }
                }
                i += 1;
            }
            if !found {
                return false;
            }
        }
        true
    }
}

fn remove_mapping_fragment(process: &mut ManagedProcess, base: Word, size: Word) -> bool {
    if base == 0 || size == 0 {
        return false;
    }
    let Some(end) = base.checked_add(size) else {
        return false;
    };
    let mut i = 0usize;
    while i < process.mappings.len() {
        let mapping = process.mappings[i];
        if mapping.base != 0 {
            let Some(mapping_end) = mapping.base.checked_add(mapping.size) else {
                return false;
            };
            if mapping.base <= base && end <= mapping_end {
                let left_size = base - mapping.base;
                let right_size = mapping_end - end;
                if left_size != 0 && right_size != 0 && free_mapping_slots(process) == 0 {
                    return false;
                }
                process.mappings[i] = ProcessMapping::EMPTY;

                if left_size != 0
                    && !insert_mapping_fragment(
                        process,
                        ProcessMapping {
                            base: mapping.base,
                            size: left_size,
                            prot: mapping.prot,
                        },
                    )
                {
                    return false;
                }
                if right_size != 0
                    && !insert_mapping_fragment(
                        process,
                        ProcessMapping {
                            base: end,
                            size: right_size,
                            prot: mapping.prot,
                        },
                    )
                {
                    return false;
                }
                return true;
            }
        }
        i += 1;
    }
    false
}

fn free_mapping_slots(process: &ManagedProcess) -> usize {
    let mut count = 0usize;
    let mut i = 0usize;
    while i < process.mappings.len() {
        if process.mappings[i].base == 0 {
            count += 1;
        }
        i += 1;
    }
    count
}

fn insert_mapping_fragment(process: &mut ManagedProcess, mapping: ProcessMapping) -> bool {
    let mut i = 0usize;
    while i < process.mappings.len() {
        if process.mappings[i].base == 0 {
            process.mappings[i] = mapping;
            return true;
        }
        i += 1;
    }
    false
}

fn root_cwd() -> [u8; LINUX_CWD_MAX] {
    let mut cwd = [0; LINUX_CWD_MAX];
    cwd[0] = b'/';
    cwd
}

fn default_files(terminal_id: Word) -> [LinuxFile; LINUX_FD_MAX] {
    let mut files = [LinuxFile::EMPTY; LINUX_FD_MAX];
    if terminal_id != 0 {
        files[0] = LinuxFile::terminal();
        files[1] = LinuxFile::terminal();
        files[2] = LinuxFile::terminal();
    }
    files
}

fn wait_target_matches(child_pid: Word, target_pid: Word) -> bool {
    target_pid == 0 || target_pid == usize::MAX as Word || target_pid == child_pid
}

fn ranges_touch_or_overlap(a_start: Word, a_end: Word, b_start: Word, b_end: Word) -> bool {
    a_start <= b_end && b_start <= a_end
}

#[derive(Clone, Copy)]
pub enum ReplyAction {
    Reply(Word, Word, Word),
    FaultContinue {
        hardware_context: crate::process::HardwareContext,
        hardware_context_count: usize,
    },
    DropReply,
}
