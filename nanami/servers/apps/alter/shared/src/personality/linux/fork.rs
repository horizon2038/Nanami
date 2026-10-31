use super::{
    arch, clone_registers_for_fork, inherit_linux_files, map_request_error, read_register_value,
    write_guest_u32, EmulationAction, LinuxSyscallContext, Runtime, Word, EINVAL, EIO, ENOMEM,
    ENOSYS, ESRCH, LINUX_CLONE_CHILD_SETTID, LINUX_CLONE_PARENT_SETTID, LINUX_CLONE_SETTLS,
    LINUX_CLONE_VM, LINUX_SIGCHLD, REG_FS_BASE, SYS_CLONE,
};

pub(super) fn sys_fork(
    runtime: &mut Runtime,
    parent_pid: Word,
    context: LinuxSyscallContext,
) -> Result<Word, i32> {
    if context.number == SYS_CLONE && (context.args[0] & LINUX_CLONE_VM) != 0 {
        return super::threads::clone_vm(runtime, parent_pid, context);
    }
    if context.number == SYS_CLONE && context.args[0] & (0x800 | 0x10000) != 0 {
        return Err(EINVAL);
    }
    if context.number == SYS_CLONE {
        let allowed = 0xff
            | LINUX_CLONE_PARENT_SETTID
            | LINUX_CLONE_CHILD_SETTID
            | LINUX_CLONE_SETTLS
            | 0x0020_0000;
        if context.args[0] & !allowed != 0 {
            return Err(ENOSYS);
        }
        if context.args[0] & 0xff != LINUX_SIGCHLD {
            return Err(EINVAL);
        }
    }
    let parent = runtime.managed_process(parent_pid).copied().ok_or(ESRCH)?;
    let image_name =
        ::core::str::from_utf8(&parent.image_name[..parent.image_name_len]).map_err(|_| EINVAL)?;
    let clone_options = clone_options(context);
    let current_fs_base = read_register_value(parent.pcb, REG_FS_BASE).unwrap_or(parent.fs_base);
    if current_fs_base != parent.fs_base {
        let _ = runtime.set_fs_base(parent_pid, current_fs_base);
    }

    let child_fs_base = if clone_options.set_tls {
        clone_options.tls
    } else {
        current_fs_base
    };

    let pcb_slot = runtime.next_pcb_slot().ok_or(ENOMEM)?;
    let child_pid = match nanami_runtime::request_process_fork(parent_pid, pcb_slot, 4)
        .map_err(map_request_error)
    {
        Ok(pid) => pid,
        Err(errno) => {
            nanami_runtime::println!(
                "[alter/linux] fork failed stage=spawn image={} pcb_slot={} errno={}",
                image_name,
                pcb_slot,
                errno
            );
            return Err(errno);
        }
    };
    let child_pcb = nanami_runtime::ipc::process_slot_descriptor(pcb_slot);

    if let Err(error) = fork_step(
        "clone registers",
        match clone_registers_for_fork(
            parent.pcb,
            child_pcb,
            context,
            clone_options.child_stack,
            child_fs_base,
        ) {
            Ok(()) => Ok(()),
            Err(error) => {
                nanami_runtime::println!(
                    "[alter/linux] fork register clone failed parent_pcb={:#x} child_pcb={:#x} err={:?}",
                    parent.pcb,
                    child_pcb,
                    error
                );
                Err(EIO)
            }
        },
    ) {
        discard_spawned_child(child_pid);
        return Err(error);
    }
    if !runtime.install_managed_child_process(
        child_pid,
        parent_pid,
        parent.owner_pid,
        child_pcb,
        parent.terminal_id,
        &parent.image_name[..parent.image_name_len],
    ) {
        nanami_runtime::println!(
            "[alter/linux] fork failed stage=install child parent={} child={} errno={}",
            parent_pid,
            child_pid,
            ENOMEM
        );
        discard_spawned_child(child_pid);
        return Err(ENOMEM);
    }
    if let Err(error) = write_clone_tid_pointers(
        runtime,
        parent_pid,
        child_pid,
        clone_options.parent_tid,
        clone_options.child_tid,
        clone_options.write_parent_tid,
        clone_options.write_child_tid,
    ) {
        discard_spawned_child(child_pid);
        runtime.remove_process(child_pid);
        return Err(error);
    }
    if let Err(error) = fork_step(
        "clone files",
        inherit_linux_files(runtime, parent_pid, child_pid),
    ) {
        discard_spawned_child(child_pid);
        runtime.remove_process(child_pid);
        return Err(error);
    }
    let mut i = 0usize;
    while i < parent.mappings.len() {
        let mapping = parent.mappings[i];
        if mapping.base != 0
            && !runtime.add_mapping(child_pid, mapping.base, mapping.size, mapping.prot)
        {
            nanami_runtime::println!(
                "[alter/linux] fork failed stage=track mappings parent={} child={} errno={}",
                parent_pid,
                child_pid,
                ENOMEM
            );
            super::close_process_files(runtime, child_pid);
            discard_spawned_child(child_pid);
            runtime.remove_process(child_pid);
            return Err(ENOMEM);
        }
        i += 1;
    }
    if let Err(error) = super::memfd::inherit(runtime, parent_pid, child_pid) {
        super::close_process_files(runtime, child_pid);
        discard_spawned_child(child_pid);
        runtime.remove_process(child_pid);
        return Err(error);
    }
    {
        let Some(child) = runtime.managed_process_mut(child_pid) else {
            discard_spawned_child(child_pid);
            runtime.remove_process(child_pid);
            return Err(ESRCH);
        };
        child.program_break = parent.program_break;
        child.mapped_break = parent.mapped_break;
        child.fs_base = child_fs_base;
        child.trace_enabled = parent.trace_enabled;
        child.diagnostics_enabled = parent.diagnostics_enabled;
        child.graphics_enabled = parent.graphics_enabled;
        child.framebuffer_size = parent.framebuffer_size;
        child.graphics_session = parent.graphics_session;
        child.personality = parent.personality;
        child.terminal_canonical = parent.terminal_canonical;
        child.terminal_echo = parent.terminal_echo;
        child.terminal_termios = parent.terminal_termios;
        child.parent_pid = parent.tgid;
    }
    runtime.inherit_signals(parent_pid, child_pid);
    super::sysv_shm::inherited(runtime, child_pid);
    if context.number == SYS_CLONE && context.args[0] & 0x0020_0000 != 0 {
        let Some(task) = runtime.tasks.ensure(child_pid) else {
            super::close_process_files(runtime, child_pid);
            discard_spawned_child(child_pid);
            runtime.remove_process(child_pid);
            return Err(ENOMEM);
        };
        task.clear_tid = clone_options.child_tid;
    }
    if cfg!(debug_assertions) {
        if let Ok(read_back_fs_base) = read_register_value(child_pcb, REG_FS_BASE) {
            if read_back_fs_base != child_fs_base {
                nanami_runtime::println!(
                    "[alter/linux] fork fsbase mismatch parent={} child={} expected={:#x} actual={:#x}",
                    parent_pid,
                    child_pid,
                    child_fs_base,
                    read_back_fs_base
                );
            }
        }
    }
    if let Err(error) = fork_step(
        "resume child",
        a9n_abi::arch::process_control_block::resume(child_pcb).map_err(|_| EIO),
    ) {
        let _ = a9n_abi::arch::process_control_block::suspend(child_pcb);
        super::close_process_files(runtime, child_pid);
        discard_spawned_child(child_pid);
        runtime.remove_process(child_pid);
        return Err(error);
    }
    Ok(child_pid)
}

