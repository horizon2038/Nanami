//! Reclaimable RAM files and real shared page aliases. No guest-data snapshots.
use super::*;
use crate::state::poll_objects::{MemoryFile, MemoryMapping, Object};
use alloc::{collections::BTreeMap, vec::Vec};

const MFD_CLOEXEC: Word = 1;
const MFD_ALLOW_SEALING: Word = 2;
const SEAL_SEAL: Word = 1;
const SEAL_SHRINK: Word = 2;
const SEAL_GROW: Word = 4;
const SEAL_WRITE: Word = 8;
const SEALS: Word = SEAL_SEAL | SEAL_SHRINK | SEAL_GROW | SEAL_WRITE;

fn object(runtime: &Runtime, id: Word) -> Result<&MemoryFile, i32> {
    match runtime.poll_objects.get(id) {
        Some(Object::Memory(file)) => Ok(file),
        _ => Err(EBADF),
    }
}

fn object_mut(runtime: &mut Runtime, id: Word) -> Result<&mut MemoryFile, i32> {
    match runtime.poll_objects.get_mut(id) {
        Some(Object::Memory(file)) => Ok(file),
        _ => Err(EBADF),
    }
}

pub(super) fn create(
    runtime: &mut Runtime,
    pid: Word,
    name: Word,
    flags: Word,
) -> Result<Word, i32> {
    if flags & !(MFD_CLOEXEC | MFD_ALLOW_SEALING) != 0 {
        return Err(EINVAL);
    }
    if read_c_string(runtime, pid, name)? > 249 {
        return Err(EINVAL);
    }
    eventfd::allocate(
        runtime,
        pid,
        LinuxFileKind::MemoryFile,
        Object::Memory(MemoryFile {
            anonymous: false,
            pages: BTreeMap::new(),
            size: 0,
            offset: 0,
            status_flags: 0,
            seals: if flags & MFD_ALLOW_SEALING == 0 {
                SEAL_SEAL
            } else {
                0
            },
            maps: Vec::new(),
        }),
        if flags & MFD_CLOEXEC != 0 {
            LINUX_O_CLOEXEC
        } else {
            0
        },
    )
}

pub(super) fn size(runtime: &Runtime, file: LinuxFile) -> Result<Word, i32> {
    Ok(object(runtime, file.resource)?.size)
}

fn page(runtime: &mut Runtime, id: Word, index: Word) -> Result<Word, i32> {
    if let Some(address) = object(runtime, id)?.pages.get(&index) {
        return Ok(*address);
    }
    let address =
        nanami_runtime::request_memory_create(LINUX_PAGE_SIZE).map_err(map_request_error)?;
    object_mut(runtime, id)?.pages.insert(index, address);
    Ok(address)
}

pub(super) fn io(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    buffer: Word,
    bytes: Word,
    positioned: Option<Word>,
    write: bool,
) -> Result<Word, i32> {
    let memory = object(runtime, file.resource)?;
    let offset = if write && memory.status_flags & LINUX_O_APPEND != 0 {
        memory.size
    } else {
        positioned.unwrap_or(memory.offset)
    };
    if (offset as isize) < 0 {
        return Err(EINVAL);
    }
    let end = offset
        .checked_add(bytes)
        .filter(|end| *end <= isize::MAX as Word)
        .ok_or(EINVAL)?;
    if write && memory.seals & SEAL_WRITE != 0 {
        return Err(EPERM);
    }
    if write && end > memory.size && memory.seals & SEAL_GROW != 0 {
        return Err(EPERM);
    }
    let count = if write {
        bytes
    } else {
        bytes.min(memory.size.saturating_sub(offset))
    };
    let mut copied = 0;
    while copied < count {
        let current = offset + copied;
        let in_page = current & (LINUX_PAGE_SIZE - 1);
        let chunk = (count - copied).min(LINUX_PAGE_SIZE - in_page);
        let result = (|| {
            let target = buffer.checked_add(copied).ok_or(EFAULT)?;
            if write {
                read_target_memory(runtime, pid, target, chunk)?;
                let address = page(runtime, file.resource, current / LINUX_PAGE_SIZE)?;
                unsafe {
                    ::core::ptr::copy_nonoverlapping(
                        runtime.posix_shm as *const u8,
                        (address + in_page) as *mut u8,
                        chunk,
                    );
                }
            } else if let Some(address) = object(runtime, file.resource)?
                .pages
                .get(&(current / LINUX_PAGE_SIZE))
            {
                super::copyout_from(runtime, pid, target, *address + in_page, chunk)?;
            } else {
                unsafe {
                    ::core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, chunk);
                }
                write_target_memory(runtime, pid, target, chunk)?;
            }
            Ok::<(), i32>(())
        })();
        if let Err(errno) = result {
            if copied == 0 {
                return Err(errno);
            }
            break;
        }
        copied += chunk;
    }
    let memory = object_mut(runtime, file.resource)?;
    if positioned.is_none() {
        memory.offset = offset + copied;
    }
    if write && copied != 0 && offset + copied > memory.size {
        truncate(runtime, file, offset + copied)?;
    }
    Ok(copied)
}

