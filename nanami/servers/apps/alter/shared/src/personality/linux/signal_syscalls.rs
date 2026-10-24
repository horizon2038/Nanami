use super::*;
use crate::common::signals::{self, Action, AltStack, Info, SS_ONSTACK};

pub(super) fn sys_rt_sigaction(
    runtime: &mut Runtime,
    pid: Word,
    signal: Word,
    act: Word,
    oldact: Word,
    size: Word,
) -> Result<Word, i32> {
    if signal == 0 || signal > 64 || size != 8 {
        return Err(EINVAL);
    }
    let new = if act != 0 {
        if signal == signals::SIGKILL || signal == signals::SIGSTOP {
            return Err(EINVAL);
        }
        read_target_memory(runtime, pid, act, 32)?;
        Some(Action {
            handler: read_shm_u64(runtime, 0),
            flags: read_shm_u64(runtime, 8),
            restorer: read_shm_u64(runtime, 16),
            mask: signals::blockable(read_shm_u64(runtime, 24) as u64),
        })
    } else {
        None
    };
    let process = runtime.managed_process(pid).ok_or(ESRCH)?;
    let old = process
        .signals()
        .map_or(Action::default(), |state| state.actions[signal - 1]);
    if oldact != 0 {
        let bytes = unsafe { ::core::slice::from_raw_parts_mut(runtime.posix_shm as *mut u8, 32) };
        for (index, value) in [old.handler, old.flags, old.restorer, old.mask as usize]
            .iter()
            .enumerate()
        {
            signal_frame::put(bytes, index * 8, *value);
        }
        write_target_memory(runtime, pid, oldact, 32)?;
    }
    if let Some(action) = new {
        let state = runtime.managed_process_mut(pid).ok_or(ESRCH)?.signals_mut();
        state.actions[signal - 1] = action;
        if action.handler == signals::SIG_IGN
            || (action.handler == signals::SIG_DFL && signals::default_ignored(signal))
        {
            state.discard(signal);
        }
    }
    Ok(0)
}

pub(super) fn sys_rt_sigprocmask(
    runtime: &mut Runtime,
    pid: Word,
    how: Word,
    set: Word,
    oldset: Word,
    size: Word,
) -> Result<Word, i32> {
    if size != 8 || (set != 0 && how > 2) {
        return Err(EINVAL);
    }
    let new = if set != 0 {
        read_target_memory(runtime, pid, set, 8)?;
        Some(signals::blockable(read_shm_u64(runtime, 0) as u64))
    } else {
        None
    };
    let old = runtime
        .managed_process(pid)
        .ok_or(ESRCH)?
        .signals()
        .map_or(0, |state| state.blocked);
    if oldset != 0 {
        write_guest_u64(runtime, pid, oldset, old as Word)?;
    }
    if let Some(new) = new {
        let state = runtime.managed_process_mut(pid).ok_or(ESRCH)?.signals_mut();
        state.blocked = match how {
            0 => old | new,
            1 => old & !new,
            _ => new,
        };
    }
    Ok(0)
}

pub(super) fn sys_rt_sigpending(
    runtime: &mut Runtime,
    pid: Word,
    set: Word,
    size: Word,
) -> Result<Word, i32> {
    if size != 8 {
        return Err(EINVAL);
    }
    let pending = runtime
        .managed_process(pid)
        .ok_or(ESRCH)?
        .signals()
        .map_or(0, |state| state.pending_mask() & state.blocked);
    write_guest_u64(runtime, pid, set, pending as Word)?;
    Ok(0)
}

pub(super) fn consume_wait(runtime: &mut Runtime, pid: Word, pointer: Word) -> Option<isize> {
    let state = runtime.managed_process_mut(pid)?.signals_mut();
    let mask = state.wait_mask?;
    let info = state.take_matching(mask)?;
    if pointer != 0 {
        if write_target_memory_from(pid, pointer, info.bytes.as_ptr() as Word, 128).is_err() {
            state.enqueue(info);
            state.wait_mask = None;
            return Some(-(EFAULT as isize));
        }
    }
    state.wait_mask = None;
    Some(info.signal() as isize)
}

pub(super) fn sys_rt_sigtimedwait(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    let result = (|| {
        if context.args[3] != 8 {
            return Err(EINVAL);
        }
        read_target_memory(runtime, pid, context.args[0], 8)?;
        let mask = signals::blockable(read_shm_u64(runtime, 0) as u64);
        runtime
            .managed_process_mut(pid)
            .ok_or(ESRCH)?
            .signals_mut()
            .wait_mask = Some(mask);
        if let Some(value) = consume_wait(runtime, pid, context.args[1]) {
            return Ok(EmulationAction::Return(value));
        }
        if context.args[2] != 0 {
            let Some(deadline) = clocks::sleep_deadline(runtime, pid, context.args[2], 1, false)?
            else {
                return Err(EAGAIN);
            };
            let process = runtime.managed_process_mut(pid).ok_or(ESRCH)?;
            process.sleep_waiting = true;
            process.sleep_deadline = deadline;
            process.sleep_context = context;
            if let Err(errno) = arm_clock_timer(runtime) {
                runtime
                    .managed_process_mut(pid)
                    .unwrap()
                    .cancel_signal_wait();
                return Err(errno);
            }
        }
        runtime.park_signal_waiter(pid, Word::MAX, context);
        Ok(EmulationAction::Park)
    })();
    match result {
        Ok(action) => action,
        Err(errno) => {
            if let Some(process) = runtime.managed_process_mut(pid) {
                process.signals_mut().wait_mask = None;
            }
            EmulationAction::Return(-(errno as isize))
        }
    }
}

