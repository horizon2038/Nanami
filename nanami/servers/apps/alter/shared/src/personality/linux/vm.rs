use super::{
    align_up_word, copy_same_process_range, honoka, map_request_error, posix,
    present_graphics_session, sys_framebuffer_mmap, write_target_memory_from, LinuxFile,
    LinuxFileKind, Runtime, Word, ALTER_FB_BYTES, EACCES, EBADF, EINVAL, EIO, ENODEV, ENOMEM,
    EOPNOTSUPP, ESRCH, LINUX_MADV_COLD, LINUX_MADV_DODUMP, LINUX_MADV_DONTDUMP,
    LINUX_MADV_DONTNEED, LINUX_MADV_FREE, LINUX_MADV_HUGEPAGE, LINUX_MADV_MERGEABLE,
    LINUX_MADV_NOHUGEPAGE, LINUX_MADV_NORMAL, LINUX_MADV_PAGEOUT, LINUX_MADV_POPULATE_READ,
    LINUX_MADV_POPULATE_WRITE, LINUX_MADV_RANDOM, LINUX_MADV_SEQUENTIAL, LINUX_MADV_UNMERGEABLE,
    LINUX_MADV_WILLNEED, LINUX_MAP_ANONYMOUS, LINUX_MAP_FIXED, LINUX_MREMAP_FIXED,
    LINUX_MREMAP_MAYMOVE, LINUX_MREMAP_SUPPORTED_FLAGS, LINUX_O_ACCMODE, LINUX_O_WRONLY,
    LINUX_PAGE_SIZE, LINUX_PROT_ALL, LINUX_PROT_NONE, LINUX_PROT_READ, LINUX_PROT_WRITE,
    LINUX_STACK_BYTES, LINUX_STACK_GUARD_BYTES, LINUX_STACK_TOP, LINUX_VIRTUAL_RESERVATION_BASE,
    LINUX_VIRTUAL_RESERVATION_LIMIT,
};

pub(super) fn sys_brk(runtime: &mut Runtime, pid: Word, requested: Word) -> Result<Word, i32> {
    let Some(process) = runtime.managed_process(pid) else {
        return Err(ESRCH);
    };
    if process.program_break == 0 {
        let (base, mapped) =
            map_anonymous_tracked(runtime, pid, 4096, LINUX_PROT_READ | LINUX_PROT_WRITE)?;
        let Some(process) = runtime.managed_process_mut(pid) else {
            return Err(ESRCH);
        };
        process.program_break = base;
        process.mapped_break = base + mapped;
    }
    let current = runtime
        .managed_process(pid)
        .map(|process| process.program_break)
        .ok_or(ESRCH)?;
    if requested == 0 || requested <= current {
        return Ok(current);
    }
    let mapped = runtime
        .managed_process(pid)
        .map(|process| process.mapped_break)
        .ok_or(ESRCH)?;
    if requested > mapped {
        let extra = align_up_word(requested - mapped, 4096);
        let (base, size) =
            map_anonymous_tracked(runtime, pid, extra, LINUX_PROT_READ | LINUX_PROT_WRITE)?;
        if base != mapped {
            return Ok(current);
        }
        let Some(process) = runtime.managed_process_mut(pid) else {
            return Err(ESRCH);
        };
        process.mapped_break = mapped + size;
    }
    let Some(process) = runtime.managed_process_mut(pid) else {
        return Err(ESRCH);
    };
    process.program_break = requested;
    Ok(requested)
}

