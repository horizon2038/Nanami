//! Range-confined protection commit and disjoint supervisor view retention.
use super::*;

pub(super) fn protect(
    runtime: &mut Runtime,
    pid: Word,
    addr: Word,
    len: Word,
    prot: Word,
) -> Result<Word, i32> {
    if addr == 0 || len == 0 || (addr & (LINUX_PAGE_SIZE - 1)) != 0 {
        return Err(EINVAL);
    }
    if (prot & !LINUX_PROT_ALL) != 0 {
        return Err(EINVAL);
    }
    if len == 0 {
        return Ok(0);
    }
    let mapped = len.checked_add(LINUX_PAGE_SIZE - 1).ok_or(ENOMEM)? & !(LINUX_PAGE_SIZE - 1);
    match runtime.mapping_protection_needed(pid, addr, mapped, prot) {
        Some(false) => return Ok(0),
        Some(true) => {},
        None => return Err(ENOMEM),
    }
    // A successful nonwritable protection edit changes only this target
    // interval. WRITE upgrades retain the complete COW invalidation policy.
    let before = (prot & LINUX_PROT_WRITE == 0)
        .then(|| super::guest_views::BeforeMmap::capture(runtime, pid)).flatten();
    let snapshot_write = super::file_remap::snapshot_write(runtime, pid, addr, mapped, prot);
    let origin = super::file_remap::before_protect(runtime, pid, prot);
    // Invalidate before touching PTEs, including partial updates whose rollback
    // fails. A subsequent cached read must revalidate current Alpha permissions.
    if let Some(process) = runtime.managed_process(pid) {
        process.mappings.invalidate_views();
    }
    if prot & LINUX_PROT_WRITE != 0 {
        if snapshot_write {
            super::private_memory::with_snapshot_write(runtime, pid, addr, mapped, || {
                super::memfd::protect(runtime, pid, addr, mapped, prot)
            })?;
        } else {
            super::private_memory::with_private_protect(runtime, pid, addr, mapped, || {
                super::memfd::protect(runtime, pid, addr, mapped, prot)
            })?;
        }
    } else {
        super::private_memory::with_mapping_mutation(runtime, pid, addr, mapped, false, || {
            super::memfd::protect(runtime, pid, addr, mapped, prot)
        })?;
    }
    if !runtime.protect_mapping(pid, addr, mapped, prot) {
        return Err(ENOMEM);
    }
    super::file_remap::after_protect(runtime, pid, origin);
    if let Some(before) = before {
        before.completed_protection(runtime, pid, addr, mapped);
    }
    Ok(0)
}

