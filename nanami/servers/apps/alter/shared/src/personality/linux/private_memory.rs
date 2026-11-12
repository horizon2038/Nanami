//! Cold private-backing mutations. Never stop/replay an already handled wait.
use super::{map_request_error, Runtime, Word, EIO};
use alloc::vec::Vec;
use nanami_runtime::{OS_PRIVATE_WRITE_DETACH, OS_PRIVATE_WRITE_QUERY, OS_PRIVATE_WRITE_DETACH_DEFERRED};

pub(super) fn needs_checked_write_preparation(runtime: &Runtime, pid: Word) -> bool {
    runtime.managed_process(pid).is_some_and(|process| process.vm_writable_private_shared)
}

pub(super) fn invalidate_write_origin(runtime: &Runtime) {
    runtime.readonly_mapping.set(crate::state::ReadonlyMapping::EMPTY);
}

/// Only for permission-checked guest writes. An ordinary writable guest VA
/// cannot be a mutable alias of the retained private readonly file snapshot.
pub(super) fn invalidate_checked_write_origin(
    runtime: &Runtime, pid: Word, address: Word, bytes: Word,
) {
    let origin = runtime.readonly_mapping.get();
    if origin.generation == 0 || bytes == 0 { return; }
    if runtime.managed_process(pid).is_some_and(|process| {
        let generation = process.mappings.generation();
        let keep = !process.exited && origin.checked_write_preserves(process.vm_group, generation, address, bytes);
        #[cfg(target_os = "none")]
        if option_env!("NANAMI_READONLY_PROTECT_DIAG") == Some("1")
            && keep && !origin.disjoint_write(generation,address,bytes)
        {
            use core::sync::atomic::{AtomicUsize,Ordering};
            static REPORTS: AtomicUsize = AtomicUsize::new(0);
            if REPORTS.fetch_add(1,Ordering::Relaxed) < 8 {
                nanami_runtime::println!("[linux.mmap] preserved-foreign-write writer={} writer-generation={} origin-generation={} bytes={}",pid,generation,origin.generation,bytes);
            }
        }
        keep
    }) { return; }
    invalidate_write_origin(runtime);
}

pub(super) fn mark_shared(runtime: &mut Runtime, pid: Word) {
    let group = runtime.vm_group(pid);
    for process in &mut runtime.managed {
        if process.pid != 0 && process.vm_group == group {
            process.vm_private_shared = true;
            process.vm_writable_private_shared = true;
        }
    }
}

/// Snapshot admission only aliases R/RX pages. Writable protection upgrades
/// still detach them before publishing WRITE. Never clear a prior fork marker.
pub(super) fn mark_readonly_shared(runtime: &mut Runtime, pid: Word) {
    let group = runtime.vm_group(pid);
    for process in &mut runtime.managed {
        if process.pid != 0 && process.vm_group == group {
            process.vm_private_shared = true;
        }
    }
}

/// Repair only an ordinary user write-protection fault on logically writable
/// private memory. Alpha is authoritative about sharing; unrelated faults must
/// keep their signal behavior. DETACH restores Alpha's tracked logical rights
/// before clearing COW. All VM peers remain stopped and views invalidated.
pub(crate) fn materialize_private_write_fault(
    runtime: &Runtime, pid: Word, address: Word, architecture_code: Word,
) -> bool {
    const PAGE: Word = 4096;
    if !cfg!(target_arch = "x86_64") || architecture_code & !7 != 0
        || architecture_code & 3 != 3
        || !runtime.managed_process(pid).is_some_and(|process| process.vm_private_shared)
    { return false; }
    let page = address & !(PAGE - 1);
    if page.checked_add(PAGE).is_none() { return false; }
    let Some(permissions) = runtime.mapping_prot(pid, page, PAGE) else { return false; };
    if permissions & !7 != 0 || permissions & 2 == 0
        || !runtime.memory_access_allowed(pid, page, PAGE, 2)
    { return false; }
    runtime.readonly_mapping.set(crate::state::ReadonlyMapping::EMPTY);
    with_vm_quiescence(runtime, pid, || {
        super::guest_views::invalidate(runtime, pid);
        // Both the sole-owner repair and a copied replacement already install
        // the final logical rights. A second PROTECT only repeats validation
        // and copy-window invalidation after that authoritative commit.
        // The commit result distinguishes an unrelated fault without QUERY.
        nanami_runtime::request_process_private_write(pid, page, PAGE, OS_PRIVATE_WRITE_DETACH)
            .map_err(map_request_error)
    }) == Ok(true)
}

