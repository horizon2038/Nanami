//! Private 2-MiB RAM. Slice keys are INTERNAL identities, never capabilities.
//! Their encoded depth exceeds WORD_BITS, so even slice zero cannot reach a
//! kernel object accidentally. Native capability exports still require real
//! 4-KiB Frames. Only this module resolves a slice to a large temporary alias.
use super::*;

pub(crate) const BITS: usize = 21;
pub(crate) const BYTES: usize = 1 << BITS;
pub(crate) const PAGES: usize = BYTES / PAGE_SIZE;
const SLICE: usize = 1 << (usize::BITS - 1);
const TEMP: usize = 0x6a00_0000;

pub(super) fn is_slice_key(key: usize) -> bool { key & SLICE != 0 }

#[derive(Clone, Copy)]
pub(crate) struct PrivateLargeAllocation { pub first: usize, pub large: bool }

#[derive(Clone, Copy, PartialEq, Eq)]
enum MappingState { Unmapped, Huge, Uncertain, Small }

#[derive(Clone, Copy)]
pub(crate) struct PrivateLargeMapping {
    pub first: usize,
    pub base: usize,
    pub permissions: usize,
    pub small: bool,
    pub quarantined: bool,
}

#[derive(Clone, Copy)]
struct Mapping { space: usize, base: usize, permissions: usize, state: MappingState, table: Option<usize> }

#[derive(Clone, Copy)]
struct Record {
    first: usize,
    owner: directory::Slot,
    huge: bool,
    failed: bool,
    quarantined: bool,
    mapping: Option<Mapping>,
}

impl Record {
    fn stable_large_mapping(&self, mapping: Mapping) -> bool {
        self.huge && !self.failed && !self.quarantined && mapping.table.is_none()
            && ((mapping.state == MappingState::Huge && mapping.permissions != 0)
                || (mapping.state == MappingState::Unmapped && mapping.permissions == 0))
    }
}

#[derive(Clone, Copy)]
struct Alias { frame: usize, ready: bool }

pub(super) struct PrivateLarge {
    // Node identity makes hot slice lookup logarithmic, independently of RAM.
    records: BTreeMap<usize, Record>,
    address_space: usize,
    aliases: [Option<Alias>; 2],
    supported: bool,
}

impl PrivateLarge {
    pub(super) fn new(address_space: usize) -> Self {
        Self { records: BTreeMap::new(), address_space, aliases: [None; 2],
            supported: cfg!(target_arch = "x86_64") }
    }
    pub(super) fn contains_node(&self, node: usize) -> bool { self.records.contains_key(&node) }
    pub(super) fn retire_space(&mut self, space: usize) {
        self.records.retain(|_, record| {
            if record.mapping.is_some_and(|mapping| mapping.space == space) { record.mapping = None; }
            record.huge || record.mapping.is_some()
        });
    }
}

impl MemoryManager {
    pub(crate) fn has_private_large_mappings(&self) -> bool { !self.private_large.records.is_empty() }

    /// A retained 4K leaf table cannot be replaced by a huge leaf. This is a
    /// normal fallback after earlier small mappings, even if all are gone.
    pub(crate) fn private_large_virtual_available(&self, space: usize, base: usize)
        -> Result<bool, CapabilityError> {
        if !self.private_large.supported { return Ok(false); }
        Ok(arch::address_space::get_unset_depth(space, base, PAGE_BITS)? != 0)
    }

    pub(super) fn complete_unmapped_private_large(&mut self, node: usize) -> Result<bool, CapabilityError> {
        let Some(record) = self.private_large.records.get(&node).copied() else { return Ok(false); };
        if record.mapping.is_some() { return Err(CapabilityError::IllegalOperation); }
        self.demote_private_large_pool(record.first)?;
        Ok(true)
    }
    pub(super) fn private_large_frame_key(&self, range: FrameRange, page: usize) -> Option<usize> {
        let record = self.private_large.records.get(&range.node()?)?;
        (record.huge && !record.failed).then(|| range.logical_descriptor(page).map(|key| key | SLICE)).flatten()
    }

