//! A private four-level Gen8+ PPGTT. Only owned surfaces are mapped; no client
//! command or GPU pointer can escape to firmware mappings in the global GGTT.
extern crate alloc;
use alloc::vec::Vec;
use nanami_runtime::{RequestError, Word};
const PAGE: Word = 4096;
const MAX_TABLES: usize = 256;
struct Leaf {
    region: Word,
    local: Word,
}
pub struct PageTables {
    root: Word,
    directories: Word,
    leaves: Vec<Leaf>,
    pat: u64,
}
fn put(base: Word, index: Word, value: u64) {
    unsafe {
        core::ptr::write_volatile((base as *mut u64).add(index), value);
    }
}
pub fn dma(bytes: Word) -> Result<(Word, Word), RequestError> {
    let (physical, local) = nanami_runtime::request_dma(bytes)?;
    if physical & 4095 != 0
        || local & 4095 != 0
        || local == 0
        || physical
            .checked_add(bytes)
            .is_none_or(|end| end > (1usize << 39))
    {
        return Err(RequestError::Unsupported);
    }
    unsafe {
        core::ptr::write_bytes(local as *mut u8, 0, bytes);
    }
    Ok((physical, local))
}
impl PageTables {
    pub fn new(pat: u64) -> Result<Self, RequestError> {
        let (physical, local) = dma(6 * PAGE)?;
        put(local, 0, (physical + PAGE) as u64 | 3);
        for index in 0..4 {
            put(
                local + PAGE,
                index,
                (physical + (2 + index) * PAGE) as u64 | 3,
            );
        }
        crate::cache::flush(local, 6 * PAGE);
        Ok(Self {
            root: physical,
            directories: local + 2 * PAGE,
            leaves: Vec::new(),
            pat,
        })
    }
    pub fn root(&self) -> Word {
        self.root
    }
    pub fn map(&mut self, gpu: Word, physical: Word, bytes: Word) -> Result<(), RequestError> {
        if gpu & 4095 != 0
            || physical & 4095 != 0
            || bytes == 0
            || bytes & 4095 != 0
            || gpu
                .checked_add(bytes)
                .is_none_or(|end| end > (1usize << 32))
            || physical
                .checked_add(bytes)
                .is_none_or(|end| end > (1usize << 39))
        {
            return Err(RequestError::InvalidArgument);
        }
        // Allocate every required table before publishing any leaf mapping.
        for region in (gpu >> 21)..=((gpu + bytes - 1) >> 21) {
            if self.leaves.iter().any(|leaf| leaf.region == region) {
                continue;
            }
            if self.leaves.len() == MAX_TABLES {
                return Err(RequestError::Unsupported);
            }
            self.leaves
                .try_reserve(1)
                .map_err(|_| RequestError::Unsupported)?;
            let (physical, local) = dma(PAGE)?;
            crate::cache::flush(local, PAGE);
            let directory = self.directories + (region / 512) * PAGE;
            put(directory, region % 512, physical as u64 | 3);
            crate::cache::flush(directory + (region % 512) * 8, 8);
            self.leaves.push(Leaf { region, local });
        }
        for offset in (0..bytes).step_by(PAGE) {
            let address = gpu + offset;
            let leaf = self
                .leaves
                .iter()
                .find(|leaf| leaf.region == address >> 21)
                .unwrap();
            let index = (address >> 12) & 511;
            let previous =
                unsafe { core::ptr::read_volatile((leaf.local as *const u64).add(index)) };
            if previous != 0 {
                return Err(RequestError::Protocol);
            }
        }
        for offset in (0..bytes).step_by(PAGE) {
            let address = gpu + offset;
            let leaf = self
                .leaves
                .iter()
                .find(|leaf| leaf.region == address >> 21)
                .unwrap();
            let index = (address >> 12) & 511;
            put(leaf.local, index, (physical + offset) as u64 | self.pat | 3);
            crate::cache::flush(leaf.local + index * 8, 8);
        }
        Ok(())
    }
    pub fn unmap(&mut self, gpu: Word, bytes: Word) {
        for offset in (0..bytes).step_by(PAGE) {
            let address = gpu + offset;
            if let Some(leaf) = self.leaves.iter().find(|leaf| leaf.region == address >> 21) {
                let index = (address >> 12) & 511;
                put(leaf.local, index, 0);
                crate::cache::flush(leaf.local + index * 8, 8);
            }
        }
    }
}
