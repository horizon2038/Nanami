//! Local names and endpoint references, without a closed list of service kinds.
use nanami_uapi::registry::{ServiceInfo, SERVICE_NAME_BYTES};
use nun::{CapabilityDescriptor, CapabilityError, Word};

const MAX_SERVICES: usize = 64;

#[derive(Clone, Copy)]
struct Entry {
    info: ServiceInfo,
    port: CapabilityDescriptor,
    managed_id: Word,
}

impl Entry {
    const EMPTY: Self = Self {
        info: ServiceInfo {
            owner_pid: 0,
            name_len: 0,
            name: [0; SERVICE_NAME_BYTES],
        },
        port: 0,
        managed_id: 0,
    };
}

pub(crate) struct Services {
    entries: [Entry; MAX_SERVICES],
    count: usize,
}

impl Services {
    pub const fn new() -> Self {
        Self {
            entries: [Entry::EMPTY; MAX_SERVICES],
            count: 0,
        }
    }

    pub fn register(
        &mut self,
        owner: Word,
        name: &str,
        port: CapabilityDescriptor,
    ) -> Result<(), CapabilityError> {
        if name.is_empty()
            || name.len() > SERVICE_NAME_BYTES
            || name.as_bytes().contains(&0)
            || port == 0
        {
            return Err(CapabilityError::InvalidArgument);
        }
        if let Some(existing) = self.entries[..self.count].iter().find(|entry| {
            entry.info.name_len == name.len() && entry.info.name[..name.len()] == *name.as_bytes()
        }) {
            // Registration is not a binding-change authority. Exact retries are harmless.
            if existing.info.owner_pid == owner && existing.port == port {
                return Ok(());
            }
            return Err(CapabilityError::PermissionDenied);
        }
        if self.count == MAX_SERVICES {
            return Err(CapabilityError::InvalidArgument);
        }
        let mut info = ServiceInfo {
            owner_pid: owner,
            name_len: name.len(),
            name: [0; SERVICE_NAME_BYTES],
        };
        info.name[..name.len()].copy_from_slice(name.as_bytes());
        self.entries[self.count] = Entry {
            info,
            port,
            managed_id: 0,
        };
        self.count += 1;
        Ok(())
    }

    pub fn resolve(&self, name: &str) -> Option<(CapabilityDescriptor, Word)> {
        self.entries[..self.count]
            .iter()
            .find(|entry| {
                entry.info.name_len == name.len()
                    && entry.info.name[..name.len()] == *name.as_bytes()
            })
            .map(|entry| (entry.port, entry.info.owner_pid))
    }

    pub fn can_publish(&self, name: &str) -> Result<(), CapabilityError> {
        if name.is_empty()
            || name.len() > SERVICE_NAME_BYTES
            || name.as_bytes().contains(&0)
            || self.count == MAX_SERVICES
        {
            return Err(CapabilityError::InvalidArgument);
        }
        if self.resolve(name).is_some() {
            return Err(CapabilityError::PermissionDenied);
        }
        Ok(())
    }

    pub fn publish(
        &mut self,
        name: &str,
        id: Word,
        port: CapabilityDescriptor,
    ) -> Result<(), CapabilityError> {
        self.can_publish(name)?;
        if id == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        self.register(0, name, port)?;
        self.entries[self.count - 1].managed_id = id;
        Ok(())
    }

    pub fn managed_id(&self, name: &str) -> Option<Word> {
        self.entries[..self.count]
            .iter()
            .find(|entry| {
                entry.managed_id != 0
                    && entry.info.name_len == name.len()
                    && entry.info.name[..name.len()] == *name.as_bytes()
            })
            .map(|entry| entry.managed_id)
    }

    /// chunk=0: local owner and byte length; chunk=1..2: two name words.
    /// Entries are append-only; registration cannot change an existing record.
    pub fn info_chunk(&self, ordinal: Word, chunk: Word) -> Option<(Word, Word)> {
        let entry = self.entries.get(..self.count)?.get(ordinal)?;
        if chunk == 0 {
            return Some((entry.info.owner_pid, entry.info.name_len));
        }
        const WORD_BYTES: usize = core::mem::size_of::<Word>();
        const CHUNK_BYTES: usize = 2 * WORD_BYTES;
        if chunk > SERVICE_NAME_BYTES.div_ceil(CHUNK_BYTES) {
            return None;
        }
        let start = (chunk - 1) * CHUNK_BYTES;
        let mut raw = [0; CHUNK_BYTES];
        let end = (start + CHUNK_BYTES).min(SERVICE_NAME_BYTES);
        raw[..end - start].copy_from_slice(&entry.info.name[start..end]);
        let mut first = [0; WORD_BYTES];
        let mut second = [0; WORD_BYTES];
        first.copy_from_slice(&raw[..WORD_BYTES]);
        second.copy_from_slice(&raw[WORD_BYTES..]);
        Some((Word::from_le_bytes(first), Word::from_le_bytes(second)))
    }
}
