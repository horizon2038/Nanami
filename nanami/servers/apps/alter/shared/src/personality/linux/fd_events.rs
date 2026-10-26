//! Timer and signal open-file descriptions; readiness uses the existing wait queue.
use super::*;
use crate::common::signals;
use crate::state::poll_objects::{Object, SignalFd, Timer};
use crate::state::readiness::READY_OBJECT;

const RECORD_BYTES: Word = 128;
const TIMER_ABSTIME: Word = 1;
const TIMER_CANCEL_ON_SET: Word = 2;

pub(super) fn timer_create(
    runtime: &mut Runtime,
    pid: Word,
    clock: Word,
    flags: Word,
) -> Result<Word, i32> {
    if !matches!(clock, 0 | 1 | 7) || flags & !(LINUX_O_CLOEXEC | LINUX_O_NONBLOCK) != 0 {
        return Err(EINVAL);
    }
    eventfd::allocate(
        runtime,
        pid,
        LinuxFileKind::TimerFd,
        Object::Timer(Timer {
            clock,
            deadline: None,
            interval: 0,
            expirations: 0,
            nonblocking: flags & LINUX_O_NONBLOCK != 0,
        }),
        flags,
    )
}

fn timer(runtime: &Runtime, pid: Word, fd: Word) -> Result<(Word, Timer), i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    match runtime.poll_objects.get(file.resource) {
        Some(Object::Timer(timer)) if file.kind == LinuxFileKind::TimerFd => {
            Ok((file.resource, *timer))
        }
        _ => Err(EINVAL),
    }
}

fn timespec_ticks(runtime: &Runtime, offset: Word) -> Result<Word, i32> {
    let seconds = read_shm_u64(runtime, offset) as isize;
    let nanos = read_shm_u64(runtime, offset + 8) as isize;
    if seconds < 0 || !(0..1_000_000_000).contains(&nanos) {
        return Err(EINVAL);
    }
    let ticks = seconds as u128 * runtime.monotonic_tick_hz as u128
        + (nanos as u128 * runtime.monotonic_tick_hz as u128).div_ceil(1_000_000_000);
    Word::try_from(ticks).map_err(|_| EINVAL)
}

fn report_timer(runtime: &mut Runtime, pid: Word, timer: Timer, output: Word) -> Result<(), i32> {
    let remaining = timer.deadline.map_or(0, |deadline| {
        deadline.saturating_sub(runtime.monotonic_ticks)
    });
    for (index, ticks) in [timer.interval, remaining].into_iter().enumerate() {
        let (seconds, nanos) = split_ticks(ticks, runtime.monotonic_tick_hz);
        unsafe {
            write_u64(runtime.posix_shm + index * 16, seconds);
            write_u64(runtime.posix_shm + index * 16 + 8, nanos);
        }
    }
    write_target_memory(runtime, pid, output, 32)
}

pub(super) fn timer_get(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    output: Word,
) -> Result<Word, i32> {
    let (id, _) = timer(runtime, pid, fd)?;
    refresh_clock(runtime)?;
    runtime.poll_objects.expire_timers(runtime.monotonic_ticks);
    let Some(Object::Timer(timer)) = runtime.poll_objects.get(id) else {
        return Err(EBADF);
    };
    report_timer(runtime, pid, *timer, output)?;
    Ok(0)
}

pub(super) fn timer_set(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    flags: Word,
    input: Word,
    output: Word,
) -> Result<Word, i32> {
    if flags & !(TIMER_ABSTIME | TIMER_CANCEL_ON_SET) != 0 {
        return Err(EINVAL);
    }
    let (id, mut proposed) = timer(runtime, pid, fd)?;
    refresh_clock(runtime)?;
    if proposed.clock == 0 && flags & TIMER_ABSTIME != 0 {
        super::realtime::realtime(runtime)?;
    }
    read_target_memory(runtime, pid, input, 32)?;
    proposed.interval = timespec_ticks(runtime, 0)?;
    let value = timespec_ticks(runtime, 16)?;
    proposed.deadline = if value == 0 {
        None
    } else if flags & TIMER_ABSTIME == 0 {
        Some(runtime.monotonic_ticks.checked_add(value).ok_or(EINVAL)?)
    } else if proposed.clock == 0 {
        let (epoch, anchor) = runtime.realtime_anchor.ok_or(EIO)?;
        let epoch_ticks =
            (epoch as u128 * runtime.monotonic_tick_hz as u128).min(Word::MAX as u128) as Word;
        Some(anchor.saturating_add(value.saturating_sub(epoch_ticks)))
    } else {
        Some(value)
    };
    // Clock changes are not exposed by Alter; CANCEL_ON_SET is therefore inert.
    proposed.expirations = 0;
    runtime.poll_objects.expire_timers(runtime.monotonic_ticks);
    let (_, previous) = timer(runtime, pid, fd)?;
    if output != 0 {
        report_timer(runtime, pid, previous, output)?;
    }
    proposed.expire(runtime.monotonic_ticks);
    *runtime.poll_objects.get_mut(id).ok_or(EBADF)? = Object::Timer(proposed);
    if let Err(errno) = arm_clock_timer(runtime) {
        *runtime.poll_objects.get_mut(id).ok_or(EBADF)? = Object::Timer(previous);
        return Err(errno);
    }
    runtime.readiness_changes |= READY_OBJECT;
    Ok(0)
}

