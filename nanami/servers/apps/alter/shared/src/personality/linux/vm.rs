use super::{
    align_up_word, copy_same_process_range, honoka, map_request_error, posix,
    present_graphics_session, sys_framebuffer_mmap, LinuxFile, LinuxFileKind, Runtime, Word,
    EACCES, EBADF, EINVAL, ENODEV, ENOMEM, EOPNOTSUPP, ESRCH, LINUX_MADV_COLD, LINUX_MADV_DODUMP,
    LINUX_MADV_DONTDUMP, LINUX_MADV_DONTNEED, LINUX_MADV_FREE, LINUX_MADV_HUGEPAGE,
    LINUX_MADV_MERGEABLE, LINUX_MADV_NOHUGEPAGE, LINUX_MADV_NORMAL, LINUX_MADV_PAGEOUT,
    LINUX_MADV_POPULATE_READ, LINUX_MADV_POPULATE_WRITE, LINUX_MADV_RANDOM, LINUX_MADV_SEQUENTIAL,
    LINUX_MADV_UNMERGEABLE, LINUX_MADV_WILLNEED, LINUX_MAP_ANONYMOUS, LINUX_MAP_FIXED,
    LINUX_MREMAP_FIXED, LINUX_MREMAP_MAYMOVE, LINUX_MREMAP_SUPPORTED_FLAGS, LINUX_O_ACCMODE,
    LINUX_O_WRONLY, LINUX_PAGE_SIZE, LINUX_PROT_ALL, LINUX_PROT_EXEC, LINUX_PROT_NONE, LINUX_PROT_READ,
    LINUX_PROT_WRITE, LINUX_STACK_BYTES, LINUX_STACK_GUARD_BYTES, LINUX_STACK_TOP,
    LINUX_VIRTUAL_RESERVATION_BASE, LINUX_VIRTUAL_RESERVATION_LIMIT,
};

pub(super) fn sys_mmap(
    runtime: &mut Runtime,
    pid: Word,
    requested_addr: Word,
    len: Word,
    prot: Word,
    flags: Word,
    fd: Word,
    offset: Word,
) -> Result<Word, i32> {
    // Only private anonymous/ordinary-file mmap confines all target backing
    // changes to its returned extent. Shared/device mappings retain global
    // invalidation. A failed request may have changed a prefix: never rebase.
    let private = flags & 3 == 2 && (flags & LINUX_MAP_ANONYMOUS != 0
        || runtime.linux_file(pid, fd).is_some_and(|file| file.kind == LinuxFileKind::Posix));
    let before = private.then(|| super::guest_views::BeforeMmap::capture(runtime, pid)).flatten();
    let result = mmap_inner(runtime, pid, requested_addr, len, prot, flags, fd, offset);
    if let (Some(before), Ok(base)) = (before, result) {
        if let Some(mapped) = len.checked_add(LINUX_PAGE_SIZE - 1).map(|bytes| bytes & !(LINUX_PAGE_SIZE - 1)) {
            before.completed(runtime, pid, base, mapped);
        }
    }
    result
}