pub(super) fn seek(
    runtime: &mut Runtime,
    file: LinuxFile,
    offset: Word,
    whence: Word,
) -> Result<Word, i32> {
    let memory = object_mut(runtime, file.resource)?;
    let base = match whence {
        0 => 0,
        1 => memory.offset,
        2 => memory.size,
        _ => return Err(EINVAL),
    };
    let next = (base as isize)
        .checked_add(offset as isize)
        .filter(|next| *next >= 0)
        .ok_or(EINVAL)? as Word;
    memory.offset = next;
    Ok(next)
}

pub(super) fn status_flags(
    runtime: &mut Runtime,
    file: LinuxFile,
    new: Option<Word>,
) -> Result<Word, i32> {
    let memory = object_mut(runtime, file.resource)?;
    if let Some(flags) = new {
        memory.status_flags = flags & (LINUX_O_APPEND | LINUX_O_NONBLOCK);
    }
    Ok(LINUX_O_RDWR | memory.status_flags)
}

pub(super) fn seals(
    runtime: &mut Runtime,
    file: LinuxFile,
    add: Option<Word>,
) -> Result<Word, i32> {
    let memory = object_mut(runtime, file.resource)?;
    let Some(add) = add else {
        return Ok(memory.seals);
    };
    if add & !SEALS != 0 {
        return Err(EINVAL);
    }
    if memory.seals & SEAL_SEAL != 0 {
        return Err(EPERM);
    }
    if add & SEAL_WRITE != 0
        && memory.seals & SEAL_WRITE == 0
        && memory
            .maps
            .iter()
            // Memfd descriptions are O_RDWR, so every unsealed shared VMA
            // may later become writable, even if initially mapped read-only.
            .any(|mapping| mapping.shared)
    {
        return Err(EBUSY);
    }
    memory.seals |= add;
    Ok(0)
}

fn alias_page(
    runtime: &mut Runtime,
    id: Word,
    mapping: MemoryMapping,
    page_offset: Word,
) -> Result<(), i32> {
    let index = (mapping.offset + page_offset) / LINUX_PAGE_SIZE;
    let source = page(runtime, id, index)?;
    let target = mapping.base + page_offset;
    nanami_runtime::request_mapping_share_at(mapping.pid, source, LINUX_PAGE_SIZE, target)
        .map_err(map_request_error)?;
    let prot = runtime
        .mapping_at(mapping.pid, target)
        .map_or(mapping.prot, |vma| vma.prot);
    nanami_runtime::request_process_mapping_protect(mapping.pid, target, LINUX_PAGE_SIZE, prot)
        .map_err(map_request_error)
}

