#[path = "state/descriptors.rs"]
mod descriptors;
#[path = "state/events.rs"]
mod events;
#[path = "state/io.rs"]
mod io;
#[path = "state/mappings.rs"]
mod mappings;
#[path = "state/processes.rs"]
mod processes;
#[path = "state/waiters.rs"]
mod waiters;

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
pub const ALTER_GRAPHICS_SESSION_MAX: usize = 4;
pub const ALTER_EVDEV_QUEUE_CAPACITY: usize = 128;
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
    VirtualDirectory,
    VirtualFile,
    EvdevKeyboard,
    EvdevMouse,
    Framebuffer,
}

#[derive(Clone, Copy)]
pub struct LinuxFile {
    pub kind: LinuxFileKind,
    pub posix_fd: Word,
    pub flags: Word,
    pub local_port: u16,
    pub peer_port: u16,
    pub peer_ip: u32,
    pub offset: Word,
    pub resource: Word,
}

impl LinuxFile {
    pub const EMPTY: Self = Self {
        kind: LinuxFileKind::Empty,
        posix_fd: 0,
        flags: 0,
        local_port: 0,
        peer_port: 0,
        peer_ip: 0,
        offset: 0,
        resource: 0,
    };

    pub const fn posix(posix_fd: Word, flags: Word, status_flags: Word) -> Self {
        Self {
            kind: LinuxFileKind::Posix,
            posix_fd,
            flags,
            local_port: 0,
            peer_port: 0,
            peer_ip: 0,
            offset: 0,
            // Posix descriptors use resource for immutable Linux open flags;
            // descriptor flags (CLOEXEC) remain separate in flags.
            resource: status_flags,
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
            offset: 0,
            resource: 0,
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
            offset: 0,
            resource: 0,
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
            offset: 0,
            resource: 0,
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
            offset: 0,
            resource: 0,
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
            offset: 0,
            resource: 0,
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
            offset: 0,
            resource: 0,
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
            offset: 0,
            resource: 0,
        }
    }