fn mmap_inner(
    runtime: &mut Runtime,
    pid: Word,
    requested_addr: Word,
    len: Word,
    _prot: Word,
    flags: Word,
    fd: Word,
    offset: Word,
) -> Result<Word, i32> {
    if len == 0 {
        return Err(EINVAL);
    }
    if (_prot & !LINUX_PROT_ALL) != 0 {
        return Err(EINVAL);
    }
    if (flags & LINUX_MAP_FIXED) != 0 && (requested_addr & (LINUX_PAGE_SIZE - 1)) != 0 {
        return Err(EINVAL);
    }
    if (flags & LINUX_MAP_ANONYMOUS) == 0 {
        let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
        return match file.kind {
            LinuxFileKind::IoUring => super::io_uring::map(
                runtime,
                pid,
                requested_addr,
                len,
                _prot,
                flags,
                file,
                offset,
            ),
            LinuxFileKind::MemoryFile => super::memfd::map(
                runtime,
                pid,
                requested_addr,
                len,
                _prot,
                flags,
                file,
                offset,
            ),
            LinuxFileKind::Framebuffer => {
                if requested_addr != 0 && (flags & LINUX_MAP_FIXED) != 0 {
                    return Err(EINVAL);
                }
                sys_framebuffer_mmap(runtime, pid, file, len, _prot, offset)
            }
            LinuxFileKind::Posix => sys_file_mmap(
                runtime,
                pid,
                requested_addr,
                len,
                _prot,
                flags,
                file,
                offset,
            ),
            _ => Err(ENODEV),
        };
    }
    if flags & 3 == 1 {
        return super::memfd::map_anonymous(runtime, pid, requested_addr, len, _prot, flags);
    }
    if requested_addr != 0 && (flags & LINUX_MAP_FIXED) != 0 {
        let fixed_base = requested_addr & !(LINUX_PAGE_SIZE - 1);
        let fixed_offset = requested_addr - fixed_base;
        let mapped = align_up_word(
            len.checked_add(fixed_offset).ok_or(ENOMEM)?,
            LINUX_PAGE_SIZE,
        );
        // MAP_FIXED replaces every overlap, including partially mapped ranges.
        sys_munmap(runtime, pid, fixed_base, mapped)?;
        if _prot == LINUX_PROT_NONE {
            if !runtime.add_mapping(pid, fixed_base, mapped, _prot) {
                return Err(ENOMEM);
            }
            return Ok(requested_addr);
        }
        map_anonymous_at_tracked(runtime, pid, fixed_base, mapped, _prot)
            .map_err(|err| {
                let errno = err;
                nanami_runtime::println!(
                    "[alter/linux] mmap fixed failed pid={} addr={:#x} base={:#x} len={:#x} mapped={:#x} prot={:#x} flags={:#x} fd={:#x} off={:#x} errno={}",
                    pid,
                    requested_addr,
                    fixed_base,
                    len,
                    mapped,
                    _prot,
                    flags,
                    fd,
                    offset,
                    errno
                );
                errno
            })?;
        return Ok(requested_addr);
    }
    if _prot == LINUX_PROT_NONE {
        let (base, _mapped) = reserve_none_mapping(runtime, pid, len)?;
        return Ok(base);
    }

    let (base, _) = map_anonymous_tracked(runtime, pid, len, _prot).map_err(|errno| {
        nanami_runtime::println!(
            "[alter/linux] mmap failed pid={} addr={:#x} len={:#x} prot={:#x} flags={:#x} fd={:#x} off={:#x} errno={}",
            pid,
            requested_addr,
            len,
            _prot,
            flags,
            fd,
            offset,
            errno
        );
        errno
    })?;
    Ok(base)
}

pub(super) fn sys_file_mmap(
    runtime: &mut Runtime,
    pid: Word,
    addr: Word,
    len: Word,
    prot: Word,
    flags: Word,
    file: LinuxFile,
    offset: Word,
) -> Result<Word, i32> {
    if offset & (LINUX_PAGE_SIZE - 1) != 0 || offset > isize::MAX as Word {
        return Err(EINVAL);
    }
    if flags & 3 != 1 && flags & 3 != 2 {
        return Err(EOPNOTSUPP);
    }
    if file.resource & LINUX_O_ACCMODE == LINUX_O_WRONLY {
        return Err(EACCES);
    }
    runtime.with_file_reader(file, |runtime| {
        sys_file_mmap_inner(runtime, pid, addr, len, prot, flags, file, offset)
    })
}

