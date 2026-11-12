use super::*;

impl Runtime {
    pub fn inherit_mappings(&mut self, parent_pid: Word, child_pid: Word) -> bool {
        let Some(source) = self
            .managed_process(parent_pid)
            .map(|parent| parent.mappings)
        else {
            return false;
        };
        let Some(child) = self.managed_process_mut(child_pid) else {
            return false;
        };
        child.mappings.initialize_from(source)
    }

    // Privileged Alpha copies may inspect retained PROT_NONE backing, but a
    // Linux copyin/copyout must obey guest permissions. Untracked ELF pages are
    // still validated by Alpha's frame lookup.
    pub fn memory_access_allowed(
        &self,
        pid: Word,
        base: Word,
        bytes: Word,
        permission: Word,
    ) -> bool {
        let Some(end) = base.checked_add(bytes) else {
            return false;
        };
        if bytes == 0 {
            return true;
        }
        let Some(process) = self.managed_process(pid) else {
            return false;
        };
        process.mappings.access_allowed(base, end, permission)
    }

    pub fn reset_stack_mapping(&mut self, pid: Word, base: Word, size: Word, prot: Word) -> bool {
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        let mut invalid = false;
        process.mappings.retain(|mapping| {
            if invalid { return true; }
            if mapping.base != 0 {
                let Some(mapping_end) = mapping.base.checked_add(mapping.size) else {
                    invalid = true;
                    return true;
                };
                let Some(end) = base.checked_add(size) else {
                    invalid = true;
                    return true;
                };
                if ranges_touch_or_overlap(base, end, mapping.base, mapping_end) {
                    return false;
                }
            }
            true
        });
        if invalid { return false; }
        self.add_mapping(pid, base, size, prot)
    }

    pub fn add_mapping(&mut self, pid: Word, base: Word, size: Word, prot: Word) -> bool {
        if base == 0 || size == 0 {
            return false;
        }
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        if !process.mappings.ensure() {
            return false;
        }
        let mut new_base = base;
        let Some(mut new_end) = base.checked_add(size) else {
            return false;
        };
        let mut invalid = false;
        process.mappings.retain(|mapping| {
            // An overflowing entry used to stop the walk after any earlier
            // removals. Keep that entry and all later entries unchanged.
            if invalid { return true; }
            if mapping.base != 0 && mapping.prot == prot {
                let Some(mapping_end) = mapping.base.checked_add(mapping.size) else {
                    invalid = true;
                    return true;
                };
                if ranges_touch_or_overlap(new_base, new_end, mapping.base, mapping_end) {
                    new_base = ::core::cmp::min(new_base, mapping.base);
                    new_end = ::core::cmp::max(new_end, mapping_end);
                    return false;
                }
            }
            true
        });
        if invalid { return false; }
        process.mappings.push(ProcessMapping {
            base: new_base,
            size: new_end - new_base,
            prot,
        })
    }

    pub fn remove_mapping(&mut self, pid: Word, base: Word, size: Word) -> bool {
        if base == 0 || size == 0 {
            return false;
        }
        let Some(end) = base.checked_add(size) else {
            return false;
        };
        if !self.has_mapping(pid, base, size) || !self.can_remove_mapping(pid, base, size) {
            return false;
        }
        self.remove_mapping_validated(pid, base, end)
    }

    // The caller has proved coverage and sufficient fragment capacity. Keep
    // this private: no dispatch or metadata mutation may separate that proof
    // from removal.
    fn remove_mapping_validated(&mut self, pid: Word, base: Word, end: Word) -> bool {
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };

