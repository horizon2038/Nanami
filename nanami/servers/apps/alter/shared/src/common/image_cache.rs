//! File snapshots with optional parsed ELF metadata, keyed by open-file identity.
use crate::elf::ElfMetadata;
use nanami_runtime::Word;

pub const ENTRIES: usize = 8;
// A maximum of 16 MiB; large one-off images use the existing reusable buffers.
pub const MAX_IMAGE_BYTES: Word = 2 * 1024 * 1024;

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

    pub fn replacement(&self) -> usize {
        self.entries
            .iter()
            .position(Option::is_none)
            .unwrap_or_else(|| {
                self.entries
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, image)| image.as_ref().unwrap().used)
                    .unwrap()
                    .0
            })
    }

    pub fn entry(&self, index: usize) -> Option<&CachedImage> {
        self.entries[index].as_ref()
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
