//! Local socket state. Peers and names are weak references; queued SCM_RIGHTS
//! own file descriptions until delivery, truncation, or socket collection.
use super::{LinuxFile, Word};
use alloc::{collections::VecDeque, vec::Vec};

pub const SOCKET_LIMIT: usize = 128;
pub const QUEUE_BYTES: usize = 64 * 1024;
pub const QUEUE_MESSAGES: usize = 256;
pub const RIGHTS_LIMIT: usize = 64;
// Linux can report one extra NUL for a full, non-NUL-terminated sun_path.
pub const ADDRESS_BYTES: usize = 111;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Stream,
    Datagram,
    SeqPacket,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Credentials {
    pub pid: u32,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Address {
    pub bytes: [u8; ADDRESS_BYTES],
    pub len: usize,
}

impl Address {
    pub const UNNAMED: Self = {
        let mut bytes = [0; ADDRESS_BYTES];
        bytes[0] = 1; // AF_UNIX, on both supported little-endian ABIs.
        Self { bytes, len: 2 }
    };
}

#[derive(Clone, PartialEq, Eq)]
pub enum Name {
    Inode(Word),
    Abstract { namespace: Word, bytes: Vec<u8> },
}

pub struct Packet {
    pub data: Vec<u8>,
    pub offset: usize,
    pub rights: Vec<LinuxFile>,
    pub source: Address,
}

pub struct Socket {
    pub id: Word,
    pub kind: Kind,
    pub address: Address,
    pub name: Option<Name>,
    pub inode_pin: Option<Word>,
    pub peer: Option<Word>,
    pub peer_address: Address,
    pub credentials: Credentials,
    pub peer_credentials: Option<Credentials>,
    pub nonblocking: bool,
    pub listening: bool,
    pub backlog: usize,
    pub pending: VecDeque<Word>,
    pub packets: VecDeque<Packet>,
    pub queued_bytes: usize,
    pub queued_rights: usize,
    pub read_closed: bool,
    pub write_closed: bool,
    pub receive_limit: usize,
    pub send_limit: usize,
    pub marked: bool,
}

impl Socket {
    fn new(id: Word, kind: Kind, credentials: Credentials, nonblocking: bool) -> Self {
        Self {
            id,
            kind,
            address: Address::UNNAMED,
            name: None,
            inode_pin: None,
            peer: None,
            peer_address: Address::UNNAMED,
            credentials,
            peer_credentials: None,
            nonblocking,
            listening: false,
            backlog: 1,
            pending: VecDeque::new(),
            packets: VecDeque::new(),
            queued_bytes: 0,
            queued_rights: 0,
            read_closed: false,
            write_closed: false,
            receive_limit: QUEUE_BYTES,
            send_limit: QUEUE_BYTES,
            marked: false,
        }
    }
}

pub struct Sockets {
    entries: Vec<Socket>,
    next_id: Word,
    sweeping: bool,
}

impl Sockets {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_id: 1,
            sweeping: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn create(
        &mut self,
        kind: Kind,
        credentials: Credentials,
        nonblocking: bool,
    ) -> Option<Word> {
        if self.entries.len() == SOCKET_LIMIT || self.next_id == Word::MAX {
            return None;
        }
        self.entries.try_reserve(1).ok()?;
        let id = self.next_id;
        self.next_id += 1;
        self.entries
            .push(Socket::new(id, kind, credentials, nonblocking));
        Some(id)
    }

    pub fn get(&self, id: Word) -> Option<&Socket> {
        self.entries.iter().find(|socket| socket.id == id)
    }

    pub fn get_mut(&mut self, id: Word) -> Option<&mut Socket> {
        self.entries.iter_mut().find(|socket| socket.id == id)
    }

    pub fn find_name(&self, name: &Name) -> Option<Word> {
        self.entries
            .iter()
            .find(|socket| socket.name.as_ref() == Some(name))
            .map(|socket| socket.id)
    }

    pub fn remove(&mut self, id: Word) -> Option<Socket> {
        let index = self.entries.iter().position(|socket| socket.id == id)?;
        Some(self.entries.swap_remove(index))
    }

    pub fn references(&self, file: LinuxFile) -> bool {
        self.entries
            .iter()
            .filter(|socket| !self.sweeping || socket.marked)
            .any(|socket| {
                socket.packets.iter().any(|packet| {
                    packet.rights.iter().any(|held| {
                        held.kind == file.kind
                            && held.posix_fd == file.posix_fd
                            && held.resource == file.resource
                    })
                })
            })
    }

    pub fn reset_marks(&mut self) {
        for socket in &mut self.entries {
            socket.marked = false;
        }
    }

    /// Pipe status is still descriptor-backed; update in-flight aliases too.
    pub fn update_flags(&mut self, file: LinuxFile, mask: Word, flags: Word) {
        for socket in &mut self.entries {
            for packet in &mut socket.packets {
                for held in &mut packet.rights {
                    if held.kind == file.kind
                        && held.posix_fd == file.posix_fd
                        && held.resource == file.resource
                    {
                        held.flags = (held.flags & !mask) | (flags & mask);
                    }
                }
            }
        }
    }

    pub fn mark(&mut self, id: Word) -> bool {
        if let Some(socket) = self.get_mut(id) {
            if !socket.marked {
                socket.marked = true;
                return true;
            }
        }
        false
    }

    // Only descriptors and accept queues retain endpoints, never peer links.
    // Iterate to a fixed point without recursion or allocation on the close path.
    pub fn trace_marks(&mut self) {
        loop {
            let mut changed = false;
            for index in 0..self.entries.len() {
                if !self.entries[index].marked {
                    continue;
                }
                for pending in 0..self.entries[index].pending.len() {
                    let id = self.entries[index].pending[pending];
                    changed |= self.mark(id);
                }
                for packet in 0..self.entries[index].packets.len() {
                    for right in 0..self.entries[index].packets[packet].rights.len() {
                        let file = self.entries[index].packets[packet].rights[right];
                        if file.kind == super::LinuxFileKind::SocketUnix {
                            changed |= self.mark(file.resource);
                        }
                    }
                }
            }
            if !changed {
                return;
            }
        }
    }

    pub fn take_unmarked(&mut self) -> Option<Socket> {
        let index = self.entries.iter().position(|socket| !socket.marked)?;
        Some(self.entries.swap_remove(index))
    }

    pub fn set_sweeping(&mut self, enabled: bool) {
        self.sweeping = enabled;
    }
}
