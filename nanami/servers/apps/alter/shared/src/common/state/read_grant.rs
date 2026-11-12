use super::{LinuxFile, LinuxFileKind, Word};

impl LinuxFile {
    // Network address words have no POSIX-file meaning. Keep its peer-bound
    // read grant here without increasing every descriptor's fixed layout.
    pub fn posix_read_grant(self) -> Word {
        if self.kind != LinuxFileKind::Posix { return 0; }
        (self.peer_ip as Word | ((self.local_port as Word) << 32) | ((self.peer_port as Word) << 48))
            & !(1 << (Word::BITS - 1))
    }

    pub fn with_posix_read_grant(mut self, grant: Word) -> Self {
        debug_assert!(self.kind == LinuxFileKind::Posix);
        // The open reply leaves its high eight bits for the file type. Never
        // turn an invalid oversized token into a different valid authority.
        let grant = if grant <= Word::MAX >> 8 { grant } else { 0 };
        self.peer_ip = grant as u32;
        self.local_port = (grant >> 32) as u16;
        self.peer_port = (grant >> 48) as u16;
        self
    }
    pub fn posix_read_cursor(self) -> bool {
        self.kind == LinuxFileKind::Posix && self.posix_read_grant() != 0
            && self.peer_port & 0x8000 != 0
    }
    pub fn with_posix_read_cursor(mut self, cursor: bool) -> Self {
        debug_assert!(self.kind == LinuxFileKind::Posix);
        // This is descriptor-local metadata in an unused token bit; strip it
        // before every VFS call. It does not itself grant server authority.
        self.peer_port &= !0x8000;
        if cursor && self.posix_read_grant() != 0 { self.peer_port |= 0x8000; }
        self
    }
}
