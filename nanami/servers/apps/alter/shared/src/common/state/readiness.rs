use super::{LinuxSyscallContext, Word, LINUX_FD_MAX};

pub const READY_INPUT: Word = 1;
pub const READY_TERMINAL: Word = 2;
pub const READY_NETWORK: Word = 4;
pub const READY_PIPE: Word = 8;
pub const READY_TIMER: Word = 16;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PollFd {
    pub fd: i32,
    pub events: i16,
    pub revents: i16,
}

impl PollFd {
    pub const EMPTY: Self = Self {
        fd: -1,
        events: 0,
        revents: 0,
    };
}

#[derive(Clone, Copy)]
pub struct ReadinessWait {
    pub context: LinuxSyscallContext,
    // Snapshot the interest set once: another process can modify shared guest
    // memory while this process is blocked. Never retain the IPC scratch buffer.
    pub entries: [PollFd; LINUX_FD_MAX],
    pub count: usize,
    pub sources: Word,
    pub deadline: Option<Word>,
    pub select: bool,
}
