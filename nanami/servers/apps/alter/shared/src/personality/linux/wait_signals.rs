use super::{
    close_process_files, map_request_error, read_shm_u64, read_target_memory, write_syscall_return,
    write_target_memory, write_u32_to_target, EmulationAction, LinuxSyscallContext, Runtime, Word,
    ECHILD, EFAULT, EINTR, EINVAL, ENOMEM, ESRCH, LINUX_KERNEL_SIGACTION_BYTES, LINUX_MINSIGSTKSZ,
    LINUX_NSIG, LINUX_SIGSET_BYTES, LINUX_SS_AUTODISARM, LINUX_SS_DISABLE, LINUX_STACK_T_BYTES,
    LINUX_WNOHANG, SYS_VFORK, SYS_WAIT4,
};

pub(super) fn sys_wait4(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    let target_pid = context.args[0];
    let status_ptr = context.args[1];
    let options = context.args[2];
    if (options & !LINUX_WNOHANG) != 0 {
        return EmulationAction::Return(-(EINVAL as isize));
    }
    if let Some((child_pid, exit_status)) = runtime.exited_child(pid, target_pid) {
        if status_ptr != 0 {
            if write_u32_to_target(runtime, pid, status_ptr, ((exit_status & 0xff) << 8) as u32)
                .is_err()
            {
                return EmulationAction::Return(-(EFAULT as isize));
            }
        }
        if let Err(error) = libnanami::request_process_reap(child_pid) {
            let errno = map_request_error(error);
            libnanami::println!(
                "[alter/linux] wait4 reap failed parent={} child={} errno={}",
                pid,
                child_pid,
                errno
            );
            return EmulationAction::Return(-(errno as isize));
        }
        close_process_files(runtime, child_pid);
        runtime.remove_process(child_pid);
        return EmulationAction::Return(child_pid as isize);
    }
    if runtime.has_child(pid, target_pid) {
        if (options & LINUX_WNOHANG) != 0 {
            return EmulationAction::Return(0);
        }
        if runtime.park_signal_waiter(pid, target_pid, context) {
            return EmulationAction::Park;
        }
        return EmulationAction::Return(-(ESRCH as isize));
    }
    EmulationAction::Return(-(ECHILD as isize))
}

pub(super) fn sys_sigaltstack(
    runtime: &mut Runtime,
    pid: Word,
    new_stack: Word,
    old_stack: Word,
) -> Result<Word, i32> {
    if old_stack != 0 {
        unsafe {
            ::core::ptr::write_unaligned(runtime.posix_shm as *mut Word, 0);
            ::core::ptr::write_unaligned((runtime.posix_shm + 8) as *mut Word, LINUX_SS_DISABLE);
            ::core::ptr::write_unaligned((runtime.posix_shm + 16) as *mut Word, 0);
        }
        write_target_memory(runtime, pid, old_stack, LINUX_STACK_T_BYTES)?;
    }
    if new_stack != 0 {
        read_target_memory(runtime, pid, new_stack, LINUX_STACK_T_BYTES)?;
        let flags = read_shm_u64(runtime, 8);
        let size = read_shm_u64(runtime, 16);
        if (flags & !(LINUX_SS_DISABLE | LINUX_SS_AUTODISARM)) != 0 {
            return Err(EINVAL);
        }
        if (flags & LINUX_SS_DISABLE) == 0 && size < LINUX_MINSIGSTKSZ {
            return Err(ENOMEM);
        }
    }
    Ok(0)
}

pub(super) fn sys_rt_sigaction(
    runtime: &mut Runtime,
    pid: Word,
    signum: Word,
    _act: Word,
    oldact: Word,
    sigset_size: Word,
) -> Result<Word, i32> {
    if signum == 0 || signum > LINUX_NSIG || sigset_size != LINUX_SIGSET_BYTES {
        return Err(EINVAL);
    }
    if oldact != 0 {
        unsafe {
            ::core::ptr::write_bytes(
                runtime.posix_shm as *mut u8,
                0,
                LINUX_KERNEL_SIGACTION_BYTES,
            );
        }
        write_target_memory(runtime, pid, oldact, LINUX_KERNEL_SIGACTION_BYTES as Word)?;
    }
    Ok(0)
}

pub(super) fn sys_rt_sigprocmask(
    runtime: &mut Runtime,
    pid: Word,
    _how: Word,
    _set: Word,
    oldset: Word,
    sigset_size: Word,
) -> Result<Word, i32> {
    if sigset_size != LINUX_SIGSET_BYTES {
        return Err(EINVAL);
    }
    if oldset != 0 {
        unsafe {
            ::core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, LINUX_SIGSET_BYTES as usize);
        }
        write_target_memory(runtime, pid, oldset, LINUX_SIGSET_BYTES)?;
    }
    Ok(0)
}

pub(super) fn sys_rt_sigsuspend(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    if runtime.exited_child(pid, 0).is_some() || !runtime.has_child(pid, 0) {
        return EmulationAction::Return(-(EINTR as isize));
    }
    if runtime.park_signal_waiter(pid, 0, context) {
        EmulationAction::Park
    } else {
        EmulationAction::Return(-(ESRCH as isize))
    }
}

pub fn wake_waiter_for_child(runtime: &mut Runtime, child_pid: Word) {
    let Some((parent_pid, parent_pcb, context)) = runtime.take_signal_waiter_for_child(child_pid)
    else {
        return;
    };

    let return_value = if context.number == SYS_WAIT4 {
        let status = runtime
            .managed_process(child_pid)
            .map(|process| process.exit_status)
            .unwrap_or(1);
        let status_ptr = context.args[1];
        if status_ptr != 0
            && write_u32_to_target(
                runtime,
                parent_pid,
                status_ptr,
                ((status & 0xff) << 8) as u32,
            )
            .is_err()
        {
            -(EFAULT as isize)
        } else if let Err(error) = libnanami::request_process_reap(child_pid) {
            let errno = map_request_error(error);
            libnanami::println!(
                "[alter/linux] wait4 wake reap failed parent={} child={} errno={}",
                parent_pid,
                child_pid,
                errno
            );
            -(errno as isize)
        } else {
            close_process_files(runtime, child_pid);
            runtime.remove_process(child_pid);
            child_pid as isize
        }
    } else if context.number == SYS_VFORK {
        child_pid as isize
    } else {
        -(EINTR as isize)
    };

    if write_syscall_return(parent_pcb, context, return_value).is_err() {
        libnanami::println!(
            "[alter/linux] waiter wake register write failed parent={} pcb={:#x}",
            parent_pid,
            parent_pcb
        );
        return;
    }
    if let Err(error) = a9n_abi::arch::process_control_block::resume(parent_pcb) {
        libnanami::println!(
            "[alter/linux] waiter wake resume failed parent={} pcb={:#x} err={:?}",
            parent_pid,
            parent_pcb,
            error
        );
    }
}
