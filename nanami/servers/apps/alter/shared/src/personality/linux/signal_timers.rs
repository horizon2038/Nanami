use super::*;
use crate::common::signals::Info;

pub(super) fn next_deadline(runtime: &Runtime) -> Option<Word> {
    runtime
        .managed
        .iter()
        .filter(|process| process.pid != 0 && !process.exited)
        .filter_map(|process| process.signals().and_then(|state| state.timer_deadline))
        .min()
}

pub(super) fn expire(runtime: &mut Runtime) {
    let now = runtime.monotonic_ticks;
    for index in 0..runtime.managed.len() {
        let process = &mut runtime.managed[index];
        if process.pid == 0 || process.exited || process.signals().is_none() {
            continue;
        }
        let pid = process.pid;
        let state = process.signals_mut();
        let Some(deadline) = state.timer_deadline.filter(|deadline| *deadline <= now) else {
            continue;
        };
        state.timer_deadline = if state.timer_interval == 0 {
            None
        } else {
            let intervals = (now - deadline) / state.timer_interval + 1;
            deadline
                .checked_add(intervals.saturating_mul(state.timer_interval))
                .filter(|next| *next > now)
        };
        let _ = signals_runtime::queue(runtime, pid, Info::new(14, 128, 0));
    }
}

fn timeval(runtime: &Runtime, offset: usize) -> Result<Word, i32> {
    let seconds = read_shm_u64(runtime, offset) as isize;
    let micros = read_shm_u64(runtime, offset + 8) as isize;
    if seconds < 0 || !(0..1_000_000).contains(&micros) {
        return Err(EINVAL);
    }
    let ticks = seconds as u128 * runtime.monotonic_tick_hz as u128
        + (micros as u128 * runtime.monotonic_tick_hz as u128).div_ceil(1_000_000);
    Word::try_from(ticks).map_err(|_| EINVAL)
}

pub(super) fn get(
    runtime: &mut Runtime,
    pid: Word,
    which: Word,
    output: Word,
) -> Result<Word, i32> {
    if which > 2 {
        return Err(EINVAL);
    }
    if which != 0 {
        return Err(EOPNOTSUPP);
    }
    refresh_clock(runtime)?;
    let (interval, remaining) =
        runtime
            .managed_process(pid)
            .ok_or(ESRCH)?
            .signals()
            .map_or((0, 0), |state| {
                (
                    state.timer_interval,
                    state.timer_deadline.map_or(0, |deadline| {
                        deadline.saturating_sub(runtime.monotonic_ticks)
                    }),
                )
            });
    let bytes = unsafe { ::core::slice::from_raw_parts_mut(runtime.posix_shm as *mut u8, 32) };
    for (index, ticks) in [interval, remaining].iter().enumerate() {
        let (seconds, nanos) = split_ticks(*ticks, runtime.monotonic_tick_hz);
        signal_frame::put(bytes, index * 16, seconds);
        signal_frame::put(bytes, index * 16 + 8, nanos / 1000);
    }
    write_target_memory(runtime, pid, output, 32)?;
    Ok(0)
}

pub(super) fn set(
    runtime: &mut Runtime,
    pid: Word,
    which: Word,
    new: Word,
    old: Word,
) -> Result<Word, i32> {
    if which > 2 {
        return Err(EINVAL);
    }
    if which != 0 {
        return Err(EOPNOTSUPP);
    }
    refresh_clock(runtime)?;
    let (interval, value) = if new == 0 {
        (0, 0)
    } else {
        read_target_memory(runtime, pid, new, 32)?;
        (timeval(runtime, 0)?, timeval(runtime, 16)?)
    };
    let deadline = if value == 0 {
        None
    } else {
        Some(runtime.monotonic_ticks.checked_add(value).ok_or(EINVAL)?)
    };
    if old != 0 {
        get(runtime, pid, which, old)?;
    }
    let state = runtime.managed_process_mut(pid).ok_or(ESRCH)?.signals_mut();
    let previous = (state.timer_interval, state.timer_deadline);
    state.timer_interval = interval;
    state.timer_deadline = deadline;
    if let Err(errno) = arm_clock_timer(runtime) {
        let state = runtime.managed_process_mut(pid).unwrap().signals_mut();
        (state.timer_interval, state.timer_deadline) = previous;
        return Err(errno);
    }
    Ok(0)
}

pub(super) fn alarm(runtime: &mut Runtime, pid: Word, seconds: Word) -> Result<Word, i32> {
    refresh_clock(runtime)?;
    let now = runtime.monotonic_ticks;
    let hz = runtime.monotonic_tick_hz;
    let state = runtime.managed_process_mut(pid).ok_or(ESRCH)?.signals_mut();
    let old = state
        .timer_deadline
        .map_or(0, |deadline| deadline.saturating_sub(now).div_ceil(hz));
    let previous = (state.timer_interval, state.timer_deadline);
    state.timer_interval = 0;
    state.timer_deadline = if seconds == 0 {
        None
    } else {
        Some(now.saturating_add(seconds.saturating_mul(hz)))
    };
    if let Err(errno) = arm_clock_timer(runtime) {
        let state = runtime.managed_process_mut(pid).unwrap().signals_mut();
        (state.timer_interval, state.timer_deadline) = previous;
        return Err(errno);
    }
    Ok(old)
}
