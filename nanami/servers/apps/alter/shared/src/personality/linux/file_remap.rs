//! Reuse only a proven unchanged private immutable-file extent.
use super::{Runtime, Word, LINUX_MAP_FIXED, LINUX_PAGE_SIZE};
use crate::state::ReadonlyMapping;

fn generation(runtime: &Runtime, pid: Word) -> Word {
    runtime.managed_process(pid).map_or(0, |process| process.mappings.generation())
}

pub(super) fn snapshot_write(runtime: &Runtime, pid: Word, base: Word, bytes: Word, prot: Word) -> bool {
    prot & 2 != 0 && prot & !7 == 0
        && runtime.readonly_mapping.get().covers_file_page(generation(runtime, pid), base, bytes)
}

/// Called only after mprotect preflight. Permissions-only changes cannot
/// modify the snapshot's retained backing. Keep this proof separate from
/// view generations: aliases must still be invalidated before touching PTEs.
pub(super) fn before_protect(runtime: &Runtime, pid: Word, prot: Word) -> ReadonlyMapping {
    let origin = runtime.readonly_mapping.get();
    if origin.generation != 0 && origin.generation == generation(runtime, pid) {
        // A failed or partially rolled-back change must never restore a proof.
        runtime.readonly_mapping.set(ReadonlyMapping::EMPTY);
        if matches!(prot, 0 | 1 | 4 | 5) {
            return origin;
        }
    }
    ReadonlyMapping::EMPTY
}

/// Only after the permissions-only operation AND VMA publication succeeded.
pub(super) fn after_protect(runtime: &Runtime, pid: Word, mut origin: ReadonlyMapping) {
    if origin.generation != 0 {
        origin.generation = generation(runtime, pid);
        runtime.readonly_mapping.set(origin);
        #[cfg(target_os = "none")]
        diagnostic_protect(runtime, pid, origin.generation);
    }
}

#[cfg(target_os = "none")]
fn diagnostic_protect(runtime: &Runtime, pid: Word, generation: Word) {
    if option_env!("NANAMI_READONLY_PROTECT_DIAG") != Some("1") { return; }
    use core::sync::atomic::{AtomicUsize, Ordering};
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    if runtime.managed_process(pid).is_some_and(|process| {
        process.image_name[..process.image_name_len].rsplit(|byte| *byte == b'/').next() == Some(&b"xfe"[..])
    }) && COUNT.fetch_add(1, Ordering::Relaxed) < 32 {
        nanami_runtime::println!("[linux.mmap] protected-immutable-proof pid={} generation={}", pid, generation);
    }
}

pub(super) fn remember(runtime: &Runtime, pid: Word, identity: Word, file_size: Word,
    base: Word, mapped: Word, offset: Word)
{
    runtime.readonly_mapping.set(ReadonlyMapping::new(
        generation(runtime, pid), identity, file_size, base, mapped, offset,
    ).in_vm(runtime.vm_group(pid)));
}

/// Only after complete initialization, final hardware rights and successful
/// VMA publication. Fresh private copied bytes have no writable guest alias.
pub(super) fn remember_copy(runtime: &Runtime, pid: Word, identity: Word, file_size: Word,
    base: Word, mapped: Word, offset: Word, prot: Word)
{
    if matches!(prot, 1 | 5) {
        runtime.readonly_mapping.set(ReadonlyMapping::copied(
            generation(runtime, pid), identity, file_size, base, mapped, offset,
        ).in_vm(runtime.vm_group(pid)));
    }
}

pub(super) fn reuse(runtime: &mut Runtime, pid: Word, identity: Word, file_size: Word,
    base: Word, mapped: Word, offset: Word, prot: Word, flags: Word)
    -> Result<bool, i32>
{
    let writable = prot & 2 != 0 && prot & !7 == 0;
    if flags & LINUX_MAP_FIXED == 0 || flags & 3 != 2 || !(matches!(prot, 1 | 5) || writable)
        || (base | mapped) & (LINUX_PAGE_SIZE - 1) != 0
    {
        return Ok(false);
    }
    let mut origin = runtime.readonly_mapping.get();
    if !origin.covers(generation(runtime, pid), identity, file_size, base, mapped, offset)
        || !runtime.can_protect_mapping(pid, base, mapped)
    {
        return Ok(false);
    }
    // Clear before any partial PTE mutation. On failure MAP_FIXED's caller
    // owns whole-range cleanup, just as with a failed new snapshot mapping.
    #[cfg(target_os = "none")]
    let previous_generation = origin.generation;
    runtime.readonly_mapping.set(ReadonlyMapping::EMPTY);
    let protection = if origin.deferred_snapshot() && !writable
        && runtime.mapping_protection_needed(pid, base, mapped, prot) == Some(false) {
        // MAP_FIXED asks for a real segment mapping even when its logical
        // rights equal the temporary whole-file R reservation's rights.
        super::private_memory::with_mapping_mutation(runtime, pid, base, mapped, false, ||
            nanami_runtime::request_process_mapping_protect(pid, base, mapped, prot)
                .map_err(super::map_request_error))
    } else { super::sys_mprotect(runtime, pid, base, mapped, prot).map(|_| ()) };
    if let Err(error) = protection {
        let _ = super::sys_munmap(runtime, pid, base, mapped);
        return Err(error);
    }
    // Read/execute changes leave every byte in the ORIGINAL extent immutable.
    // Retain that extent for a following segment, including strict subranges.
    // WRITE goes through the ordinary mprotect private-write guard (including
    // COW/peer quiescence). Its bytes may change as soon as the task resumes:
    // consume the whole proof rather than re-certifying any writable extent.
    if !writable {
        origin.generation = generation(runtime, pid);
        runtime.readonly_mapping.set(origin);
    } else {
        runtime.readonly_mapping.set(ReadonlyMapping::EMPTY);
    }
    #[cfg(target_os = "none")]
    if option_env!("NANAMI_READONLY_PROTECT_DIAG") == Some("1") {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        if runtime.managed_process(pid).is_some_and(|process| {
            process.image_name[..process.image_name_len].rsplit(|byte| *byte == b'/').next() == Some(&b"xfe"[..])
        }) && COUNT.fetch_add(1, Ordering::Relaxed) < 32 {
            nanami_runtime::println!("[linux.mmap] reused-proof pid={} before={} after={} bytes={} shared={} prot={} file-size={} offset={}",
                pid, previous_generation, generation(runtime, pid), mapped, origin.shared_file(), prot, file_size, offset);
        }
    }
    #[cfg(target_os = "none")]
    if option_env!("NANAMI_READONLY_REMAP_DIAG") == Some("1") {
        use core::sync::atomic::{AtomicBool, Ordering};
        static REPORTED: AtomicBool = AtomicBool::new(false);
        if runtime.managed_process(pid).is_some_and(|process| {
            process.image_name[..process.image_name_len].rsplit(|byte| *byte == b'/').next()
                == Some(&b"xfe"[..])
        }) && !REPORTED.swap(true, Ordering::Relaxed) {
            nanami_runtime::println!("[linux.mmap] reused-immutable-extent pid={} bytes={}", pid, mapped);
        }
    }
    Ok(true)
}
