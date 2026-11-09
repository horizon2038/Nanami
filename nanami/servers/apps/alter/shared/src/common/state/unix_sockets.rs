//! Local socket state. Peers and names are weak references; queued SCM_RIGHTS
//! own file descriptions until delivery, truncation, or socket collection.
use super::{LinuxFile, Word};
use alloc::{collections::VecDeque, vec::Vec};

pub const SOCKET_LIMIT: usize = 128;
#[cfg(test)]
static RIGHTS_PACKET_VISITS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
// A hint, never an identity or capacity limit. Collisions retain the original
// exact-ID search. Explicit optout builds keep no additional runtime storage.
const LOOKUP_SLOTS: usize = match option_env!("NANAMI_UNIX_SOCKET_LOOKUP") {
    Some(value) if value.len() == 1 && value.as_bytes()[0] == b'1' => SOCKET_LIMIT,
    None => SOCKET_LIMIT,
    _ => 0,
};
pub const QUEUE_BYTES: usize = 64 * 1024;
pub const QUEUE_MESSAGES: usize = 256;
pub const RIGHTS_LIMIT: usize = 64;
// Retain at most one small, drained payload per endpoint. Large transfers
// must not leave a queue-sized allocation behind on every idle connection.
pub const STREAM_SPARE_BYTES: usize = 4096;
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
    stream_spare: Vec<u8>,
    pub queued_bytes: usize,
    // Exact sum of packet.rights lengths; update with every enqueue/drain.
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
            stream_spare: Vec::new(),
            queued_bytes: 0,
            queued_rights: 0,
            read_closed: false,
            write_closed: false,
            receive_limit: QUEUE_BYTES,
            send_limit: QUEUE_BYTES,
            marked: false,
        }
    }

    pub fn take_stream_buffer(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.stream_spare)
    }

    pub fn recycle_stream_buffer(&mut self, mut data: Vec<u8>) {
        if self.kind == Kind::Stream && !self.read_closed
            && data.capacity() <= STREAM_SPARE_BYTES
            && data.capacity() > self.stream_spare.capacity()
        {
            // Old payload bytes stay outside Vec's initialized length. Only
            // a successful checked copyin publishes the next message's bytes.
            data.clear();
            self.stream_spare = data;
        }
    }
}

pub struct Sockets {
    entries: Vec<Socket>,
    lookup: [usize; LOOKUP_SLOTS],
    next_id: Word,
    sweeping: bool,
}

