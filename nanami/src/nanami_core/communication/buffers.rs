//! Opaque managed session data. No queue layout, application state or device
//! semantics live here. Backing remains charged; mappings carry no Frame caps.
use super::bindings::SESSIONS;
use nanami_uapi::service::{ServiceId, SessionBuffer, SessionId, SESSION_IDENTIFIER_BIT};
use nun::{CapabilityError, Word};

const PAGE: Word = 4096;

#[derive(Clone, Copy, Debug, Default)]
pub struct Mapping {
    pub owner: Word,
    pub address: Word,
    pub installed: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct Buffer {
    pub service: ServiceId,
    pub physical: Word,
    pub pages: usize,
    pub client: Mapping,
    pub provider: Mapping,
    pub candidate: Option<Mapping>,
    pub closed: bool,
    pub ready: bool,
}

pub trait Effects {
    fn participants(&mut self, client: Word, provider: Word) -> Result<(), CapabilityError>;
    fn allocate(&mut self, pages: usize) -> Result<Word, CapabilityError>;
    fn reserve(&mut self, owner: Word, pages: usize) -> Result<Word, CapabilityError>;
    fn zero(&mut self, physical: Word) -> Result<(), CapabilityError>;
    fn map(&mut self, owner: Word, address: Word, physical: Word) -> Result<(), MapError>;
    fn unmap(&mut self, owner: Word, address: Word, physical: Word) -> Result<(), CapabilityError>;
    fn receiver(
        &mut self,
        service: ServiceId,
        owner: Word,
        slot: Word,
    ) -> Result<(), CapabilityError>;
}

pub struct Buffers {
    entries: [Option<Buffer>; SESSIONS],
}

pub enum MapError {
    NotInstalled(CapabilityError),
    MaybeInstalled(CapabilityError),
}

fn index(id: SessionId) -> Result<usize, CapabilityError> {
    if id.0 & SESSION_IDENTIFIER_BIT == 0 {
        return Err(CapabilityError::InvalidArgument);
    }
    (id.0 & !SESSION_IDENTIFIER_BIT)
        .checked_sub(1)
        .filter(|index| *index < SESSIONS)
        .ok_or(CapabilityError::InvalidArgument)
}

fn map(
    mapping: &mut Mapping,
    physical: Word,
    pages: usize,
    effects: &mut impl Effects,
) -> Result<(), CapabilityError> {
    for page in 0..pages {
        let result = effects.map(
            mapping.owner,
            mapping.address + page * PAGE,
            physical + page,
        );
        match result {
            Ok(()) => mapping.installed = page + 1,
            Err(MapError::NotInstalled(error)) => return Err(error),
            Err(MapError::MaybeInstalled(error)) => {
                mapping.installed = page + 1;
                return Err(error);
            }
        }
    }
    Ok(())
}

fn unmap(
    mapping: &mut Mapping,
    physical: Word,
    effects: &mut impl Effects,
) -> Result<(), CapabilityError> {
    while mapping.installed != 0 {
        let page = mapping.installed - 1;
        effects.unmap(
            mapping.owner,
            mapping.address + page * PAGE,
            physical + page,
        )?;
        mapping.installed -= 1;
    }
    Ok(())
}

impl Buffers {
    pub const fn new() -> Self {
        Self {
            entries: [None; SESSIONS],
        }
    }

    pub fn get(&self, id: SessionId) -> Result<Option<Buffer>, CapabilityError> {
        Ok(self.entries[index(id)?])
    }