    /// Reserve still-untyped, aligned RAM. Every error after reservation keeps
    /// a quarantined physical range; absence of authority alone is a fallback.
    pub(crate) fn allocate_private_large_frame(&mut self)
        -> Result<Option<PrivateLargeAllocation>, CapabilityError>
    {
        if !self.private_large.supported { return Ok(None); }
        self.ensure_metadata_capacity()?;
        let mut selected = None;
        for (&(index, start), source) in &self.physical_sources {
            if self.initial_generics[index].is_device() { continue; }
            match self.physical_allocator.as_mut().ok_or(CapabilityError::InvalidArgument)?
                .allocate_aligned_in_range(start, source.end, BYTES) {
                Ok(allocation) => { selected = Some((index, allocation.base_page)); break; }
                Err(PhysicalAllocError::OutOfMemory) => {}
                Err(error) => return Err(map_physical_alloc_error(error)),
            }
        }
        let Some((index, first)) = selected else { return Ok(None); };
        self.construct_private_large_frame(index, first).map(Some)
    }

    /// The constructor already owns this complete physical reservation. Only
    /// untouched RAM qualifies; this never reallocates RAM or retypes a pool
    /// Frame. The caller must demote before publishing any native/guest leaf.
    pub(crate) fn prepare_reserved_large_frame(&mut self, first: usize)
        -> Result<Option<PrivateLargeAllocation>, CapabilityError>
    {
        if !self.private_large.supported || first & (PAGES - 1) != 0 { return Ok(None); }
        let address = first.checked_mul(PAGE_SIZE).ok_or(CapabilityError::InvalidArgument)?;
        let end = address.checked_add(BYTES).ok_or(CapabilityError::InvalidArgument)?;
        let source = |memory: &Self| {
            if memory.frame_range(first).is_some()
                || memory.physical_frames.range(first..first + PAGES).next().is_some()
            { return None; }
            (0..memory.initial_generic_count).filter(|&index|
                !memory.initial_generics[index].is_device()
                    && memory.source_range(index, address).is_some_and(|(_, limit)| end <= limit))
                .min_by_key(|&index| memory.initial_generics[index].size_radix)
        };
        if source(self).is_none() { return Ok(None); }
        self.ensure_metadata_capacity()?;
        // Refilling metadata may consume an untyped source. Revalidate it.
        let Some(index) = source(self) else { return Ok(None); };
        self.construct_private_large_frame(index, first).map(Some)
    }

    fn construct_private_large_frame(&mut self, index: usize, first: usize)
        -> Result<PrivateLargeAllocation, CapabilityError>
    {
        // This sentinel owns failed prefix/node/source conversions too.
        self.record_private_large_range(first, 0, None, false, true);
        let node_slot = self.capability_directory.allocate(&mut self.node_pool)?;
        self.node_pool.create_node(9, node_slot)?;
        let node = node_slot.descriptor();
        let owner = self.take_physical_source_slot(index, first * PAGE_SIZE, BITS)?;
        self.record_private_large_range(first, owner.descriptor(), Some(node), false, true);
        self.private_large.records.insert(node, Record { first, owner, huge: true, failed: true,
            quarantined: false, mapping: None });
        match arch::generic::convert(owner.descriptor(), CapabilityType::Frame, BITS, 1, node, 0) {
            Ok(()) => {
                self.private_large.records.get_mut(&node).unwrap().failed = false;
                self.record_private_large_range(first, owner.descriptor(), Some(node), false, false);
                Ok(PrivateLargeAllocation { first, large: true })
            }
            Err(CapabilityError::InvalidArgument | CapabilityError::InvalidDepth | CapabilityError::IllegalOperation) => {
                self.private_large.supported = false;
                self.demote_private_large_pool(first)?;
                Ok(PrivateLargeAllocation { first, large: false })
            }
            Err(error) => Err(error),
        }
    }

    fn slice(&self, key: usize) -> Result<(usize, Record, usize), CapabilityError> {
        let key = key & !SLICE;
        let depth = crate::nanami_utils::descriptor::descriptor_depth(key);
        if depth < 8 + 9 || depth > nun::WORD_BITS { return Err(CapabilityError::InvalidArgument); }
        let parent_depth = depth - 9;
        let payload = (usize::MAX >> 8) & (usize::MAX << (nun::WORD_BITS - parent_depth));
        let node = (key & payload) | ((parent_depth - 8) << (nun::WORD_BITS - 8));
        let record = *self.private_large.records.get(&node).ok_or(CapabilityError::InvalidArgument)?;
        let page = (key >> (nun::WORD_BITS - depth)) & (PAGES - 1);
        if !record.huge || record.failed || make_child_slot_descriptor(node, 9, page) != key {
            return Err(CapabilityError::InvalidArgument);
        }
        Ok((node, record, page))
    }

