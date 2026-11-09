//! Frozen previous protection and its readonly adapters.
use super::*;
use poll_objects::{MemoryFile, MemoryMapping, Object};
const SEAL_WRITE: Word = 8;

fn object(runtime: &Runtime, id: Word) -> Result<&MemoryFile, i32> {
    match runtime.poll_objects.get(id) {
        Some(Object::Memory(file)) => Ok(file),
        _ => Err(EBADF),
    }
}


fn mappings(runtime: &Runtime, pid: Word) -> impl Iterator<Item = (Word, MemoryMapping)> + '_ {
    let mut group = None;
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
                .map(move |mapping| (id, mapping))
        })
        .filter(move |(_, mapping)| {
            mapping.pid == pid
                || runtime.vm_group(mapping.pid)
                    == *group.get_or_insert_with(|| runtime.vm_group(pid))
        })
        .map(move |(id, mut mapping)| {
            mapping.pid = pid;
            (id, mapping)
        })
}


pub(super) fn previous_protect(
    runtime: &Runtime,
    pid: Word,
    base: Word,
    bytes: Word,
    prot: Word,
) -> Result<(), i32> {
    let end = base.checked_add(bytes).ok_or(ENOMEM)?;
    let maps: Vec<_> = mappings(runtime, pid).collect();
    // Reject a sealed shared mapping before touching any permission.
    for (id, mapping) in &maps {
        if mapping.base < end
            && mapping.base + mapping.bytes > base
            && mapping.readonly_attachment
            && prot & LINUX_PROT_WRITE != 0
        {
            return Err(EACCES);
        }
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
