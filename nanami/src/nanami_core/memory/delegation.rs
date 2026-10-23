//! Real Generic authority partitions. A failed handoff remains quarantined.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DelegationId(usize);
impl DelegationId {
    pub fn value(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DelegationState {
    Reserved,
    Prepared,
    HandedOff,
    Reclaimed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DelegationInfo {
    pub address: usize,
    pub bytes: usize,
    pub state: DelegationState,
}

pub(super) struct Delegation {
    info: DelegationInfo,
    initial_index: usize,
    authority: Option<directory::Slot>,
    resources: Vec<MemoryResource>,
}
impl Delegation {
    pub(super) fn overlaps(&self, address: usize, bytes: usize) -> bool {
        self.info.state != DelegationState::Reclaimed
            && address < self.info.address.saturating_add(self.info.bytes)
            && self.info.address < address.saturating_add(bytes)
    }
}

impl MemoryManager {
    pub fn delegation_records(&self) -> impl Iterator<Item = (DelegationId, DelegationInfo)> + '_ {
        self.delegations
            .iter()
            .enumerate()
            .map(|(index, record)| (DelegationId(index + 1), record.info))
    }

    pub(super) fn delegated_range(&self, address: usize, bytes: usize) -> bool {
        self.delegations
            .iter()
            .any(|record| record.overlaps(address, bytes))
    }
    /// Keep two 256-MiB bootstrap extents, then successively larger untouched
    /// Generics for recursive delegation. Splitting every budget into two equal
    /// halves makes both halves partly occupied by bootstrap, so a 1-GiB Alpha
    /// cannot delegate even 512 MiB despite having enough total free RAM.
    pub fn reserve_alpha_memory(&mut self, radix: usize) -> Result<DelegationId, CapabilityError> {
        if !(29..usize::BITS as usize).contains(&radix) {
            return Err(CapabilityError::InvalidArgument);
        }
        let bytes = 1usize << radix;
        self.ensure_metadata_capacity()?;
        let mut selected = None;
        for (&(index, start), source) in &self.physical_sources {
            if self.initial_generics[index].is_device() {
                continue;
            }
            let allocation = self
                .physical_allocator
                .as_mut()
                .ok_or(CapabilityError::InvalidArgument)?
                .allocate_aligned_in_range(start, source.end, bytes);
            if let Ok(allocation) = allocation {
                selected = Some((index, allocation.base_page << PAGE_BITS));
                break;
            }
        }
        let (initial_index, address) = selected.ok_or(CapabilityError::InvalidArgument)?;
        let index = self.delegations.len();
        // Record the reservation BEFORE any subsequent conversion can fail.
        self.delegations.push(Delegation {
            info: DelegationInfo {
                address,
                bytes,
                state: DelegationState::Reserved,
            },
            initial_index,
            authority: None,
            resources: Vec::new(),
        });
        let authority = self.take_physical_source_slot(initial_index, address, radix)?;
        self.delegations[index].authority = Some(authority);
        let mut resource_address = address;
        for resource_radix in core::iter::once(28).chain(28..radix) {
            let slot = self.capability_directory.allocate(&mut self.node_pool)?;
            arch::generic::convert(
                authority.descriptor(),
                CapabilityType::Generic,
                resource_radix,
                1,
                slot.node,
                slot.index,
            )?;
            self.delegations[index].resources.push(MemoryResource {
                capability: slot.descriptor(),
                address: resource_address,
                size_radix: resource_radix,
                flags: 0,
                consumed_bytes: 0,
            });
            resource_address += 1usize << resource_radix;
        }
        self.delegations[index].info.state = DelegationState::Prepared;
        Ok(DelegationId(index + 1))
    }

    pub fn delegation_info(&self, id: DelegationId) -> Option<DelegationInfo> {
        self.delegations
            .get(id.0.checked_sub(1)?)
            .map(|entry| entry.info)
    }

    /// Obtain the one-shot launch input. The caller must not roll back a partly
    /// completed MOVE; even failed preparation can have transferred authority.
    pub fn take_delegated_resources(
        &mut self,
        id: DelegationId,
    ) -> Result<Vec<MemoryResource>, CapabilityError> {
        let record =
            id.0.checked_sub(1)
                .and_then(|index| self.delegations.get_mut(index))
                .ok_or(CapabilityError::InvalidArgument)?;
        if record.info.state != DelegationState::Prepared {
            return Err(CapabilityError::IllegalOperation);
        }
        record.info.state = DelegationState::HandedOff;
        Ok(record.resources.clone())
    }

    /// # Safety
    /// All processes using this partition must be stopped, all outside IPC and
    /// DMA quiesced, and launcher aliases removed. A Suspend return alone is not
    /// proof of remote-core completion. On any revoke failure RAM stays reserved.
    pub unsafe fn reclaim_delegation(&mut self, id: DelegationId) -> Result<(), CapabilityError> {
        let index =
            id.0.checked_sub(1)
                .ok_or(CapabilityError::InvalidArgument)?;
        let record = self
            .delegations
            .get(index)
            .ok_or(CapabilityError::InvalidArgument)?;
        if record.info.state == DelegationState::Reclaimed {
            return Err(CapabilityError::IllegalOperation);
        }
        let authority = record.authority.ok_or(CapabilityError::IllegalOperation)?;
        let info = record.info;
        let initial_index = record.initial_index;
        arch::node::revoke(authority.node, authority.index)?;
        self.physical_sources.insert(
            (initial_index, info.address),
            PhysicalSource {
                end: info.address + info.bytes,
                descriptor: authority.descriptor(),
            },
        );
        // Bypass the public free guard only AFTER successful revocation.
        self.physical_allocator
            .as_mut()
            .ok_or(CapabilityError::InvalidArgument)?
            .free(info.address, info.bytes)
            .map_err(map_physical_alloc_error)?;
        self.delegations[index].info.state = DelegationState::Reclaimed;
        Ok(())
    }
}
