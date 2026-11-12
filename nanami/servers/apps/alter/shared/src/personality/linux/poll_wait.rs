use super::{
    arm_clock_timer, poll_deadline, poll_timeout, read_poll_set, record_syscall_result,
    refresh_clock, scan_readiness, write_poll_set, write_poll_timeout, EmulationAction,
    LinuxSyscallContext, Runtime, Word, EAGAIN, ESRCH,
};
use crate::state::readiness::*;

#[cfg(test)]
static PENDING_WAIT_BYTES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
pub(super) fn take_pending_wait_bytes() -> usize {
    PENDING_WAIT_BYTES.swap(0, core::sync::atomic::Ordering::Relaxed)
}

fn retain_pending_wait(runtime: &mut Runtime, index: usize, wait: &ReadinessWait) {
    // Small ordinary waitsets retain their immutable interest snapshot. The
    // scan updates only sources/revents; publish the deadline as well. I/O
    // retry contexts keep the original whole-snapshot replacement below.
    let number = wait.context.number;
    // Bound the strided field stores; larger sets keep the contiguous copy.
    if wait.count <= 8 && (matches!(number,
        super::SYS_POLL | super::SYS_PPOLL | super::SYS_SELECT | super::SYS_PSELECT6)
        || super::epoll::is_wait(number))
    {
        if let Some(stored) = runtime.managed[index].readiness_wait.as_mut() {
            stored.sources = wait.sources;
            stored.deadline = wait.deadline;
            for (target, scanned) in stored.entries[..wait.count].iter_mut()
                .zip(&wait.entries[..wait.count])
            {
                target.revents = scanned.revents;
            }
            #[cfg(test)]
            PENDING_WAIT_BYTES.fetch_add(
                core::mem::size_of_val(&wait.sources) + core::mem::size_of_val(&wait.deadline)
                    + wait.count * core::mem::size_of::<i16>(),
                core::sync::atomic::Ordering::Relaxed,
            );
            return;
        }
    }
    runtime.managed[index].readiness_wait = Some(*wait);
    #[cfg(test)]
    PENDING_WAIT_BYTES.fetch_add(core::mem::size_of::<ReadinessWait>(), core::sync::atomic::Ordering::Relaxed);
}

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

fn completion_needs_clock(context: LinuxSyscallContext) -> bool {
    // Integer poll/epoll timeouts are input-only. Other wait contexts retain
    // their completion sample, including APIs that write remaining time.
    context.number != super::SYS_POLL && !super::epoll::is_wait(context.number)
}