    /// Read INTERNAL slice identities from one current huge allocation.
    /// Validate the whole span before changing output. These keys never confer
    /// kernel capability or mapping authority and must not escape as Frames.
    pub(crate) fn read_private_large_frames(
        &self, first: usize, frames: &mut [CapabilityDescriptor],
    ) -> Result<(), CapabilityError> {
        if frames.is_empty() { return Ok(()); }
        let address = first.checked_mul(PAGE_SIZE).ok_or(CapabilityError::InvalidArgument)?;
        let bytes = frames.len().checked_mul(PAGE_SIZE).ok_or(CapabilityError::InvalidArgument)?;
        address.checked_add(bytes).ok_or(CapabilityError::InvalidArgument)?;
        let range = self.frame_range(first).ok_or(CapabilityError::InvalidArgument)?;
        let key = self.private_large_frame_key(range, first).ok_or(CapabilityError::InvalidArgument)?;
        let (node, record, page) = self.slice(key)?;
        if record.quarantined || record.first.checked_add(page) != Some(first)
            || frames.len() > PAGES - page
        { return Err(CapabilityError::InvalidArgument); }
        for (index, frame) in frames.iter_mut().enumerate() {
            *frame = make_child_slot_descriptor(node, 9, page + index) | SLICE;
        }
        Ok(())
    }

    fn ensure_large_tables(&mut self, space: usize, address: usize) -> CapabilityResult {
        loop {
            let depth = arch::address_space::get_unset_depth(space, address, BITS)?;
            if depth == 0 { return Ok(()); }
            if depth > 3 { return Err(CapabilityError::InvalidDepth); }
            // PageTables retains the Generic and AS ownership. No 4K-table
            // readiness hint is published by a successful large-leaf setup.
            let (table, _) = self.page_tables.allocate(self.kernel_object_generic, space, depth, true)?;
            arch::address_space::map(space, table, address, Attribute::ALL)?;
        }
    }

    /// Returns the addressed 4K slice and contiguous remaining pages. Callers
    /// validate current process ownership/permissions before using this alias.
    pub(crate) fn map_private_large_alias(&mut self, key: usize, pinned: Option<usize>)
        -> Result<Option<(usize, usize)>, CapabilityError>
    {
        if !is_slice_key(key) { return Ok(None); }
        let (node, _, page) = self.slice(key)?;
        let frame = make_child_slot_descriptor(node, 9, 0);
        if let Some(index) = self.private_large.aliases.iter().position(|alias|
            alias.is_some_and(|alias| alias.frame == frame && alias.ready)) {
            return Ok(Some((TEMP + index * BYTES + page * PAGE_SIZE, PAGES - page)));
        }
        let index = usize::from(pinned.is_some_and(|va| (TEMP..TEMP + BYTES).contains(&va)));
        self.clear_private_large_alias(index)?;
        let space = self.private_large.address_space;
        let address = TEMP + index * BYTES;
        self.ensure_large_tables(space, address)?;
        self.private_large.aliases[index] = Some(Alias { frame, ready: false });
        arch::address_space::map(space, frame, address, Attribute::READ | Attribute::WRITE)?;
        self.private_large.aliases[index].as_mut().unwrap().ready = true;
        Ok(Some((address + page * PAGE_SIZE, PAGES - page)))
    }

    fn clear_private_large_alias(&mut self, index: usize) -> CapabilityResult {
        if let Some(alias) = self.private_large.aliases[index].as_mut() {
            alias.ready = false;
            arch::address_space::unmap(self.private_large.address_space, alias.frame, TEMP + index * BYTES)?;
            self.private_large.aliases[index] = None;
        }
        Ok(())
    }

    pub(crate) fn map_private_large_frame(&mut self, first: usize, space: usize,
        base: usize, permissions: usize) -> CapabilityResult {
        if base & (BYTES - 1) != 0 || permissions & !7 != 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let node = self.frame_range(first).and_then(|range| range.node()).ok_or(CapabilityError::InvalidArgument)?;
        let record = self.private_large.records.get_mut(&node).ok_or(CapabilityError::InvalidArgument)?;
        if !record.huge || record.failed || record.first != first || record.mapping.is_some() {
            return Err(CapabilityError::IllegalOperation);
        }
        record.mapping = Some(Mapping { space, base, permissions, state: MappingState::Unmapped, table: None });
        if permissions == 0 { return Ok(()); }
        self.ensure_large_tables(space, base)?;
        self.private_large.records.get_mut(&node).unwrap().mapping.as_mut().unwrap().state = MappingState::Uncertain;
        arch::address_space::map(space, make_child_slot_descriptor(node, 9, 0), base,
            Attribute::from_bits(permissions).ok_or(CapabilityError::InvalidArgument)?)?;
        self.private_large.records.get_mut(&node).unwrap().mapping.as_mut().unwrap().state = MappingState::Huge;
        Ok(())
    }

