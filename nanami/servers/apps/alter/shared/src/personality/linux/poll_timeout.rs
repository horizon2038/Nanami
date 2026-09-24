use super::*;
use crate::state::readiness::ReadinessWait;

pub(super) fn poll_timeout(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> Result<Option<u128>, i32> {
    let (mask, size) = match context.number {
        SYS_PPOLL => (context.args[3], context.args[4]),
        SYS_PSELECT6 if context.args[5] != 0 => {
            read_target_memory(runtime, pid, context.args[5], 16)?;
            unsafe {
                (
                    (runtime.posix_shm as *const Word).read_unaligned(),
                    ((runtime.posix_shm + 8) as *const Word).read_unaligned(),
                )
            }
        }
        _ => (0, 0),
    };
    if mask != 0 {
        if size != 8 {
            return Err(EINVAL);
        }
        read_target_memory(runtime, pid, mask, 8)?;
        let mask = unsafe { (runtime.posix_shm as *const u64).read_unaligned() };
        // Signal delivery/masking is not implemented yet. An empty mask has
        // no effect; reject an actual mask instead of silently ignoring it.
        if mask & !((1 << (9 - 1)) | (1 << (19 - 1))) != 0 {
            return Err(EOPNOTSUPP);
        }
    }
    if context.number == SYS_POLL {
        // Linux poll takes an int, even on a 64-bit syscall ABI.
        let millis = context.args[2] as i32;
        return Ok(if millis < 0 {
            None
        } else {
            Some(millis as u128 * 1_000_000)
        });
    }
    let ptr = timeout_pointer(context);
    if ptr == 0 {
        return Ok(None);
    }
    read_target_memory(runtime, pid, ptr, LINUX_TIMESPEC_BYTES)?;
    let (seconds, fraction) = unsafe {
        (
            (runtime.posix_shm as *const i64).read_unaligned(),
            ((runtime.posix_shm + 8) as *const i64).read_unaligned(),
        )
    };
    let scale = if context.number == SYS_SELECT {
        1_000_000
    } else {
        1_000_000_000
    };
    if seconds < 0 || !(0..scale).contains(&fraction) {
        return Err(EINVAL);
    }
    Ok(Some(
        seconds as u128 * 1_000_000_000 + fraction as u128 * (1_000_000_000 / scale) as u128,
    ))
}

pub(super) fn poll_deadline(now: Word, hz: Word, nanos: u128) -> Word {
    // Split seconds/fraction to avoid overflowing u128 for very long waits.
    let ticks = (nanos / 1_000_000_000 * hz as u128)
        .saturating_add(((nanos % 1_000_000_000) * hz as u128).div_ceil(1_000_000_000));
    now.saturating_add(ticks.min(Word::MAX as u128) as Word)
}

pub(super) fn write_poll_timeout(
    runtime: &mut Runtime,
    pid: Word,
    wait: &ReadinessWait,
) -> Result<(), i32> {
    let ptr = timeout_pointer(wait.context);
    if ptr == 0 {
        return Ok(());
    }
    let ticks = wait
        .deadline
        .unwrap_or(runtime.monotonic_ticks)
        .saturating_sub(runtime.monotonic_ticks);
    let hz = runtime.monotonic_tick_hz;
    let (seconds, nanos) = if ticks == 0 {
        (0, 0)
    } else {
        (
            ticks / hz,
            ((ticks % hz) as u128 * 1_000_000_000 / hz as u128) as Word,
        )
    };
    unsafe {
        write_u64(runtime.posix_shm, seconds);
        write_u64(
            runtime.posix_shm + 8,
            if wait.context.number == SYS_SELECT {
                nanos / 1000
            } else {
                nanos
            },
        );
    }
    // Raw ppoll/pselect6 update the timeout; libc preserves its own copy.
    write_target_memory(runtime, pid, ptr, LINUX_TIMESPEC_BYTES)
}

fn timeout_pointer(context: LinuxSyscallContext) -> Word {
    match context.number {
        SYS_PPOLL => context.args[2],
        SYS_SELECT | SYS_PSELECT6 => context.args[4],
        _ => 0,
    }
}
