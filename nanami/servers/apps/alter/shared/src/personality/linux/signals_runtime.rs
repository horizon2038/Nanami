//! Signal delivery to parked tasks and synchronously suspended running tasks.
//! Suspend must complete before reading a running target's register context.
use super::*;
use crate::common::signals::{
    self, Action, AltStack, Info, SA_NODEFER, SA_ONSTACK, SA_RESETHAND, SA_RESTART, SA_RESTORER,
    SS_AUTODISARM, SS_ONSTACK,
};
use crate::process::{
    configure_personality_syscall_reply, read_hardware_context, write_hardware_context,
    HardwareContext,
};

pub enum Delivery {
    Unchanged,
    Handler,
    Stopped,
    Terminate(Word),
}

pub fn queue(runtime: &mut Runtime, pid: Word, info: Info) -> Result<(), i32> {
    queue_to(runtime, pid, info, false)
}

pub fn queue_group(runtime: &mut Runtime, pid: Word, info: Info) -> Result<(), i32> {
    let group = runtime.thread_group(pid);
    let target = runtime
        .managed
        .iter()
        .find(|process| process.pid != 0 && !process.exited && process.tgid == group)
        .map(|process| process.pid)
        .ok_or(ESRCH)?;
    if signals::stop_signal(info.signal()) || info.signal() == signals::SIGCONT {
        let targets: alloc::vec::Vec<_> = runtime
            .managed
            .iter()
            .filter(|process| process.pid != 0 && !process.exited && process.tgid == group)
            .map(|process| process.pid)
            .collect();
        // Default job-control actions affect every task, not just one recipient.
        if runtime
            .managed_process(target)
            .and_then(|process| process.signals())
            .is_none_or(|signals| signals.action(info.signal()).handler == signals::SIG_DFL)
        {
            for tid in targets {
                queue_to(runtime, tid, info, false)?;
            }
            return Ok(());
        }
    }
    queue_to(runtime, target, info, true)
}

fn queue_to(runtime: &mut Runtime, pid: Word, info: Info, group: bool) -> Result<(), i32> {
    let signal = info.signal();
    if signal == 0 || signal > 64 {
        return Err(EINVAL);
    }
    let process = runtime.managed_process_mut(pid).ok_or(ESRCH)?;
    if process.exited {
        return Err(ESRCH);
    }
    if process.signals().is_none() && signals::default_ignored(signal) {
        return Ok(());
    }
    if signal == signals::SIGCONT {
        let state = process.signals_mut();
        if state.stopped {
            state.stopped = false;
            state.child_event = Some(0xffff);
            state.child_notify = true;
        }
    }
    let state = process.signals_mut();
    let action = state.action(signal);
    // Stop/continue cancellation applies even when the signal itself is ignored.
    if signal == signals::SIGCONT {
        state.discard_stops();
    } else if signals::stop_signal(signal) {
        state.discard(signals::SIGCONT);
    }
    let ignored = action.handler == signals::SIG_IGN
        || (action.handler == signals::SIG_DFL
            && signals::default_ignored(signal)
            && state.blocked & signals::bit(signal) == 0
            && state
                .wait_mask
                .is_none_or(|mask| mask & signals::bit(signal) == 0));
    let accepted = if ignored {
        true
    } else if group {
        state.enqueue_group(info)
    } else {
        state.enqueue(info)
    };
    if !accepted {
        return Err(EAGAIN);
    }
    runtime.signals_pending = true;
    runtime.readiness_changes |= crate::state::readiness::READY_OBJECT;
    Ok(())
}

fn restorer(runtime: &mut Runtime, pid: Word, action: Action) -> Result<Word, i32> {
    if action.flags & SA_RESTORER != 0 {
        if !signal_frame::user_address(action.restorer) {
            return Err(EFAULT);
        }
        return Ok(action.restorer);
    }
    let existing = runtime
        .managed_process(pid)
        .ok_or(ESRCH)?
        .signals()
        .map_or(0, |state| state.trampoline);
    if existing != 0 {
        return Ok(existing);
    }
    let (address, _) =
        map_anonymous_tracked(runtime, pid, 4096, LINUX_PROT_READ | LINUX_PROT_WRITE)?;
    let result = write_target_memory_from(
        pid,
        address,
        signal_frame::TRAMPOLINE.as_ptr() as Word,
        signal_frame::TRAMPOLINE.len() as Word,
    )
    .and_then(|_| {
        sys_mprotect(
            runtime,
            pid,
            address,
            4096,
            LINUX_PROT_READ | LINUX_PROT_EXEC,
        )
    });
    if let Err(errno) = result {
        let _ = sys_munmap(runtime, pid, address, 4096);
        return Err(errno);
    }
    runtime
        .managed_process_mut(pid)
        .ok_or(ESRCH)?
        .signals_mut()
        .trampoline = address;
    Ok(address)
}