        let mut cursor = base;
        while cursor < end {
            let mut found = false;
            let mut i = 0usize;
            while i < process.mappings.len() {
                let mapping = process.mappings[i];
                if mapping.base != 0 {
                    let Some(mapping_end) = mapping.base.checked_add(mapping.size) else {
                        return false;
                    };
                    if mapping.base <= cursor && cursor < mapping_end {
                        if cursor == base && mapping.base == base && mapping_end < end
                            && remove_whole_entries(process, base, end)
                        {
                            return true;
                        }
                        let chunk_end = ::core::cmp::min(mapping_end, end);
                        if !remove_mapping_fragment(process, i, cursor, chunk_end) {
                            return false;
                        }
                        cursor = chunk_end;
                        found = true;
                        break;
                    }
                }
                i += 1;
            }
            if !found {
                return false;
            }
        }
        true
    }

    pub fn mapping_at(&self, pid: Word, address: Word) -> Option<ProcessMapping> {
        self.managed_process(pid)?
            .mappings
            .iter()
            .copied()
            .find(|mapping| {
                mapping.base != 0
                    && address >= mapping.base
                    && address - mapping.base < mapping.size
            })
    }

    pub fn next_mapping(&self, pid: Word, address: Word, end: Word) -> Option<ProcessMapping> {
        self.managed_process(pid)?
            .mappings
            .iter()
            .copied()
            .filter(|mapping| {
                mapping.base != 0
                    && mapping.base < end
                    && mapping.base.saturating_add(mapping.size) > address
            })
            .min_by_key(|mapping| mapping.base)
    }

    pub fn mapping_prot(&self, pid: Word, base: Word, size: Word) -> Option<Word> {
        if base == 0 || size == 0 {
            return None;
        }
        let end = base.checked_add(size)?;
        let process = self.managed_process(pid)?;
        let mut cursor = CoverageCursor::new(&process.mappings, base, end);
        let mut out = None;
        while let Some((_, mapping)) = cursor.next_chunk().ok()? {
            if let Some(prot) = out {
                if prot != mapping.prot {
                    return None;
                }
            } else {
                out = Some(mapping.prot);
            }
        }
        out
    }

    pub fn protect_mapping(&mut self, pid: Word, base: Word, size: Word, prot: Word) -> bool {
        let Some(needed) = self.mapping_protection_needed(pid, base, size, prot) else { return false; };
        if !needed { return true; }
        // This preflight proves coverage and reserves one more
        // slot than removal requires. Do not repeat both proofs in remove_mapping.
        self.remove_mapping_validated(pid, base, base + size)
            && self.add_mapping(pid, base, size, prot)
    }

    /// One coverage walk proves existence and compares the selected rights.
    /// None rejects the range/capacity; false preserves the no-change fast path.
    /// No proof survives this call: publication validates current metadata again.
    pub fn mapping_protection_needed(&self, pid: Word, base: Word, size: Word, prot: Word) -> Option<bool> {
        if base == 0 || size == 0 { return None; }
        let end = base.checked_add(size)?;
        let process = self.managed_process(pid)?;
        let mut cursor = CoverageCursor::new(&process.mappings, base, end);
        let mut unchanged = true;
        while let Some((_, mapping)) = cursor.next_chunk().ok()? {
            unchanged &= mapping.prot == prot;
        }
        // As before, unchanged ranges need no replacement slot and do not
        // inspect unrelated metadata after the complete coverage proof.
        if unchanged { return Some(false); }
        protection_capacity(process, base, end).then_some(true)
    }

    pub fn can_remove_mapping(&self, pid: Word, base: Word, size: Word) -> bool {
        let Some(end) = base.checked_add(size) else {
            return false;
        };
        let Some(process) = self.managed_process(pid) else {
            return false;
        };
        let mut required = 0;
        for mapping in process.mappings.iter().filter(|mapping| mapping.base != 0) {
            let Some(mapping_end) = mapping.base.checked_add(mapping.size) else {
                return false;
            };
            if mapping_end <= base || mapping.base >= end {
                required += 1;
            } else {
                required += usize::from(mapping.base < base);
                required += usize::from(mapping_end > end);
            }
        }
        required <= process.mappings.capacity()
    }

    /// Reserve enough metadata space before changing hardware permissions.
    pub fn can_protect_mapping(&self, pid: Word, base: Word, size: Word) -> bool {
        if !self.has_mapping(pid, base, size) {
            return false;
        }
        let Some(end) = base.checked_add(size) else {
            return false;
        };
        let Some(process) = self.managed_process(pid) else {
            return false;
        };
        protection_capacity(process, base, end)
    }

    pub fn has_mapping(&self, pid: Word, base: Word, size: Word) -> bool {
        if base == 0 || size == 0 {
            return false;
        }
        let Some(end) = base.checked_add(size) else {
            return false;
        };
        let Some(process) = self.managed_process(pid) else {
            return false;
        };
        let mut cursor = CoverageCursor::new(&process.mappings, base, end);
        loop {
            match cursor.next_chunk() {
                Ok(Some(_)) => {}
                Ok(None) => return true,
                Err(()) => return false,
            }
        }
    }
}

fn protection_capacity(process: &ManagedProcess, base: Word, end: Word) -> bool {
    let mut required = 1; // The replacement protection interval.
    for mapping in process.mappings.iter().filter(|mapping| mapping.base != 0) {
        let Some(mapping_end) = mapping.base.checked_add(mapping.size) else { return false; };
        if mapping_end <= base || mapping.base >= end { required += 1; }
        else {
            required += usize::from(mapping.base < base);
            required += usize::from(mapping_end > end);
        }
    }
    required <= process.mappings.capacity()
}

#[cfg(test)]
static COVERAGE_VISITS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
pub(super) fn take_coverage_visits() -> usize {
    COVERAGE_VISITS.swap(0, core::sync::atomic::Ordering::Relaxed)
}

/// Request-local elimination only: never reorder overlapping legacy entries.
/// Once an entry ends at/before cursor it cannot cover any later cursor.
struct CoverageCursor<'a> {
    mappings: &'a [ProcessMapping],
    cursor: Word,
    end: Word,
    candidates: Option<[Word; ALTER_PROCESS_MAPPING_MAX.div_ceil(Word::BITS as usize)]>,
}

