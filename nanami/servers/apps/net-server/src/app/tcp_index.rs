//! Fixed-capacity indices for the TCP table. No allocation on packet ingress;
//! IDs encode a slot plus its generation, so stale or foreign IDs fail closed.
use super::*;

const BUCKETS: usize = TCP_MAX_CONNECTIONS * 2;
const NONE: u16 = u16::MAX;
const FREE_WORDS: usize = TCP_MAX_CONNECTIONS.div_ceil(64);
const GENERATIONS: u32 = u32::MAX / TCP_MAX_CONNECTIONS as u32;
const _: () = assert!(TCP_MAX_CONNECTIONS < NONE as usize);

pub(super) struct TcpIndex {
    buckets: [u16; BUCKETS],
    next: [u16; TCP_MAX_CONNECTIONS],
    generations: [u32; TCP_MAX_CONNECTIONS],
    free: [u64; FREE_WORDS],
}

impl TcpIndex {
    pub(super) const EMPTY: Self = Self {
        buckets: [NONE; BUCKETS],
        next: [NONE; TCP_MAX_CONNECTIONS],
        generations: [0; TCP_MAX_CONNECTIONS],
        free: [u64::MAX; FREE_WORDS],
    };

    fn bucket(ip: [u8; 4], peer_port: u16, local_port: u16) -> usize {
        let key = u32::from_be_bytes(ip) ^ ((peer_port as u32) << 16 | local_port as u32);
        let hash = (key ^ (key >> 16)).wrapping_mul(0x9e37_79b1);
        ((hash ^ (hash >> 16)) as usize) % BUCKETS
    }

    pub(super) fn find(
        &self,
        connections: &[TcpConnection; TCP_MAX_CONNECTIONS],
        ip: [u8; 4],
        peer_port: u16,
        local_port: u16,
    ) -> Option<usize> {
        let mut slot = self.buckets[Self::bucket(ip, peer_port, local_port)];
        while slot != NONE {
            let conn = &connections[slot as usize];
            if conn.peer_ip == ip && conn.peer_port == peer_port && conn.local_port == local_port {
                return Some(slot as usize);
            }
            slot = self.next[slot as usize];
        }
        None
    }

    pub(super) fn vacant(&self) -> Option<usize> {
        for (word, bits) in self.free.iter().enumerate() {
            if *bits != 0 {
                let index = word * 64 + bits.trailing_zeros() as usize;
                return (index < TCP_MAX_CONNECTIONS).then_some(index);
            }
        }
        None
    }

    /// Register a vacant slot and issue its next nonzero 32-bit wire ID.
    pub(super) fn insert(
        &mut self,
        slot: usize,
        ip: [u8; 4],
        peer_port: u16,
        local_port: u16,
    ) -> Word {
        debug_assert!(self.free[slot / 64] & (1 << (slot % 64)) != 0);
        self.free[slot / 64] &= !(1 << (slot % 64));
        let bucket = Self::bucket(ip, peer_port, local_port);
        self.next[slot] = self.buckets[bucket];
        self.buckets[bucket] = slot as u16;
        let generation = self.generations[slot];
        self.generations[slot] = (generation + 1) % GENERATIONS;
        generation as Word * TCP_MAX_CONNECTIONS + slot + 1
    }

    pub(super) fn remove(&mut self, slot: usize, conn: &TcpConnection) {
        if !conn.active {
            return;
        }
        let bucket = Self::bucket(conn.peer_ip, conn.peer_port, conn.local_port);
        let mut current = self.buckets[bucket];
        let mut previous = NONE;
        while current != NONE {
            if current as usize == slot {
                if previous == NONE {
                    self.buckets[bucket] = self.next[slot];
                } else {
                    self.next[previous as usize] = self.next[slot];
                }
                self.next[slot] = NONE;
                self.free[slot / 64] |= 1 << (slot % 64);
                return;
            }
            previous = current;
            current = self.next[current as usize];
        }
        debug_assert!(false, "active TCP slot missing from index");
    }
}

pub(super) fn active_tcp_connection_index(
    runtime: &NetRuntime,
    owner_id: Word,
    connection_id: Word,
) -> Option<usize> {
    if connection_id == 0 {
        return runtime
            .tcp_connections
            .iter()
            .position(|conn| conn.active && conn.owner_id == owner_id);
    }
    let index = (connection_id - 1) % TCP_MAX_CONNECTIONS;
    let conn = &runtime.tcp_connections[index];
    (conn.active && conn.owner_id == owner_id && conn.connection_id == connection_id)
        .then_some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_id_stays_nonzero_and_32_bit_when_last_slot_generation_wraps() {
        let mut index = TcpIndex::EMPTY;
        let slot = TCP_MAX_CONNECTIONS - 1;
        index.generations[slot] = GENERATIONS - 1;
        let id = index.insert(slot, [10, 0, 0, 1], 1234, 80);
        assert!(id != 0 && id <= u32::MAX as Word);
        assert_eq!((id - 1) % TCP_MAX_CONNECTIONS, slot);
        index.remove(
            slot,
            &TcpConnection {
                active: true,
                peer_ip: [10, 0, 0, 1],
                peer_port: 1234,
                local_port: 80,
                ..TcpConnection::EMPTY
            },
        );
        assert_eq!(
            index.insert(slot, [10, 0, 0, 1], 1234, 80),
            TCP_MAX_CONNECTIONS
        );
    }
}