fn enter_handler(
    runtime: &mut Runtime,
    pid: Word,
    registers: &mut HardwareContext,
    context: Option<LinuxSyscallContext>,
    info: Info,
    action: Action,
) -> Result<(), i32> {
    if !signal_frame::user_address(action.handler) {
        return Err(EFAULT);
    }
    let restorer = restorer(runtime, pid, action)?;
    let process = runtime.managed_process(pid).ok_or(ESRCH)?;
    let state = process.signals().ok_or(ESRCH)?;
    let original_stack = registers[signal_frame::SP];
    let mut stack = state.altstack;
    let onstack = stack.contains(original_stack);
    let mask = state.suspend_mask.unwrap_or(state.blocked);
    let top =
        if action.flags & SA_ONSTACK != 0 && stack.flags & signals::SS_DISABLE == 0 && !onstack {
            stack.sp.checked_add(stack.size).ok_or(EFAULT)?
        } else {
            original_stack
        };
    let address = signal_frame::frame_address(top).ok_or(EFAULT)?;
    if !signal_frame::user_address(address)
        || (stack.contains(top.saturating_sub(1)) && address < stack.sp)
    {
        return Err(EFAULT);
    }
    if let Some(context) = context {
        configure_personality_syscall_reply(
            registers,
            context,
            -(EINTR as isize),
            OsPersonality::Linux,
        );
        if action.flags & SA_RESTART != 0 && restartable(context) {
            signal_frame::restart(registers, context);
        } else if context.number == SYS_NANOSLEEP || context.number == SYS_CLOCK_NANOSLEEP {
            sleep_remainder(runtime, pid, context)?;
        }
    }
    if runtime.posix_shm_size < signal_frame::FRAME_BYTES {
        return Err(ENOMEM);
    }
    let frame = unsafe {
        ::core::slice::from_raw_parts_mut(runtime.posix_shm as *mut u8, signal_frame::FRAME_BYTES)
    };
    signal_frame::save(frame, registers, restorer);
    if onstack {
        stack.flags |= SS_ONSTACK;
    }
    signal_frame::put_stack(frame, signal_frame::STACK_OFFSET, stack);
    signal_frame::put(frame, signal_frame::MASK_OFFSET, mask as Word);
    frame[signal_frame::INFO_OFFSET..signal_frame::INFO_OFFSET + 128].copy_from_slice(&info.bytes);
    write_target_memory(runtime, pid, address, signal_frame::FRAME_BYTES)?;
    let process = runtime.managed_process_mut(pid).ok_or(ESRCH)?;
    process.cancel_signal_wait();
    let state = process.signals_mut();
    state.suspend_mask = None;
    state.wait_mask = None;
    state.blocked = signals::blockable(state.blocked | action.mask);
    if action.flags & SA_NODEFER == 0 {
        state.blocked |= signals::bit(info.signal());
    }
    if action.flags & SA_RESETHAND != 0 {
        state.set_action(info.signal(), Action::default());
    }
    if stack.flags & SS_AUTODISARM != 0 {
        state.altstack = AltStack::DISABLED;
    }
    signal_frame::enter(registers, address, action.handler, info.signal(), restorer);
    Ok(())
}

fn restartable(context: LinuxSyscallContext) -> bool {
    if context.number == SYS_FUTEX {
        // An absolute WAIT_BITSET can be replayed without extending its timeout.
        // Relative timed WAIT would require a restart-block deadline instead.
        return context.args[3] == 0 || context.args[1] & 0x7f == 9;
    }
    matches!(
        context.number,
        SYS_READ
            | SYS_READV
            | SYS_WRITE
            | SYS_WRITEV
            | SYS_WAIT4
            | SYS_ACCEPT
            | SYS_ACCEPT4
            | SYS_CONNECT
            | SYS_RECVFROM
            | SYS_RECVMSG
            | SYS_SENDTO
            | SYS_SENDMSG
    )
}