pub(super) fn with_private_write<T>(
    runtime: &Runtime,
    pid: Word,
    address: Word,
    bytes: Word,
    operation: impl FnOnce() -> Result<T, i32>,
) -> Result<T, i32> {
    with_private_write_inner(runtime, pid, address, bytes, Preparation::Guarded, operation)
}

/// Only after the caller's logical WRITE permission check. Readonly image/file
/// aliases cannot back this range; a fork marker keeps the full COW protocol.
pub(super) fn with_checked_private_write<T>(
    runtime: &Runtime, pid: Word, address: Word, bytes: Word,
    operation: impl FnOnce() -> Result<T, i32>,
) -> Result<T, i32> {
    with_private_write_inner(runtime, pid, address, bytes, Preparation::Checked, operation)
}

/// With no runnable VM peer or cached alias, an empty DETACH costs the same
/// request as QUERY(false). Shared backing needs only DETACH, not both requests.
pub(super) fn with_private_protect<T>(
    runtime: &Runtime,
    pid: Word,
    address: Word,
    bytes: Word,
    operation: impl FnOnce() -> Result<T, i32>,
) -> Result<T, i32> {
    with_private_write_inner(runtime, pid, address, bytes, Preparation::Protect, operation)
}

enum Preparation { Snapshot, Guarded, Protect, Checked }

/// An undivided huge leaf is wholly contained in one logical VMA. A prior
/// partial mutation would have split that leaf before splitting this metadata.
fn may_have_large_backing(runtime: &Runtime, pid: Word, address: Word, bytes: Word) -> bool {
    if bytes == 0 { return false; }
    let Some(end) = address.checked_add(bytes) else { return false; };
    runtime.managed_process(pid).is_some_and(|process| process.mappings.iter().any(|mapping|
        mapping.size >= 1 << 21 && mapping.base < end
            && mapping.base.checked_add(mapping.size).is_some_and(|limit| address < limit)))
}

/// Only the synchronous Alpha mutation belongs inside this barrier. Callers
/// publish mutable personality metadata after it returns. Existing COW/write
/// guards use their own combined barrier below, without nesting this helper.
pub(super) fn with_mapping_mutation<T>(runtime: &Runtime, pid: Word,
    address: Word, bytes: Word, creating: bool,
    operation: impl FnOnce() -> Result<T, i32>) -> Result<T, i32> {
    let large = if creating { bytes >= 1 << 21 }
        else { may_have_large_backing(runtime, pid, address, bytes) };
    if !large {
        return operation();
    }
    with_vm_quiescence(runtime, pid, || {
        super::guest_views::invalidate_range(runtime, pid, address, bytes);
        operation()
    })
}

/// The caller already validated a live immutable-snapshot extent. Its pages
/// can be detached directly; never omit quiescence, view invalidation or DETACH.
pub(super) fn with_snapshot_write<T>(
    runtime: &Runtime,
    pid: Word,
    address: Word,
    bytes: Word,
    operation: impl FnOnce() -> Result<T, i32>,
) -> Result<T, i32> {
    with_private_write_inner(runtime, pid, address, bytes, Preparation::Snapshot, operation)
}

