//! System V shared RAM, scoped to an execution context. Pages and aliases use
//! memfd's existing fork/unmap/exec machinery, not copied guest snapshots.
use super::*;
use crate::state::poll_objects::{MemoryFile, Object, SysvSegment};
use alloc::{collections::BTreeMap, vec::Vec};

const IPC_CREAT: Word = 0o1000;
const IPC_EXCL: Word = 0o2000;
const SHM_RDONLY: Word = 0o10000;
const SHM_RND: Word = 0o20000;
const SHM_EXEC: Word = 0o100000;
const SEGMENT_LIMIT: Word = 32 * 1024 * 1024;
const CONTEXT_LIMIT: Word = 64 * 1024 * 1024;
const ENOSPC: i32 = 28;

fn scope(runtime: &Runtime, pid: Word) -> Result<Word, i32> {
    Ok(runtime.managed_process(pid).ok_or(ESRCH)?.rootfs.leader)
}

fn segment(runtime: &Runtime, pid: Word, id: Word) -> Result<&MemoryFile, i32> {
    let scope = scope(runtime, pid)?;
    match runtime.poll_objects.get(id) {
        Some(Object::Memory(memory)) if memory.sysv.as_ref().is_some_and(|s| s.scope == scope) => {
            Ok(memory)
        }
        _ => Err(EINVAL),
    }
}

fn metadata(runtime: &mut Runtime, id: Word) -> Result<&mut SysvSegment, i32> {
    match runtime.poll_objects.get_mut(id) {
        Some(Object::Memory(memory)) => memory.sysv.as_mut().ok_or(EINVAL),
        _ => Err(EINVAL),
    }
}

fn now(runtime: &mut Runtime) -> Word {
    super::realtime::realtime(runtime).map_or(0, |(seconds, _)| seconds)
}

pub(super) fn get(
    runtime: &mut Runtime,
    pid: Word,
    key: Word,
    size: Word,
    flags: Word,
) -> Result<Word, i32> {
    if flags & !(0o777 | IPC_CREAT | IPC_EXCL) != 0 {
        return Err(EOPNOTSUPP);
    }
    let scope = scope(runtime, pid)?;
    let key = key as u32;
    if key != 0 {
        if let Some((id, memory)) =
            runtime
                .poll_objects
                .iter()
                .find_map(|(id, object)| match object {
                    Object::Memory(memory)
                        if memory
                            .sysv
                            .as_ref()
                            .is_some_and(|s| s.scope == scope && s.key == key && !s.removed) =>
                    {
                        Some((id, memory))
                    }
                    _ => None,
                })
        {
            if flags & (IPC_CREAT | IPC_EXCL) == IPC_CREAT | IPC_EXCL {
                return Err(EEXIST);
            }
            return if size > memory.size {
                Err(EINVAL)
            } else {
                Ok(id)
            };
        }
        if flags & IPC_CREAT == 0 {
            return Err(ENOENT);
        }
    }
    if size == 0 || size > SEGMENT_LIMIT {
        return Err(EINVAL);
    }
    let committed: Word = runtime
        .poll_objects
        .iter()
        .filter_map(|(_, object)| match object {
            Object::Memory(memory) if memory.sysv.as_ref().is_some_and(|s| s.scope == scope) => {
                Some(align_up_word(memory.size, LINUX_PAGE_SIZE))
            }
            _ => None,
        })
        .sum();
    if committed + align_up_word(size, LINUX_PAGE_SIZE) > CONTEXT_LIMIT {
        return Err(ENOSPC);
    }
    let created = now(runtime);
    runtime
        .poll_objects
        .insert(Object::Memory(MemoryFile {
            sysv: Some(SysvSegment {
                scope,
                key,
                mode: flags as u16 & 0o777,
                creator: pid,
                last_pid: 0,
                created,
                attached: 0,
                detached: 0,
                removed: false,
            }),
            anonymous: true,
            backing: None,
            pages: BTreeMap::new(),
            size,
            offset: 0,
            status_flags: 0,
            seals: 0,
            maps: Vec::new(),
        }))
        .ok_or(ENOSPC)
}