impl Sockets {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            lookup: [0; LOOKUP_SLOTS],
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
        self.remember(self.entries.len() - 1);
        Some(id)
    }

    pub fn get(&self, id: Word) -> Option<&Socket> {
        self.index(id).map(|index| &self.entries[index])
    }

    pub fn get_mut(&mut self, id: Word) -> Option<&mut Socket> {
        let index = self.index(id)?;
        Some(&mut self.entries[index])
    }

    fn index(&self, id: Word) -> Option<usize> {
        if LOOKUP_SLOTS != 0 {
            if let Some(index) = self.lookup[id % SOCKET_LIMIT].checked_sub(1) {
                if self.entries.get(index).is_some_and(|socket| socket.id == id) {
                    return Some(index);
                }
            }
        }
        self.entries.iter().position(|socket| socket.id == id)
    }

    fn remember(&mut self, index: usize) {
        if LOOKUP_SLOTS != 0 {
            self.lookup[self.entries[index].id % SOCKET_LIMIT] = index + 1;
        }
    }

    fn remove_index(&mut self, index: usize) -> Socket {
        if LOOKUP_SLOTS != 0 {
            let slot = self.entries[index].id % SOCKET_LIMIT;
            if self.lookup[slot] == index + 1 {
                self.lookup[slot] = 0;
            }
        }
        let removed = self.entries.swap_remove(index);
        if index < self.entries.len() {
            self.remember(index);
        }
        removed
    }

    pub fn find_name(&self, name: &Name) -> Option<Word> {
        self.entries
            .iter()
            .find(|socket| socket.name.as_ref() == Some(name))
            .map(|socket| socket.id)
    }

    pub fn remove(&mut self, id: Word) -> Option<Socket> {
        let index = self.index(id)?;
        Some(self.remove_index(index))
    }

    pub fn references(&self, file: LinuxFile) -> bool {
        self.entries
            .iter()
            .filter(|socket| (!self.sweeping || socket.marked) && socket.queued_rights != 0)
            .any(|socket| {
                socket.packets.iter().any(|packet| {
                    #[cfg(test)]
                    RIGHTS_PACKET_VISITS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    packet.rights.iter().any(|held| {
                        held.kind == file.kind
                            && held.posix_fd == file.posix_fd
                            && held.resource == file.resource
                    })
                })
            })
    }

    #[cfg(test)]
    pub fn take_rights_packet_visits() -> usize {
        RIGHTS_PACKET_VISITS.swap(0, core::sync::atomic::Ordering::Relaxed)
    }

    pub fn reset_marks(&mut self) {
        for socket in &mut self.entries {
            socket.marked = false;
        }
    }

    /// Pipe status is still descriptor-backed; update in-flight aliases too.
    pub fn update_flags(&mut self, file: LinuxFile, mask: Word, flags: Word) {
        for socket in &mut self.entries {
            if socket.queued_rights == 0 { continue; }
            for packet in &mut socket.packets {
                #[cfg(test)]
                RIGHTS_PACKET_VISITS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
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
    // Each discovered endpoint is queued once. Reverse-ordered rights chains
    // must not force a complete rescan of every marked packet at each depth.
    // Entry indices stay stable until tracing finishes and sweep begins.
    pub fn trace_marks(&mut self) {
        const _: () = assert!(SOCKET_LIMIT <= u8::MAX as usize + 1);
        let mut pending = [0u8; SOCKET_LIMIT];
        let mut count = 0;
        for (index, socket) in self.entries.iter().enumerate() {
            if socket.marked {
                pending[count] = index as u8;
                count += 1;
            }
        }
        while count != 0 {
            count -= 1;
            let index = usize::from(pending[count]);
            for accept in 0..self.entries[index].pending.len() {
                let id = self.entries[index].pending[accept];
                self.queue_mark(id, &mut pending, &mut count);
            }
            // Accept queues remain strong roots even without ancillary FDs.
            if self.entries[index].queued_rights == 0 { continue; }
            for packet in 0..self.entries[index].packets.len() {
                #[cfg(test)]
                RIGHTS_PACKET_VISITS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                for right in 0..self.entries[index].packets[packet].rights.len() {
                    let file = self.entries[index].packets[packet].rights[right];
                    if file.kind == super::LinuxFileKind::SocketUnix {
                        self.queue_mark(file.resource, &mut pending, &mut count);
                    }
                }
            }
        }
    }

    fn queue_mark(&mut self, id: Word, pending: &mut [u8; SOCKET_LIMIT], count: &mut usize) {
        if let Some(index) = self.index(id) {
            if !self.entries[index].marked {
                self.entries[index].marked = true;
                pending[*count] = index as u8;
                *count += 1;
            }
        }
    }

    #[cfg(test)]
    pub fn take_unmarked(&mut self) -> Option<Socket> {
        self.take_unmarked_from(&mut 0)
    }

    /// One sweep only: callers must not create, remove or remark other entries
    /// until it finishes. Recheck the removed slot after swap_remove; the marked
    /// prefix before it remains stable, so no subsequent sweep step scans it.
    pub fn take_unmarked_from(&mut self, cursor: &mut usize) -> Option<Socket> {
        let offset = self.entries.get(*cursor..)?.iter().position(|socket| !socket.marked)?;
        let index = *cursor + offset;
        *cursor = index;
        Some(self.remove_index(index))
    }

    pub fn set_sweeping(&mut self, enabled: bool) {
        self.sweeping = enabled;
    }
}