    pub fn session_at(&self, service: ServiceId, ordinal: usize) -> Option<SessionId> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                entry
                    .filter(|buffer| buffer.service == service && buffer.ready && !buffer.closed)
                    .map(|_| SessionId(SESSION_IDENTIFIER_BIT | (index + 1)))
            })
            .nth(ordinal)
    }

    pub fn has_mapping(&self, owner: Word) -> bool {
        self.entries.iter().flatten().any(|buffer| {
            (buffer.client.owner == owner && buffer.client.installed != 0)
                || (buffer.provider.owner == owner && buffer.provider.installed != 0)
                || buffer
                    .candidate
                    .is_some_and(|mapping| mapping.owner == owner && mapping.installed != 0)
        })
    }

    pub fn create(
        &mut self,
        id: SessionId,
        service: ServiceId,
        client: Word,
        provider: Word,
        pages: usize,
        effects: &mut impl Effects,
    ) -> Result<SessionBuffer, CapabilityError> {
        let slot = index(id)?;
        if client == 0 || provider == 0 || client == provider || !(1..=16).contains(&pages) {
            return Err(CapabilityError::InvalidArgument);
        }
        if let Some(buffer) = self.entries[slot] {
            if buffer.service != service || buffer.client.owner != client || buffer.pages != pages {
                return Err(CapabilityError::PermissionDenied);
            }
            if buffer.ready && !buffer.closed {
                return Ok(SessionBuffer {
                    address: buffer.client.address,
                    bytes: pages * PAGE,
                });
            }
            return Err(CapabilityError::Fatal);
        }
        effects.participants(client, provider)?;
        let physical = effects.allocate(pages)?;
        let mut buffer = Buffer {
            service,
            physical,
            pages,
            client: Mapping {
                owner: client,
                ..Mapping::default()
            },
            provider: Mapping {
                owner: provider,
                ..Mapping::default()
            },
            candidate: None,
            closed: false,
            ready: false,
        };
        let result = (|| {
            buffer.client.address = effects.reserve(client, pages)?;
            buffer.provider.address = effects.reserve(provider, pages)?;
            for page in 0..pages {
                effects.zero(physical + page)?;
            }
            map(&mut buffer.client, physical, pages, effects)?;
            map(&mut buffer.provider, physical, pages, effects)?;
            Ok(())
        })();
        if let Err(error) = result {
            let provider_clean = unmap(&mut buffer.provider, physical, effects).is_ok();
            let client_clean = unmap(&mut buffer.client, physical, effects).is_ok();
            buffer.closed = true;
            self.entries[slot] = Some(buffer);
            // Allocation and VA remain charged even after successful cleanup.
            // Never free backing with uncertain aliases or mapping effects.
            if !provider_clean || !client_clean {
                return Err(CapabilityError::Fatal);
            }
            return Err(error);
        }
        buffer.ready = true;
        let view = SessionBuffer {
            address: buffer.client.address,
            bytes: pages * PAGE,
        };
        self.entries[slot] = Some(buffer);
        Ok(view)
    }

    pub fn view(&self, id: SessionId, provider: Word) -> Result<SessionBuffer, CapabilityError> {
        let buffer = self.get(id)?.ok_or(CapabilityError::InvalidArgument)?;
        if buffer.closed
            || !buffer.ready
            || buffer.provider.owner != provider
            || buffer.provider.installed != buffer.pages
        {
            return Err(CapabilityError::PermissionDenied);
        }
        Ok(SessionBuffer {
            address: buffer.provider.address,
            bytes: buffer.pages * PAGE,
        })
    }

    pub fn withdraw(
        &mut self,
        service: ServiceId,
        owner: Word,
        effects: &mut impl Effects,
    ) -> Result<(), CapabilityError> {
        for buffer in self
            .entries
            .iter_mut()
            .flatten()
            .filter(|buffer| buffer.service == service && buffer.ready && !buffer.closed)
        {
            if buffer.provider.owner != owner
                || unmap(&mut buffer.provider, buffer.physical, effects).is_err()
            {
                return Err(CapabilityError::Fatal);
            }
        }
        Ok(())
    }

    /// Map every opaque buffer before the endpoint receiver is installed. On
    /// failure preserve the old metadata and remove all candidate mappings.
    pub fn install(
        &mut self,
        service: ServiceId,
        owner: Word,
        slot: Word,
        effects: &mut impl Effects,
    ) -> Result<(), CapabilityError> {
        let result = (|| {
            for buffer in self
                .entries
                .iter_mut()
                .flatten()
                .filter(|buffer| buffer.service == service && buffer.ready && !buffer.closed)
            {
                if buffer.provider.installed != 0 || buffer.candidate.is_some() {
                    return Err(CapabilityError::Fatal);
                }
                if buffer.client.owner == owner {
                    return Err(CapabilityError::PermissionDenied);
                }
                effects.participants(buffer.client.owner, owner)?;
                let address = if buffer.provider.owner == owner {
                    buffer.provider.address
                } else {
                    effects.reserve(owner, buffer.pages)?
                };
                buffer.candidate = Some(Mapping {
                    owner,
                    address,
                    installed: 0,
                });
                map(
                    buffer.candidate.as_mut().unwrap(),
                    buffer.physical,
                    buffer.pages,
                    effects,
                )?;
            }
            effects.receiver(service, owner, slot)
        })();
        if let Err(error) = result {
            let mut clean = true;
            for buffer in self
                .entries
                .iter_mut()
                .flatten()
                .filter(|buffer| buffer.service == service)
            {
                if let Some(candidate) = buffer.candidate.as_mut() {
                    if unmap(candidate, buffer.physical, effects).is_err() {
                        clean = false;
                    } else {
                        buffer.candidate = None;
                    }
                }
            }
            if !clean {
                return Err(CapabilityError::Fatal);
            }
            return Err(error);
        }
        for buffer in self
            .entries
            .iter_mut()
            .flatten()
            .filter(|buffer| buffer.service == service)
        {
            if let Some(candidate) = buffer.candidate.take() {
                buffer.provider = candidate;
            }
        }
        Ok(())
    }

    /// Caller must quiesce the Service before withdrawing an attached buffer.
    pub fn close(
        &mut self,
        id: SessionId,
        effects: &mut impl Effects,
    ) -> Result<(), CapabilityError> {
        let Some(buffer) = self.entries[index(id)?].as_mut() else {
            return Ok(());
        };
        if buffer.provider.installed != 0 || buffer.candidate.is_some() {
            return Err(CapabilityError::IllegalOperation);
        }
        if unmap(&mut buffer.client, buffer.physical, effects).is_err() {
            return Err(CapabilityError::Fatal);
        }
        buffer.closed = true;
        Ok(())
    }
}
