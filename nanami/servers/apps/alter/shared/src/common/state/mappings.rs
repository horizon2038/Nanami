use super::*;

impl Runtime {
    pub fn reset_stack_mapping(&mut self, pid: Word, base: Word, size: Word, prot: Word) -> bool {
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        let mut i = 0usize;
        while i < process.mappings.len() {
            let mapping = process.mappings[i];
            if mapping.base != 0 {
                let Some(mapping_end) = mapping.base.checked_add(mapping.size) else {
                    return false;
                };
                let Some(end) = base.checked_add(size) else {
                    return false;
                };
                if ranges_touch_or_overlap(base, end, mapping.base, mapping_end) {
                    process.mappings[i] = ProcessMapping::EMPTY;
                }
            }
            i += 1;
        }
        self.add_mapping(pid, base, size, prot)
    }

    pub fn add_mapping(&mut self, pid: Word, base: Word, size: Word, prot: Word) -> bool {
        if base == 0 || size == 0 {
            return false;
        }
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        let mut new_base = base;
        let Some(mut new_end) = base.checked_add(size) else {
            return false;
        };
        let mut i = 0usize;
        while i < process.mappings.len() {
            let mapping = process.mappings[i];
            if mapping.base != 0 && mapping.prot == prot {
                let Some(mapping_end) = mapping.base.checked_add(mapping.size) else {
                    return false;
                };
                if ranges_touch_or_overlap(new_base, new_end, mapping.base, mapping_end) {
                    new_base = ::core::cmp::min(new_base, mapping.base);
                    new_end = ::core::cmp::max(new_end, mapping_end);
                    process.mappings[i] = ProcessMapping::EMPTY;
                }
            }
            i += 1;
        }
        i = 0;
        while i < process.mappings.len() {
            let mapping = process.mappings[i];
            if mapping.base == 0 {
                process.mappings[i] = ProcessMapping {
                    base: new_base,
                    size: new_end - new_base,
                    prot,
                };
                return true;
            }
            i += 1;
        }
        false
    }

    pub fn remove_mapping(&mut self, pid: Word, base: Word, size: Word) -> bool {
        if base == 0 || size == 0 {
            return false;
        }
        let Some(end) = base.checked_add(size) else {
            return false;
        };
        if !self.has_mapping(pid, base, size) {
            return false;
        }
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
                        let chunk_end = ::core::cmp::min(mapping_end, end);
                        if !remove_mapping_fragment(process, cursor, chunk_end - cursor) {
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
        let mut cursor = base;
        let mut out = None;
        while cursor < end {
            let mut found = false;
            let mut i = 0usize;
            while i < process.mappings.len() {
                let mapping = process.mappings[i];
                if mapping.base != 0 {
                    let mapping_end = mapping.base.checked_add(mapping.size)?;
                    if mapping.base <= cursor && cursor < mapping_end {
                        if let Some(prot) = out {
                            if prot != mapping.prot {
                                return None;
                            }
                        } else {
                            out = Some(mapping.prot);
                        }
                        cursor = ::core::cmp::min(mapping_end, end);
                        found = true;
                        break;
                    }
                }
                i += 1;
            }
            if !found {
                return None;
            }
        }
        out
    }

    pub fn protect_mapping(&mut self, pid: Word, base: Word, size: Word, prot: Word) -> bool {
        if !self.has_mapping(pid, base, size) {
            return false;
        }
        self.remove_mapping(pid, base, size) && self.add_mapping(pid, base, size, prot)
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
                        cursor = ::core::cmp::min(mapping_end, end);
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
}

fn remove_mapping_fragment(process: &mut ManagedProcess, base: Word, size: Word) -> bool {
    if base == 0 || size == 0 {
        return false;
    }
    let Some(end) = base.checked_add(size) else {
        return false;
    };
    let mut i = 0usize;
    while i < process.mappings.len() {
        let mapping = process.mappings[i];
        if mapping.base != 0 {
            let Some(mapping_end) = mapping.base.checked_add(mapping.size) else {
                return false;
            };
            if mapping.base <= base && end <= mapping_end {
                let left_size = base - mapping.base;
                let right_size = mapping_end - end;
                if left_size != 0 && right_size != 0 && free_mapping_slots(process) == 0 {
                    return false;
                }
                process.mappings[i] = ProcessMapping::EMPTY;

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
                return true;
            }
        }
        i += 1;
    }
    false
}

fn free_mapping_slots(process: &ManagedProcess) -> usize {
    let mut count = 0usize;
    let mut i = 0usize;
    while i < process.mappings.len() {
        if process.mappings[i].base == 0 {
            count += 1;
        }
        i += 1;
    }
    count
}

fn insert_mapping_fragment(process: &mut ManagedProcess, mapping: ProcessMapping) -> bool {
    let mut i = 0usize;
    while i < process.mappings.len() {
        if process.mappings[i].base == 0 {
            process.mappings[i] = mapping;
            return true;
        }
        i += 1;
    }
    false
}

fn ranges_touch_or_overlap(a_start: Word, a_end: Word, b_start: Word, b_end: Word) -> bool {
    a_start <= b_end && b_start <= a_end
}