    pub(crate) fn private_large_mapping(&self, space: usize, start: usize, end: usize)
        -> Option<PrivateLargeMapping> {
        self.private_large_mapping_requiring_split(space, start, end, |_, _| false)
    }

    /// Preserve only stable whole Frames that the caller proves can be copied
    /// without exporting small capabilities. Pending/failed ownership must
    /// still reach the ordinary split, rollback and quarantine path.
    pub(crate) fn private_large_mapping_requiring_split(&self, space: usize,
        start: usize, end: usize, mut preserve: impl FnMut(usize, usize) -> bool)
        -> Option<PrivateLargeMapping> {
        if start >= end { return None; }
        self.private_large.records.values().find_map(|record| {
            let mapping = record.mapping?;
            if mapping.space != space || mapping.base >= end || start >= mapping.base + BYTES {
                return None;
            }
            if record.stable_large_mapping(mapping) && preserve(mapping.base, mapping.permissions) { return None; }
            Some(PrivateLargeMapping { first: record.first, base: mapping.base,
                permissions: mapping.permissions, small: mapping.state == MappingState::Small,
                quarantined: record.quarantined })
        })
    }

    /// Exact, stable huge ownership, including an acknowledged PROT_NONE leaf.
    /// Pending split/map/unmap and quarantined records stay on the cleanup path.
    pub(crate) fn private_large_protection(&self, space: usize, base: usize)
        -> Option<PrivateLargeMapping> {
        self.stable_private_large_mappings(space, base, base.checked_add(1)?)
            .find(|mapping| mapping.base == base)
    }

