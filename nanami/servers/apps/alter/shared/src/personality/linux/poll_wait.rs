use super::{
    arm_clock_timer, poll_deadline, poll_timeout, read_poll_set, record_syscall_result,
    refresh_clock, scan_readiness, write_poll_set, write_poll_timeout, EmulationAction,
    LinuxSyscallContext, Runtime, Word, EAGAIN, ESRCH,
};
use crate::state::readiness::*;

pub(super) fn sys_poll_action(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    let mask = match super::poll_timeout::temporary_signal_mask(runtime, pid, context) {
        Ok(mask) => mask,
        Err(errno) => return EmulationAction::Return(-(errno as isize)),
    };
    if let Some(mask) = mask {
        let Some(process) = runtime.managed_process_mut(pid) else {
            return EmulationAction::Return(-(ESRCH as isize));
        };
        let state = process.signals_mut();
        state.suspend_mask = Some(state.blocked);
        state.blocked = mask;
    }
    match start_wait(runtime, pid, context) {
        Ok(Some(value)) => {
            restore_signal_mask(runtime, pid);
            EmulationAction::Return(value as isize)
        }
        Ok(None) => EmulationAction::Park,
        Err(errno) => {
            restore_signal_mask(runtime, pid);
            EmulationAction::Return(-(errno as isize))
        }
    }
}

fn restore_signal_mask(runtime: &mut Runtime, pid: Word) {
    if let Some(process) = runtime.managed_process_mut(pid) {
        if process
            .signals()
            .is_some_and(|state| state.suspend_mask.is_some())
        {
            let state = process.signals_mut();
            state.blocked = state.suspend_mask.take().unwrap();
        }
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
    if let Some(result) = super::unix::retry(runtime, pid, wait.context) {
        return result;
    }
    if let Some(result) = super::pipe_wait::retry(runtime, pid, wait.context) {
        return result;
    }
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
        if process.pid == 0 || process.exited || runtime.is_signal_stopped(process.pid) {
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
            if wait.context.number == super::SYS_IO_URING_ENTER {
                return match super::io_uring::retry(runtime, pid, wait.context) {
                    Err(EAGAIN) => {
                        wait.sources = READY_OBJECT;
                        Ok(None)
                    }
                    result => result.map(Some),
                };
            }
            if wait.context.number == super::SYS_FLOCK {
                return match super::flock::lock(
                    runtime,
                    pid,
                    wait.context.args[0],
                    wait.context.args[1],
                ) {
                    Err(EAGAIN) => {
                        wait.sources = READY_FLOCK;
                        Ok(None)
                    }
                    result => result.map(Some),
                };
            }
            if ready == 0 && !expired {
                return Ok(None);
            }
            if wait.deadline.is_some() && !clock_sampled {
                refresh_clock(runtime)?;
                clock_sampled = true;
            }
            match complete_wait(runtime, pid, &wait) {
                Err(EAGAIN) => Ok(None),
                result => result.map(Some),
            }
        });
        let value = match result {
            Ok(None) => {
                // Another process can modify a shared epoll interest set.
                // Retain the newly discovered notification sources even when
                // this change did not immediately make an FD ready.
                runtime.managed[index].readiness_wait = Some(wait);
                continue;
            }
            Ok(Some(value)) => value as isize,
            Err(errno) => -(errno as isize),
        };
        runtime.managed[index].readiness_wait = None;
        restore_signal_mask(runtime, pid);
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
            nanami_runtime::println!("[alter/linux] poll alarm failed errno={}", errno);
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
    if identifier & nanami_client::posix::POSIX_NOTIFICATION_LOCK_CHANGE != 0 {
        wake_readiness_waiters(runtime, READY_FLOCK);
    }
    let sources = (if identifier & nanami_client::terminal::TERMINAL_NOTIFICATION_INPUT != 0 {
        READY_TERMINAL
    } else {
        0
    }) | (if identifier & nanami_client::net::NET_NOTIFICATION_RX != 0 {
        READY_NETWORK
    } else {
        0
    }) | (if identifier & nanami_client::input::INPUT_NOTIFICATION_IDENTIFIER != 0 {
        READY_INPUT
    } else {
        0
    });
    if sources != 0 {
        wake_readiness_waiters(runtime, sources);
    }
}
