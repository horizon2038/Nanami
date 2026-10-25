use super::{
    arm_clock_timer, map_request_error, read_target_memory, write_target_memory, write_u64,
    EmulationAction, LinuxSyscallContext, Runtime, Word, EFAULT, EINVAL, EIO, EOPNOTSUPP, ESRCH,
    LINUX_CPU_MASK_BYTES, LINUX_PAGE_SIZE, LINUX_TIMESPEC_BYTES,
};

pub(super) fn sys_time(runtime: &mut Runtime, pid: Word, tloc: Word) -> Result<Word, i32> {
    let (seconds, _) = super::realtime::realtime(runtime)?;
    if tloc != 0 {
        unsafe {
            write_u64(runtime.posix_shm, seconds);
        }
        write_target_memory(runtime, pid, tloc, 8).map_err(|_| EFAULT)?;
    }
    Ok(seconds)
}

pub(super) fn sys_gettimeofday(
    runtime: &mut Runtime,
    pid: Word,
    timeval: Word,
) -> Result<Word, i32> {
    if timeval != 0 {
        let (seconds, nanos) = super::realtime::realtime(runtime)?;
        unsafe {
            write_u64(runtime.posix_shm, seconds);
            write_u64(runtime.posix_shm + 8, nanos / 1000);
        }
        write_target_memory(runtime, pid, timeval, 16)?;
    }
    Ok(0)
}

pub(super) fn sys_sched_getaffinity(
    runtime: &mut Runtime,
    current_pid: Word,
    target_pid: Word,
    cpuset_size: Word,
    mask: Word,
) -> Result<Word, i32> {
    if mask == 0 || cpuset_size < LINUX_CPU_MASK_BYTES || cpuset_size > LINUX_PAGE_SIZE {
        return Err(EINVAL);
    }
    if target_pid != 0 && target_pid != current_pid && runtime.managed_process(target_pid).is_none()
    {
        return Err(ESRCH);
    }
    unsafe {
        ::core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, cpuset_size as usize);
        ::core::ptr::write_unaligned(runtime.posix_shm as *mut u64, 1);
    }
    write_target_memory(runtime, current_pid, mask, cpuset_size)?;
    Ok(LINUX_CPU_MASK_BYTES)
}

pub(super) fn sys_clock_gettime(
    runtime: &mut Runtime,
    pid: Word,
    clock_id: Word,
    timespec: Word,
) -> Result<Word, i32> {
    if timespec == 0 {
        return Err(EFAULT);
    }
    let (seconds, nanoseconds) = match clock_id {
        0 | 5 | 8 => super::realtime::realtime(runtime)?,
        1 | 4 | 6 | 7 | 9 => {
            refresh_clock(runtime)?;
            split_ticks(runtime.monotonic_ticks, runtime.monotonic_tick_hz)
        }
        // No per-process CPU accounting or TAI offset is available. Do not
        // misrepresent system uptime as process/thread CPU time or TAI.
        _ => return Err(EINVAL),
    };
    unsafe {
        write_u64(runtime.posix_shm, seconds);
        write_u64(runtime.posix_shm + 8, nanoseconds);
    }
    write_target_memory(runtime, pid, timespec, LINUX_TIMESPEC_BYTES)?;
    Ok(0)
}

pub(super) fn sys_clock_getres(
    runtime: &mut Runtime,
    pid: Word,
    clock_id: Word,
    timespec: Word,
) -> Result<Word, i32> {
    match clock_id {
        0 | 1 | 4 | 5 | 6 | 7 | 8 | 9 => {}
        _ => return Err(EINVAL),
    }
    if timespec != 0 {
        refresh_clock(runtime)?;
        let hz = runtime.monotonic_tick_hz;
        if hz == 0 {
            return Err(EIO);
        }
        let nanos = 1_000_000_000usize.div_ceil(hz).max(1);
        unsafe {
            write_u64(runtime.posix_shm, nanos / 1_000_000_000);
            write_u64(runtime.posix_shm + 8, nanos % 1_000_000_000);
        }
        write_target_memory(runtime, pid, timespec, LINUX_TIMESPEC_BYTES).map_err(|_| EFAULT)?;
    }
    Ok(0)
}

pub(super) fn split_ticks(ticks: Word, tick_hz: Word) -> (Word, Word) {
    if tick_hz == 1_000_000_000 {
        // HPET already reports nanoseconds. Constant division avoids the
        // generic 128-bit scale/divide on every clock_gettime call.
        (ticks / 1_000_000_000, ticks % 1_000_000_000)
    } else {
        (
            ticks / tick_hz,
            ((ticks % tick_hz) as u128 * 1_000_000_000 / tick_hz as u128) as Word,
        )
    }
}