pub(super) fn sys_mmap(
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
        let (base, granted) = libnanami::request_process_map_anonymous_at(pid, fixed_base, mapped)
            .map_err(|err| {
                let errno = map_request_error(err);
                libnanami::println!(
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
        if base != fixed_base || granted < mapped || !runtime.add_mapping(pid, base, granted, _prot)
        {
            return Err(ENOMEM);
        }
        return Ok(requested_addr);
    }
    if _prot == LINUX_PROT_NONE {
        let (base, _mapped) = reserve_none_mapping(runtime, pid, len)?;
        return Ok(base);
    }

    let (base, _) = map_anonymous_tracked(runtime, pid, len, _prot).map_err(|errno| {
        libnanami::println!(
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
    if flags & 3 != 2 || prot == LINUX_PROT_NONE {
        return Err(EOPNOTSUPP);
    }
    if file.resource & LINUX_O_ACCMODE == LINUX_O_WRONLY {
        return Err(EACCES);
    }
    let (_, file_size, kind, _, _) =
        posix::posix_fstat(runtime.posix_port, file.posix_fd).map_err(map_request_error)?;
    if kind != posix::POSIX_FILE_TYPE_REGULAR {
        return Err(ENODEV);
    }
    let mapped = len.checked_add(LINUX_PAGE_SIZE - 1).ok_or(ENOMEM)? & !(LINUX_PAGE_SIZE - 1);
    let base = sys_mmap(
        runtime,
        pid,
        addr,
        mapped,
        prot,
        flags | LINUX_MAP_ANONYMOUS,
        Word::MAX,
        0,
    )?;
    let result = (|| {
        let bytes = mapped.min(file_size.saturating_sub(offset));
        let mut copied = 0;
        while copied < bytes {
            let chunk = (bytes - copied).min(runtime.posix_read_buffer_size());
            if chunk == 0 {
                return Err(EIO);
            }
            let (read, source) = runtime
                .pread_posix(file.posix_fd, 0, chunk, offset + copied)
                .map_err(map_request_error)?;
            if read == 0 {
                break;
            }
            if read > chunk {
                return Err(EIO);
            }
            write_target_memory_from(pid, base + copied, source, read)?;
            copied += read;
        }
        Ok(base)
    })();
    if result.is_err() {
        let _ = sys_munmap(runtime, pid, base, mapped);
    }
    result
}

pub(super) fn sys_munmap(
    runtime: &mut Runtime,
    pid: Word,
    addr: Word,
    len: Word,
) -> Result<Word, i32> {
    if addr == 0 || len == 0 || (addr & (LINUX_PAGE_SIZE - 1)) != 0 {
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
    let mut cursor = addr;
    while let Some(mapping) = runtime.next_mapping(pid, cursor, end) {
        let start = cursor.max(mapping.base);
        let next = end.min(mapping.base.checked_add(mapping.size).ok_or(EINVAL)?);
        if mapping.prot != LINUX_PROT_NONE {
            release_present_mapping_pages(runtime, pid, start, next - start)?;
        }
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
        runtime.graphics[index].framebuffer_bytes = ALTER_FB_BYTES;
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
    Ok(0)
}

pub(super) fn sys_mprotect(
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
    let mapped = len.checked_add(LINUX_PAGE_SIZE - 1).ok_or(ENOMEM)? & !(LINUX_PAGE_SIZE - 1);
    if !runtime.has_mapping(pid, addr, mapped) {
        return Err(ENOMEM);
    }
    let old_prot = runtime.mapping_prot(pid, addr, mapped).unwrap_or(Word::MAX);
    if old_prot == prot {
        return Ok(0);
    }
    if prot == LINUX_PROT_NONE {
        release_present_mapping_pages(runtime, pid, addr, mapped)?;
    } else {
        ensure_present_mapping_pages(runtime, pid, addr, mapped)?;
    }
    if !runtime.protect_mapping(pid, addr, mapped, prot) {
        return Err(ENOMEM);
    }
    Ok(0)
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
    let mut base = LINUX_VIRTUAL_RESERVATION_BASE;

    loop {
        let end = base.checked_add(mapped).ok_or(ENOMEM)?;
        if end > LINUX_VIRTUAL_RESERVATION_LIMIT {
            return Err(ENOMEM);
        }

        let mut conflict_end = base;
        let mut i = 0usize;
        while i < process.mappings.len() {
            let mapping = process.mappings[i];
            if mapping.base != 0 {
                let mapping_end = mapping.base.checked_add(mapping.size).ok_or(ENOMEM)?;
                if base < mapping_end && mapping.base < end {
                    conflict_end = ::core::cmp::max(conflict_end, mapping_end);
                }
            }
            i += 1;
        }
        if conflict_end == base {
            if !runtime.add_mapping(pid, base, mapped, LINUX_PROT_NONE) {
                return Err(ENOMEM);
            }
            return Ok((base, mapped));
        }
        base = conflict_end
            .checked_add(LINUX_PAGE_SIZE - 1)
            .ok_or(ENOMEM)?
            & !(LINUX_PAGE_SIZE - 1);
    }
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
        if mapping.prot != LINUX_PROT_NONE {
            libnanami::request_process_mapping_release(pid, cursor, next - cursor)
                .map_err(map_request_error)?;
        }
        cursor = next;
    }
    Ok(())
}

pub(super) fn ensure_present_mapping_pages(
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
        if mapping.prot == LINUX_PROT_NONE {
            let (mapped_base, mapped) =
                libnanami::request_process_map_anonymous_at(pid, cursor, next - cursor)
                    .map_err(map_request_error)?;
            if mapped_base != cursor || mapped < next - cursor {
                return Err(ENOMEM);
            }
        }
        cursor = next;
    }
    Ok(())
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

    let old_mapped = align_up_word(old_size, LINUX_PAGE_SIZE);
    let new_mapped = align_up_word(new_size, LINUX_PAGE_SIZE);
    let prot = runtime
        .mapping_prot(pid, old_addr, old_mapped)
        .ok_or(EINVAL)?;

    if new_mapped == old_mapped {
        return Ok(old_addr);
    }
    if new_mapped < old_mapped {
        sys_munmap(runtime, pid, old_addr + new_mapped, old_mapped - new_mapped)?;
        return Ok(old_addr);
    }

    let old_end = old_addr.checked_add(old_mapped).ok_or(ENOMEM)?;
    let extra = new_mapped - old_mapped;
    if (flags & LINUX_MREMAP_FIXED) == 0 {
        if let Ok((base, granted)) =
            libnanami::request_process_map_anonymous_at(pid, old_end, extra)
                .map_err(map_request_error)
        {
            if base == old_end && granted >= extra && runtime.add_mapping(pid, base, granted, prot)
            {
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
    copy_same_process_range(
        runtime,
        pid,
        old_addr,
        target,
        ::core::cmp::min(old_mapped, new_mapped),
    )?;
    sys_munmap(runtime, pid, old_addr, old_mapped)?;
    Ok(target)
}

pub(super) fn map_anonymous_tracked(
    runtime: &mut Runtime,
    pid: Word,
    len: Word,
    prot: Word,
) -> Result<(Word, Word), i32> {
    let (base, mapped) =
        libnanami::request_process_map_anonymous(pid, len).map_err(map_request_error)?;
    if !runtime.add_mapping(pid, base, mapped, prot) {
        return Err(ENOMEM);
    }
    Ok((base, mapped))
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
    )
}
