use super::{
    arm_clock_timer, poll_deadline, poll_timeout, read_poll_set, record_syscall_result,
    refresh_clock, scan_readiness, write_poll_set, write_poll_timeout, EmulationAction,
    LinuxSyscallContext, Runtime, Word, ESRCH,
};
use crate::state::readiness::*;

pub(super) fn sys_poll_action(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    match start_wait(runtime, pid, context) {
        Ok(Some(value)) => EmulationAction::Return(value as isize),
        Ok(None) => EmulationAction::Park,
        Err(errno) => EmulationAction::Return(-(errno as isize)),
    }
}

fn start_wait(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> Result<Option<Word>, i32> {
    let timeout = poll_timeout(runtime, pid, context)?;
    let mut wait = read_poll_set(runtime, pid, context)?;
    if let Some(nanos) = timeout.filter(|nanos| *nanos != 0) {
        refresh_clock(runtime)?;
        wait.deadline = Some(poll_deadline(
            runtime.monotonic_ticks,
            runtime.monotonic_tick_hz,
            nanos,
        ));
    }
    let ready = scan_readiness(runtime, pid, &mut wait)?;
    if ready != 0 || timeout == Some(0) {
        if wait.deadline.is_some() {
            refresh_clock(runtime)?;
        }
        return complete_wait(runtime, pid, &wait).map(Some);
    }
    // Notifications are attached before the readiness query. IPC preserves
    // notifications received during the query, closing the check/park race.
    runtime
        .managed_process_mut(pid)
        .ok_or(ESRCH)?
        .readiness_wait = Some(wait);
    if let Err(errno) = arm_clock_timer(runtime) {
        runtime.managed_process_mut(pid).unwrap().readiness_wait = None;
        return Err(errno);
    }
    Ok(None)
}

fn complete_wait(runtime: &mut Runtime, pid: Word, wait: &ReadinessWait) -> Result<Word, i32> {
    let ready = write_poll_set(runtime, pid, wait)?;
    write_poll_timeout(runtime, pid, wait)?;
    Ok(ready)
}

pub fn wake_readiness_waiters(runtime: &mut Runtime, sources: Word) {
    let timer = sources & READY_TIMER != 0;
    let mut clock_sampled = timer;
    let mut completed = false;
    for index in 0..runtime.managed.len() {
        let process = &runtime.managed[index];
        let Some(wait) = process.readiness_wait.as_ref() else {
            continue;
        };
        if process.pid == 0 || process.exited {
            continue;
        }
        let expired = timer
            && wait
                .deadline
                .is_some_and(|deadline| deadline <= runtime.monotonic_ticks);
        if !expired && wait.sources & sources == 0 {
            continue;
        }
        let (pid, pcb, personality) = (process.pid, process.pcb, process.personality);
        let mut wait = *wait;
        let result = scan_readiness(runtime, pid, &mut wait).and_then(|ready| {
            if ready == 0 && !expired {
                return Ok(None);
            }
            if wait.deadline.is_some() && !clock_sampled {
                refresh_clock(runtime)?;
                clock_sampled = true;
            }
            complete_wait(runtime, pid, &wait).map(Some)
        });
        let value = match result {
            Ok(None) => continue,
            Ok(Some(value)) => value as isize,
            Err(errno) => -(errno as isize),
        };
        runtime.managed[index].readiness_wait = None;
        completed = true;
        record_syscall_result(runtime, pid, wait.context.number, value);
        if crate::process::write_personality_syscall_return(pcb, wait.context, value, personality)
            .is_ok()
        {
            let _ = a9n_abi::arch::process_control_block::resume(pcb);
        }
    }
    if completed && !timer {
        if let Err(errno) = arm_clock_timer(runtime) {
            libnanami::println!("[alter/linux] poll alarm failed errno={}", errno);
        }
    }
}

pub fn handle_readiness_changes(runtime: &mut Runtime) {
    let sources = ::core::mem::take(&mut runtime.readiness_changes);
    if sources != 0 {
        wake_readiness_waiters(runtime, sources);
    }
}

pub fn handle_readiness_notification(runtime: &mut Runtime, identifier: Word) {
    let sources = (if identifier & nanami_services::terminal::TERMINAL_NOTIFICATION_INPUT != 0 {
        READY_TERMINAL
    } else {
        0
    }) | (if identifier & nanami_services::net::NET_NOTIFICATION_RX != 0 {
        READY_NETWORK
    } else {
        0
    }) | (if identifier & nanami_services::input::INPUT_NOTIFICATION_IDENTIFIER != 0 {
        READY_INPUT
    } else {
        0
    });
    if sources != 0 {
        wake_readiness_waiters(runtime, sources);
    }
}