    pub const fn virtual_node(kind: LinuxFileKind, node: Word, flags: Word) -> Self {
        Self {
            kind,
            posix_fd: 0,
            flags,
            local_port: 0,
            peer_port: 0,
            peer_ip: 0,
            offset: 0,
            resource: node,
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
    pub graphics_enabled: bool,
    pub graphics_session: Word,
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
    pub device_read_waiting: bool,
    pub device_read_fd: Word,
    pub device_read_buffer: Word,
    pub device_read_len: Word,
    pub device_read_context: LinuxSyscallContext,
    pub sleep_waiting: bool,
    pub sleep_ticks_remaining: Word,
    pub sleep_context: LinuxSyscallContext,
    pub mappings: [ProcessMapping; ALTER_PROCESS_MAPPING_MAX],
}

#[derive(Clone, Copy)]
pub struct FaultProcess {
    pub pid: Word,
    pub pcb: Word,
    pub personality: OsPersonality,
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
        graphics_enabled: false,
        graphics_session: 0,
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
        device_read_waiting: false,
        device_read_fd: 0,
        device_read_buffer: 0,
        device_read_len: 0,
        device_read_context: LinuxSyscallContext::EMPTY,
        sleep_waiting: false,
        sleep_ticks_remaining: 0,
        sleep_context: LinuxSyscallContext::EMPTY,
        mappings: [ProcessMapping::EMPTY; ALTER_PROCESS_MAPPING_MAX],
    };
}

#[derive(Clone, Copy)]
pub struct GraphicsSession {
    pub active: bool,
    pub root_pid: Word,
    pub honoka_port: Word,
    pub present_notification: Word,
    pub window_id: Word,
    pub width: Word,
    pub height: Word,
    pub damage_queue: Word,
    pub framebuffer: Word,
    pub framebuffer_bytes: Word,
    pub input_queue: Word,
    pub keyboard_events: [Word; ALTER_EVDEV_QUEUE_CAPACITY],
    pub keyboard_head: usize,
    pub keyboard_tail: usize,
    pub mouse_events: [Word; ALTER_EVDEV_QUEUE_CAPACITY],
    pub mouse_head: usize,
    pub mouse_tail: usize,
    pub mouse_x: i32,
    pub mouse_y: i32,
    pub mouse_position_valid: bool,
    pub guest_pid: Word,
    pub guest_framebuffer: Word,
    pub guest_framebuffer_bytes: Word,
}

impl GraphicsSession {
    pub const EMPTY: Self = Self {
        active: false,
        root_pid: 0,
        honoka_port: 0,
        present_notification: 0,
        window_id: 0,
        width: 0,
        height: 0,
        damage_queue: 0,
        framebuffer: 0,
        framebuffer_bytes: 0,
        input_queue: 0,
        keyboard_events: [0; ALTER_EVDEV_QUEUE_CAPACITY],
        keyboard_head: 0,
        keyboard_tail: 0,
        mouse_events: [0; ALTER_EVDEV_QUEUE_CAPACITY],
        mouse_head: 0,
        mouse_tail: 0,
        mouse_x: 0,
        mouse_y: 0,
        mouse_position_valid: false,
        guest_pid: 0,
        guest_framebuffer: 0,
        guest_framebuffer_bytes: 0,
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
    pub timer_port: Word,
    pub clock_timer_armed: bool,
    pub monotonic_ticks: Word,
    pub monotonic_tick_hz: Word,
    pub network_port: Word,
    pub network_shm: Word,
    pub network_shm_size: Word,
    pub next_ephemeral_port: u16,
    pub input_port: Word,
    pub input_queue: Word,
    pub input_queue_size: Word,
    pub honoka_port: Word,
    pub honoka_pid: Word,
    pub keyboard_events: [Word; ALTER_EVDEV_QUEUE_CAPACITY],
    pub keyboard_head: usize,
    pub keyboard_tail: usize,
    pub mouse_events: [Word; ALTER_EVDEV_QUEUE_CAPACITY],
    pub mouse_head: usize,
    pub mouse_tail: usize,
    pub graphics: [GraphicsSession; ALTER_GRAPHICS_SESSION_MAX],
    pub client_shm: Word,
    pub client_shm_size: Word,
    pub loaded_entry: Word,
    pub loaded_segment_count: Word,
    pub exec_image_buffer: Word,
    pub exec_image_buffer_size: Word,
    pub interpreter_image_buffer: Word,
    pub interpreter_image_buffer_size: Word,
    pub exec_snapshot_buffer: Word,
    pub exec_snapshot_buffer_size: Word,
    pub exec_stack_buffer: Word,
    pub exec_stack_buffer_size: Word,
    pub fork_image_cache: [Option<CachedElfImage>; FORK_IMAGE_CACHE_ENTRIES],
    pub trapped_faults: Word,
    // False for the normal non-strace case. Once enabled it remains true so
    // hot syscall paths can skip all trace-related process-table scans.
    pub trace_ever_enabled: bool,
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
            timer_port: 0,
            clock_timer_armed: false,
            monotonic_ticks: 0,
            monotonic_tick_hz: 0,
            network_port: 0,
            network_shm: 0,
            network_shm_size: 0,
            next_ephemeral_port: 49152,
            input_port: 0,
            input_queue: 0,
            input_queue_size: 0,
            honoka_port: 0,
            honoka_pid: 0,
            keyboard_events: [0; ALTER_EVDEV_QUEUE_CAPACITY],
            keyboard_head: 0,
            keyboard_tail: 0,
            mouse_events: [0; ALTER_EVDEV_QUEUE_CAPACITY],
            mouse_head: 0,
            mouse_tail: 0,
            graphics: [GraphicsSession::EMPTY; ALTER_GRAPHICS_SESSION_MAX],
            client_shm: 0,
            client_shm_size: 0,
            loaded_entry: 0,
            loaded_segment_count: 0,
            exec_image_buffer: 0,
            exec_image_buffer_size: 0,
            interpreter_image_buffer: 0,
            interpreter_image_buffer_size: 0,
            exec_snapshot_buffer: 0,
            exec_snapshot_buffer_size: 0,
            exec_stack_buffer: 0,
            exec_stack_buffer_size: 0,
            fork_image_cache: [None; FORK_IMAGE_CACHE_ENTRIES],
            trapped_faults: 0,
            trace_ever_enabled: false,
            managed: [ManagedProcess::EMPTY; ALTER_MANAGED_PROCESS_MAX],
            pipes: [LinuxPipe::EMPTY; LINUX_PIPE_MAX],
        }
    }
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

#[derive(Clone, Copy)]
pub enum ReplyAction {
    Reply(Word, Word, Word),
    FaultContinue {
        hardware_context: crate::process::HardwareContext,
        hardware_context_count: usize,
    },
    DropReply,
}
