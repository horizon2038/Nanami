const CAPACITY: usize = nanami_client::net::NET_ARP_MAX_ENTRIES;

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

    pub(super) fn snapshot(&self, out: &mut [u8]) -> Option<usize> {
        use nanami_client::net::NET_ARP_ENTRY_BYTES;
        let bytes = self.len * NET_ARP_ENTRY_BYTES;
        let out = out.get_mut(..bytes)?;
        for (entry, record) in self.entries[..self.len]
            .iter()
            .zip(out.chunks_exact_mut(NET_ARP_ENTRY_BYTES))
        {
            record[..4].copy_from_slice(&entry.ip);
            record[4..10].copy_from_slice(&entry.mac);
            record[10..].fill(0);
        }
        Some(self.len)
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
