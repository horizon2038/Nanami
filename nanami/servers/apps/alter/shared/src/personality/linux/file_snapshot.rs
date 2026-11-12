//! Alias only immutable file pages; requested zero pages have separate backing.
use super::{map_request_error, private_memory, Runtime, Word, EINVAL, LINUX_PAGE_SIZE};

const DEFERRED: bool = match option_env!("NANAMI_DEFER_ELF_SNAPSHOTS") {
        Some(value) => value.len() == 1 && value.as_bytes()[0] == b'1',
        None => cfg!(target_arch = "x86_64"),
};
pub(super) fn deferred_prefix(runtime: &Runtime, source: Word, bytes: Word, prot: Word) -> Option<Word> {
    if !DEFERRED || prot != 1 { return None; }
    runtime.image_cache.elf_snapshot_prefix(source, bytes)
}

/// The complete stable pread window belongs to this call. An uncached DSO
/// needs no native source object: initialize guest-owned RAM directly, with
/// deferred small PTEs and the ordinary huge-page opportunity preserved.
pub(super) fn map_uncached_readonly(runtime: &mut Runtime, pid: Word, base: Word,
    mapped: Word, source: Word, bytes: Word, offset: Word, permissions: Option<Word>,
) -> Result<bool, i32> {
    if !DEFERRED || !crate::common::image_cache::IMMUTABLE_STORAGE || !crate::common::image_cache::UNMAPPED_SOURCES
        || permissions != Some(1) || offset != 0 || bytes < 256 * 1024
        || bytes > mapped || mapped > runtime.posix_image_read_buffer_size()
        || source == 0 || source.checked_add(bytes).is_none()
        || base == 0 || base.checked_add(mapped).is_none()
        || (base | mapped) & (LINUX_PAGE_SIZE - 1) != 0 { return Ok(false); }
    // The caller just read these bytes into its owned POSIX window. Parse
    // before making any other I/O request or constructing an opaque source.
    let image = unsafe { core::slice::from_raw_parts(source as *const u8, bytes) };
    let Ok(metadata) = crate::elf::parse_elf64_image(image) else { return Ok(false); };
    let Some(prefix) = crate::common::image_cache::deferred_elf_prefix(&metadata, bytes) else { return Ok(false); };
    private_memory::with_mapping_mutation(runtime, pid, base, mapped, true, ||
        nanami_runtime::request_process_map_initialized_deferred_at(pid, base, mapped,
            source, bytes, prefix).map_err(map_request_error))?;
    #[cfg(target_os = "none")]
    if option_env!("NANAMI_DIRECT_DEFERRED_DIAG") == Some("1") {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static REPORTS: AtomicUsize = AtomicUsize::new(0);
        if REPORTS.fetch_add(1, Ordering::Relaxed) < 16 {
            nanami_runtime::println!("[linux.mmap] direct-deferred-elf pid={} bytes={} pages={} eager-prefix={} native-sources=0",
                pid, bytes, mapped / LINUX_PAGE_SIZE, prefix / LINUX_PAGE_SIZE);
        }
    }
    Ok(true)
}

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
    private_memory::mark_readonly_shared(runtime, pid);
    // From here, errors may leave published aliases. The mmap caller owns
    // whole-range cleanup; never retry with a copy constructor.
    if let Some(prefix) = deferred_prefix(runtime, source, bytes, prot) {
        nanami_runtime::request_process_map_private_snapshot_prefix(pid, base, backed, source, prefix)
            .map_err(map_request_error)?;
        #[cfg(target_os = "none")]
        if option_env!("NANAMI_DEFER_ELF_DIAG") == Some("1")
            && runtime.managed_process(pid).is_some_and(|process|
                process.image_name[..process.image_name_len].rsplit(|byte| *byte == b'/').next()
                    == Some(&b"xfe"[..])) {
            use core::sync::atomic::{AtomicUsize, Ordering};
            static REPORTS: AtomicUsize = AtomicUsize::new(0);
            if REPORTS.fetch_add(1, Ordering::Relaxed) < 16 {
                nanami_runtime::println!("[linux.mmap] deferred-elf-files pid={} pages={} eager={}",
                    pid, backed / LINUX_PAGE_SIZE, prefix / LINUX_PAGE_SIZE);
            }
        }
    } else {
        nanami_runtime::request_process_map_private_snapshot(pid, base, backed, source, prot)
            .map_err(map_request_error)?;
    }
    #[cfg(target_os = "none")]
    if option_env!("NANAMI_IMAGE_SOURCE_DIAG") == Some("1")
        && runtime.image_cache.unmapped_source_range(source, backed)
        && runtime.managed_process(pid).is_some_and(|process|
            process.image_name[..process.image_name_len].rsplit(|byte| *byte == b'/').next()
                == Some(&b"xfe"[..]))
    {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static REPORTS: AtomicUsize = AtomicUsize::new(0);
        if REPORTS.fetch_add(1, Ordering::Relaxed) < 16 {
            nanami_runtime::println!("[linux.mmap] unmapped-source-files pid={} bytes={} pages={} native-maps=0", pid, bytes, backed / LINUX_PAGE_SIZE);
        }
    }
    if backed < mapped {
        let zero_base = base + backed;
        let zero_bytes = mapped - backed;
        private_memory::with_mapping_mutation(runtime, pid, zero_base, zero_bytes, true, || {
        if final_permissions {
            nanami_runtime::request_process_map_initialized_protected_at(
                pid, zero_base, zero_bytes, 0, 0, prot,
            ).map_err(map_request_error)?;
        } else {
            nanami_runtime::request_process_map_initialized_at(pid, zero_base, zero_bytes, 0, 0)
                .map_err(map_request_error)?;
        }
        Ok(())
        })?;
        if !final_permissions {
            private_memory::with_mapping_mutation(runtime, pid, zero_base, zero_bytes, false, ||
                nanami_runtime::request_process_mapping_protect(pid, zero_base, zero_bytes, prot)
                    .map_err(map_request_error))?;
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