pub(super) fn attach(
    runtime: &mut Runtime,
    pid: Word,
    id: Word,
    address: Word,
    flags: Word,
) -> Result<Word, i32> {
    if flags & !(SHM_RDONLY | SHM_RND | SHM_EXEC) != 0 {
        return Err(EINVAL);
    }
    let memory = segment(runtime, pid, id)?;
    let size = memory.size;
    if flags & SHM_EXEC != 0 && memory.sysv.as_ref().ok_or(EINVAL)?.mode & 0o111 == 0 {
        return Err(EACCES);
    }
    let address = if flags & SHM_RND != 0 {
        address & !(LINUX_PAGE_SIZE - 1)
    } else {
        address
    };
    if address & (LINUX_PAGE_SIZE - 1) != 0 {
        return Err(EINVAL);
    }
    if address != 0 {
        let bytes = align_up_word(size, LINUX_PAGE_SIZE);
        let end = address.checked_add(bytes).ok_or(EINVAL)?;
        if address < LINUX_VIRTUAL_RESERVATION_BASE || end > LINUX_VIRTUAL_RESERVATION_LIMIT {
            return Err(EINVAL);
        }
        // SHM_REMAP is not accepted. In particular, never replace an existing
        // VMA as a side effect of a failed shmat at an occupied address.
        if runtime.next_mapping(pid, address, end).is_some() {
            return Err(EINVAL);
        }
    }
    let prot = LINUX_PROT_READ
        | if flags & SHM_RDONLY == 0 {
            LINUX_PROT_WRITE
        } else {
            0
        }
        | if flags & SHM_EXEC != 0 {
            LINUX_PROT_EXEC
        } else {
            0
        };
    let file = LinuxFile {
        kind: LinuxFileKind::MemoryFile,
        resource: id,
        posix_fd: id,
        flags: if flags & SHM_RDONLY != 0 {
            LINUX_O_RDONLY
        } else {
            LINUX_O_RDWR
        },
        ..LinuxFile::EMPTY
    };
    let base = memfd::map(
        runtime,
        pid,
        address,
        size,
        prot,
        1 | if address != 0 { LINUX_MAP_FIXED } else { 0 },
        file,
        0,
    )?;
    let time = now(runtime);
    let info = metadata(runtime, id)?;
    info.attached = time;
    info.last_pid = pid;
    Ok(base)
}

pub(super) fn detach(runtime: &mut Runtime, pid: Word, address: Word) -> Result<Word, i32> {
    let found = runtime
        .poll_objects
        .iter()
        .find_map(|(id, object)| match object {
            Object::Memory(memory)
                if memory.sysv.is_some()
                    && memory.maps.iter().any(|m| {
                        runtime.vm_group(m.pid) == runtime.vm_group(pid)
                            && m.attachment == Some(address)
                    }) =>
            {
                Some(id)
            }
            _ => None,
        })
        .ok_or(EINVAL)?;
    let ranges: Vec<_> = segment(runtime, pid, found)?
        .maps
        .iter()
        .filter(|m| {
            runtime.vm_group(m.pid) == runtime.vm_group(pid) && m.attachment == Some(address)
        })
        .map(|m| (m.base, m.bytes))
        .collect();
    let time = now(runtime);
    let info = metadata(runtime, found)?;
    info.detached = time;
    info.last_pid = pid;
    for (base, bytes) in ranges {
        sys_munmap(runtime, pid, base, bytes)?;
    }
    Ok(0)
}