pub(super) fn sys_vfork(
    runtime: &mut Runtime,
    parent_pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    match sys_fork(runtime, parent_pid, context) {
        Ok(child_pid) => {
            if runtime.park_signal_waiter(parent_pid, child_pid, context) {
                EmulationAction::Park
            } else {
                EmulationAction::Return(-(ESRCH as isize))
            }
        }
        Err(errno) => EmulationAction::Return(-(errno as isize)),
    }
}

pub(super) fn discard_spawned_child(pid: Word) {
    let _ = nanami_runtime::request_process_kill(pid, 1);
    let _ = nanami_runtime::request_process_reap(pid);
}

pub(super) fn fork_step<T>(stage: &str, result: Result<T, i32>) -> Result<T, i32> {
    result.map_err(|error| {
        nanami_runtime::println!("[alter/linux] fork failed stage={} errno={}", stage, error);
        error
    })
}

#[derive(Clone, Copy)]
pub(super) struct CloneOptions {
    pub(super) child_stack: Word,
    pub(super) parent_tid: Word,
    pub(super) child_tid: Word,
    pub(super) tls: Word,
    pub(super) write_parent_tid: bool,
    pub(super) write_child_tid: bool,
    pub(super) set_tls: bool,
}

pub(super) fn clone_options(context: LinuxSyscallContext) -> CloneOptions {
    let flags = if context.number == SYS_CLONE {
        context.args[0]
    } else {
        LINUX_SIGCHLD
    };
    CloneOptions {
        child_stack: if context.number == SYS_CLONE {
            context.args[1]
        } else {
            0
        },
        parent_tid: if context.number == SYS_CLONE {
            context.args[2]
        } else {
            0
        },
        child_tid: if context.number == SYS_CLONE {
            arch::clone_child_tid(context.args)
        } else {
            0
        },
        tls: if context.number == SYS_CLONE {
            arch::clone_tls(context.args)
        } else {
            0
        },
        write_parent_tid: (flags & LINUX_CLONE_PARENT_SETTID) != 0,
        write_child_tid: (flags & LINUX_CLONE_CHILD_SETTID) != 0,
        set_tls: (flags & LINUX_CLONE_SETTLS) != 0,
    }
}

pub(super) fn write_clone_tid_pointers(
    runtime: &mut Runtime,
    parent_pid: Word,
    child_pid: Word,
    parent_tid: Word,
    child_tid: Word,
    write_parent_tid: bool,
    write_child_tid: bool,
) -> Result<(), i32> {
    if write_parent_tid {
        write_guest_u32(runtime, parent_pid, parent_tid, child_pid as u32)?;
    }
    if write_child_tid {
        write_guest_u32(runtime, child_pid, child_tid, child_pid as u32)?;
    }
    Ok(())
}