pub(super) fn sys_sigaltstack(
    runtime: &mut Runtime,
    pid: Word,
    new: Word,
    old: Word,
) -> Result<Word, i32> {
    let process = runtime.managed_process(pid).ok_or(ESRCH)?;
    let pcb = process.pcb;
    let stack = process
        .signals()
        .map_or(AltStack::DISABLED, |state| state.altstack);
    let sp = crate::process::read_register_value(pcb, crate::process::REG_RSP).map_err(|_| EIO)?;
    let onstack = stack.contains(sp);
    let proposed = if new != 0 {
        read_target_memory(runtime, pid, new, 24)?;
        let bytes = unsafe { ::core::slice::from_raw_parts(runtime.posix_shm as *const u8, 24) };
        let mut proposed = signal_frame::get_stack(bytes, 0);
        if onstack {
            return Err(EPERM);
        }
        if proposed.flags & !(signals::SS_DISABLE | signals::SS_AUTODISARM) != 0 {
            return Err(EINVAL);
        }
        if proposed.flags & signals::SS_DISABLE != 0 {
            proposed = AltStack::DISABLED;
        } else {
            if proposed.size < signal_frame::MINSIGSTKSZ {
                return Err(ENOMEM);
            }
            if proposed
                .sp
                .checked_add(proposed.size)
                .is_none_or(|end| !signal_frame::user_address(end))
            {
                return Err(EINVAL);
            }
        }
        Some(proposed)
    } else {
        None
    };
    if old != 0 {
        let mut reported = stack;
        if onstack {
            reported.flags |= SS_ONSTACK;
        }
        let bytes = unsafe { ::core::slice::from_raw_parts_mut(runtime.posix_shm as *mut u8, 24) };
        signal_frame::put_stack(bytes, 0, reported);
        write_target_memory(runtime, pid, old, 24)?;
    }
    if let Some(stack) = proposed {
        runtime
            .managed_process_mut(pid)
            .ok_or(ESRCH)?
            .signals_mut()
            .altstack = stack;
    }
    Ok(0)
}

pub(super) fn sys_rt_sigsuspend(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    let result = (|| {
        if context.args[1] != 8 {
            return Err(EINVAL);
        }
        read_target_memory(runtime, pid, context.args[0], 8)?;
        let mask = signals::blockable(read_shm_u64(runtime, 0) as u64);
        let state = runtime.managed_process_mut(pid).ok_or(ESRCH)?.signals_mut();
        state.suspend_mask = Some(state.blocked);
        state.blocked = mask;
        runtime.park_signal_waiter(pid, Word::MAX, context);
        Ok(())
    })();
    match result {
        Ok(()) => EmulationAction::Park,
        Err(errno) => EmulationAction::Return(-(errno as isize)),
    }
}

pub(super) fn sys_tgkill(
    runtime: &mut Runtime,
    caller: Word,
    tgid: Word,
    tid: Word,
    signal: Word,
) -> Result<Word, i32> {
    // Alter currently supports one thread per process, not CLONE_THREAD.
    if (tgid as isize) <= 0 || (tid as isize) <= 0 || signal > 64 {
        return Err(EINVAL);
    }
    if tgid != tid {
        return Err(ESRCH);
    }
    sys_kill(runtime, caller, tid, 0)?;
    if signal != 0 {
        signals_runtime::queue(runtime, tid, Info::new(signal, -6, caller))?;
    }
    Ok(0)
}

pub(super) fn sys_sigqueue(
    runtime: &mut Runtime,
    caller: Word,
    target: Word,
    signal: Word,
    info: Word,
) -> Result<Word, i32> {
    if signal == 0 || signal > 64 {
        return Err(EINVAL);
    }
    sys_kill(runtime, caller, target, 0)?;
    read_target_memory(runtime, caller, info, 128)?;
    let mut info = Info { bytes: [0; 128] };
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            runtime.posix_shm as *const u8,
            info.bytes.as_mut_ptr(),
            128,
        );
    }
    let code = i32::from_le_bytes(info.bytes[8..12].try_into().unwrap());
    // Do not allow forging kernel-generated siginfo or SI_TKILL for another task.
    if code >= 0 || (code == -6 && caller != target) {
        return Err(EPERM);
    }
    info.bytes[..4].copy_from_slice(&(signal as u32).to_le_bytes());
    signals_runtime::queue(runtime, target, info)?;
    Ok(0)
}