fn scan_wait(
    runtime: &mut Runtime, pid: Word, wait: &mut ReadinessWait,
) -> Result<(Word, Option<Word>), i32> {
    if super::epoll::is_wait(wait.context.number) {
        if let Some(file) = runtime.linux_file(pid, wait.context.args[0])
            .filter(|file| file.kind == super::LinuxFileKind::Epoll)
        {
            // complete() changes cursor/ONESHOT only after successful output;
            // an empty collection has no output or commit. The full empty
            // scan also attaches the same input interests before parking.
            wait.sources = READY_OBJECT | super::epoll::sources(runtime, file.resource);
            let value = super::epoll::complete(runtime, pid, wait.context)?;
            let ready = Word::from(value != 0);
            wait.entries[0].revents = if ready != 0 { super::LINUX_POLLIN } else { 0 };
            return Ok((ready, Some(value)));
        }
    }
    // A closed or recycled descriptor keeps the ordinary scan/error path.
    scan_readiness(runtime, pid, wait).map(|ready| (ready, None))
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
    let (ready, completion) = scan_wait(runtime, pid, &mut wait)?;
    if ready != 0 || timeout == Some(0) {
        if wait.deadline.is_some() && completion_needs_clock(context) {
            refresh_clock(runtime)?;
        }
        #[cfg(feature = "wait-shape")]
        super::wait_shape::note(runtime, pid, &wait, 0, timeout, Some(ready), 0, false, 0);
        return match completion {
            Some(value) => Ok(Some(value)),
            None => complete_wait(runtime, pid, &wait).map(Some),
        };
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
    #[cfg(feature = "wait-shape")]
    super::wait_shape::note(runtime, pid, &wait, 1, timeout, Some(ready), 0, false, 0);
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
    wake_readiness_targets(runtime, sources, UnixWakeTargets::default());
}

fn unix_wait_matches(runtime: &Runtime, pid: Word, wait: &ReadinessWait,
    sources: Word, targets: UnixWakeTargets) -> bool
{
    let relevant = wait.sources & sources;
    if relevant & !(READY_UNIX_READ | READY_UNIX_WRITE) != 0 { return true; }
    // Consult current descriptions rather than caching descriptor identities:
    // another task may replace an FD through CLONE_FILES while this one waits.
    for entry in &wait.entries[..wait.count] {
        if entry.fd < 0 { continue; }
        let Some(file) = runtime.linux_file(pid, entry.fd as Word) else { return true; };
        // Epoll follows registered open descriptions, and may be changed by
        // another task. Keep its existing scan and all other provider fallbacks.
        if file.kind != super::LinuxFileKind::SocketUnix { return true; }
        if targets.matches(relevant, file.resource,
            entry.events & (super::LINUX_POLLIN | super::LINUX_POLLRDNORM) != 0,
            entry.events & (super::LINUX_POLLOUT | super::LINUX_POLLWRNORM) != 0)
        { return true; }
    }
    false
}

fn wake_readiness_targets(runtime: &mut Runtime, sources: Word, targets: UnixWakeTargets) {
    let timer = sources & READY_TIMER != 0;
    let mut clock_sampled = timer;
    let mut completed = false;
    let mut deadline_changed = false;
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
        if !expired && targets.qualified(sources)
            && option_env!("NANAMI_UNIX_ENDPOINT_WAKE") != Some("0")
            && !unix_wait_matches(runtime, process.pid, wait, sources, targets)
        {
            #[cfg(target_os = "none")]
            if option_env!("NANAMI_UNIX_ENDPOINT_WAKE_DIAG") == Some("1") {
                use core::sync::atomic::{AtomicUsize, Ordering};
                static SKIPPED: AtomicUsize = AtomicUsize::new(0);
                let count = SKIPPED.fetch_add(1, Ordering::Relaxed) + 1;
                if count <= 8 || count.is_power_of_two() {
                    nanami_runtime::println!("[unix.wake] unrelated count={} pid={} call={} sources={}",
                        count, process.pid, wait.context.number, sources);
                }
            }
            continue;
        }
        if runtime.is_signal_stopped(process.pid) {
            continue;
        }
        let (pid, pcb, personality) = (process.pid, process.pcb, process.personality);
        let mut wait = *wait;
        let previous_deadline = wait.deadline;
        super::pty::update_deadline(runtime, pid, &mut wait);
        if previous_deadline != wait.deadline {
            deadline_changed = true;
            // update_deadline changes only this field. Publish it before the
            // scan/PTY retry without copying the entire interest snapshot.
            runtime.managed[index].readiness_wait.as_mut().unwrap().deadline = wait.deadline;
        }
        #[cfg(feature = "wait-shape")]
        let mut scanned = None;
        let result = scan_wait(runtime, pid, &mut wait).and_then(|(ready, completion)| {
            #[cfg(feature = "wait-shape")]
            { scanned = Some(ready); }
            if let Some(result) = super::pty::retry(runtime, pid, wait.context) {
                return match result {
                    Err(EAGAIN) => Ok(None),
                    result => result.map(Some),
                };
            }
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
            if wait.deadline.is_some()
                && !clock_sampled
                && completion_needs_clock(wait.context)
            {
                refresh_clock(runtime)?;
                clock_sampled = true;
            }
            let result = match completion {
                Some(value) => Ok(value),
                None => complete_wait(runtime, pid, &wait),
            };
            match result {
                Err(EAGAIN) => Ok(None),
                result => result.map(Some),
            }
        });
        #[cfg(feature = "wait-shape")]
        {
            let (phase, value) = match result {
                Ok(None) => (2, 0),
                Ok(Some(value)) => (3, value as isize),
                Err(errno) => (4, -(errno as isize)),
            };
            super::wait_shape::note(runtime, pid, &wait, phase, None, scanned, sources, expired, value);
        }
        let value = match result {
            Ok(None) => {
                // Another process can modify a shared epoll interest set.
                // Retain the newly discovered notification sources even when
                // this change did not immediately make an FD ready.
                retain_pending_wait(runtime, index, &wait);
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
    if (completed || deadline_changed) && !timer {
        if let Err(errno) = arm_clock_timer(runtime) {
            nanami_runtime::println!("[alter/linux] poll alarm failed errno={}", errno);
        }
    }
}

pub fn handle_readiness_changes(runtime: &mut Runtime) {
    // Completing a reader can free a buffer for a writer already scanned in
    // this pass. Propagate that change before blocking in receive again.
    // Each successful I/O removes one waiter; bound passes by the task count
    // as a safeguard against a provider reporting changes without progress.
    for _ in 0..=runtime.managed.len() {
        let sources = ::core::mem::take(&mut runtime.readiness_changes);
        if sources == 0 {
            break;
        }
        let targets = ::core::mem::take(&mut runtime.unix_wake_targets);
        wake_readiness_targets(runtime, sources, targets);
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