pub(super) fn map(
    runtime: &mut Runtime,
    pid: Word,
    address: Word,
    bytes: Word,
    prot: Word,
    flags: Word,
    file: LinuxFile,
    offset: Word,
) -> Result<Word, i32> {
    if offset & (LINUX_PAGE_SIZE - 1) != 0
        || offset > isize::MAX as Word
        || offset.checked_add(bytes).is_none()
    {
        return Err(EINVAL);
    }
    if flags & 3 != 1 && flags & 3 != 2 {
        return Err(EINVAL);
    }
    let shared = flags & 3 == 1;
    if shared
        && prot & LINUX_PROT_WRITE != 0
        && object(runtime, file.resource)?.seals & SEAL_WRITE != 0
    {
        return Err(EPERM);
    }
    object_mut(runtime, file.resource)?
        .maps
        .try_reserve(1)
        .map_err(|_| ENOMEM)?;
    let bytes = bytes.checked_add(LINUX_PAGE_SIZE - 1).ok_or(ENOMEM)? & !(LINUX_PAGE_SIZE - 1);
    let base = if flags & LINUX_MAP_FIXED != 0 {
        sys_mmap(
            runtime,
            pid,
            address,
            bytes,
            LINUX_PROT_NONE,
            (flags & !3) | 2 | LINUX_MAP_ANONYMOUS,
            Word::MAX,
            0,
        )?
    } else {
        reserve_none_mapping(runtime, pid, bytes)?.0
    };
    if !runtime.protect_mapping(pid, base, bytes, prot) {
        let _ = sys_munmap(runtime, pid, base, bytes);
        return Err(ENOMEM);
    }
    let mapping = MemoryMapping {
        pid,
        base,
        bytes,
        offset,
        prot,
        shared,
    };
    object_mut(runtime, file.resource)?.maps.push(mapping);
    let valid = bytes.min(align_up_word(
        object(runtime, file.resource)?.size.saturating_sub(offset),
        LINUX_PAGE_SIZE,
    ));
    let result = (|| {
        for page_offset in (0..valid).step_by(LINUX_PAGE_SIZE) {
            if shared {
                alias_page(runtime, file.resource, mapping, page_offset)?;
            } else {
                nanami_runtime::request_process_map_anonymous_at(
                    pid,
                    base + page_offset,
                    LINUX_PAGE_SIZE,
                )
                .map_err(map_request_error)?;
                if let Some(source) = object(runtime, file.resource)?
                    .pages
                    .get(&((offset + page_offset) / LINUX_PAGE_SIZE))
                {
                    write_target_memory_from(pid, base + page_offset, *source, LINUX_PAGE_SIZE)?;
                }
                nanami_runtime::request_process_mapping_protect(
                    pid,
                    base + page_offset,
                    LINUX_PAGE_SIZE,
                    prot,
                )
                .map_err(map_request_error)?;
            }
        }
        Ok::<(), i32>(())
    })();
    if let Err(errno) = result {
        let _ = sys_munmap(runtime, pid, base, bytes);
        return Err(errno);
    }
    Ok(base)
}

fn mappings(runtime: &Runtime, pid: Word) -> Vec<(Word, MemoryMapping)> {
    runtime
        .poll_objects
        .iter()
        .flat_map(|(id, object)| {
            let maps = match object {
                Object::Memory(memory) => memory.maps.as_slice(),
                _ => &[],
            };
            maps.iter()
                .copied()
                .filter(move |mapping| runtime.vm_group(mapping.pid) == runtime.vm_group(pid))
                .map(move |mut mapping| {
                    mapping.pid = pid;
                    (id, mapping)
                })
        })
        .collect()
}

pub(super) fn mapping_at(
    runtime: &Runtime,
    pid: Word,
    address: Word,
) -> Option<(Word, MemoryMapping)> {
    runtime
        .poll_objects
        .iter()
        .find_map(|(id, object)| match object {
            Object::Memory(memory) => memory
                .maps
                .iter()
                .copied()
                .find(|mapping| {
                    runtime.vm_group(mapping.pid) == runtime.vm_group(pid)
                        && address >= mapping.base
                        && address - mapping.base < mapping.bytes
                })
                .map(|mut mapping| {
                    mapping.pid = pid;
                    (id, mapping)
                }),
            _ => None,
        })
}

