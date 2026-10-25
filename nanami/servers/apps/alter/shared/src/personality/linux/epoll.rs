use super::*;
use crate::state::poll_objects::{Epoll, Object, Watch, WATCH_LIMIT};
use crate::state::readiness::*;
use alloc::vec::Vec;

const ONESHOT: u32 = 1 << 30;
const EDGE: u32 = 1 << 31;
#[cfg(target_arch = "x86_64")]
const EVENT_BYTES: usize = 12;
#[cfg(target_arch = "aarch64")]
const EVENT_BYTES: usize = 16;

pub(super) fn is_wait(number: Word) -> bool {
    matches!(number, SYS_EPOLL_WAIT | SYS_EPOLL_PWAIT)
}

fn object(runtime: &Runtime, id: Word) -> Result<&Epoll, i32> {
    match runtime.poll_objects.get(id) {
        Some(Object::Epoll(epoll)) => Ok(epoll),
        _ => Err(EBADF),
    }
}

fn object_mut(runtime: &mut Runtime, id: Word) -> Result<&mut Epoll, i32> {
    match runtime.poll_objects.get_mut(id) {
        Some(Object::Epoll(epoll)) => Ok(epoll),
        _ => Err(EBADF),
    }
}

pub(super) fn create(runtime: &mut Runtime, pid: Word, flags: Word) -> Result<Word, i32> {
    if flags & !LINUX_O_CLOEXEC != 0 {
        return Err(EINVAL);
    }
    super::eventfd::allocate(
        runtime,
        pid,
        LinuxFileKind::Epoll,
        Object::Epoll(Epoll {
            watches: Vec::new(),
            cursor: 0,
        }),
        flags,
    )
}

pub(super) fn control(
    runtime: &mut Runtime,
    pid: Word,
    epfd: Word,
    op: Word,
    fd: Word,
    event: Word,
) -> Result<Word, i32> {
    let epoll = runtime.linux_file(pid, epfd).ok_or(EBADF)?;
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if epoll.kind != LinuxFileKind::Epoll || super::eventfd::same_file(epoll, file) {
        return Err(EINVAL);
    }
    if matches!(
        file.kind,
        LinuxFileKind::Posix
            | LinuxFileKind::VirtualFile
            | LinuxFileKind::VirtualDirectory
            | LinuxFileKind::Framebuffer
    ) {
        return Err(EPERM);
    }
    if file.kind == LinuxFileKind::Epoll {
        return Err(EOPNOTSUPP);
    }
    if !(1..=3).contains(&op) {
        return Err(EINVAL);
    }
    let (events, data) = if op == 2 {
        (0, 0)
    } else {
        if event == 0 {
            return Err(EFAULT);
        }
        read_target_memory(runtime, pid, event, EVENT_BYTES).map_err(|_| EFAULT)?;
        let events = unsafe { (runtime.posix_shm as *const u32).read_unaligned() };
        // The initial backend is level-triggered, with one-shot rearming.
        // Never silently reinterpret edge-triggered or exclusive requests.
        if events & (EDGE | (1 << 28)) != 0 {
            return Err(EOPNOTSUPP);
        }
        let offset = EVENT_BYTES - 8;
        let data = unsafe { ((runtime.posix_shm + offset) as *const u64).read_unaligned() };
        (events, data)
    };
    let terminal = runtime.terminal_id(pid).unwrap_or(0);
    let epoll = object_mut(runtime, epoll.resource)?;
    let index = epoll
        .watches
        .iter()
        .position(|watch| watch.fd == fd && super::eventfd::same_file(watch.file, file));
    match (op, index) {
        (1, Some(_)) => return Err(EEXIST),
        (1, None) => {
            if epoll.watches.len() == WATCH_LIMIT {
                return Err(ENOSPC);
            }
            epoll.watches.try_reserve(1).map_err(|_| ENOMEM)?;
            epoll.watches.push(Watch {
                fd,
                file,
                terminal,
                events,
                data,
                armed: true,
            });
        }
        (2, Some(index)) => {
            epoll.watches.remove(index);
        }
        (3, Some(index)) => {
            epoll.watches[index] = Watch {
                fd,
                file,
                terminal,
                events,
                data,
                armed: true,
            };
        }
        _ => return Err(ENOENT),
    }
    runtime.readiness_changes |= READY_OBJECT;
    Ok(0)
}