fn sys_file_mmap_inner(
    runtime: &mut Runtime,
    pid: Word,
    addr: Word,
    len: Word,
    prot: Word,
    flags: Word,
    file: LinuxFile,
    offset: Word,
) -> Result<Word, i32> {
    let (identity, file_size, kind, _, _) = runtime.file_reader.fstat(
        runtime.posix_port,
        file.posix_fd,
        posix::POSIX_STAT_INCLUDE_VERSION,
    )
    .map_err(map_request_error)?;
    if kind != posix::POSIX_FILE_TYPE_REGULAR {
        return Err(ENODEV);
    }
    if flags & 3 == 1 {
        return super::memfd::map_read_file(
            runtime, pid, addr, len, prot, flags, file, file_size, offset,
        );
    }
    if prot == LINUX_PROT_NONE {
        return Err(EOPNOTSUPP);
    }
    let mapped = len.checked_add(LINUX_PAGE_SIZE - 1).ok_or(ENOMEM)? & !(LINUX_PAGE_SIZE - 1);
    if super::file_remap::reuse(runtime, pid, identity, file_size, addr, mapped, offset, prot, flags)? {
        return Ok(addr);
    }
    // The outer private file mmap commits the one saved view generation only
    // after population and final protection succeed, not at this reservation.
    let base = mmap_inner(
        runtime,
        pid,
        addr,
        mapped,
        LINUX_PROT_NONE,
        flags | LINUX_MAP_ANONYMOUS,
        Word::MAX,
        0,
    )?;
    let result = (|| {
        if !runtime.can_protect_mapping(pid, base, mapped) {
            return Err(ENOMEM);
        }
        let bytes = mapped.min(file_size.saturating_sub(offset));
        let mut initialized = false;
        let mut final_permissions = false;
        let mut deferred_initialization = false;
        if bytes != 0 {
            let file_source = if prot == LINUX_PROT_READ || prot == (LINUX_PROT_READ | LINUX_PROT_EXEC) {
                crate::loader::readonly_file_mapping_source
            } else {
                crate::loader::file_mapping_source
            };
            if let Some(source) = file_source(
                runtime,
                file.posix_fd,
                identity,
                file_size,
                offset,
                bytes,
            )
            .map_err(super::map_load_error)?
            {
                if super::file_snapshot::map_readonly(
                    runtime, pid, base, mapped, source, bytes, prot, FINAL_PERMISSIONS,
                )? {
                    if !runtime.protect_mapping(pid, base, mapped, prot) {
                        return Err(ENOMEM);
                    }
                    super::file_remap::remember(runtime, pid, identity, file_size, base, mapped, offset);
                    if super::file_snapshot::deferred_prefix(runtime, source, bytes, prot).is_some() {
                        runtime.readonly_mapping.set(runtime.readonly_mapping.get().with_deferred_snapshot());
                    }
                    return Ok(base);
                } else {
                    super::private_memory::with_mapping_mutation(runtime, pid, base, mapped, true, || {
                    if FINAL_PERMISSIONS {
                        nanami_runtime::request_process_map_initialized_protected_at(
                            pid, base, mapped, source, bytes, prot,
                        ).map_err(map_request_error)?;
                    } else {
                        nanami_runtime::request_process_map_initialized_at(
                            pid, base, mapped, source, bytes,
                        ).map_err(map_request_error)?;
                    }
                    Ok(())
                    })?;
                    final_permissions = FINAL_PERMISSIONS;
                }
                initialized = true;
            }
        }
        if !initialized {
            deferred_initialization = super::file_mapping::initialize(
                runtime, pid, base, mapped, file, offset, bytes,
                STREAMED_FINAL_PERMISSIONS.then_some(prot),
            )?;
            final_permissions = STREAMED_FINAL_PERMISSIONS;
            #[cfg(target_os = "none")]
            if final_permissions && option_env!("NANAMI_STREAMED_MMAP_DIAG") == Some("1") {
                use core::sync::atomic::{AtomicBool, Ordering};
                static FILES: AtomicBool = AtomicBool::new(false);
                if runtime.managed_process(pid).is_some_and(|process| {
                    process.image_name[..process.image_name_len].rsplit(|byte| *byte == b'/').next()
                        == Some(&b"xfe"[..])
                }) && !FILES.swap(true, Ordering::Relaxed) {
                    nanami_runtime::println!("[linux.mmap] streamed-final-files pid={} bytes={} mapped={}", pid, bytes, mapped);
                }
            }
        }
        if !final_permissions {
            set_hardware_protection(runtime, pid, base, mapped, prot)?;
        }
        if !runtime.protect_mapping(pid, base, mapped, prot) {
            return Err(ENOMEM);
        }
        super::file_remap::remember_copy(runtime, pid, identity, file_size, base, mapped, offset, prot);
        if deferred_initialization {
            runtime.readonly_mapping.set(runtime.readonly_mapping.get().with_deferred_snapshot());
        }
        Ok(base)
    })();
    if result.is_err() {
        let _ = sys_munmap(runtime, pid, base, mapped);
    }
    result
}

// Initialize private file backing before publishing leaves with final rights.
// Requires the matching Alpha constructor; =0 restores the old two-step path.
const FINAL_PERMISSIONS: bool = match option_env!("NANAMI_MAP_FINAL_PERMISSIONS") {
    Some(value) => value.len() == 1 && value.as_bytes()[0] == b'1',
    None => true,
};