pub(super) fn signal_create(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    mask: Word,
    size: Word,
    flags: Word,
) -> Result<Word, i32> {
    if size != 8 || flags & !(LINUX_O_CLOEXEC | LINUX_O_NONBLOCK) != 0 {
        return Err(EINVAL);
    }
    read_target_memory(runtime, pid, mask, 8)?;
    let mask = signals::blockable(read_shm_u64(runtime, 0) as u64);
    if fd as i32 != -1 {
        let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
        let Some(Object::Signals(object)) = runtime.poll_objects.get_mut(file.resource) else {
            return Err(EINVAL);
        };
        object.mask = mask;
        runtime.readiness_changes |= READY_OBJECT;
        return Ok(fd);
    }
    eventfd::allocate(
        runtime,
        pid,
        LinuxFileKind::SignalFd,
        Object::Signals(SignalFd {
            mask,
            nonblocking: flags & LINUX_O_NONBLOCK != 0,
        }),
        flags,
    )
}

pub(super) fn read(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    buffer: Word,
    bytes: Word,
) -> Result<Word, i32> {
    if file.kind == LinuxFileKind::TimerFd {
        if bytes < 8 {
            return Err(EINVAL);
        }
        refresh_clock(runtime)?;
        runtime.poll_objects.expire_timers(runtime.monotonic_ticks);
        let Some(Object::Timer(timer)) = runtime.poll_objects.get(file.resource) else {
            return Err(EBADF);
        };
        let count = timer.expirations;
        if count == 0 {
            return Err(EAGAIN);
        }
        write_guest_u64(runtime, pid, buffer, count as Word)?;
        let Some(Object::Timer(timer)) = runtime.poll_objects.get_mut(file.resource) else {
            return Err(EBADF);
        };
        timer.expirations = 0;
        runtime.readiness_changes |= READY_OBJECT;
        return Ok(8);
    }
    if bytes < RECORD_BYTES {
        return Err(EINVAL);
    }
    let Some(Object::Signals(object)) = runtime.poll_objects.get(file.resource) else {
        return Err(EBADF);
    };
    let mask = object.mask;
    let mut copied = 0;
    while copied + RECORD_BYTES <= bytes {
        let state = runtime.managed_process_mut(pid).ok_or(ESRCH)?.signals_mut();
        let Some((info, group)) = state.take_matching_record(mask) else {
            break;
        };
        let record = signal_record(info);
        if let Err(errno) = super::copyout_from(
            runtime,
            pid,
            buffer.checked_add(copied).ok_or(EFAULT)?,
            record.as_ptr() as Word,
            RECORD_BYTES,
        ) {
            runtime
                .managed_process_mut(pid)
                .ok_or(ESRCH)?
                .signals_mut()
                .restore_pending(info, group);
            if copied == 0 {
                return Err(errno);
            }
            break;
        }
        copied += RECORD_BYTES;
    }
    if copied == 0 {
        return Err(EAGAIN);
    }
    runtime.readiness_changes |= READY_OBJECT;
    Ok(copied)
}

pub(super) fn signal_record(info: signals::Info) -> [u8; 128] {
    let mut output = [0; 128];
    output[..4].copy_from_slice(&info.bytes[..4]); // signo
    output[4..8].copy_from_slice(&info.bytes[4..8]); // errno
    output[8..12].copy_from_slice(&info.bytes[8..12]); // code
    let code = i32::from_le_bytes(info.bytes[8..12].try_into().unwrap());
    if code == 0 || code == -1 || code == -6 || info.signal() == signals::SIGCHLD {
        output[12..20].copy_from_slice(&info.bytes[16..24]); // pid, uid
    }
    if code == -1 {
        output[44..48].copy_from_slice(&info.bytes[24..28]); // integer sigval
        output[48..56].copy_from_slice(&info.bytes[24..32]); // pointer sigval
    } else if info.signal() == signals::SIGCHLD {
        output[40..44].copy_from_slice(&info.bytes[24..28]); // status
        output[56..72].copy_from_slice(&info.bytes[32..48]); // utime, stime
    } else if matches!(info.signal(), 4 | 5 | 7 | 8 | 11) && code > 0 {
        output[72..80].copy_from_slice(&info.bytes[16..24]); // fault address
    }
    output
}

pub(super) fn readiness(runtime: &mut Runtime, pid: Word, file: LinuxFile) -> Result<i16, i32> {
    if file.kind == LinuxFileKind::TimerFd {
        refresh_clock(runtime)?;
        runtime.poll_objects.expire_timers(runtime.monotonic_ticks);
    }
    let ready = match runtime.poll_objects.get(file.resource) {
        Some(Object::Timer(timer)) => timer.expirations != 0,
        Some(Object::Signals(object)) => runtime
            .managed_process(pid)
            .and_then(|process| process.signals())
            .is_some_and(|state| state.pending_mask() & object.mask != 0),
        _ => return Err(EBADF),
    };
    Ok(if ready {
        LINUX_POLLIN | LINUX_POLLRDNORM
    } else {
        0
    })
}