pub(super) fn wait_set(
    runtime: &Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> Result<ReadinessWait, i32> {
    let file = runtime.linux_file(pid, context.args[0]).ok_or(EBADF)?;
    if file.kind != LinuxFileKind::Epoll || context.args[2] as i32 <= 0 {
        return Err(EINVAL);
    }
    if context.args[1] == 0 {
        return Err(EFAULT);
    }
    object(runtime, file.resource)?;
    let mut entries = [PollFd::EMPTY; LINUX_FD_MAX];
    entries[0] = PollFd {
        fd: context.args[0] as i32,
        events: LINUX_POLLIN,
        revents: 0,
    };
    Ok(ReadinessWait {
        context,
        entries,
        count: 1,
        sources: 0,
        deadline: None,
        select: false,
    })
}

pub(super) fn sources(runtime: &Runtime, id: Word) -> Word {
    object(runtime, id).map_or(READY_OBJECT, |epoll| {
        epoll
            .watches
            .iter()
            .filter(|watch| watch.armed)
            .fold(READY_OBJECT, |sources, watch| {
                sources | super::readiness::file_sources(watch.file)
            })
    })
}

fn watch_events(runtime: &mut Runtime, pid: Word, watch: Watch) -> Result<u32, i32> {
    if !watch.armed {
        return Ok(0);
    }
    // A registration follows the open description, not a recycled FD number.
    let current = runtime.linux_file(pid, watch.fd).filter(|file| {
        super::eventfd::same_file(*file, watch.file)
            && (file.kind != LinuxFileKind::Terminal
                || runtime.terminal_id(pid) == Some(watch.terminal))
    });
    let owner = current.map(|_| (pid, watch.fd)).or_else(|| {
        runtime.managed.iter().find_map(|process| {
            if process.pid == 0
                || (watch.file.kind == LinuxFileKind::Terminal
                    && process.terminal_id != watch.terminal)
            {
                return None;
            }
            process
                .files
                .iter()
                .position(|file| super::eventfd::same_file(*file, watch.file))
                .map(|fd| (process.pid, fd))
        })
    });
    let Some((owner, fd)) = owner else {
        return Ok(0);
    };
    let mut pumped = false;
    let available = super::readiness::fd_readiness(
        runtime,
        if watch.file.kind == LinuxFileKind::Terminal {
            pid
        } else {
            owner
        },
        fd,
        watch.file,
        watch.events as i16,
        &mut pumped,
    )?;
    Ok(available as u16 as u32 & (watch.events | LINUX_POLLERR as u32 | LINUX_POLLHUP as u32))
}

pub(super) fn ready(runtime: &mut Runtime, pid: Word, id: Word) -> Result<bool, i32> {
    let count = object(runtime, id)?.watches.len();
    for index in 0..count {
        let watch = object(runtime, id)?.watches[index];
        if watch_events(runtime, pid, watch)? != 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn complete(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, context.args[0]).ok_or(EBADF)?;
    let id = file.resource;
    let count = object(runtime, id)?.watches.len();
    let max = (context.args[2] as usize)
        .min(WATCH_LIMIT)
        .min(runtime.posix_shm_size / EVENT_BYTES);
    if max == 0 {
        return Err(EINVAL);
    }
    let start = object(runtime, id)?.cursor;
    let mut bytes = [0u8; WATCH_LIMIT * EVENT_BYTES];
    let mut selected = [0usize; WATCH_LIMIT];
    let mut written = 0;
    for offset in 0..count {
        let index = (start + offset) % count;
        let watch = object(runtime, id)?.watches[index];
        let events = watch_events(runtime, pid, watch)?;
        if events == 0 {
            continue;
        }
        let output = &mut bytes[written * EVENT_BYTES..(written + 1) * EVENT_BYTES];
        output[..4].copy_from_slice(&events.to_le_bytes());
        output[EVENT_BYTES - 8..].copy_from_slice(&watch.data.to_le_bytes());
        selected[written] = index;
        written += 1;
        if written == max {
            break;
        }
    }
    if written != 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                runtime.posix_shm as *mut u8,
                written * EVENT_BYTES,
            );
        }
        write_target_memory(runtime, pid, context.args[1], written * EVENT_BYTES)
            .map_err(|_| EFAULT)?;
        let epoll = object_mut(runtime, id)?;
        for index in &selected[..written] {
            if epoll.watches[*index].events & ONESHOT != 0 {
                epoll.watches[*index].armed = false;
            }
        }
        epoll.cursor = (selected[written - 1] + 1) % count;
    }
    Ok(written)
}

pub(super) fn forget_closed(runtime: &mut Runtime, pid: Word, file: LinuxFile) {
    if runtime.poll_objects.is_empty() {
        return;
    }
    let terminal = runtime.terminal_id(pid).unwrap_or(0);
    let referenced = runtime.managed.iter().any(|process| {
        process.pid != 0
            && (file.kind != LinuxFileKind::Terminal || process.terminal_id == terminal)
            && process
                .files
                .iter()
                .any(|other| super::eventfd::same_file(*other, file))
    });
    if !referenced {
        runtime.poll_objects.forget_file(file, terminal);
    }
}