pub(super) fn remap(
    runtime: &mut Runtime,
    pid: Word,
    old: Word,
    old_size: Word,
    new_size: Word,
    flags: Word,
    new_address: Word,
    prot: Word,
) -> Result<Option<Word>, i32> {
    let Some((id, mapping)) = mapping_at(runtime, pid, old) else {
        return Ok(None);
    };
    if old
        .checked_add(old_size)
        .is_none_or(|end| end > mapping.base + mapping.bytes)
    {
        return Err(EINVAL);
    }
    if flags & LINUX_MREMAP_MAYMOVE == 0 {
        return Err(ENOMEM);
    }
    let offset = mapping.offset + old - mapping.base;
    if flags & LINUX_MREMAP_FIXED != 0 {
        let end = new_address.checked_add(new_size).ok_or(EINVAL)?;
        if new_address < old + old_size && old < end {
            return Err(EINVAL);
        }
    }
    let file = LinuxFile {
        kind: LinuxFileKind::MemoryFile,
        posix_fd: id,
        resource: id,
        ..LinuxFile::EMPTY
    };
    if object(runtime, id)?.anonymous && offset + new_size > object(runtime, id)?.size {
        truncate(runtime, file, offset + new_size)?;
    }
    let mmap_flags = if mapping.shared { 1 } else { 2 }
        | if flags & LINUX_MREMAP_FIXED != 0 {
            LINUX_MAP_FIXED
        } else {
            0
        };
    let target = map(
        runtime,
        pid,
        new_address,
        new_size,
        prot,
        mmap_flags,
        file,
        offset,
    )?;
    if !mapping.shared {
        let bytes = old_size.min(new_size).min(align_up_word(
            object(runtime, id)?.size.saturating_sub(offset),
            LINUX_PAGE_SIZE,
        ));
        if let Err(errno) = copy_same_process_range(runtime, pid, old, target, bytes) {
            let _ = sys_munmap(runtime, pid, target, new_size);
            return Err(errno);
        }
    }
    sys_munmap(runtime, pid, old, old_size)?;
    Ok(Some(target))
}

pub(crate) fn bus_fault(runtime: &Runtime, pid: Word, address: Word) -> bool {
    mapping_at(runtime, pid, address).is_some_and(|(id, mapping)| {
        object(runtime, id).is_ok_and(|file| {
            mapping.offset.saturating_add(address - mapping.base)
                >= align_up_word(file.size, LINUX_PAGE_SIZE)
        })
    })
}

pub(super) fn inherit(runtime: &mut Runtime, parent: Word, child: Word) -> Result<(), i32> {
    for (id, mapping) in mappings(runtime, parent) {
        object_mut(runtime, id)?
            .maps
            .try_reserve(1)
            .map_err(|_| ENOMEM)?;
        let child_mapping = MemoryMapping {
            pid: child,
            ..mapping
        };
        object_mut(runtime, id)?.maps.push(child_mapping);
        let valid = mapping.bytes.min(align_up_word(
            object(runtime, id)?.size.saturating_sub(mapping.offset),
            LINUX_PAGE_SIZE,
        ));
        for offset in (0..mapping.bytes).step_by(LINUX_PAGE_SIZE) {
            let target = mapping.base + offset;
            if mapping.shared || offset >= valid {
                nanami_runtime::request_process_mapping_release_sparse(
                    child,
                    target,
                    LINUX_PAGE_SIZE,
                )
                .map_err(map_request_error)?;
            }
            if offset >= valid {
                continue;
            }
            if mapping.shared {
                alias_page(runtime, id, child_mapping, offset)?;
            } else {
                let prot = runtime
                    .mapping_at(parent, target)
                    .map_or(mapping.prot, |vma| vma.prot);
                nanami_runtime::request_process_mapping_protect(
                    child,
                    target,
                    LINUX_PAGE_SIZE,
                    prot,
                )
                .map_err(map_request_error)?;
            }
        }
    }
    Ok(())
}