    pub(crate) fn stable_private_large_mappings(&self, space: usize, start: usize, end: usize)
        -> impl Iterator<Item = PrivateLargeMapping> + '_ {
        self.private_large.records.values().filter_map(move |record| {
            let mapping = record.mapping?;
            (mapping.space == space && mapping.base < end && start < mapping.base + BYTES
                && record.stable_large_mapping(mapping))
                .then_some(PrivateLargeMapping { first: record.first, base: mapping.base,
                    permissions: mapping.permissions, small: false, quarantined: false })
        })
    }

    /// The supervisor keeps the VM barrier throughout forward work and repair.
    /// Never retype RAM or drop ownership for a whole-leaf permission change.
    pub(crate) fn protect_private_large_frame(&mut self, first: usize, permissions: usize) -> CapabilityResult {
        if permissions & !7 != 0 { return Err(CapabilityError::InvalidArgument); }
        let node = self.frame_range(first).and_then(|range| range.node()).ok_or(CapabilityError::InvalidArgument)?;
        let record = self.private_large.records.get(&node).ok_or(CapabilityError::InvalidArgument)?;
        let old = record.mapping
            .ok_or(CapabilityError::InvalidArgument)?;
        if record.first != first || !record.stable_large_mapping(old) {
            return Err(CapabilityError::IllegalOperation);
        }
        if old.permissions == permissions { return Ok(()); }
        let frame = make_child_slot_descriptor(node, 9, 0);
        let change = (|| {
            if old.permissions == 0 && permissions != 0 {
                // A cloned inaccessible range has never needed guest tables.
                // Prepare only the upper levels when it is first made visible.
                self.ensure_large_tables(old.space, old.base)?;
            }
            self.unmap_private_large_frame(first)?;
            if permissions != 0 {
                self.private_large.records.get_mut(&node).unwrap().mapping.as_mut().unwrap().state = MappingState::Uncertain;
                arch::address_space::map(old.space, frame, old.base,
                    Attribute::from_bits(permissions).ok_or(CapabilityError::InvalidArgument)?)?;
            }
            let mapping = self.private_large.records.get_mut(&node).unwrap().mapping.as_mut().unwrap();
            mapping.permissions = permissions;
            mapping.state = if permissions == 0 { MappingState::Unmapped } else { MappingState::Huge };
            Ok(())
        })();
        if let Err(error) = change {
            // Both failures before and after installation require a fresh
            // UNMAP ACK before the old leaf (or confirmed absence) is restored.
            let repair: CapabilityResult = (|| {
                self.unmap_private_large_frame(first)?;
                if old.permissions != 0 {
                    self.private_large.records.get_mut(&node).unwrap().mapping.as_mut().unwrap().state = MappingState::Uncertain;
                    arch::address_space::map(old.space, frame, old.base,
                        Attribute::from_bits(old.permissions).ok_or(CapabilityError::Fatal)?)?;
                }
                *self.private_large.records.get_mut(&node).unwrap().mapping.as_mut().unwrap() = old;
                Ok(())
            })();
            if repair.is_err() {
                self.quarantine_private_large_mapping(first);
                return Err(CapabilityError::Fatal);
            }
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn unmap_private_large_frame(&mut self, first: usize) -> CapabilityResult {
        let node = self.frame_range(first).and_then(|range| range.node()).ok_or(CapabilityError::InvalidArgument)?;
        let record = self.private_large.records.get_mut(&node).ok_or(CapabilityError::InvalidArgument)?;
        let mapping = record.mapping.as_mut().ok_or(CapabilityError::InvalidArgument)?;
        if matches!(mapping.state, MappingState::Huge | MappingState::Uncertain) {
            mapping.state = MappingState::Uncertain;
            arch::address_space::unmap(mapping.space, make_child_slot_descriptor(node, 9, 0), mapping.base)?;
            mapping.state = MappingState::Unmapped;
        }
        Ok(())
    }

    /// Requires an acknowledged huge UNMAP (or a retired AS), with all VM
    /// users quiescent until their replacement 4K PTEs have been published.
    pub(crate) fn demote_private_large_pool(&mut self, first: usize) -> CapabilityResult {
        let node = self.frame_range(first).and_then(|range| range.node()).ok_or(CapabilityError::InvalidArgument)?;
        let record = *self.private_large.records.get(&node).ok_or(CapabilityError::InvalidArgument)?;
        if record.mapping.is_some_and(|mapping| !matches!(mapping.state, MappingState::Unmapped | MappingState::Small)) {
            return Err(CapabilityError::IllegalOperation);
        }
        if !record.huge {
            if record.mapping.is_none() { self.private_large.records.remove(&node); }
            return Ok(());
        }
        for index in 0..2 { self.clear_private_large_alias(index)?; }
        // A failed batch may have installed Frame caps. No 4K PTE exists yet,
        // so a later retry also requires this isolated owner's revoke ACK.
        self.private_large.records.get_mut(&node).unwrap().failed = true;
        self.record_private_large_range(first, record.owner.descriptor(), Some(node), false, true);
        arch::node::revoke(record.owner.node, record.owner.index)?;
        arch::generic::convert(record.owner.descriptor(), CapabilityType::Frame, PAGE_BITS, PAGES, node, 0)?;
        self.record_private_large_range(first, record.owner.descriptor(), Some(node), true, false);
        let record = self.private_large.records.get_mut(&node).unwrap();
        record.huge = false;
        record.failed = false;
        if let Some(mapping) = record.mapping.as_mut() { mapping.state = MappingState::Small; }
        else { self.private_large.records.remove(&node); }
        Ok(())
    }

    pub(crate) fn ensure_private_large_split_table(&mut self, first: usize,
        vm: &mut impl VmTracker) -> CapabilityResult {
        let node = self.frame_range(first).and_then(|range| range.node()).ok_or(CapabilityError::InvalidArgument)?;
        let record = self.private_large.records[&node];
        let mapping = record.mapping.ok_or(CapabilityError::InvalidArgument)?;
        if record.huge || mapping.state != MappingState::Small { return Err(CapabilityError::IllegalOperation); }
        let depth = arch::address_space::get_unset_depth(mapping.space, mapping.base, PAGE_BITS)?;
        match depth {
            0 => Ok(()),
            1..=3 => {
                // An inaccessible fork allocation may never have had an
                // upper table. Track its first 4K table for split rollback.
                if depth > 1 { self.ensure_large_tables(mapping.space, mapping.base)?; }
                let (table, slot) = self.page_tables.allocate(self.kernel_object_generic, mapping.space, 1, true)?;
                // Keep the intended table even if MAP fails after publishing
                // its pointer. Rollback can then unlink it before huge remap.
                self.private_large.records.get_mut(&node).unwrap().mapping.as_mut().unwrap().table = Some(slot);
                vm.record_page_table(mapping.base, slot).map_err(|_| CapabilityError::Fatal)?;
                arch::address_space::map(mapping.space, table, mapping.base, Attribute::ALL)
            }
            _ => Err(CapabilityError::Fatal),
        }
    }

    /// Failure rollback only, while the same VM barrier remains held. Unlink
    /// the whole private leaf table and acknowledge its full TLB shootdown
    /// before revoking any 4K Frames. No privileged 4K aliases were created
    /// during this synchronous split transaction.
    pub(crate) fn restore_private_large_frame(&mut self, first: usize) -> CapabilityResult {
        let node = self.frame_range(first).and_then(|range| range.node()).ok_or(CapabilityError::InvalidArgument)?;
        let record = self.private_large.records[&node];
        let mapping = record.mapping.ok_or(CapabilityError::InvalidArgument)?;
        self.unmap_private_large_frame(first)?;
        if let Some(slot) = mapping.table {
            arch::address_space::unmap(mapping.space,
                make_child_slot_descriptor(self.page_tables.node_descriptor(), PAGE_TABLE_POOL_RADIX, slot), mapping.base)?;
        }
        if !record.huge || record.failed {
            for index in 0..2 { self.clear_private_large_alias(index)?; }
            {
                let current = self.private_large.records.get_mut(&node).unwrap();
                current.huge = true;
                current.failed = true;
            }
            self.record_private_large_range(first, record.owner.descriptor(), Some(node), false, true);
            arch::node::revoke(record.owner.node, record.owner.index)?;
            arch::generic::convert(record.owner.descriptor(), CapabilityType::Frame, BITS, 1, node, 0)?;
            self.private_large.records.get_mut(&node).unwrap().failed = false;
        }
        self.record_private_large_range(first, record.owner.descriptor(), Some(node), false, false);
        if mapping.permissions == 0 {
            let state = self.private_large.records.get_mut(&node).unwrap().mapping.as_mut().unwrap();
            state.table = None;
            state.state = MappingState::Unmapped;
            return Ok(());
        }
        self.ensure_large_tables(mapping.space, mapping.base)?;
        let state = self.private_large.records.get_mut(&node).unwrap().mapping.as_mut().unwrap();
        state.table = None;
        state.state = MappingState::Uncertain;
        arch::address_space::map(mapping.space, make_child_slot_descriptor(node, 9, 0), mapping.base,
            Attribute::from_bits(mapping.permissions).ok_or(CapabilityError::Fatal)?)?;
        self.private_large.records.get_mut(&node).unwrap().mapping.as_mut().unwrap().state = MappingState::Huge;
        Ok(())
    }

    pub(crate) fn quarantine_private_large_mapping(&mut self, first: usize) {
        if let Some(node) = self.frame_range(first).and_then(|range| range.node()) {
            if let Some(record) = self.private_large.records.get_mut(&node) { record.quarantined = true; }
        }
    }

    pub(crate) fn finish_private_large_split(&mut self, first: usize) -> CapabilityResult {
        let node = self.frame_range(first).and_then(|range| range.node()).ok_or(CapabilityError::InvalidArgument)?;
        let record = self.private_large.records.get(&node).ok_or(CapabilityError::InvalidArgument)?;
        if record.huge || record.failed { return Err(CapabilityError::IllegalOperation); }
        self.private_large.records.remove(&node);
        Ok(())
    }

    pub(super) fn prepare_private_large_free(&mut self, address: usize, bytes: usize) -> CapabilityResult {
        if bytes == 0 { return Ok(()); }
        let end = address.checked_add(bytes).ok_or(CapabilityError::InvalidArgument)?;
        loop {
            let selected = self.private_large.records.values().find(|record|
                record.first * PAGE_SIZE < end && address < record.first * PAGE_SIZE + BYTES).copied();
            let Some(record) = selected else { break; };
            if record.mapping.is_some() { return Err(CapabilityError::IllegalOperation); }
            self.demote_private_large_pool(record.first)?;
        }
        let first = address >> PAGE_BITS;
        let end_page = ((end - 1) >> PAGE_BITS) + 1;
        // A failed node/source preparation may have only a sentinel, without
        // a private-large record. Check interior chunks of a larger owner too.
        if self.frame_range(first).is_some_and(|range| range.private_quarantined())
            || self.physical_frames.range(first..end_page)
                .any(|(_, range)| range.private_quarantined())
        {
            return Err(CapabilityError::IllegalOperation);
        }
        Ok(())
    }
}
