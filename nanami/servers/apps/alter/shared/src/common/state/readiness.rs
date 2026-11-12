use super::{LinuxSyscallContext, Word, LINUX_FD_MAX};

pub const READY_INPUT: Word = 1;
pub const READY_TERMINAL: Word = 2;
pub const READY_NETWORK: Word = 4;
pub const READY_PIPE: Word = 8;
pub const READY_TIMER: Word = 16;
pub const READY_OBJECT: Word = 32;
pub const READY_UNIX: Word = 64;
pub const READY_FLOCK: Word = 128;
pub const READY_PTY: Word = 256;
// READY_UNIX remains the general state-change source (close, shutdown, etc.).
// Data arrival and capacity release need only wake matching I/O interests.
pub const READY_UNIX_READ: Word = 512;
pub const READY_UNIX_WRITE: Word = 1024;
// READY_PIPE remains the EOF/EPIPE/description state-change source.
pub const READY_PIPE_READ: Word = 2048;
pub const READY_PIPE_WRITE: Word = 4096;

/// Bounded hints for data/capacity notifications, never socket identities.
/// Colliding IDs and unqualified notifications may cause extra scans only.
#[derive(Clone, Copy, Default)]
pub struct UnixWakeTargets {
    read: Word,
    write: Word,
}

impl UnixWakeTargets {
    pub const EMPTY: Self = Self { read: 0, write: 0 };

    fn bit(id: Word) -> Word { 1 << (id % Word::BITS as Word) }

    pub fn note_read(&mut self, id: Word) { self.read |= Self::bit(id); }

    pub fn note_write(&mut self, id: Option<Word>) {
        self.write |= id.map_or(Word::MAX, Self::bit);
    }

    pub fn qualified(self, sources: Word) -> bool {
        (sources & READY_UNIX_READ != 0 && self.read != 0 && self.read != Word::MAX)
            || (sources & READY_UNIX_WRITE != 0 && self.write != 0 && self.write != Word::MAX)
    }

    pub fn matches(self, sources: Word, id: Word, read: bool, write: bool) -> bool {
        // Zero also supports producers that only set the old source flags.
        (read && sources & READY_UNIX_READ != 0
            && (self.read == 0 || self.read & Self::bit(id) != 0))
            || (write && sources & READY_UNIX_WRITE != 0
                && (self.write == 0 || self.write & Self::bit(id) != 0))
    }
}

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