fn with_private_write_inner<T>(
    runtime: &Runtime,
    pid: Word,
    address: Word,
    bytes: Word,
    preparation: Preparation,
    operation: impl FnOnce() -> Result<T, i32>,
) -> Result<T, i32> {
    if bytes != 0 {
        // Privileged copies can change readonly bytes without a VMA mutation.
        // Independent of optional guest-view flags and of the COW query result.
        match preparation {
            Preparation::Checked => invalidate_checked_write_origin(runtime, pid, address, bytes),
            _ => invalidate_write_origin(runtime),
        }
    }
    let shared = match preparation {
        Preparation::Snapshot => true,
        Preparation::Guarded | Preparation::Protect => runtime.managed_process(pid).is_some_and(|p| p.vm_private_shared),
        Preparation::Checked => needs_checked_write_preparation(runtime, pid),
    };
    let large = matches!(preparation, Preparation::Guarded | Preparation::Protect)
        && may_have_large_backing(runtime, pid, address, bytes);
    if bytes == 0 || (!shared && !large) {
        return operation();
    }
    let query = shared && match preparation {
        Preparation::Snapshot => false,
        Preparation::Guarded | Preparation::Protect | Preparation::Checked => {
            // Also applies to ordinary copyout: without a runnable peer or
            // cached view, DETACH itself can answer an empty preparation.
            // Never skip the authoritative operation or its VM barrier.
            let group = runtime.vm_group(pid);
            (0..runtime.managed.len()).any(|index| active_vm_user(runtime, index, group))
                || super::guest_views::has_cached_owner(runtime, pid)
        }
    };
    let detach = shared && (!query || nanami_runtime::request_process_private_write(pid, address, bytes, OS_PRIVATE_WRITE_QUERY)
        .map_err(map_request_error)?);
    if !detach && !large {
        return operation();
    }
    with_vm_quiescence(runtime, pid, || {
        super::guest_views::invalidate(runtime, pid);
        if detach {
            // This caller follows with protection while every peer remains
            // stopped. Avoid an intermediate old-rights PTE; failed protection
            // leaves initialized old-rights backing handled by snapshot faults.
            let mode = if matches!(preparation, Preparation::Snapshot | Preparation::Protect) {
                OS_PRIVATE_WRITE_DETACH_DEFERRED
            } else { OS_PRIVATE_WRITE_DETACH };
            nanami_runtime::request_process_private_write(pid, address, bytes, mode)
                .map_err(map_request_error)?;
        }
        operation()
    })
}

/// Fork also needs this barrier: an earlier permission downgrade may still
/// have a writable TLB entry on a running peer even when the PTE is read-only.
/// With no active peer, the current delivered fault needs no additional IPC.
pub(super) fn with_vm_quiescence<T>(
    runtime: &Runtime,
    pid: Word,
    operation: impl FnOnce() -> Result<T, i32>,
) -> Result<T, i32> {
    let group = runtime.vm_group(pid);
    let mut stopped = Vec::new();
    for index in 0..runtime.managed.len() {
        if !active_vm_user(runtime, index, group) {
            continue;
        }
        let process = &runtime.managed[index];
        // An unaccepted fault may be cancelled/replayed: emulation has not run.
        // Do not resume a failed stop whose remote state is not acknowledged.
        if a9n_abi::arch::process_control_block::suspend(process.pcb).is_err() {
            resume(&stopped);
            return Err(EIO);
        }
        stopped.push((process.pid, process.pcb));
    }
    let result = operation();
    resume(&stopped);
    result
}

fn active_vm_user(runtime: &Runtime, index: usize, group: Word) -> bool {
    let process = &runtime.managed[index];
    !(process.pid == 0
            || process.exited
            || process.vm_group != group
            || process.pid == runtime.current_fault_pid
            || process.rootfs.prepared
            || process.parked_context().is_some()
            || process
                .signals()
                .is_some_and(|state| state.stopped || state.stopped_context.is_some()))
}

fn resume(stopped: &[(Word, Word)]) {
    for &(pid, pcb) in stopped.iter().rev() {
        if a9n_abi::arch::process_control_block::resume(pcb).is_err() {
            nanami_runtime::println!("[alter/linux] private VM resume failed pid={}", pid);
            // Match the signal pump's failed-resume policy; exit processing
            // updates managed state through the existing Alpha event path.
            let _ = nanami_runtime::request_process_kill(pid, 11);
        }
    }
}
