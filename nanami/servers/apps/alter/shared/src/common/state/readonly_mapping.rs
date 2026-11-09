//! One immutable file extent, valid only while its VMA generation is unchanged.
use super::Word;

#[derive(Clone, Copy)]
pub struct ReadonlyMapping {
    pub generation: Word,
    identity: Word,
    file_size: Word,
    base: Word,
    size: Word,
    offset: Word,
}

impl ReadonlyMapping {
    pub const EMPTY: Self = Self {
        generation: 0, identity: 0, file_size: 0, base: 0, size: 0, offset: 0,
    };

    pub fn new(generation: Word, identity: Word, file_size: Word,
        base: Word, size: Word, offset: Word) -> Self
    {
        if generation == 0 || identity >> 32 == 0 || base == 0 || size == 0
            || base.checked_add(size).is_none() || offset.checked_add(size).is_none()
        {
            return Self::EMPTY;
        }
        Self { generation, identity, file_size, base, size, offset }
    }

    pub fn covers(self, generation: Word, identity: Word, file_size: Word,
        base: Word, size: Word, offset: Word) -> bool
    {
        self.covers_range(generation, base, size)
            && self.identity == identity && self.file_size == file_size
            && self.offset.checked_add(base - self.base) == Some(offset)
    }

    /// Backing provenance only. Writers still need ordinary VM quiescence and
    /// Alpha detachment; this does not grant permissions or reuse file content.
    pub fn covers_range(self, generation: Word, base: Word, size: Word) -> bool {
        self.generation != 0 && self.generation == generation
            && size != 0 && base >= self.base
            && base.checked_add(size).is_some_and(|end| end <= self.base + self.size)
    }

    /// Page-aligned callers can skip a sharing query only in the aliased file
    /// prefix. An entirely zero-filled suffix keeps the ordinary query path.
    pub fn covers_file_page(self, generation: Word, base: Word, size: Word) -> bool {
        self.covers_range(generation, base, size)
            && self.offset.checked_add(base - self.base)
                .is_some_and(|offset| offset < self.file_size)
    }
}