pub(super) fn sys_nanosleep_action(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    sleep_action(runtime, pid, context, context.args[0], 1, false)
}

pub(super) fn sys_clock_nanosleep_action(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    match context.args[0] {
        0 | 1 | 7 => {}
        // Known clocks without a sleep backend. Never substitute uptime for
        // CPU/TAI/alarm clocks, or imply that RAW/COARSE support sleeping.
        2 | 4 | 5 | 6 | 8 | 9 | 11 => {
            return EmulationAction::Return(-(EOPNOTSUPP as isize));
        }
        _ => return EmulationAction::Return(-(EINVAL as isize)),
    }
    sleep_action(
        runtime,
        pid,
        context,
        context.args[2],
        context.args[0],
        context.args[1] & 1 != 0, // TIMER_ABSTIME; Linux ignores other flag bits.
    )
}

fn sleep_action(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
    request: Word,
    clock_id: Word,
    absolute: bool,
) -> EmulationAction {
    let deadline = match sleep_deadline(runtime, pid, request, clock_id, absolute) {
        Ok(Some(deadline)) => deadline,
        Ok(None) => return EmulationAction::Return(0),
        Err(errno) => return EmulationAction::Return(-(errno as isize)),
    };
    let Some(process) = runtime.managed_process_mut(pid) else {
        return EmulationAction::Return(-(ESRCH as isize));
    };
    process.sleep_waiting = true;
    process.sleep_deadline = deadline;
    process.sleep_context = context;
    if let Err(errno) = arm_clock_timer(runtime) {
        let process = runtime.managed_process_mut(pid).unwrap();
        process.sleep_waiting = false;
        process.sleep_deadline = 0;
        process.sleep_context = LinuxSyscallContext::EMPTY;
        return EmulationAction::Return(-(errno as isize));
    }
    EmulationAction::Park
}

pub(super) fn sleep_deadline(
    runtime: &mut Runtime,
    pid: Word,
    request: Word,
    clock_id: Word,
    absolute: bool,
) -> Result<Option<Word>, i32> {
    if request == 0 {
        return Err(EFAULT);
    }
    read_target_memory(runtime, pid, request, LINUX_TIMESPEC_BYTES).map_err(|_| EFAULT)?;

    let seconds = unsafe { ::core::ptr::read_unaligned(runtime.posix_shm as *const i64) };
    let nanoseconds = unsafe { ::core::ptr::read_unaligned((runtime.posix_shm + 8) as *const i64) };
    if seconds < 0 || !(0..1_000_000_000).contains(&nanoseconds) {
        return Err(EINVAL);
    }

    if !absolute && seconds == 0 && nanoseconds == 0 {
        return Ok(None);
    }
    let mut seconds = seconds as Word;
    let nanoseconds = nanoseconds as Word;
    let base = if absolute && clock_id == 0 {
        super::realtime::realtime(runtime)?;
        let (epoch, anchor) = runtime.realtime_anchor.ok_or(EIO)?;
        if seconds < epoch {
            return Ok(None);
        }
        seconds -= epoch;
        // Convert from the same RTC anchor as CLOCK_REALTIME, avoiding a
        // second rounding of now's fractional nanoseconds at low tick rates.
        anchor
    } else {
        refresh_clock(runtime)?;
        if absolute {
            0
        } else {
            runtime.monotonic_ticks
        }
    };
    let hz = runtime.monotonic_tick_hz as u128;
    let ticks = (seconds as u128 * hz + (nanoseconds as u128 * hz).div_ceil(1_000_000_000))
        .min(Word::MAX as u128) as Word;
    let deadline = base.saturating_add(ticks);
    if deadline <= runtime.monotonic_ticks {
        return Ok(None);
    }
    Ok(Some(deadline))
}

pub(super) fn refresh_clock(runtime: &mut Runtime) -> Result<(), i32> {
    if runtime.timer_port == 0 {
        // Initial absence is not a permanent failure: policy can start/import
        // the provider later. Never substitute a clock or call descriptor zero.
        nanami_client::registry::connect_timer_service(crate::abi::SLOT_TIMER_SERVICE)
            .map_err(map_request_error)?;
        runtime.timer_port =
            nanami_runtime::ipc::process_slot_descriptor(crate::abi::SLOT_TIMER_SERVICE);
    }
    let (ticks, tick_hz) = nanami_client::timer::timer_service_monotonic_ticks(runtime.timer_port)
        .map_err(map_request_error)?;
    if tick_hz == 0 {
        return Err(EIO);
    }
    runtime.monotonic_ticks = ticks;
    runtime.monotonic_tick_hz = tick_hz;
    Ok(())
}