// Private file fallback only. Anonymous mmap retains its existing constructor.
const STREAMED_FINAL_PERMISSIONS: bool = FINAL_PERMISSIONS
    && match option_env!("NANAMI_STREAMED_MMAP_FINAL") {
        Some(value) => value.len() == 1 && value.as_bytes()[0] == b'1',
        None => true,
    };

pub(super) fn sys_munmap(
    runtime: &mut Runtime,
    pid: Word,
    addr: Word,
    len: Word,
) -> Result<Word, i32> {
    if (addr & (LINUX_PAGE_SIZE - 1)) != 0 {
        return Err(EINVAL);
    }
    let mapped = len.checked_add(LINUX_PAGE_SIZE - 1).ok_or(ENOMEM)? & !(LINUX_PAGE_SIZE - 1);
    let framebuffer_index = runtime.graphics.iter().position(|session| {
        session.active
            && session.guest_pid == pid
            && session.guest_framebuffer == addr
            && align_up_word(session.guest_framebuffer_bytes, LINUX_PAGE_SIZE) == mapped
    });
    if let Some(index) = framebuffer_index {
        let _ = present_graphics_session(runtime, index as Word + 1, pid);
    }
    let end = addr.checked_add(mapped).ok_or(EINVAL)?;
    if !runtime.can_remove_mapping(pid, addr, mapped) {
        return Err(ENOMEM);
    }
    // A failed release can already have removed a prefix of the hardware
    // mappings. Reject old read leases even if logical metadata stays intact.
    if let Some(process) = runtime.managed_process(pid) {
        process.mappings.invalidate_views();
    }
    super::memfd::unmap(runtime, pid, addr, mapped)?;
    let mut cursor = addr;
    while let Some(mapping) = runtime.next_mapping(pid, cursor, end) {
        let start = cursor.max(mapping.base);
        let next = end.min(mapping.base.checked_add(mapping.size).ok_or(EINVAL)?);
        release_present_mapping_pages(runtime, pid, start, next - start)?;
        if !runtime.remove_mapping(pid, start, next - start) {
            return Err(ENOMEM);
        }
        cursor = next;
    }
    if let Some(index) = framebuffer_index {
        let session = runtime.graphics[index];
        honoka::honoka_detach_logical_framebuffer(session.honoka_port, session.window_id)
            .map_err(map_request_error)?;
        runtime.graphics[index].guest_pid = 0;
        runtime.graphics[index].guest_framebuffer = 0;
        runtime.graphics[index].guest_framebuffer_bytes = 0;
        runtime.graphics[index].framebuffer = 0;
    }
    Ok(0)
}

pub(super) fn sys_msync(
    runtime: &mut Runtime,
    pid: Word,
    addr: Word,
    len: Word,
) -> Result<Word, i32> {
    if addr == 0 || len == 0 {
        return Err(EINVAL);
    }
    let mut index = 0usize;
    while index < runtime.graphics.len() {
        let session = runtime.graphics[index];
        if session.active
            && session.guest_pid == pid
            && session.guest_framebuffer != 0
            && runtime
                .managed_process(pid)
                .map(|process| process.graphics_session == index as Word + 1)
                .unwrap_or(false)
            && addr >= session.guest_framebuffer
            && addr.saturating_add(len)
                <= session
                    .guest_framebuffer
                    .saturating_add(session.guest_framebuffer_bytes)
        {
            present_graphics_session(runtime, index as Word + 1, pid)?;
            return Ok(0);
        }
        index += 1;
    }
    super::memfd::sync_files(runtime, pid, addr, len)?;
    Ok(0)
}

pub(super) fn sys_mprotect(
    runtime: &mut Runtime, pid: Word, addr: Word, len: Word, prot: Word,
) -> Result<Word, i32> {
    super::memory_protection::protect(runtime, pid, addr, len, prot)
}

