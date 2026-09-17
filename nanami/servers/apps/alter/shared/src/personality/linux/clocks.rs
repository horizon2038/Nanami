use super::{
    arm_clock_timer, map_request_error, read_target_memory, write_target_memory, write_u64,
    EmulationAction, LinuxSyscallContext, Runtime, Word, EFAULT, EINVAL, EIO, ESRCH,
    LINUX_CPU_MASK_BYTES, LINUX_ITIMERVAL_BYTES, LINUX_ITIMER_PROF, LINUX_PAGE_SIZE,
    LINUX_TIMESPEC_BYTES,
};

pub(super) fn sys_gettimeofday(
    runtime: &mut Runtime,
    pid: Word,
    timeval: Word,
) -> Result<Word, i32> {
    if timeval != 0 {
        unsafe {
            write_u64(runtime.posix_shm, 0);
            write_u64(runtime.posix_shm + 8, 0);
        }
        write_target_memory(runtime, pid, timeval, 16)?;
    }
    Ok(0)
}

pub(super) fn sys_setitimer(
    runtime: &mut Runtime,
    pid: Word,
    which: Word,
    new_value: Word,
    old_value: Word,
) -> Result<Word, i32> {
    if which > LINUX_ITIMER_PROF {
        return Err(EINVAL);
    }
    if new_value != 0 {
        read_target_memory(runtime, pid, new_value, LINUX_ITIMERVAL_BYTES)?;
    }
    if old_value != 0 {
        unsafe {
            ::core::ptr::write_bytes(
                runtime.posix_shm as *mut u8,
                0,
                LINUX_ITIMERVAL_BYTES as usize,
            );
        }
        write_target_memory(runtime, pid, old_value, LINUX_ITIMERVAL_BYTES)?;
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
    match clock_id {
        0..=9 | 11 => {}
        _ => return Err(EINVAL),
    }
    refresh_clock(runtime)?;
    let tick_hz = runtime.monotonic_tick_hz;
    if tick_hz == 0 {
        return Err(EIO);
    }
    let seconds = runtime.monotonic_ticks / tick_hz;
    let nanoseconds =
        ((runtime.monotonic_ticks % tick_hz) as u128 * 1_000_000_000 / tick_hz as u128) as Word;
    unsafe {
        write_u64(runtime.posix_shm, seconds);
        write_u64(runtime.posix_shm + 8, nanoseconds);
    }
    write_target_memory(runtime, pid, timespec, LINUX_TIMESPEC_BYTES)?;
    Ok(0)
}

pub(super) fn sys_nanosleep_action(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    let request = context.args[0];
    if request == 0 {
        return EmulationAction::Return(-(EFAULT as isize));
    }
    if let Err(errno) = read_target_memory(runtime, pid, request, LINUX_TIMESPEC_BYTES) {
        return EmulationAction::Return(-(errno as isize));
    }

    let seconds = unsafe { ::core::ptr::read_unaligned(runtime.posix_shm as *const i64) };
    let nanoseconds = unsafe { ::core::ptr::read_unaligned((runtime.posix_shm + 8) as *const i64) };
    if seconds < 0 || !(0..1_000_000_000).contains(&nanoseconds) {
        return EmulationAction::Return(-(EINVAL as isize));
    }

    if seconds == 0 && nanoseconds == 0 {
        return EmulationAction::Return(0);
    }
    if let Err(errno) = refresh_clock(runtime) {
        return EmulationAction::Return(-(errno as isize));
    }

    let hz = runtime.monotonic_tick_hz as u128;
    let ticks = (seconds as u128 * hz + (nanoseconds as u128 * hz).div_ceil(1_000_000_000))
        .min(Word::MAX as u128) as Word;
    let deadline = runtime.monotonic_ticks.saturating_add(ticks);
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

pub(super) fn refresh_clock(runtime: &mut Runtime) -> Result<(), i32> {
    let (ticks, tick_hz) =
        nanami_services::timer::timer_service_monotonic_ticks(runtime.timer_port)
            .map_err(map_request_error)?;
    if tick_hz == 0 {
        return Err(EIO);
    }
    runtime.monotonic_ticks = ticks;
    runtime.monotonic_tick_hz = tick_hz;
    Ok(())
}