impl<'a> CoverageCursor<'a> {
    fn new(mappings: &'a [ProcessMapping], cursor: Word, end: Word) -> Self {
        Self { mappings, cursor, end, candidates: None }
    }

    fn next_chunk(&mut self) -> Result<Option<(usize, ProcessMapping)>, ()> {
        if self.cursor >= self.end { return Ok(None); }
        if self.candidates.is_none() {
            // The common one-VMA query keeps the original linear first-match
            // walk and never initializes a bitmap. Only continued coverage
            // needs request-local elimination state.
            for (index, &mapping) in self.mappings.iter().enumerate() {
                #[cfg(test)]
                COVERAGE_VISITS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if mapping.base == 0 { continue; }
                let end = mapping.base.checked_add(mapping.size).ok_or(())?;
                if mapping.base <= self.cursor && self.cursor < end {
                    self.cursor = end.min(self.end);
                    if self.cursor < self.end {
                        let mut candidates = [Word::MAX; ALTER_PROCESS_MAPPING_MAX.div_ceil(Word::BITS as usize)];
                        candidates[index / Word::BITS as usize] &= !(1usize << (index % Word::BITS as usize));
                        self.candidates = Some(candidates);
                    }
                    return Ok(Some((index, mapping)));
                }
            }
            return Err(());
        }
        let candidates = self.candidates.as_mut().unwrap();
        for word in 0..self.mappings.len().div_ceil(Word::BITS as usize) {
            let mut pending = candidates[word];
            while pending != 0 {
                let bit = pending.trailing_zeros() as usize;
                let mask = 1usize << bit;
                pending &= !mask;
                let index = word * Word::BITS as usize + bit;
                let Some(&mapping) = self.mappings.get(index) else { break; };
                #[cfg(test)]
                COVERAGE_VISITS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if mapping.base == 0 {
                    candidates[word] &= !mask;
                    continue;
                }
                let end = mapping.base.checked_add(mapping.size).ok_or(())?;
                if end <= self.cursor {
                    candidates[word] &= !mask;
                } else if mapping.base <= self.cursor {
                    self.cursor = end.min(self.end);
                    candidates[word] &= !mask;
                    return Ok(Some((index, mapping)));
                }
            }
        }
        Err(())
    }
}

fn remove_whole_entries(process: &mut ManagedProcess, base: Word, end: Word) -> bool {
    let mut selected = [0usize; ALTER_PROCESS_MAPPING_MAX.div_ceil(Word::BITS as usize)];
    let mut count = 0;
    let mut cursor = CoverageCursor::new(&process.mappings, base, end);
    loop {
        let start = cursor.cursor;
        match cursor.next_chunk() {
            Ok(Some((index, mapping))) => {
                // Partial boundaries keep their original ordered split path.
                // No metadata has changed if this plan cannot be completed.
                if mapping.base != start || mapping.base + mapping.size > end {
                    return false;
                }
                selected[index / Word::BITS as usize] |= 1usize << (index % Word::BITS as usize);
                count += 1;
            }
            Ok(None) => break,
            Err(()) => return false,
        }
    }
    if count < 3 { return false; }
    // The coverage cursor selected only whole entries, in the same first-
    // covering order as removal. No appended boundary fragment can alter a
    // later selection. Stable retain also leaves unselected overlaps in place.
    let mut index = 0;
    process.mappings.retain(|_| {
        let keep = selected[index / Word::BITS as usize] & (1usize << (index % Word::BITS as usize)) == 0;
        index += 1;
        keep
    });
    true
}

fn remove_mapping_fragment(process: &mut ManagedProcess, i: usize, base: Word, end: Word) -> bool {
    // The coverage walk has just selected this exact entry and checked its end.
    // Re-scanning from entry zero used to repeat the same search for every chunk.
    let mapping = process.mappings[i];
    let mapping_end = mapping.base + mapping.size;
    let left_size = base - mapping.base;
    let right_size = mapping_end - end;
    if left_size != 0 && right_size != 0 && free_mapping_slots(process) == 0 {
        return false;
    }
    process.mappings.remove(i);

    if left_size != 0
        && !insert_mapping_fragment(
            process,
            ProcessMapping {
                base: mapping.base,
                size: left_size,
                prot: mapping.prot,
            },
        )
    {
        return false;
    }
    if right_size != 0
        && !insert_mapping_fragment(
            process,
            ProcessMapping {
                base: end,
                size: right_size,
                prot: mapping.prot,
            },
        )
    {
        return false;
    }
    true
}

fn free_mapping_slots(process: &ManagedProcess) -> usize {
    process.mappings.capacity() - process.mappings.len()
}

fn insert_mapping_fragment(process: &mut ManagedProcess, mapping: ProcessMapping) -> bool {
    process.mappings.push(mapping)
}

fn ranges_touch_or_overlap(a_start: Word, a_end: Word, b_start: Word, b_end: Word) -> bool {
    a_start <= b_end && b_start <= a_end
}
