//! File snapshots with optional parsed ELF metadata, keyed by open-file identity.
use crate::elf::ElfMetadata;
use nanami_runtime::Word;

pub const ENTRIES: usize = 48;
pub const MAX_RETAINED_BYTES: Word = 16 * 1024 * 1024;
// More small-file slots within the original byte budget; large images bypass it.
pub const MAX_IMAGE_BYTES: Word = 2 * 1024 * 1024;
const PAGE_BYTES: Word = 4096;
// Experimental prerequisite for readonly private mappings. Keep the ordinary
// in-place heap policy until the complete copy-reduction path is validated.
pub const IMMUTABLE_STORAGE: bool = match option_env!("NANAMI_IMMUTABLE_SNAPSHOTS") {
    Some(value) => value.len() == 1 && value.as_bytes()[0] == b'1',
    None => false,
};

#[derive(Clone, Copy)]
pub struct CachedImage {
    pub identity: Word,
    pub address: Word,
    pub size: Word,
    pub capacity: Word,
    pub metadata: Option<ElfMetadata>,
    used: u64,
}

pub struct ImageCache {
    entries: [Option<CachedImage>; ENTRIES],
    clock: u64,
}

impl ImageCache {
    pub const fn new() -> Self {
        Self {
            entries: [None; ENTRIES],
            clock: 0,
        }
    }

    fn tick(&mut self) -> u64 {
        if self.clock == u64::MAX {
            for entry in self.entries.iter_mut().flatten() {
                entry.used = 0;
            }
            self.clock = 0;
        }
        self.clock += 1;
        self.clock
    }

    pub fn get(&mut self, identity: Word, size: Word) -> Option<&CachedImage> {
        if identity >> 32 == 0 {
            return None;
        }
        let index = self.entries.iter().position(|entry| {
            entry
                .as_ref()
                .is_some_and(|image| image.identity == identity && image.size == size)
        })?;
        let used = self.tick();
        let entry = self.entries[index].as_mut()?;
        entry.used = used;
        Some(entry)
    }

    pub fn retained_bytes(&self) -> Word {
        self.entries
            .iter()
            .flatten()
            .map(|image| image.capacity)
            .sum()
    }

    pub fn immutable_range(&self, address: Word, bytes: Word) -> bool {
        IMMUTABLE_STORAGE
            && bytes != 0
            && self.entries.iter().flatten().any(|image| {
                image.identity != 0
                    && address >= image.address
                    && address
                        .checked_add(bytes)
                        .zip(image.address.checked_add(image.capacity))
                        .is_some_and(|(end, limit)| end <= limit)
            })
    }

    pub fn allocation_capacity(&self, index: usize, size: Word) -> Word {
        if IMMUTABLE_STORAGE {
            // A published snapshot is retired, never refilled in place. Its
            // reclaimable RAM object needs no geometric heap-growth reserve.
            return size.next_multiple_of(PAGE_BYTES);
        }
        let requested = match self.entry(index) {
            Some(previous) if previous.capacity >= size => return previous.capacity,
            Some(_) => size.next_power_of_two(),
            None => size,
        };
        requested.next_multiple_of(PAGE_BYTES)
    }

    /// Admission is optional. Keep outstanding main ELF bytes intact while
    /// loading PT_INTERP; otherwise a budget-constrained victim could be MRU.
    pub fn replacement(&self, size: Word, protected_address: Option<Word>) -> Option<usize> {
        if size == 0 || size > MAX_IMAGE_BYTES {
            return None;
        }
        let retained = self.retained_bytes();
        self.entries
            .iter()
            .enumerate()
            .filter(|(index, entry)| {
                if entry.is_some_and(|image| Some(image.address) == protected_address) {
                    return false;
                }
                let previous = entry.map_or(0, |image| image.capacity);
                retained - previous + self.allocation_capacity(*index, size) <= MAX_RETAINED_BYTES
            })
            .min_by_key(|(_, entry)| entry.map_or(0, |image| image.used))
            .map(|(index, _)| index)
    }

    pub fn entry(&self, index: usize) -> Option<&CachedImage> {
        self.entries[index].as_ref()
    }

    /// Retain ownership after a failed/partial release, but never serve a hit
    /// from a mapping whose publication or retirement has not completed.
    pub fn invalidate(&mut self, index: usize) {
        if let Some(entry) = self.entries[index].as_mut() {
            entry.identity = 0;
            entry.size = 0;
            entry.metadata = None;
        }
    }

    /// Only after successful release of the recorded allocation.
    pub fn remove(&mut self, index: usize) {
        self.entries[index] = None;
    }

    pub fn insert(
        &mut self,
        index: usize,
        identity: Word,
        address: Word,
        size: Word,
        capacity: Word,
        metadata: Option<ElfMetadata>,
    ) {
        let used = self.tick();
        self.entries[index] = Some(CachedImage {
            identity,
            address,
            size,
            capacity,
            metadata,
            used,
        });
    }
}
