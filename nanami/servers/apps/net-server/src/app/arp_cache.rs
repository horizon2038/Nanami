const CAPACITY: usize = 32;

#[derive(Clone, Copy)]
struct Entry {
    ip: [u8; 4],
    mac: [u8; 6],
}

/// Bounded neighbor cache; updating a neighbor does not evict another one.
pub(super) struct ArpCache {
    entries: [Entry; CAPACITY],
    len: usize,
    next: usize,
}

impl ArpCache {
    pub(super) const EMPTY: Self = Self {
        entries: [Entry {
            ip: [0; 4],
            mac: [0; 6],
        }; CAPACITY],
        len: 0,
        next: 0,
    };

    pub(super) fn lookup(&self, ip: [u8; 4]) -> Option<[u8; 6]> {
        self.entries[..self.len]
            .iter()
            .find(|entry| entry.ip == ip)
            .map(|entry| entry.mac)
    }

    pub(super) fn update(&mut self, ip: [u8; 4], mac: [u8; 6]) {
        if let Some(entry) = self.entries[..self.len]
            .iter_mut()
            .find(|entry| entry.ip == ip)
        {
            entry.mac = mac;
            return;
        }
        self.entries[self.next] = Entry { ip, mac };
        self.next = (self.next + 1) % CAPACITY;
        self.len = core::cmp::min(self.len + 1, CAPACITY);
    }
}
