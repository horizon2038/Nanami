//! Alias only immutable file pages; requested zero pages have separate backing.
use super::{map_request_error, private_memory, Runtime, Word, EINVAL, LINUX_PAGE_SIZE};

pub(super) fn map_readonly(
    runtime: &mut Runtime, pid: Word, base: Word, mapped: Word,
    source: Word, bytes: Word, prot: Word, final_permissions: bool,
) -> Result<bool, i32> {
    if !cfg!(target_arch = "x86_64") || !matches!(prot, 1 | 5)
        || bytes == 0 || bytes > mapped
        || (source | mapped) & (LINUX_PAGE_SIZE - 1) != 0
    {
        return Ok(false);
    }
    let backed = bytes.checked_add(LINUX_PAGE_SIZE - 1).ok_or(EINVAL)?
        & !(LINUX_PAGE_SIZE - 1);
    if backed > mapped || !runtime.image_cache.immutable_range(source, backed) {
        return Ok(false);
    }
    if base & (LINUX_PAGE_SIZE - 1) != 0 || base.checked_add(mapped).is_none() {
        return Err(EINVAL);
    }
    private_memory::mark_shared(runtime, pid);
    // From here, errors may leave published aliases. The mmap caller owns
    // whole-range cleanup; never retry with a copy constructor.
    nanami_runtime::request_process_map_private_snapshot(pid, base, backed, source, prot)
        .map_err(map_request_error)?;
    if backed < mapped {
        let zero_base = base + backed;
        let zero_bytes = mapped - backed;
        if final_permissions {
            nanami_runtime::request_process_map_initialized_protected_at(
                pid, zero_base, zero_bytes, 0, 0, prot,
            ).map_err(map_request_error)?;
        } else {
            nanami_runtime::request_process_map_initialized_at(pid, zero_base, zero_bytes, 0, 0)
                .map_err(map_request_error)?;
            nanami_runtime::request_process_mapping_protect(pid, zero_base, zero_bytes, prot)
                .map_err(map_request_error)?;
        }
    }
    #[cfg(target_os = "none")]
    if bytes > 2 * 1024 * 1024 && option_env!("NANAMI_LARGE_SNAPSHOT_DIAG") == Some("1") {
        use core::sync::atomic::{AtomicBool, Ordering};
        static FILES: AtomicBool = AtomicBool::new(false);
        if runtime.managed_process(pid).is_some_and(|process| {
            process.image_name[..process.image_name_len].rsplit(|byte| *byte == b'/').next()
                == Some(&b"xfe"[..])
        }) && !FILES.swap(true, Ordering::Relaxed) {
            nanami_runtime::println!("[linux.mmap] large-snapshot-files pid={} file={} alias={} zero={}", pid, bytes, backed, mapped - backed);
        }
    }
    Ok(true)
}