pub(super) fn control(
    runtime: &mut Runtime,
    pid: Word,
    id: Word,
    command: Word,
    output: Word,
) -> Result<Word, i32> {
    let memory = segment(runtime, pid, id)?;
    match command {
        0 => {
            // IPC_RMID: hide the key now; retain pages until the final detach.
            metadata(runtime, id)?.removed = true;
            memfd::collect(runtime, id);
        }
        1 => {
            // IPC_SET: root-only credentials, but permission bits persist.
            read_target_memory(runtime, pid, output, 48)?;
            if read_shm_u32(runtime, 4) != 0 || read_shm_u32(runtime, 8) != 0 {
                return Err(EOPNOTSUPP);
            }
            let mode = read_shm_u32(runtime, 20) as u16 & 0o777;
            let time = now(runtime);
            let info = metadata(runtime, id)?;
            info.mode = mode;
            info.created = time;
        }
        2 => {
            // IPC_STAT: Linux ipc64_perm + shmid64_ds (112 bytes).
            let info = memory.sysv.as_ref().ok_or(EINVAL)?;
            let mut output_bytes = [0u8; 112];
            let put = |out: &mut [u8], offset: usize, value: Word| {
                out[offset..offset + 8].copy_from_slice(&(value as u64).to_le_bytes())
            };
            output_bytes[..4]
                .copy_from_slice(&if info.removed { 0 } else { info.key }.to_le_bytes());
            let mode = info.mode as u32 | if info.removed { 0o1000 } else { 0 };
            output_bytes[20..24].copy_from_slice(&mode.to_le_bytes());
            put(&mut output_bytes, 48, memory.size);
            put(&mut output_bytes, 56, info.attached);
            put(&mut output_bytes, 64, info.detached);
            put(&mut output_bytes, 72, info.created);
            output_bytes[80..84].copy_from_slice(&(info.creator as u32).to_le_bytes());
            output_bytes[84..88].copy_from_slice(&(info.last_pid as u32).to_le_bytes());
            let attachments = memory
                .maps
                .iter()
                .enumerate()
                .filter(|(index, m)| {
                    !memory.maps[..*index]
                        .iter()
                        .any(|prior| prior.pid == m.pid && prior.attachment == m.attachment)
                })
                .count();
            put(&mut output_bytes, 88, attachments);
            copyout_from(
                runtime,
                pid,
                output,
                output_bytes.as_ptr() as Word,
                output_bytes.len(),
            )?;
        }
        _ => return Err(EINVAL),
    }
    Ok(0)
}

pub(super) fn context_exit(runtime: &mut Runtime, pid: Word) {
    let Ok(scope) = scope(runtime, pid) else {
        return;
    };
    if runtime
        .managed
        .iter()
        .any(|p| p.pid != 0 && p.pid != pid && !p.exited && p.rootfs.leader == scope)
    {
        return;
    }
    let ids: Vec<_> = runtime
        .poll_objects
        .iter()
        .filter_map(|(id, object)| match object {
            Object::Memory(memory) if memory.sysv.as_ref().is_some_and(|s| s.scope == scope) => {
                Some(id)
            }
            _ => None,
        })
        .collect();
    for id in ids {
        if let Ok(info) = metadata(runtime, id) {
            info.removed = true;
        }
        memfd::collect(runtime, id);
    }
}

pub(super) fn inherited(runtime: &mut Runtime, pid: Word) {
    let ids: Vec<_> = runtime
        .poll_objects
        .iter()
        .filter_map(|(id, object)| match object {
            Object::Memory(memory)
                if memory.sysv.is_some()
                    && memory.maps.iter().any(|mapping| mapping.pid == pid) =>
            {
                Some(id)
            }
            _ => None,
        })
        .collect();
    if ids.is_empty() {
        return;
    }
    let time = now(runtime);
    for id in ids {
        if let Ok(info) = metadata(runtime, id) {
            info.attached = time;
            info.last_pid = pid;
        }
    }
}

pub(super) fn leaving_vm(runtime: &mut Runtime, pid: Word) {
    if runtime.managed.iter().any(|p| {
        p.pid != 0 && p.pid != pid && !p.exited && runtime.vm_group(p.pid) == runtime.vm_group(pid)
    }) {
        return;
    }
    let ids: Vec<_> = runtime
        .poll_objects
        .iter()
        .filter_map(|(id, object)| match object {
            Object::Memory(memory)
                if memory.sysv.is_some()
                    && memory.maps.iter().any(|mapping| mapping.pid == pid) =>
            {
                Some(id)
            }
            _ => None,
        })
        .collect();
    if ids.is_empty() {
        return;
    }
    let time = now(runtime);
    for id in ids {
        if let Ok(info) = metadata(runtime, id) {
            info.detached = time;
            info.last_pid = pid;
        }
    }
}
