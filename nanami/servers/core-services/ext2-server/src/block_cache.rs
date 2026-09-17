//! Bounded, indexed block cache. Dirty entries are never evicted or replayed
//! after an uncertain device error; successful barriers make them evictable.
use alloc::{collections::BTreeMap, vec, vec::Vec};
use libnanami::{RequestError, Word};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BlockKind {
    Data,
    Metadata,
}

#[derive(Clone, Copy)]
struct Entry {
    block: Option<usize>,
    dirty: Option<BlockKind>,
    referenced: bool,
}

pub(super) struct BlockCache {
    index: BTreeMap<usize, usize>,
    entries: Vec<Entry>,
    data: Vec<u8>,
    scratch_backup: Vec<u8>,
    block_size: usize,
    hand: usize,
    dirty_count: usize,
    error: Option<RequestError>,
}

impl BlockCache {
    pub(super) fn new(block_size: usize, budget: usize, transfer_bytes: usize) -> Self {
        assert!(block_size != 0 && budget >= transfer_bytes && transfer_bytes >= block_size);
        let capacity = budget / block_size;
        Self {
            index: BTreeMap::new(),
            entries: vec![
                Entry {
                    block: None,
                    dirty: None,
                    referenced: false
                };
                capacity
            ],
            data: vec![0; capacity * block_size],
            scratch_backup: vec![0; transfer_bytes],
            block_size,
            hand: 0,
            dirty_count: 0,
            error: None,
        }
    }

    pub(super) fn check_writable(&self) -> Result<(), RequestError> {
        self.error.map_or(Ok(()), Err)
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.dirty_count != 0
    }

    pub(super) fn contains(&self, block: usize) -> bool {
        self.index.contains_key(&block)
    }

    pub(super) fn load(&mut self, block: usize, target: &mut [u8]) -> bool {
        let Some(&slot) = self.index.get(&block) else {
            return false;
        };
        self.entries[slot].referenced = true;
        target.copy_from_slice(&self.data[slot * self.block_size..(slot + 1) * self.block_size]);
        true
    }

    pub(super) fn has_room_for(&self, block: usize, count: usize) -> bool {
        let additional = (block..block + count)
            .filter(|block| {
                self.index
                    .get(block)
                    .is_none_or(|&slot| self.entries[slot].dirty.is_none())
            })
            .count();
        additional <= self.entries.len() - self.dirty_count
    }

    // CLOCK second-chance eviction. Reads can bypass a completely dirty cache;
    // writes reserve room (with writeback if necessary) before calling store.
    pub(super) fn store(&mut self, block: usize, source: &[u8], dirty: Option<BlockKind>) {
        let slot = if let Some(&slot) = self.index.get(&block) {
            // An uncached read must never replace a pending write.
            if dirty.is_none() && self.entries[slot].dirty.is_some() {
                return;
            }
            slot
        } else {
            if self.dirty_count == self.entries.len() {
                assert!(dirty.is_none());
                return;
            }
            let mut chosen = None;
            for _ in 0..self.entries.len() * 2 {
                let slot = self.hand;
                self.hand = (self.hand + 1) % self.entries.len();
                let entry = &mut self.entries[slot];
                if entry.dirty.is_some() {
                    continue;
                }
                if entry.referenced {
                    entry.referenced = false;
                } else {
                    chosen = Some(slot);
                    break;
                }
            }
            let Some(slot) = chosen else {
                assert!(dirty.is_none());
                return;
            };
            if let Some(old) = self.entries[slot].block {
                self.index.remove(&old);
            }
            self.index.insert(block, slot);
            slot
        };
        if dirty.is_some() && self.entries[slot].dirty.is_none() {
            self.dirty_count += 1;
        }
        self.entries[slot] = Entry {
            block: Some(block),
            dirty,
            referenced: true,
        };
        self.data[slot * self.block_size..(slot + 1) * self.block_size].copy_from_slice(source);
    }

    pub(super) fn flush(&mut self, port: Word, scratch: &mut [u8]) -> Result<(), RequestError> {
        self.check_writable()?;
        if !self.is_dirty() {
            return Ok(());
        }
        // Pressure writeback may interrupt an allocation with staged metadata
        // or payload in the shared buffer. Preserve it even on failure.
        self.scratch_backup.copy_from_slice(scratch);
        let result = self.flush_dirty(port, scratch);
        scratch.copy_from_slice(&self.scratch_backup);
        match result {
            Ok(()) => {
                for entry in &mut self.entries {
                    entry.dirty = None;
                }
                self.dirty_count = 0;
            }
            Err(error) => self.error = Some(error),
        }
        result
    }

    fn flush_dirty(&self, port: Word, scratch: &mut [u8]) -> Result<(), RequestError> {
        let max_blocks = scratch.len() / self.block_size;
        // Persist data before publishing metadata. This is ordered writeback,
        // not a journal: interrupted metadata updates can still require fsck.
        for kind in [BlockKind::Data, BlockKind::Metadata] {
            let mut first = 0;
            let mut count = 0;
            let mut wrote = false;
            for (&block, &slot) in &self.index {
                if self.entries[slot].dirty != Some(kind) {
                    continue;
                }
                if count != 0 && (block != first + count || count == max_blocks) {
                    self.write_run(port, first, count)?;
                    count = 0;
                }
                if count == 0 {
                    first = block;
                }
                scratch[count * self.block_size..(count + 1) * self.block_size].copy_from_slice(
                    &self.data[slot * self.block_size..(slot + 1) * self.block_size],
                );
                count += 1;
                wrote = true;
            }
            if count != 0 {
                self.write_run(port, first, count)?;
            }
            if wrote {
                nanami_services::block::block_device_flush(port)?;
            }
        }
        Ok(())
    }

    fn write_run(&self, port: Word, block: usize, count: usize) -> Result<(), RequestError> {
        let written = nanami_services::block::block_device_write(port, block, count, 0)?;
        if written == count * self.block_size {
            Ok(())
        } else {
            Err(RequestError::Protocol)
        }
    }
}
