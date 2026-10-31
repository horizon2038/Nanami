//! Bounded content identities, independent of timestamps and pathname aliases.
//! Eviction loses a hit, never freshness: every insertion gets a new token.
const ENTRIES: usize = 256;

pub(super) struct Versions {
    entries: [Option<(u32, u32)>; ENTRIES],
    next: usize,
    serial: u32,
    used: usize,
}

impl Versions {
    pub(super) const fn new() -> Self {
        Self {
            entries: [None; ENTRIES],
            next: 0,
            serial: 0,
            used: 0,
        }
    }

    pub(super) fn get(&mut self, inode: u32) -> u32 {
        if let Some((_, version)) = self.entries[..self.used]
            .iter()
            .flatten()
            .find(|(key, _)| *key == inode)
        {
            return *version;
        }
        // Zero disables caching. Never wrap and accidentally validate old data.
        let Some(version) = self.serial.checked_add(1) else {
            return 0;
        };
        self.serial = version;
        let index = self.entries[..self.used]
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.next);
        self.entries[index] = Some((inode, version));
        self.used = self.used.max(index + 1);
        self.next = (index + 1) % ENTRIES;
        version
    }

    pub(super) fn invalidate(&mut self, inode: u32) {
        for entry in &mut self.entries[..self.used] {
            if entry.is_some_and(|(key, _)| key == inode) {
                *entry = None;
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_eviction_and_inode_reuse_never_reuse_a_token() {
        let mut versions = Versions::new();
        let first = versions.get(1);
        assert_ne!(first, 0);
        assert_eq!(versions.get(1), first);
        versions.invalidate(1);
        let changed = versions.get(1);
        assert_ne!(changed, first);
        for inode in 2..=(ENTRIES as u32 + 2) {
            versions.get(inode);
        }
        assert_ne!(versions.get(1), changed);
    }

    #[test]
    fn exhaustion_disables_new_cache_identities_without_wrapping() {
        let mut versions = Versions::new();
        versions.serial = u32::MAX - 1;
        assert_eq!(versions.get(1), u32::MAX);
        assert_eq!(versions.get(2), 0);
        versions.invalidate(1);
        assert_eq!(versions.get(1), 0);
    }
}