pub(super) fn sys_madvise(
    runtime: &Runtime,
    pid: Word,
    addr: Word,
    len: Word,
    advice: Word,
) -> Result<Word, i32> {
    if (addr & (LINUX_PAGE_SIZE - 1)) != 0 {
        return Err(EINVAL);
    }
    if len == 0 {
        return Ok(0);
    }
    if !matches!(
        advice,
        LINUX_MADV_NORMAL
            | LINUX_MADV_RANDOM
            | LINUX_MADV_SEQUENTIAL
            | LINUX_MADV_WILLNEED
            | LINUX_MADV_DONTNEED
            | LINUX_MADV_FREE
            | LINUX_MADV_MERGEABLE
            | LINUX_MADV_UNMERGEABLE
            | LINUX_MADV_HUGEPAGE
            | LINUX_MADV_NOHUGEPAGE
            | LINUX_MADV_DONTDUMP
            | LINUX_MADV_DODUMP
            | LINUX_MADV_COLD
            | LINUX_MADV_PAGEOUT
            | LINUX_MADV_POPULATE_READ
            | LINUX_MADV_POPULATE_WRITE
    ) {
        return Err(EINVAL);
    }
    let mapped = len.checked_add(LINUX_PAGE_SIZE - 1).ok_or(ENOMEM)? & !(LINUX_PAGE_SIZE - 1);
    if !runtime.has_mapping(pid, addr, mapped) {
        return Err(ENOMEM);
    }

    // Nanami currently keeps anonymous mappings resident. These Linux advice
    // values are optimization hints, so preserving the mapping is compatible.
    Ok(0)
}

pub(super) fn reserve_none_mapping(
    runtime: &mut Runtime,
    pid: Word,
    len: Word,
) -> Result<(Word, Word), i32> {
    let mapped = len.checked_add(LINUX_PAGE_SIZE - 1).ok_or(ENOMEM)? & !(LINUX_PAGE_SIZE - 1);
    let process = runtime.managed_process(pid).ok_or(ESRCH)?;
    let base = super::address_gap::private_first_fit(
        process.mappings.iter().map(|mapping| (mapping.base, mapping.size)),
        LINUX_VIRTUAL_RESERVATION_BASE,
        mapped,
        LINUX_VIRTUAL_RESERVATION_LIMIT,
        process.mappings.reservation_cursor(),
    ).ok_or(ENOMEM)?;
    if !runtime.add_mapping(pid, base, mapped, LINUX_PROT_NONE) {
        return Err(ENOMEM);
    }
    runtime.managed_process(pid).ok_or(ESRCH)?.mappings
        .advance_reservation_cursor(base + mapped);
    Ok((base, mapped))
}

pub(super) fn release_present_mapping_pages(
    runtime: &Runtime,
    pid: Word,
    base: Word,
    size: Word,
) -> Result<(), i32> {
    let mut cursor = base;
    let end = base.checked_add(size).ok_or(ENOMEM)?;
    while cursor < end {
        let mapping = runtime.mapping_at(pid, cursor).ok_or(ENOMEM)?;
        let next = end.min(mapping.base.checked_add(mapping.size).ok_or(ENOMEM)?);
        super::private_memory::with_mapping_mutation(runtime, pid, cursor, next - cursor, false, ||
            nanami_runtime::request_process_mapping_release_sparse(pid, cursor, next - cursor)
                .map_err(map_request_error))?;
        cursor = next;
    }
    Ok(())
}

fn set_hardware_protection(runtime: &Runtime, pid: Word, base: Word, size: Word, prot: Word) -> Result<(), i32> {
    super::private_memory::with_mapping_mutation(runtime, pid, base, size, false, ||
        nanami_runtime::request_process_mapping_protect(pid, base, size, prot).map_err(map_request_error))
}

