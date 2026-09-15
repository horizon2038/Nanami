use super::{
    map_request_error, present_mapped_framebuffers, read_target_memory, record_syscall_result,
    write_target_memory, write_u64, EmulationAction, LinuxSyscallContext, Runtime, Word,
    ALTER_FB_PRESENT_HZ, ALTER_SLEEP_TICK_HZ, ALTER_SLEEP_TICK_MILLISECONDS,
    ALTER_SLEEP_TICK_NANOSECONDS, EFAULT, EINVAL, EIO, ESRCH, LINUX_CPU_MASK_BYTES,
    LINUX_ITIMERVAL_BYTES, LINUX_ITIMER_PROF, LINUX_PAGE_SIZE, LINUX_TIMESPEC_BYTES, SYS_NANOSLEEP,
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
    ensure_clock_timer(runtime)?;
    let tick_hz = runtime.monotonic_tick_hz;
    if tick_hz == 0 {
        return Err(EIO);
    }
    let seconds = runtime.monotonic_ticks / tick_hz;
    let nanoseconds = (runtime.monotonic_ticks % tick_hz).saturating_mul(1_000_000_000 / tick_hz);
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

    let ticks = (seconds as Word)
        .saturating_mul(ALTER_SLEEP_TICK_HZ)
        .saturating_add(
            (nanoseconds as Word).saturating_add(ALTER_SLEEP_TICK_NANOSECONDS - 1)
                / ALTER_SLEEP_TICK_NANOSECONDS,
        );
    if ticks == 0 {
        return EmulationAction::Return(0);
    }
    if let Err(errno) = ensure_clock_timer(runtime) {
        return EmulationAction::Return(-(errno as isize));
    }

    let Some(process) = runtime.managed_process_mut(pid) else {
        return EmulationAction::Return(-(ESRCH as isize));
    };
    process.sleep_waiting = true;
    process.sleep_ticks_remaining = ticks;
    process.sleep_context = context;
    EmulationAction::Park
}

pub(super) fn ensure_clock_timer(runtime: &mut Runtime) -> Result<(), i32> {
    if runtime.clock_timer_armed {
        return Ok(());
    }
    let (ticks, tick_hz) =
        nanami_services::timer::timer_service_monotonic_ticks(runtime.timer_port)
            .map_err(map_request_error)?;
    if tick_hz == 0 {
        return Err(EIO);
    }
    nanami_services::timer::timer_service_interval_on_notification_milliseconds(
        runtime.timer_port,
        ALTER_SLEEP_TICK_MILLISECONDS,
        libnanami::PROCESS_SLOT_NOTIFICATION,
    )
    .map_err(map_request_error)?;
    runtime.monotonic_ticks = ticks;
    runtime.monotonic_tick_hz = tick_hz;
    runtime.clock_timer_armed = true;
    Ok(())
}

pub fn handle_timer_notification(runtime: &mut Runtime, identifier: Word) {
    if (identifier & nanami_services::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT) == 0
        || !runtime.clock_timer_armed
    {
        return;
    }
    let previous_ticks = runtime.monotonic_ticks;
    let elapsed_ticks =
        match nanami_services::timer::timer_service_monotonic_ticks(runtime.timer_port) {
            Ok((ticks, tick_hz)) if tick_hz != 0 => {
                runtime.monotonic_ticks = ticks;
                runtime.monotonic_tick_hz = tick_hz;
                ticks.saturating_sub(previous_ticks).max(1)
            }
            _ => {
                runtime.monotonic_ticks = runtime.monotonic_ticks.saturating_add(1);
                1
            }
        };

    let tick_hz = runtime.monotonic_tick_hz;
    if tick_hz != 0
        && previous_ticks.saturating_mul(ALTER_FB_PRESENT_HZ) / tick_hz
            != runtime.monotonic_ticks.saturating_mul(ALTER_FB_PRESENT_HZ) / tick_hz
    {
        present_mapped_framebuffers(runtime);
    }

    let mut index = 0usize;
    while index < runtime.managed.len() {
        let process = runtime.managed[index];
        if process.pid == 0 || !process.sleep_waiting {
            index += 1;
            continue;
        }
        if process.sleep_ticks_remaining > elapsed_ticks {
            runtime.managed[index].sleep_ticks_remaining -= elapsed_ticks;
            index += 1;
            continue;
        }

        runtime.managed[index].sleep_waiting = false;
        runtime.managed[index].sleep_ticks_remaining = 0;
        runtime.managed[index].sleep_context = LinuxSyscallContext::EMPTY;
        record_syscall_result(runtime, process.pid, SYS_NANOSLEEP, 0);
        if crate::process::write_personality_syscall_return(
            process.pcb,
            process.sleep_context,
            0,
            process.personality,
        )
        .is_ok()
        {
            let _ = a9n_abi::arch::process_control_block::resume(process.pcb);
        }
        index += 1;
    }
}