pub(super) fn protect(
    runtime: &Runtime,
    pid: Word,
    base: Word,
    bytes: Word,
    prot: Word,
) -> Result<(), i32> {
    let end = base.checked_add(bytes).ok_or(ENOMEM)?;
    let maps = mappings(runtime, pid);
    // Reject a sealed shared mapping before touching any permission.
    for (id, mapping) in &maps {
        if mapping.base < end
            && mapping.base + mapping.bytes > base
            && mapping.shared
            && prot & LINUX_PROT_WRITE != 0
            && object(runtime, *id)?.seals & SEAL_WRITE != 0
        {
            return Err(EACCES);
        }
    }
    let mut updates = Vec::new();
    let mut cursor = base;
    while cursor < end {
        let vma = runtime.mapping_at(pid, cursor).ok_or(ENOMEM)?;
        let vma_end = end.min(vma.base + vma.size);
        if let Some((id, mapping)) = maps
            .iter()
            .find(|(_, mapping)| cursor >= mapping.base && cursor < mapping.base + mapping.bytes)
        {
            let next = vma_end.min(mapping.base + mapping.bytes);
            let file_end = mapping.base.saturating_add(align_up_word(
                object(runtime, *id)?.size.saturating_sub(mapping.offset),
                LINUX_PAGE_SIZE,
            ));
            if cursor < file_end {
                updates.push((cursor, next.min(file_end) - cursor, vma.prot));
            }
            cursor = next;
        } else {
            let next = maps
                .iter()
                .filter(|(_, mapping)| mapping.base > cursor)
                .map(|(_, mapping)| mapping.base)
                .min()
                .unwrap_or(vma_end)
                .min(vma_end);
            updates.push((cursor, next - cursor, vma.prot));
            cursor = next;
        }
    }
    for (index, &(address, bytes, _)) in updates.iter().enumerate() {
        if let Err(error) =
            nanami_runtime::request_process_mapping_protect(pid, address, bytes, prot)
        {
            for &(address, bytes, previous) in updates[..index].iter().rev() {
                nanami_runtime::request_process_mapping_protect(pid, address, bytes, previous)
                    .map_err(map_request_error)?;
            }
            return Err(map_request_error(error));
        }
    }
    Ok(())
}

pub(super) fn unmap(runtime: &mut Runtime, pid: Word, base: Word, bytes: Word) -> Result<(), i32> {
    let end = base.checked_add(bytes).ok_or(EINVAL)?;
    for (id, mapping) in mappings(runtime, pid) {
        let start = base.max(mapping.base);
        let stop = end.min(mapping.base + mapping.bytes);
        if start >= stop {
            continue;
        }
        object_mut(runtime, id)?
            .maps
            .try_reserve(1)
            .map_err(|_| ENOMEM)?;
        for va in (start..stop).step_by(LINUX_PAGE_SIZE) {
            nanami_runtime::request_process_mapping_release_sparse(pid, va, LINUX_PAGE_SIZE)
                .map_err(map_request_error)?;
        }
        let owner = object(runtime, id)?
            .maps
            .iter()
            .find(|other| {
                other.base == mapping.base && runtime.vm_group(other.pid) == runtime.vm_group(pid)
            })
            .map(|other| other.pid)
            .ok_or(EINVAL)?;
        let memory = object_mut(runtime, id)?;
        memory
            .maps
            .retain(|other| other.pid != owner || other.base != mapping.base);
        if mapping.base < start {
            memory.maps.push(MemoryMapping {
                bytes: start - mapping.base,
                ..mapping
            });
        }
        if stop < mapping.base + mapping.bytes {
            memory.maps.push(MemoryMapping {
                base: stop,
                bytes: mapping.base + mapping.bytes - stop,
                offset: mapping.offset + stop - mapping.base,
                ..mapping
            });
        }
        collect(runtime, id);
    }
    Ok(())
}

pub(super) fn truncate(runtime: &mut Runtime, file: LinuxFile, length: Word) -> Result<Word, i32> {
    if length > isize::MAX as Word {
        return Err(EINVAL);
    }
    let memory = object(runtime, file.resource)?;
    let old = memory.size;
    if (length < old && memory.seals & SEAL_SHRINK != 0)
        || (length > old && memory.seals & SEAL_GROW != 0)
    {
        return Err(EPERM);
    }
    let maps = memory.maps.clone();
    let old_pages = align_up_word(old, LINUX_PAGE_SIZE);
    let new_pages = align_up_word(length, LINUX_PAGE_SIZE);
    for mapping in &maps {
        let first = old_pages
            .min(new_pages)
            .saturating_sub(mapping.offset)
            .min(mapping.bytes);
        let last = old_pages
            .max(new_pages)
            .saturating_sub(mapping.offset)
            .min(mapping.bytes);
        for offset in (first..last).step_by(LINUX_PAGE_SIZE) {
            if length < old {
                nanami_runtime::request_process_mapping_release_sparse(
                    mapping.pid,
                    mapping.base + offset,
                    LINUX_PAGE_SIZE,
                )
                .map_err(map_request_error)?;
            } else if mapping.shared {
                alias_page(runtime, file.resource, *mapping, offset)?;
            } else {
                nanami_runtime::request_process_map_anonymous_at(
                    mapping.pid,
                    mapping.base + offset,
                    LINUX_PAGE_SIZE,
                )
                .map_err(map_request_error)?;
                let prot = runtime
                    .mapping_at(mapping.pid, mapping.base + offset)
                    .map_or(mapping.prot, |vma| vma.prot);
                nanami_runtime::request_process_mapping_protect(
                    mapping.pid,
                    mapping.base + offset,
                    LINUX_PAGE_SIZE,
                    prot,
                )
                .map_err(map_request_error)?;
            }
        }
    }
    if length < old {
        // Clear truncated bytes, including the partial final page, so regrowth cannot expose them.
        for (&index, &address) in &object(runtime, file.resource)?.pages {
            let page_start = index * LINUX_PAGE_SIZE;
            if page_start + LINUX_PAGE_SIZE > length {
                let start = length.saturating_sub(page_start).min(LINUX_PAGE_SIZE);
                unsafe {
                    ::core::ptr::write_bytes(
                        (address + start) as *mut u8,
                        0,
                        LINUX_PAGE_SIZE - start,
                    );
                }
            }
        }
    }
    object_mut(runtime, file.resource)?.size = length;
    Ok(0)
}