pub(super) fn sys_mremap(
    runtime: &mut Runtime,
    pid: Word,
    old_addr: Word,
    old_size: Word,
    new_size: Word,
    flags: Word,
    new_addr: Word,
) -> Result<Word, i32> {
    if old_addr == 0 || old_size == 0 || new_size == 0 || (old_addr & (LINUX_PAGE_SIZE - 1)) != 0 {
        return Err(EINVAL);
    }
    if (flags & !LINUX_MREMAP_SUPPORTED_FLAGS) != 0 {
        return Err(EINVAL);
    }
    if (flags & LINUX_MREMAP_FIXED) != 0
        && ((flags & LINUX_MREMAP_MAYMOVE) == 0
            || new_addr == 0
            || (new_addr & (LINUX_PAGE_SIZE - 1)) != 0)
    {
        return Err(EINVAL);
    }

    let old_mapped =
        old_size.checked_add(LINUX_PAGE_SIZE - 1).ok_or(ENOMEM)? & !(LINUX_PAGE_SIZE - 1);
    let new_mapped =
        new_size.checked_add(LINUX_PAGE_SIZE - 1).ok_or(ENOMEM)? & !(LINUX_PAGE_SIZE - 1);
    let prot = runtime
        .mapping_prot(pid, old_addr, old_mapped)
        .ok_or(EINVAL)?;

    if new_mapped == old_mapped && flags & LINUX_MREMAP_FIXED == 0 {
        return Ok(old_addr);
    }
    if new_mapped < old_mapped && flags & LINUX_MREMAP_FIXED == 0 {
        sys_munmap(runtime, pid, old_addr + new_mapped, old_mapped - new_mapped)?;
        return Ok(old_addr);
    }

    let old_end = old_addr.checked_add(old_mapped).ok_or(ENOMEM)?;
    if let Some(target) = super::memfd::remap(
        runtime, pid, old_addr, old_mapped, new_mapped, flags, new_addr, prot,
    )? {
        return Ok(target);
    }
    let extra = new_mapped.saturating_sub(old_mapped);
    if (flags & LINUX_MREMAP_FIXED) == 0 && extra != 0 {
        if let Ok((base, granted)) = super::private_memory::with_mapping_mutation(
            runtime, pid, old_end, extra, true, ||
            nanami_runtime::request_process_map_anonymous_at(pid, old_end, extra).map_err(map_request_error))
        {
            if base == old_end && granted >= extra && runtime.add_mapping(pid, base, granted, prot)
            {
                set_hardware_protection(runtime, pid, base, granted, prot)?;
                return Ok(old_addr);
            }
        }
    }

    if (flags & LINUX_MREMAP_MAYMOVE) == 0 {
        return Err(ENOMEM);
    }

    let target = if (flags & LINUX_MREMAP_FIXED) != 0 {
        sys_mmap(
            runtime,
            pid,
            new_addr,
            new_mapped,
            prot,
            LINUX_MAP_FIXED | LINUX_MAP_ANONYMOUS,
            !0,
            0,
        )?
    } else {
        sys_mmap(
            runtime,
            pid,
            0,
            new_mapped,
            prot,
            LINUX_MAP_ANONYMOUS,
            !0,
            0,
        )?
    };
    if let Err(errno) = copy_same_process_range(
        runtime,
        pid,
        old_addr,
        target,
        ::core::cmp::min(old_mapped, new_mapped),
    ) {
        let _ = sys_munmap(runtime, pid, target, new_mapped);
        return Err(errno);
    }
    sys_munmap(runtime, pid, old_addr, old_mapped)?;
    Ok(target)
}

pub(super) fn map_anonymous_tracked(
    runtime: &mut Runtime,
    pid: Word,
    len: Word,
    prot: Word,
) -> Result<(Word, Word), i32> {
    map_anonymous_tracked_impl(runtime, pid, None, len, prot)
}

pub(super) fn map_anonymous_at_tracked(
    runtime: &mut Runtime, pid: Word, base: Word, len: Word, prot: Word,
) -> Result<(Word, Word), i32> {
    map_anonymous_tracked_impl(runtime, pid, Some(base), len, prot)
}

fn map_anonymous_tracked_impl(
    runtime: &mut Runtime, pid: Word, requested: Option<Word>, len: Word, prot: Word,
) -> Result<(Word, Word), i32> {
    super::anonymous_mapping::map(runtime, pid, requested, len, prot)
}

pub(super) fn register_linux_stack_mappings(runtime: &mut Runtime, pid: Word) -> bool {
    let stack_base = LINUX_STACK_TOP - LINUX_STACK_BYTES;
    runtime.reset_stack_mapping(
        pid,
        stack_base,
        LINUX_STACK_BYTES,
        LINUX_PROT_READ | LINUX_PROT_WRITE,
    ) && runtime.add_mapping(
        pid,
        LINUX_STACK_TOP,
        LINUX_STACK_GUARD_BYTES,
        LINUX_PROT_NONE,
    ) && set_hardware_protection(
        runtime,
        pid,
        stack_base,
        LINUX_STACK_BYTES,
        LINUX_PROT_READ | LINUX_PROT_WRITE,
    )
    .is_ok()
}
