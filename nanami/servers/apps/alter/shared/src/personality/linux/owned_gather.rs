//! Bounded checked-read plans whose destinations belong to one private owner.
use core::{marker::PhantomData, mem::MaybeUninit};
use nanami_runtime::ProcessMemoryTransfer;

const MAX_FIELDS: usize = 4;

pub(super) struct Plan<'a> {
    entries: [ProcessMemoryTransfer; MAX_FIELDS],
    count: usize,
    _owner: PhantomData<&'a mut [MaybeUninit<u8>]>,
}

impl Plan<'_> {
    pub(super) fn entries(&self) -> &[ProcessMemoryTransfer] {
        &self.entries[..self.count]
    }
}

/// Keep small leased reads and the original checked-copy chunk boundaries.
/// A plan covers exactly the accepted prefix, ignoring unused trailing iovecs.
pub(super) fn plan<'a>(
    bases: &[usize],
    lengths: &[usize],
    destination: &'a mut [MaybeUninit<u8>],
    quantum: usize,
    lease_limit: usize,
) -> Option<Plan<'a>> {
    if bases.len() < 2 || bases.len() != lengths.len()
        || destination.is_empty() || destination.len() > quantum
        || lengths.iter().all(|&bytes| bytes <= lease_limit)
    {
        return None;
    }
    let mut plan = Plan {
        entries: [ProcessMemoryTransfer { target: 0, local: 0, bytes: 0 }; MAX_FIELDS],
        count: 0,
        _owner: PhantomData,
    };
    let mut copied = 0;
    let mut bulk = false;
    for (&base, &length) in bases.iter().zip(lengths) {
        let bytes = length.min(destination.len() - copied);
        if bytes == 0 { continue; }
        if base == 0 || base.checked_add(bytes).is_none() || plan.count == MAX_FIELDS {
            return None;
        }
        let end = copied + bytes;
        plan.entries[plan.count] = ProcessMemoryTransfer {
            target: base,
            local: destination[copied..end].as_mut_ptr() as usize,
            bytes,
        };
        plan.count += 1;
        copied = end;
        bulk |= bytes > lease_limit;
        if copied == destination.len() { break; }
    }
    (copied == destination.len() && plan.count > 1 && bulk).then_some(plan)
}