pub(super) fn collect(runtime: &mut Runtime, id: Word) {
    let Ok(memory) = object(runtime, id) else {
        return;
    };
    if !memory.maps.is_empty() {
        return;
    }
    let file = LinuxFile {
        kind: LinuxFileKind::MemoryFile,
        posix_fd: id,
        resource: id,
        ..LinuxFile::EMPTY
    };
    if runtime.unix_sockets.references(file)
        || runtime.managed.iter().any(|process| {
            process.pid != 0
                && process
                    .files
                    .iter()
                    .any(|other| eventfd::same_file(*other, file))
        })
    {
        return;
    }
    let pages: Vec<_> = memory
        .pages
        .iter()
        .map(|(&index, &address)| (index, address))
        .collect();
    for (index, address) in pages {
        if nanami_runtime::request_mapping_release(address, LINUX_PAGE_SIZE).is_err() {
            return;
        }
        if let Ok(memory) = object_mut(runtime, id) {
            memory.pages.remove(&index);
        }
    }
    runtime.poll_objects.remove(id);
}

pub(super) fn forget_process(runtime: &mut Runtime, pid: Word) {
    // Alpha tears down the address space at exec/exit. Drop only the bookkeeping here.
    let replacement = runtime
        .managed
        .iter()
        .find(|process| {
            process.pid != pid
                && process.pid != 0
                && !process.exited
                && process.vm_group == runtime.vm_group(pid)
        })
        .map(|process| process.pid);
    let ids: Vec<_> = runtime
        .poll_objects
        .iter()
        .filter_map(|(id, object)| {
            if matches!(object, Object::Memory(_)) {
                Some(id)
            } else {
                None
            }
        })
        .collect();
    for id in ids {
        if let Ok(memory) = object_mut(runtime, id) {
            if let Some(replacement) = replacement {
                for mapping in &mut memory.maps {
                    if mapping.pid == pid {
                        mapping.pid = replacement;
                    }
                }
            } else {
                memory.maps.retain(|mapping| mapping.pid != pid);
            }
        }
        collect(runtime, id);
    }
}

pub(super) fn map_anonymous(
    runtime: &mut Runtime,
    pid: Word,
    address: Word,
    bytes: Word,
    prot: Word,
    flags: Word,
) -> Result<Word, i32> {
    let id = runtime
        .poll_objects
        .insert(Object::Memory(MemoryFile {
            anonymous: true,
            pages: BTreeMap::new(),
            size: bytes,
            offset: 0,
            status_flags: 0,
            seals: SEAL_SEAL,
            maps: Vec::new(),
        }))
        .ok_or(ENOMEM)?;
    let file = LinuxFile {
        kind: LinuxFileKind::MemoryFile,
        posix_fd: id,
        resource: id,
        ..LinuxFile::EMPTY
    };
    let result = map(runtime, pid, address, bytes, prot, flags, file, 0);
    if result.is_err() {
        collect(runtime, id);
    }
    result
}
