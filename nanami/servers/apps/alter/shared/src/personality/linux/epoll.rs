use super::*;
use crate::state::poll_objects::{Epoll, Object, Watch, WATCH_LIMIT};
use crate::state::readiness::*;
use alloc::vec::Vec;

const ONESHOT: u32 = 1 << 30;
const EDGE: u32 = 1 << 31;

#[cfg(test)]
static WATCH_QUERIES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
pub(super) fn take_watch_queries() -> usize {
    WATCH_QUERIES.swap(0, core::sync::atomic::Ordering::Relaxed)
}

fn regular_kind(kind: LinuxFileKind) -> bool {
    matches!(kind, LinuxFileKind::Posix | LinuxFileKind::MemoryFile
        | LinuxFileKind::VirtualFile | LinuxFileKind::VirtualDirectory
        | LinuxFileKind::Framebuffer)
}
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
    runtime.poll_objects.epoll_mut(id).ok_or(EBADF)
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
            source_mask: core::cell::Cell::new(None),
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
    if regular_kind(file.kind) {
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
    epoll.source_mask.set(None);
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
        if let Some(sources) = epoll.source_mask.get() {
            return sources;
        }
        let sources = epoll
            .watches
            .iter()
            .filter(|watch| watch.armed)
            .fold(READY_OBJECT, |sources, watch| {
                sources | super::readiness::file_sources(watch.file, watch.events as i16)
            });
        epoll.source_mask.set(Some(sources));
        sources
    })
}

fn watch_events(runtime: &mut Runtime, pid: Word, watch: Watch, input_pumped: &mut bool) -> Result<u32, i32> {
    #[cfg(test)]
    WATCH_QUERIES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
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
    let available = super::readiness::fd_readiness(
        runtime,
        if matches!(
            watch.file.kind,
            LinuxFileKind::Terminal | LinuxFileKind::SignalFd
        ) {
            pid
        } else {
            owner
        },
        fd,
        watch.file,
        watch.events as i16,
        input_pumped,
    )?;
    Ok(available as u16 as u32 & (watch.events | LINUX_POLLERR as u32 | LINUX_POLLHUP as u32))
}

pub(super) fn ready(runtime: &mut Runtime, pid: Word, id: Word, input_pumped: &mut bool) -> Result<bool, i32> {
    let count = object(runtime, id)?.watches.len();
    for index in 0..count {
        let watch = object(runtime, id)?.watches[index];
        if watch_events(runtime, pid, watch, input_pumped)? != 0 {
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
    // Empty readiness attempts must not initialize event/selection buffers.
    // Keep both request-local and materialize them only on the first event.
    let mut output = None;
    let mut written = 0;
    // One drain covers all input sessions, as in a regular poll scan. Keep
    // this local to the batch so the next readiness attempt drains again.
    let mut input_pumped = false;
    // DEL can leave cursor beyond the new count. Normalize once, then keep
    // the request-local index in range without a division per watched FD.
    let mut next_index = if count == 0 { 0 } else { start % count };
    for _ in 0..count {
        let index = next_index;
        next_index += 1;
        if next_index == count {
            next_index = 0;
        }
        let watch = object(runtime, id)?.watches[index];
        let events = watch_events(runtime, pid, watch, &mut input_pumped)?;
        if events == 0 {
            continue;
        }
        let (bytes, selected) = output.get_or_insert_with(|| (
            [0u8; WATCH_LIMIT * EVENT_BYTES], [0usize; WATCH_LIMIT],
        ));
        let event = &mut bytes[written * EVENT_BYTES..(written + 1) * EVENT_BYTES];
        event[..4].copy_from_slice(&events.to_le_bytes());
        event[EVENT_BYTES - 8..].copy_from_slice(&watch.data.to_le_bytes());
        selected[written] = index;
        written += 1;
        if written == max {
            break;
        }
    }
    if let Some((bytes, selected)) = output.as_ref() {
        write_events(runtime, pid, context.args[1], &bytes[..written * EVENT_BYTES])?;
        let epoll = object_mut(runtime, id)?;
        for index in &selected[..written] {
            if epoll.watches[*index].events & ONESHOT != 0 {
                epoll.watches[*index].armed = false;
                epoll.source_mask.set(None);
            }
        }
        let next = selected[written - 1] + 1;
        epoll.cursor = if next == count { 0 } else { next };
    }
    Ok(written)
}

fn write_events(runtime: &mut Runtime, pid: Word, output: Word, bytes: &[u8]) -> Result<(), i32> {
    // The event bytes remain owned for the entire synchronous copyout. Avoid
    // staging through SHM and keep the existing bounded writable-view path.
    super::copyout_owned(runtime, pid, output, bytes).map_err(|_| EFAULT)
}

pub(super) fn forget_closed(runtime: &mut Runtime, pid: Word, file: LinuxFile) {
    // control() rejects these immutable description kinds. No registration
    // can match them, so global FD/SCM_RIGHTS reference discovery cannot affect
    // watch cleanup. This does not skip the caller's actual close/release.
    if regular_kind(file.kind)
        || matches!(file.kind, LinuxFileKind::Empty | LinuxFileKind::Epoll)
        || runtime.poll_objects.is_empty()
    {
        return;
    }
    let terminal = runtime.terminal_id(pid).unwrap_or(0);
    // Without a matching registration, discovering aliases cannot change
    // cleanup. Include disabled one-shot watches in this exact identity test.
    if !runtime.poll_objects.has_file_watch(file, terminal) {
        return;
    }
    let referenced = runtime.managed.iter().any(|process| {
        process.pid != 0
            && (file.kind != LinuxFileKind::Terminal || process.terminal_id == terminal)
            && process
                .files
                .iter()
                .any(|other| super::eventfd::same_file(*other, file))
    });
    if !referenced && !runtime.unix_sockets.references(file) {
        runtime.poll_objects.forget_file(file, terminal);
    }
}