fn sleep_remainder(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> Result<(), i32> {
    let pointer = if context.number == SYS_NANOSLEEP {
        context.args[1]
    } else {
        if context.args[1] & 1 != 0 {
            return Ok(());
        }
        context.args[3]
    };
    if pointer == 0 {
        return Ok(());
    }
    refresh_clock(runtime)?;
    let deadline = runtime.managed_process(pid).ok_or(ESRCH)?.sleep_deadline;
    let (seconds, nanos) = split_ticks(
        deadline.saturating_sub(runtime.monotonic_ticks),
        runtime.monotonic_tick_hz,
    );
    unsafe {
        write_u64(runtime.posix_shm, seconds);
        write_u64(runtime.posix_shm + 8, nanos);
    }
    write_target_memory(runtime, pid, pointer, 16)
}

pub fn deliver(
    runtime: &mut Runtime,
    pid: Word,
    registers: &mut HardwareContext,
    interrupted: Option<LinuxSyscallContext>,
) -> Delivery {
    // Linux defers ordinary handlers while vfork waits for exec/exit. The child
    // release will wake the parent; SIGKILL must still be able to terminate it.
    if interrupted.is_some_and(|context| context.number == SYS_VFORK)
        && runtime
            .managed_process(pid)
            .and_then(|process| process.signals())
            .is_none_or(|state| state.pending_mask() & signals::bit(signals::SIGKILL) == 0)
    {
        return Delivery::Unchanged;
    }
    if let Some(context) = interrupted {
        if context.number == SYS_RT_SIGTIMEDWAIT {
            if let Some(value) = super::signal_syscalls::consume_wait(runtime, pid, context.args[1])
            {
                runtime
                    .managed_process_mut(pid)
                    .unwrap()
                    .cancel_signal_wait();
                configure_personality_syscall_reply(
                    registers,
                    context,
                    value,
                    OsPersonality::Linux,
                );
                return Delivery::Handler;
            }
        }
    }
    loop {
        let Some(process) = runtime.managed_process_mut(pid) else {
            return Delivery::Terminate(11);
        };
        let Some(_) = process.signals() else {
            return Delivery::Unchanged;
        };
        let state = process.signals_mut();
        let Some(info) = state.next() else {
            return if state.stopped {
                Delivery::Stopped
            } else {
                Delivery::Unchanged
            };
        };
        let signal = info.signal();
        let action = state.action(signal);
        if signal == signals::SIGKILL {
            state.exit_signal = signal;
            return Delivery::Terminate(signal);
        }
        if action.handler == signals::SIG_IGN {
            continue;
        }
        if action.handler == signals::SIG_DFL {
            if signals::default_ignored(signal) {
                continue;
            }
            if signals::stop_signal(signal) {
                state.stopped = true;
                state.child_event = Some(((signal as u32) << 8) | 0x7f);
                state.child_notify = true;
                if interrupted.is_none() {
                    state.stopped_context = Some(*registers);
                }
                notify_child(runtime, pid, 5, signal);
                return Delivery::Stopped;
            }
            state.exit_signal = signal;
            return Delivery::Terminate(signal);
        }
        if enter_handler(runtime, pid, registers, interrupted, info, action).is_err() {
            runtime
                .managed_process_mut(pid)
                .unwrap()
                .signals_mut()
                .exit_signal = 11;
            return Delivery::Terminate(11);
        }
        return Delivery::Handler;
    }
}

pub fn restore(
    runtime: &mut Runtime,
    pid: Word,
    registers: &mut HardwareContext,
) -> Result<(), ()> {
    let address = signal_frame::return_frame_address(registers).ok_or(())?;
    if !signal_frame::user_address(address) || runtime.posix_shm_size < signal_frame::FRAME_BYTES {
        return Err(());
    }
    read_target_memory(runtime, pid, address, signal_frame::FRAME_BYTES).map_err(|_| ())?;
    let frame = unsafe {
        ::core::slice::from_raw_parts(runtime.posix_shm as *const u8, signal_frame::FRAME_BYTES)
    };
    if !signal_frame::restore(frame, registers) {
        return Err(());
    }
    let mask = signals::blockable(signal_frame::get(frame, signal_frame::MASK_OFFSET) as u64);
    let mut stack = signal_frame::get_stack(frame, signal_frame::STACK_OFFSET);
    stack.flags &= !SS_ONSTACK;
    if stack.flags & !(signals::SS_DISABLE | SS_AUTODISARM) != 0
        || (stack.flags & signals::SS_DISABLE == 0
            && (stack.size < signal_frame::MINSIGSTKSZ
                || stack
                    .sp
                    .checked_add(stack.size)
                    .is_none_or(|end| !signal_frame::user_address(end))))
    {
        return Err(());
    }
    let state = runtime.managed_process_mut(pid).ok_or(())?.signals_mut();
    state.blocked = mask;
    state.altstack = stack;
    runtime.signals_pending = true;
    Ok(())
}

/// The event loop serializes delivery with syscall emulation. Its current fault
/// is excluded until the pending reply is sent; other unaccepted fault IPCs can
/// be cancelled by Suspend, which prepares their syscall for replay.
pub fn pump(runtime: &mut Runtime, excluded: Word) {
    if !runtime.signals_pending {
        return;
    }
    let mut continued = false;
    for index in 0..runtime.managed.len() {
        let process = runtime.managed[index];
        if process.pid == 0 || process.exited {
            continue;
        }
        let Some(state) = process.signals() else {
            continue;
        };
        if let Some(status) = state.child_event.filter(|_| state.child_notify) {
            continued |= status == 0xffff;
            notify_child(
                runtime,
                process.pid,
                if status == 0xffff { 6 } else { 5 },
                (status >> 8) as Word,
            );
        }
        if process.pid == excluded {
            continue;
        }
        let state = runtime.managed[index].signals().unwrap();
        let saved = if !state.stopped {
            state.stopped_context
        } else {
            None
        };
        let waiting = process.parked_context();
        let pending_mask = state.pending_mask();
        let kill = pending_mask & signals::bit(signals::SIGKILL) != 0;
        if kill {
            terminate(runtime, process.pid, 9);
            continue;
        }
        // A created OCI process must not start merely because it has a signal.
        if process.rootfs.prepared {
            continue;
        }
        let eligible = pending_mask & (!state.blocked | state.wait_mask.unwrap_or(0));
        if saved.is_none() && (state.stopped || eligible == 0) {
            continue;
        }
        let suspended = saved.is_none() && waiting.is_none();
        if suspended && a9n_abi::arch::process_control_block::suspend(process.pcb).is_err() {
            // Do not inspect a context without the remote CPU's stop ACK.
            // Leave its pending signals intact so a later pump can retry.
            continue;
        }
        let mut registers = match saved
            .map(Ok)
            .unwrap_or_else(|| read_hardware_context(process.pcb))
        {
            Ok(registers) => registers,
            Err(()) => {
                terminate(runtime, process.pid, 11);
                continue;
            }
        };
        let interrupted = if saved.is_some() { None } else { waiting };
        let delivery = deliver(runtime, process.pid, &mut registers, interrupted);
        match delivery {
            Delivery::Terminate(signal) => terminate(runtime, process.pid, signal),
            Delivery::Stopped => {}
            Delivery::Handler | Delivery::Unchanged
                if suspended || saved.is_some() || matches!(delivery, Delivery::Handler) =>
            {
                runtime.managed[index].signals_mut().stopped_context = None;
                let changed = saved.is_some() || matches!(delivery, Delivery::Handler);
                if (changed && write_hardware_context(process.pcb, &registers).is_err())
                    || a9n_abi::arch::process_control_block::resume(process.pcb).is_err()
                {
                    terminate(runtime, process.pid, 11);
                }
            }
            _ => {}
        }
    }
    runtime.signals_pending = runtime.managed.iter().any(|process| {
        process.pid != 0
            && !process.exited
            && process.signals().is_some_and(|state| {
                state.has_pending()
                    || state.child_notify
                    || (!state.stopped && state.stopped_context.is_some())
            })
    });
    if continued {
        super::futex::pump(runtime);
        // Readiness may have arrived while a process was stopped. Recheck once,
        // and re-arm expired sleep/poll deadlines instead of losing their wakeup.
        super::wake_terminal_readers(runtime);
        super::wake_network_waiters(runtime);
        super::wake_device_readers(runtime);
        super::wake_readiness_waiters(runtime, Word::MAX);
        for index in 0..runtime.managed.len() {
            let pid = runtime.managed[index].pid;
            if pid != 0 {
                super::wake_waiter_for_child(runtime, pid);
            }
        }
        if runtime.timer_port != 0 {
            let _ = arm_clock_timer(runtime);
        }
    }
}

pub(super) fn timed_wait_expired(runtime: &mut Runtime, pid: Word, number: Word) -> isize {
    if number != SYS_RT_SIGTIMEDWAIT {
        return 0;
    }
    if let Some(process) = runtime.managed_process_mut(pid) {
        process.cancel_signal_wait();
        process.signals_mut().wait_mask = None;
    }
    -(EAGAIN as isize)
}

pub fn terminate(runtime: &mut Runtime, pid: Word, signal: Word) {
    if runtime
        .managed_process(pid)
        .is_none_or(|process| process.exited)
    {
        return;
    }
    super::threads::exit_peers(runtime, pid, 128 + signal);
    if nanami_runtime::request_process_kill(pid, signal).is_err() {
        return;
    }
    runtime
        .managed_process_mut(pid)
        .unwrap()
        .signals_mut()
        .exit_signal = signal;
    runtime.mark_process_exited(pid, 128 + signal);
    close_process_files(runtime, pid);
    crate::notify_terminal_process_exit(runtime, pid, 128 + signal);
    notify_child(runtime, pid, 2, signal);
}

pub fn notify_child(runtime: &mut Runtime, child: Word, code: i32, status: Word) {
    let Some(snapshot) = runtime.managed_process(child).copied() else {
        return;
    };
    if snapshot.pid != snapshot.tgid {
        // Threads are not wait4 children and do not independently generate SIGCHLD.
        if snapshot.exited && nanami_runtime::request_process_reap(child).is_ok() {
            runtime.remove_process(child);
        }
        if let Some(leader) = runtime.managed_process(snapshot.tgid).copied() {
            if leader.exited && !runtime.has_live_threads(leader.pid) {
                crate::notify_terminal_process_exit(runtime, leader.pid, leader.exit_status);
                let signal = leader.signals().map_or(0, |signals| signals.exit_signal);
                notify_child(
                    runtime,
                    leader.pid,
                    if signal == 0 { 1 } else { 2 },
                    if signal == 0 {
                        leader.exit_status
                    } else {
                        signal
                    },
                );
            }
        }
        return;
    }
    if snapshot.exited && runtime.has_live_threads(child) {
        return;
    }
    let Some(process) = runtime.managed_process_mut(child) else {
        return;
    };
    let parent = process.parent_pid;
    if process.signal_state != 0 {
        process.signals_mut().child_notify = false;
    }
    // Capture siginfo before wait4 can reap the child record.
    let action = runtime
        .managed_process(parent)
        .and_then(|process| process.signals())
        .map_or(Action::default(), |state| state.action(signals::SIGCHLD));
    if runtime
        .managed_process(parent)
        .is_some_and(|process| process.signal_waiting && process.signal_context.number == SYS_VFORK)
    {
        super::wake_waiter_for_child(runtime, child);
    }
    if code <= 3
        && (action.handler == signals::SIG_IGN || action.flags & signals::SA_NOCLDWAIT != 0)
    {
        if nanami_runtime::request_process_reap(child).is_ok() {
            close_process_files(runtime, child);
            runtime.remove_process(child);
            // wait4 must report ECHILD after the last auto-reaped child, not hang.
            if let Some(parent_process) = runtime.managed_process(parent) {
                let context = parent_process.signal_context;
                if parent_process.signal_waiting
                    && context.number == SYS_WAIT4
                    && !parent_process.signal_stopped()
                    && !runtime.has_child(parent, context.args[0])
                {
                    let pcb = parent_process.pcb;
                    runtime.managed_process_mut(parent).unwrap().signal_waiting = false;
                    if write_syscall_return(pcb, context, -(ECHILD as isize)).is_ok() {
                        let _ = a9n_abi::arch::process_control_block::resume(pcb);
                    }
                }
            }
        }
    }
    super::wake_waiter_for_child(runtime, child);
    if code >= 5 && action.flags & signals::SA_NOCLDSTOP != 0 {
        return;
    }
    let _ = queue_group(runtime, parent, Info::child(child, code, status));
}
