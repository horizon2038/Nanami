use super::*;

#[derive(Clone, Copy)]
pub struct ClientMemory {
    pub pid: Word,
    pub address: Word,
    pub peer: Word,
    pub bytes: Word,
}
impl ClientMemory {
    pub const EMPTY: Self = Self {
        pid: 0,
        address: 0,
        peer: 0,
        bytes: 0,
    };
}

impl Runtime {
    pub fn client_memory(&self, pid: Word) -> Option<(Word, Word)> {
        self.clients
            .iter()
            .find(|client| client.pid == pid && pid != 0)
            .map(|client| (client.address, client.bytes))
    }
    pub fn attached_client(&self, pid: Word) -> Option<ClientMemory> {
        self.clients
            .iter()
            .find(|client| client.pid == pid && pid != 0)
            .copied()
    }
    pub fn has_client_slot(&mut self) -> bool {
        if self.clients.iter().any(|client| client.pid == 0) {
            return true;
        }
        // Only the cold attach path scans native liveness. Reap stale views
        // before refusing a new client; normal syscalls do not enter here.
        for client in &mut self.clients {
            if nanami_runtime::request_process_alive(client.pid) == Ok(false)
                && nanami_runtime::request_mapping_release(client.address, client.bytes).is_ok()
            {
                *client = ClientMemory::EMPTY;
            }
        }
        self.clients.iter().any(|client| client.pid == 0)
    }
    pub fn remember_client(&mut self, pid: Word, address: Word, peer: Word, bytes: Word) -> bool {
        let slot = self
            .clients
            .iter()
            .position(|client| client.pid == pid)
            .or_else(|| self.clients.iter().position(|client| client.pid == 0));
        let Some(slot) = slot else {
            return false;
        };
        self.clients[slot] = ClientMemory {
            pid,
            address,
            peer,
            bytes,
        };
        true
    }
}
