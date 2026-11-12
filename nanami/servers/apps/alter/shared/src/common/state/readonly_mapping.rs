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
    shared_file: bool,
    deferred_snapshot: bool,
    vm_group: Word,
}

impl ReadonlyMapping {
    pub fn in_vm(mut self, vm_group: Word) -> Self {
        self.vm_group = vm_group;
        self
    }

    /// A checked write in another known VM cannot change this private readonly
    /// extent, even at the same VA. A stale retained generation stays inert:
    /// reuse still requires the owner's current, never-reused generation.
    pub fn checked_write_preserves(self, vm_group: Word, generation: Word, base: Word, size: Word) -> bool {
        if self.generation == 0 || generation == 0 || vm_group == 0 || base.checked_add(size).is_none() {
            return false;
        }
        if self.vm_group != 0 && vm_group != 0 && self.vm_group != vm_group {
            return true;
        }
        self.disjoint_write(generation, base, size)
    }
    /// Checked writes to disjoint VAs in this same VM cannot alter this private
    /// immutable extent. Protection/remap changes still invalidate generation.
    pub fn disjoint_write(self, generation: Word, base: Word, size: Word) -> bool {
        self.generation != 0 && self.generation == generation
            && base.checked_add(size).is_some_and(|end| {
                end <= self.base || base >= self.base + self.size
            })
    }
    pub const EMPTY: Self = Self {
        generation: 0, identity: 0, file_size: 0, base: 0, size: 0, offset: 0, shared_file: false, deferred_snapshot: false, vm_group: 0,
    };

    pub fn new(generation: Word, identity: Word, file_size: Word,
        base: Word, size: Word, offset: Word) -> Self
    {
        if generation == 0 || identity >> 32 == 0 || base == 0 || size == 0
            || base.checked_add(size).is_none() || offset.checked_add(size).is_none()
        {
            return Self::EMPTY;
        }
        Self { generation, identity, file_size, base, size, offset, shared_file: true, deferred_snapshot: false, vm_group: 0 }
    }

    /// Fresh private copies are immutable under R/RX just like aliases, but
    /// must not be mistaken for shared snapshot pages during WRITE upgrades.
    pub fn copied(generation: Word, identity: Word, file_size: Word,
        base: Word, size: Word, offset: Word) -> Self
    {
        let mut origin = Self::new(generation, identity, file_size, base, size, offset);
        origin.shared_file = false;
        origin
    }

    pub fn shared_file(self) -> bool { self.shared_file }
    pub fn with_deferred_snapshot(mut self) -> Self { self.deferred_snapshot = true; self }
    pub fn deferred_snapshot(self) -> bool { self.generation != 0 && self.deferred_snapshot }

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
        self.shared_file && self.covers_range(generation, base, size)
            && self.offset.checked_add(base - self.base)
                .is_some_and(|offset| offset < self.file_size)
    }
}
