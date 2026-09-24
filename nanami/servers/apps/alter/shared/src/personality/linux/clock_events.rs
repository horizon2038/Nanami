use super::{
    map_request_error, present_mapped_framebuffers, record_syscall_result, refresh_clock,
    LinuxSyscallContext, Runtime, Word, ALTER_FB_PRESENT_HZ, SYS_NANOSLEEP,
};

pub(super) fn arm_clock_timer(runtime: &mut Runtime) -> Result<(), i32> {
    if runtime
        .graphics
        .iter()
        .any(|session| session.active && session.guest_pid != 0 && session.guest_framebuffer != 0)
    {
        if runtime.framebuffer_deadline.is_none() {
            runtime.framebuffer_deadline = Some(next_frame(runtime));
        }
    } else {
        runtime.framebuffer_deadline = None;
    }
    let deadline = runtime
        .managed
        .iter()
        .filter(|process| process.pid != 0 && process.sleep_waiting)
        .map(|process| process.sleep_deadline)
        .chain(runtime.managed.iter()
            .filter(|process| process.pid != 0 && !process.exited)
            .filter_map(|process| process.readiness_wait.as_ref().and_then(|wait| wait.deadline)))
        .chain(runtime.framebuffer_deadline)
        .min();
    if runtime.clock_deadline != deadline {
        nanami_services::timer::timer_service_set_alarm_ticks(
            runtime.timer_port,
            libnanami::PROCESS_SLOT_NOTIFICATION,
            deadline,
        )
        .map_err(map_request_error)?;
        runtime.clock_deadline = deadline;
    }
    Ok(())
}

fn next_frame(runtime: &Runtime) -> Word {
    // Keep the 60 Hz phase, even after a delayed notification or clock query.
    let hz = runtime.monotonic_tick_hz as u128;
    let frame = runtime.monotonic_ticks as u128 * ALTER_FB_PRESENT_HZ as u128 / hz + 1;
    (frame * hz)
        .div_ceil(ALTER_FB_PRESENT_HZ as u128)
        .min(Word::MAX as u128) as Word
}

pub fn handle_timer_notification(runtime: &mut Runtime, identifier: Word) {
    if identifier & nanami_services::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT == 0 {
        return;
    }
    if let Err(errno) = refresh_clock(runtime) {
        // Do not fabricate elapsed time on clocksource failure.
        libnanami::println!("[alter/linux] timer clock failed errno={}", errno);
        return;
    }
    // A queued notification can outlive replacement/cancellation of its alarm.
    if runtime
        .clock_deadline
        .is_some_and(|deadline| deadline <= runtime.monotonic_ticks)
    {
        runtime.clock_deadline = None;
    }
    if runtime
        .framebuffer_deadline
        .is_some_and(|deadline| deadline <= runtime.monotonic_ticks)
    {
        present_mapped_framebuffers(runtime);
        runtime.framebuffer_deadline = Some(next_frame(runtime));
    }

    for index in 0..runtime.managed.len() {
        let process = &runtime.managed[index];
        if process.pid == 0
            || !process.sleep_waiting
            || process.sleep_deadline > runtime.monotonic_ticks
        {
            continue;
        }
        let (pid, pcb, context, personality) = (process.pid, process.pcb, process.sleep_context, process.personality);
        runtime.managed[index].sleep_waiting = false;
        runtime.managed[index].sleep_deadline = 0;
        runtime.managed[index].sleep_context = LinuxSyscallContext::EMPTY;
        record_syscall_result(runtime, pid, SYS_NANOSLEEP, 0);
        if crate::process::write_personality_syscall_return(
            pcb,
            context,
            0,
            personality,
        )
        .is_ok()
        {
            let _ = a9n_abi::arch::process_control_block::resume(pcb);
        }
    }
    super::wake_readiness_waiters(runtime, crate::state::readiness::READY_TIMER);
    if let Err(errno) = arm_clock_timer(runtime) {
        libnanami::println!("[alter/linux] timer alarm failed errno={}", errno);
    }
}
