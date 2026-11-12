//! First-fit reservation without restarting after every overlapping VMA.
use super::{Word, LINUX_PAGE_SIZE};

/// Prefer an unused aligned interval for a large private reservation. History
/// is only a starting hint: scan every current VMA and retain ordinary first
/// fit when the preferred region/alignment cannot satisfy the request.
pub(super) fn private_first_fit(
    mappings: impl Iterator<Item = (Word, Word)> + Clone,
    base: Word,
    bytes: Word,
    limit: Word,
    cursor: Word,
) -> Option<Word> {
    const LARGE: Word = 1 << 21;
    if cfg!(target_arch = "x86_64") && bytes >= LARGE {
        if let Some(preferred) = base.max(cursor).checked_add(LARGE - 1)
            .map(|base| base & !(LARGE - 1)) {
            if let Some(found) = scan(mappings.clone(), preferred, bytes, limit, LARGE) {
                return Some(found);
            }
        }
    }
    first_fit(mappings, base, bytes, limit)
}

pub(super) fn first_fit(
    mappings: impl Iterator<Item = (Word, Word)> + Clone,
    base: Word,
    bytes: Word,
    limit: Word,
) -> Option<Word> {
    scan(mappings, base, bytes, limit, LINUX_PAGE_SIZE)
}

fn scan(
    mappings: impl Iterator<Item = (Word, Word)> + Clone,
    mut base: Word,
    bytes: Word,
    limit: Word,
    alignment: Word,
) -> Option<Word> {
    let mut end = base.checked_add(bytes).filter(|end| *end <= limit)?;
    let mut walk = mappings.clone();
    let mut earlier_future = Word::MAX;
    while let Some((start, size)) = walk.next() {
        if start == 0 { continue; }
        let stop = start.checked_add(size)?;
        if base < stop && start < end {
            // Every start before this overlapping VMA's end is unsuitable.
            base = stop.checked_add(alignment - 1)? & !(alignment - 1);
            end = base.checked_add(bytes).filter(|end| *end <= limit)?;
            // Only a previously visited future VMA can become a new conflict.
            // In address order there are none: consume the whole list once.
            if end > earlier_future {
                walk = mappings.clone();
                earlier_future = Word::MAX;
            }
        } else if start >= end {
            earlier_future = earlier_future.min(start);
        }
    }
    Some(base)
}
