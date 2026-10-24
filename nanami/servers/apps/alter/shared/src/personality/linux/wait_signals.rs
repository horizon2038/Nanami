use super::*;

pub(super) fn sys_wait4(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    let target = context.args[0];
    let options = context.args[2];
    if options & !(LINUX_WNOHANG | LINUX_WUNTRACED | LINUX_WCONTINUED) != 0 {
        return EmulationAction::Return(-(EINVAL as isize));
    }
    if let Some((child, status, exited)) = runtime.child_wait_event(
        pid,
        target,
        options & LINUX_WUNTRACED != 0,
        options & LINUX_WCONTINUED != 0,
    ) {
        let result = (|| {
            if context.args[1] != 0 {
                write_u32_to_target(runtime, pid, context.args[1], status)?;
            }
            if context.args[3] != 0 {
                unsafe {
                    ::core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, 144);
                }
                write_target_memory(runtime, pid, context.args[3], 144)?;
            }
            if exited {
                nanami_runtime::request_process_reap(child).map_err(map_request_error)?;
                close_process_files(runtime, child);
                runtime.remove_process(child);
            } else {
                runtime
                    .managed_process_mut(child)
                    .ok_or(ESRCH)?
                    .signals_mut()
                    .child_event = None;
            }
            Ok(child)
        })();
        return EmulationAction::Return(result_to_linux_return(result));
    }
    if !runtime.has_child(pid, target) {
        return EmulationAction::Return(-(ECHILD as isize));
    }
    if options & LINUX_WNOHANG != 0 {
        return EmulationAction::Return(0);
    }
    if runtime.park_signal_waiter(pid, target, context) {
        EmulationAction::Park
    } else {
        EmulationAction::Return(-(ESRCH as isize))
    }
}

pub fn wake_waiter_for_child(runtime: &mut Runtime, child_pid: Word) {
    let Some(child) = runtime.managed_process(child_pid) else {
        return;
    };
    let Some(parent) = runtime.managed_process(child.parent_pid) else {
        return;
    };
    if !parent.signal_waiting || parent.signal_stopped() {
        return;
    }
    let (pid, pcb, context) = (parent.pid, parent.pcb, parent.signal_context);
    let value = if context.number == SYS_WAIT4 {
        match sys_wait4(runtime, pid, context) {
            EmulationAction::Return(value) => value,
            _ => return,
        }
    } else if context.number == SYS_VFORK && (child.exited || child.job.executed) {
        child_pid as isize
    } else {
        return;
    };
    if let Some(parent) = runtime.managed_process_mut(pid) {
        parent.signal_waiting = false;
        parent.signal_context = LinuxSyscallContext::EMPTY;
    }
    if write_syscall_return(pcb, context, value).is_ok() {
        let _ = a9n_abi::arch::process_control_block::resume(pcb);
    }
}
