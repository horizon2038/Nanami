//! Read-only POSIX session identity, not a cache of authorization decisions.
//! The current protocol has no credential mutation. Clear on same-slot session
//! rebinding, or before adding any credential-changing operation to its owner.
use super::Word;

#[derive(Clone, Copy)]
pub enum IdentityField {
    RealUser,
    EffectiveUser,
    RealGroup,
    EffectiveGroup,
}

pub struct IdentityCache {
    port: Word,
    values: [Word; 4],
    known: u8,
}

impl IdentityCache {
    pub const fn new() -> Self {
        Self {
            port: 0,
            values: [0; 4],
            known: 0,
        }
    }

    pub fn clear(&mut self) {
        self.known = 0;
    }

    pub fn get_or_query<E>(
        &mut self,
        port: Word,
        field: IdentityField,
        query: impl FnOnce() -> Result<Word, E>,
    ) -> Result<Word, E> {
        if self.port != port {
            self.clear();
            self.port = port;
        }
        let index = field as usize;
        let bit = 1u8 << index;
        if self.known & bit != 0 {
            return Ok(self.values[index]);
        }
        let value = query()?;
        self.values[index] = value;
        self.known |= bit;
        Ok(value)
    }
}
